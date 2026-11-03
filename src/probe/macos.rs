//! 采集层。macOS 走 macmon 的 IOReport 通道，免 sudo 拿到功耗与温度，
//! 缺页计数走 host_statistics。
//!
//! macmon 返回 `Box<dyn Error>`，这里统一转成 anyhow 语境，
//! 让上层看到的错误带得上前因后果。

use std::time::Instant;

use anyhow::{Context, Result};

use crate::probe::refresh_procs;
use crate::snapshot::{Core, Cpu, Fan, Gpu, Mem, Power, Proc, Snapshot, Temp};

/// host_info.h: HOST_VM_INFO
const HOST_VM_INFO: i32 = 2;
/// vm_statistics_data_t 的字段个数，也是 host_statistics 要的 count。
const VM_STAT_COUNT: u32 = 15;
/// pageins 在 vm_statistics 里的下标。
///
/// 本来想用 faults（缺页陷阱总数，语义最正），实测发现它和 zero_fill_count、
/// cow_faults 一样是废弃字段——现代 macOS 内核恒返回 -1，SDK 头文件里还留着
/// 但早就不填了。pageins 是 faults 的子集，只计真正从磁盘换页的 major fault，
/// 单调递增且每秒有几百次变化，是这份结构里还活着的最接近的替代。
const VM_STAT_PAGEINS: usize = 6;

unsafe extern "C" {
    fn mach_host_self() -> u32;
    fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
}

/// 读取系统累计换页次数（major page fault）。
///
/// 用定长数组接收而不是声明结构体：`HOST_VM_INFO_COUNT` 就是 15 个 `integer_t`，
/// 字段连续无填充，数组下标比结构体布局更不容易出错。
fn vm_faults() -> Option<u32> {
    let mut buf = [0i32; VM_STAT_COUNT as usize];
    let mut count = VM_STAT_COUNT;
    let kr = unsafe { host_statistics(mach_host_self(), HOST_VM_INFO, buf.as_mut_ptr(), &mut count) };
    if std::env::var("PT_DEBUG_VM").is_ok() {
        eprintln!("[vm] kr={kr} count={count} buf={buf:?}");
    }
    if kr == 0 { Some(buf[VM_STAT_PAGEINS] as u32) } else { None }
}

pub struct Probe {
    sampler: macmon::Sampler,
    soc: macmon::SocInfo,
    sys: sysinfo::System,
    cores: f64,
    /// 上一帧的累计缺页数，用于取差分。
    last_faults: Option<u32>,
}

impl Probe {
    pub fn new() -> Result<Self> {
        let sampler = macmon::Sampler::new().map_err(|e| anyhow::anyhow!("{e}"))
            .context("初始化 Apple Silicon 采样器失败，确认运行在 Apple Silicon 且系统版本受支持")?;
        let soc = sampler.get_soc_info().clone();
        let cores = f64::from(soc.cpu_tiers.iter().map(|t| t.cores).sum::<u8>()).max(1.0);
        let mut sys = sysinfo::System::new().map_err(|e| anyhow::anyhow!("{e}")).context("读取系统信息失败")?;
        // 首次刷新建立 CPU 时间基线，sysinfo 的占比是两次刷新之间的差值。
        refresh_procs(&mut sys);
        Ok(Self { sampler, soc, sys, cores, last_faults: vm_faults() })
    }

    /// 芯片名，标题栏与硬件折算都用它。
    pub fn chip_name(&self) -> &str {
        &self.soc.chip_name
    }

    pub fn mac_model(&self) -> &str {
        &self.soc.mac_model
    }

    /// 逻辑核心总数。
    pub fn core_count(&self) -> usize {
        self.soc.cpu_tiers.iter().map(|t| t.cores as usize).sum()
    }

