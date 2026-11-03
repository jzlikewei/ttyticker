//! 一次采样的不可变快照。
//!
//! 采集层与上层之间唯一的契约。probe 把平台差异消化在内部，
//! catalog / candle / ui 只认这个结构，新增平台等于新增一个 Probe 实现，
//! 上层一行不动。

use std::time::Instant;

/// 单个 CPU 核心的当次读数。
#[derive(Clone, Debug, Default)]
pub struct Core {
    /// 核心标签，形如 `E0` / `P3`，E 能效核、P 性能核、S 超大核。
    pub label: String,
    pub freq_mhz: f64,
    /// 活跃率 0..1
    pub active_ratio: f64,
    /// 频率加权活跃驻留 0..1
    pub scaled_ratio: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Cpu {
    /// 整机活跃率 0..1。
    pub active_ratio: f64,
    /// 频率缩放比 0..1，与标称频率相乘得到实时频率。
    pub scaled_ratio: f64,
    /// 整机实时频率 MHz，按各档位核心数加权。
    pub freq_mhz: f64,
    pub cores: Vec<Core>,
}

#[derive(Clone, Debug, Default)]
pub struct Gpu {
    pub active_ratio: f64,
    pub scaled_ratio: f64,
    pub freq_mhz: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Power {
    pub cpu: f64,
    pub gpu: f64,
    /// 神经网络引擎功耗，Apple Silicon 独有。
    pub ane: f64,
    /// CPU + GPU + ANE 之和。
    pub package: f64,
    /// 整机功耗估算，部分机型不可得。
    pub sys: f64,
    /// 内存功耗。
    pub ram: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Temp {
    /// 平台可能不提供温度，缺数据时为 None，界面显示占位符。
    pub cpu: Option<f64>,
    pub gpu: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Mem {
    pub used: u64,
    pub total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
}

impl Mem {
    pub fn ratio(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.used as f64 / self.total as f64 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Fan {
    pub name: String,
    pub rpm: u32,
    pub max_rpm: Option<u32>,
}

/// 单个进程的当次读数，对应行情终端里的个股。
#[derive(Clone, Debug, Default)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// 整机口径的 CPU 占比 0..100，与任务管理器一致。
    pub cpu_pct: f64,
    pub mem_bytes: u64,
    pub threads: usize,
    /// 进程属主的 UID。平台拿不到时为 None，界面显示占位符。
    pub uid: Option<u32>,
}

/// 一个用户聚合出的席位，对应龙虎榜的一行。
///
/// A 股龙虎榜公布的是异动股的前五买卖席位，这里是资源占用前十的属主，
/// 同一个概念：谁在动，动多大。
#[derive(Clone, Debug, Default)]
pub struct Seat {
    pub uid: u32,
    pub name: String,
    /// 该席位全部进程的整机 CPU 占比之和。
    pub cpu_pct: f64,
    pub mem_bytes: u64,
    pub procs: usize,
}

/// 一次采样的全部读数，构造后不再修改。
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub at: Instant,
    pub cpu: Cpu,
    pub gpu: Gpu,
    pub power: Power,
    pub temp: Temp,
    pub mem: Mem,
    pub fans: Vec<Fan>,
    pub procs: Vec<Proc>,
    /// 按 CPU 占比从高到低的席位列表，龙虎榜直接取前十条。
    pub seats: Vec<Seat>,
    /// 缺页陷阱速率，次/秒。K 线的成交量取自这里。
    ///
    /// 不用 CPU 占用当量能：那样主图与副图讲的是同一件事，两条曲线完全正相关。
    /// 缺页速率反映内存访问的密集程度，与占用率不同步，
    /// 两者背离时通常是系统在换页或被中断打断。
    pub faults_per_sec: f64,
    /// 本机是否拿得到 GPU 读数。拿不到时界面显示占位符而不是 0。
    pub has_gpu: bool,
    /// 本机是否拿得到功耗读数。RAPL 在容器与虚拟机里通常不可读。
    pub has_power: bool,
}

impl Snapshot {
    /// 内存已用字节数换算成 GB。
    pub fn mem_used_gb(&self) -> f64 {
        self.mem.used as f64 / 1024.0 / 1024.0 / 1024.0
    }

    pub fn swap_used_gb(&self) -> f64 {
        self.mem.swap_used as f64 / 1024.0 / 1024.0 / 1024.0
    }
}
