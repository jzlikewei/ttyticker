//! Linux 采集层：全部走 /proc 与 /sys，不依赖任何私有框架。
//!
//! 与 macOS 侧的差异在于采样模型：macmon 需要阻塞一段时间测功耗，
//! 而 /proc 是瞬时快照，所以这里在 interval 两端各读一次做差分，
//! 对外仍然是「阻塞 duration 毫秒返回一个 Snapshot」。
//!
//! 拿不到的项不伪装成 0：GPU 与功耗各有能力标记，catalog 据此返回 None，
//! 界面显示占位符。容器与虚拟机里 RAPL 通常不可读，这是常态。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::probe::refresh_procs;
use crate::snapshot::{Core, Cpu, Fan, Gpu, Mem, Power, Proc, Snapshot, Temp};

/// 一颗核心的累计时间片。
#[derive(Clone, Copy, Default)]
struct CpuTimes {
    busy: u64,
    total: u64,
}

/// 一个 RAPL 功耗域。
struct Rapl {
    energy: PathBuf,
    name: String,
    /// 计数器回绕点，差分为负时用它补回来。
    max: u64,
}

struct TempSensor {
    input: PathBuf,
    label: String,
    chip: String,
}

struct FanSensor {
    input: PathBuf,
    label: String,
}

/// sysfs 上能读到的 GPU。
struct GpuSource {
    busy: PathBuf,
    freq: Option<PathBuf>,
    vram_used: Option<PathBuf>,
}

pub struct Probe {
    sys: sysinfo::System,
    chip: String,
    model: String,
    cores: usize,
    prev_cpu: Vec<CpuTimes>,
    prev_faults: Option<u64>,
    rapl: Vec<Rapl>,
    prev_energy: Option<(Vec<u64>, Instant)>,
    temps: Vec<TempSensor>,
    fans: Vec<FanSensor>,
    gpu: Option<GpuSource>,
}

impl Probe {
    pub fn new() -> Result<Self> {
        let mut sys = sysinfo::System::new()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("读取系统信息失败")?;
        refresh_procs(&mut sys);

        let cores = num_cores();
        let (chip, model) = cpu_identity();
        Ok(Self {
            sys,
            chip,
            model,
            cores,
            prev_cpu: read_cpu_times().unwrap_or_default(),
            prev_faults: read_pgfault(),
            rapl: scan_rapl(),
            prev_energy: None,
            temps: Vec::new(),
            fans: Vec::new(),
            gpu: scan_gpu(),
        })
        .map(|mut p: Probe| {
            let (t, f) = scan_hwmon();
            p.temps = t;
            p.fans = f;
            p
        })
    }

    /// 标题栏显示的芯片名。
    pub fn chip_name(&self) -> &str {
        &self.chip
    }

    /// Linux 上没有机型标识，用内核版本代替。
    pub fn mac_model(&self) -> &str {
        &self.model
    }

    pub fn core_count(&self) -> usize {
        self.cores
    }

    /// 阻塞 duration 毫秒完成一次采样。
    pub fn sample(&mut self, duration_ms: u32) -> Result<Snapshot> {
        let t0 = Instant::now();
        let cpu0 = read_cpu_times().unwrap_or_default();
        let energy0 = self.read_energy();
        let faults0 = read_pgfault();

        std::thread::sleep(Duration::from_millis(u64::from(duration_ms)));

        let cpu1 = read_cpu_times().unwrap_or_default();
        let energy1 = self.read_energy();
        let faults1 = read_pgfault();
        let dt = t0.elapsed().as_secs_f64().max(1e-6);

        let ratios = ratios(&self.prev_cpu, &cpu1);
        let whole = ratios.first().copied().unwrap_or(0.0);
        let per_core: Vec<f64> = ratios.iter().skip(1).copied().collect();
        self.prev_cpu = cpu1.clone();

        let cores: Vec<Core> = per_core
            .iter()
            .enumerate()
            .map(|(i, r)| Core {
                label: format!("C{i}"),
                freq_mhz: read_freq(i).unwrap_or(0.0),
                active_ratio: *r,
                // Linux 的 cpufreq 只给当前频率，没有「频率加权驻留」这个量。
                scaled_ratio: *r,
            })
            .collect();
        let freq_mhz = if cores.is_empty() {
            0.0
        } else {
            cores.iter().map(|c| c.freq_mhz).sum::<f64>() / cores.len() as f64
        };

        let faults_per_sec = match (faults1, faults0) {
            (Some(n), Some(p)) => n.saturating_sub(p) as f64 / dt,
            _ => 0.0,
        };
        self.prev_faults = faults1;

        let power = self.power(energy0, energy1, dt);
        let mem = read_mem().unwrap_or_default();
        let (cpu_temp, gpu_temp) = self.temps_c();
        refresh_procs(&mut self.sys);
        let procs = self.procs();
        // 先聚合再 move 进 Snapshot。
        let seats = crate::probe::aggregate_seats(&procs);

        Ok(Snapshot {
            at: Instant::now(),
            cpu: Cpu {
                active_ratio: whole,
                scaled_ratio: whole,
                freq_mhz,
                cores,
            },
            gpu: self.gpu_read(),
            power,
            temp: Temp { cpu: cpu_temp, gpu: gpu_temp },
            mem,
            fans: self.fan_read(),
            procs,
            seats,
            faults_per_sec,
            has_gpu: self.gpu.is_some(),
            has_power: !self.rapl.is_empty(),
        })
    }