    /// 阻塞 duration 毫秒完成一次采样。
    pub fn sample(&mut self, duration_ms: u32) -> Result<Snapshot> {
        let raw = self.sampler.get_metrics(duration_ms)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("采样失败")?;
        refresh_procs(&mut self.sys);
        let procs = self.procs();
        let faults_per_sec = self.fault_rate(duration_ms);
        let mut snap = convert(&raw, Instant::now());
        snap.procs = procs;
        snap.faults_per_sec = faults_per_sec;
        snap.seats = crate::probe::aggregate_seats(&snap.procs);
        Ok(snap)
    }

    /// 缺页陷阱速率。累计值是 u32 会回绕，wrapping_sub 是模运算，回绕时依然给出正确差值。
    fn fault_rate(&mut self, duration_ms: u32) -> f64 {
        let now = vm_faults();
        let rate = match (now, self.last_faults) {
            (Some(n), Some(p)) => {
                let delta = n.wrapping_sub(p) as f64;
                delta / (f64::from(duration_ms) / 1000.0)
            }
            _ => 0.0,
        };
        self.last_faults = now;
        rate
    }

    /// 进程快照，按整机 CPU 占比从高到低。
    fn procs(&self) -> Vec<Proc> {
        let mut v: Vec<Proc> = self.sys.processes().iter().map(|(pid, p)| Proc {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().to_string(),
            // sysinfo 的 cpu_usage 以单核满载为 100%，除以核心数得到整机口径。
            cpu_pct: f64::from(p.cpu_usage()) / self.cores,
            mem_bytes: p.memory(),
            threads: p.tasks().map(|t| t.len()).unwrap_or(0),
            // Uid 是 #[repr(transparent)] 包着 libc::uid_t 且实现了 Deref，两次解引用拿到 u32。
            uid: p.user_id().map(|u| **u),
        }).collect();
        v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        v
    }
}

fn convert(m: &macmon::Metrics, at: Instant) -> Snapshot {
    let mut cores = Vec::new();
    for tier in &m.cpu_tiers {
        for c in &tier.cores {
            cores.push(Core {
                label: format!("{}{}", tier.label, c.core_id),
                freq_mhz: f64::from(c.freq_mhz),
                active_ratio: f64::from(c.active_ratio),
                scaled_ratio: f64::from(c.scaled_ratio),
            });
        }
    }

    Snapshot {
        at,
        cpu: Cpu {
            active_ratio: f64::from(m.cpu_active_ratio),
            scaled_ratio: f64::from(m.cpu_scaled_ratio),
            freq_mhz: weighted_freq(&m.cpu_tiers),
            cores,
        },
        gpu: Gpu {
            active_ratio: f64::from(m.gpu_active_ratio),
            scaled_ratio: f64::from(m.gpu_scaled_ratio),
            freq_mhz: f64::from(m.gpu_freq_mhz),
        },
        power: Power {
            cpu: f64::from(m.cpu_power),
            gpu: f64::from(m.gpu_power),
            ane: f64::from(m.ane_power),
            package: f64::from(m.all_power),
            sys: f64::from(m.sys_power),
            ram: f64::from(m.ram_power),
        },
        temp: Temp {
            cpu: m.temp.cpu_temp_avg.map(f64::from),
            gpu: m.temp.gpu_temp_avg.map(f64::from),
        },
        mem: Mem {
            used: m.memory.ram_usage,
            total: m.memory.ram_total,
            swap_used: m.memory.swap_usage,
            swap_total: m.memory.swap_total,
        },
        fans: m.fans.iter()
            .map(|f| Fan { name: f.name.clone(), rpm: f.rpm, max_rpm: f.max_rpm })
            .collect(),
        procs: Vec::new(),
        seats: Vec::new(),
        faults_per_sec: 0.0,
        has_gpu: true,
        has_power: true,
    }
}

/// 整机频率按各档位的核心数加权，避免大小核互相拉平。
fn weighted_freq(tiers: &[macmon::CpuTierMetrics]) -> f64 {
    let mut sum = 0.0;
    let mut count = 0.0;
    for t in tiers {
        let n = t.cores.len().max(1) as f64;
        sum += f64::from(t.freq_mhz) * n;
        count += n;
    }
    if count == 0.0 { 0.0 } else { sum / count }
}