    fn read_energy(&self) -> Vec<u64> {
        self.rapl.iter().map(|r| read_u64(&r.energy).unwrap_or(0)).collect()
    }

    /// RAPL 的能量计数器会回绕，差分为负时用 max_energy_range_uj 补回来。
    fn power(&mut self, prev: Vec<u64>, now: Vec<u64>, dt: f64) -> Power {
        let mut p = Power::default();
        for (i, r) in self.rapl.iter().enumerate() {
            let (Some(&a), Some(&b)) = (prev.get(i), now.get(i)) else { continue };
            let delta = if b >= a { b - a } else { b + r.max - a };
            let watts = delta as f64 / 1e6 / dt;
            if r.name.starts_with("package") {
                p.package = watts;
                p.cpu = watts;
            } else if r.name.starts_with("core") && p.cpu == 0.0 {
                p.cpu = watts;
            } else if r.name.starts_with("dram") {
                p.ram = watts;
            }
        }
        p.sys = p.package;
        let _ = self.prev_energy.take();
        p
    }

    /// CPU 温度取 coretemp / k10temp / zenpower 这类芯片的均值。
    fn temps_c(&self) -> (Option<f64>, Option<f64>) {
        let cpu_chips = ["coretemp", "k10temp", "zenpower"];
        let vals: Vec<f64> = self
            .temps
            .iter()
            .filter(|t| cpu_chips.contains(&t.chip.as_str()))
            .filter_map(|t| read_u64(&t.input).map(|v| v as f64 / 1000.0))
            .collect();
        if vals.is_empty() {
            (None, None)
        } else {
            (Some(vals.iter().sum::<f64>() / vals.len() as f64), None)
        }
    }

    fn fan_read(&self) -> Vec<Fan> {
        self.fans
            .iter()
            .filter_map(|f| {
                read_u64(&f.input).map(|rpm| Fan {
                    name: f.label.clone(),
                    rpm: rpm as u32,
                    max_rpm: None,
                })
            })
            .collect()
    }

    /// sysfs 上的 GPU：amdgpu 暴露 gpu_busy_percent，频率走 pp_dpm_sclk。
    fn gpu_read(&self) -> Gpu {
        let Some(g) = &self.gpu else { return Gpu::default() };
        let active = read_u64(&g.busy).map(|v| v as f64 / 100.0).unwrap_or(0.0);
        let freq = g.freq.as_ref().and_then(|p| read_u64(p)).unwrap_or(0) as f64;
        Gpu { active_ratio: active, scaled_ratio: active, freq_mhz: freq }
    }

    fn procs(&self) -> Vec<Proc> {
        let cores = self.cores.max(1) as f64;
        let mut v: Vec<Proc> = self
            .sys
            .processes()
            .iter()
            .map(|(pid, p)| Proc {
                pid: pid.as_u32(),
                name: p.name().to_string_lossy().to_string(),
                cpu_pct: f64::from(p.cpu_usage()) / cores,
                mem_bytes: p.memory(),
                threads: p.tasks().map(|t| t.len()).unwrap_or(0),
                uid: p.user_id().map(|u| **u),
            })
            .collect();
        v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
        v
    }
}

fn read_u64(p: &Path) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// /proc/stat 的 cpu 行：user nice system idle iowait irq softirq steal ...
///
/// busy 不含 idle 与 iowait——iowait 是等 IO 的时间，算空闲更贴近
/// 「这颗核有没有在干活」的直觉。
fn read_cpu_times() -> Option<Vec<CpuTimes>> {
    let s = fs::read_to_string("/proc/stat").ok()?;
    let mut out = Vec::new();
    for line in s.lines() {
        if !line.starts_with("cpu") {
            break;
        }
        let mut it = line.split_whitespace();
        let label = it.next()?;
        if label != "cpu" && !label[3..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let v: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
        if v.len() < 5 {
            continue;
        }
        let idle = v[3] + v[4];
        let total: u64 = v[..8.min(v.len())].iter().sum();
        out.push(CpuTimes { busy: total.saturating_sub(idle), total });
    }
    Some(out)
}

fn ratios(prev: &[CpuTimes], now: &[CpuTimes]) -> Vec<f64> {
    now.iter()
        .enumerate()
        .map(|(i, n)| {
            let Some(p) = prev.get(i) else { return 0.0 };
            let db = n.busy.saturating_sub(p.busy) as f64;
            let dt = n.total.saturating_sub(p.total) as f64;
            if dt > 0.0 { db / dt } else { 0.0 }
        })
        .collect()
}

/// 当前频率。cpufreq 拿得到就用它，容器与虚拟机里通常没有这个目录，
/// 回退到 /proc/cpuinfo 的 MHz 行。
fn read_freq(core: usize) -> Option<f64> {
    let khz = read_u64(Path::new(&format!(
        "/sys/devices/system/cpu/cpu{core}/cpufreq/scaling_cur_freq"
    )))?;
    Some(khz as f64 / 1000.0)
}

fn read_mem() -> Option<Mem> {
    let s = fs::read_to_string("/proc/meminfo").ok()?;
    let mut m: HashMap<&str, u64> = HashMap::new();
    for line in s.lines() {
        let mut it = line.split_whitespace();
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            if let Ok(n) = v.parse::<u64>() {
                m.insert(k.trim_end_matches(':'), n * 1024);
            }
        }
    }
    let total = *m.get("MemTotal")?;
    let avail = m.get("MemAvailable").copied().unwrap_or(0);
    let swap_total = m.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = m.get("SwapFree").copied().unwrap_or(0);
    Some(Mem {
        used: total.saturating_sub(avail),
        total,
        swap_used: swap_total.saturating_sub(swap_free),
        swap_total,
    })
}

/// 累计缺页数，取自 /proc/vmstat 的 pgfault。
fn read_pgfault() -> Option<u64> {
    let s = fs::read_to_string("/proc/vmstat").ok()?;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("pgfault ") {
            return v.trim().parse().ok();
        }
    }
    None
}

fn num_cores() -> usize {
    fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.lines().filter(|l| l.starts_with("processor")).count())
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

fn cpu_identity() -> (String, String) {
    let mut model = String::from("Linux");
    if let Ok(s) = fs::read_to_string("/proc/cpuinfo") {
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("model name") {
                if let Some(name) = v.split(':').nth(1) {
                    model = name.trim().to_string();
                    break;
                }
            }
        }
    }
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    (model, kernel)
}

/// 扫 /sys/class/powercap 下的 RAPL 域，含子域（core、uncore、dram）。
fn scan_rapl() -> Vec<Rapl> {
    let mut out = Vec::new();
    let root = Path::new("/sys/class/powercap");
    let Ok(rd) = fs::read_dir(root) else { return out };
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        collect_rapl(&dir, &mut out);
        if let Ok(sub) = fs::read_dir(&dir) {
            for s in sub.flatten() {
                if s.path().is_dir() {
                    collect_rapl(&s.path(), &mut out);
                }
            }
        }
    }
    out
}

fn collect_rapl(dir: &Path, out: &mut Vec<Rapl>) {
    let energy = dir.join("energy_uj");
    if !energy.exists() {
        return;
    }
    let name = fs::read_to_string(dir.join("name"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let max = read_u64(&dir.join("max_energy_range_uj")).unwrap_or(u64::MAX);
    out.push(Rapl { energy, name, max });
}

/// 扫 hwmon 下的温度与风扇节点。
fn scan_hwmon() -> (Vec<TempSensor>, Vec<FanSensor>) {
    let mut temps = Vec::new();
    let mut fans = Vec::new();
    let Ok(rd) = fs::read_dir("/sys/class/hwmon") else { return (temps, fans) };
    for e in rd.flatten() {
        let dir = e.path();
        let chip = fs::read_to_string(dir.join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let Ok(files) = fs::read_dir(&dir) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            let Some(idx) = name
                .strip_prefix("temp")
                .and_then(|r| r.strip_suffix("_input"))
            else {
                if let Some(i) = name.strip_prefix("fan").and_then(|r| r.strip_suffix("_input")) {
                    fans.push(FanSensor { input: f.path(), label: format!("fan{i}") });
                }
                continue;
            };
            let label = fs::read_to_string(dir.join(format!("temp{idx}_label")))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| format!("temp{idx}"));
            temps.push(TempSensor { input: f.path(), label, chip: chip.clone() });
        }
    }
    (temps, fans)
}

/// 找 sysfs 上可读的 GPU。amdgpu 暴露 gpu_busy_percent，最通用。
fn scan_gpu() -> Option<GpuSource> {
    let rd = fs::read_dir("/sys/class/drm").ok()?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = e.path().join("device");
        let busy = dev.join("gpu_busy_percent");
        if busy.exists() {
            return Some(GpuSource {
                busy,
                freq: Some(dev.join("gt_act_freq_mhz")).filter(|p| p.exists()),
                vram_used: Some(dev.join("mem_info_vram_used")).filter(|p| p.exists()),
            });
        }
    }
    None
}