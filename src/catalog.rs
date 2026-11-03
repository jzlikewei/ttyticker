//! 指标目录：每个指标的身份、量纲与取值口径。
//!
//! 这是 Snapshot 与界面之间唯一的桥。采集层只吐 Snapshot，
//! 界面只认 MetricId，两边都通过这里对话，量纲与精度只在这处定义。

use crate::snapshot::Snapshot;

/// 指标的量纲，决定显示后缀与涨跌文案的单位。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Percent,
    MHz,
    Watt,
    Celsius,
    GB,
    Rpm,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Percent => "%",
            Unit::MHz => "MHz",
            Unit::Watt => "W",
            Unit::Celsius => "°C",
            Unit::GB => "GB",
            Unit::Rpm => "rpm",
        }
    }
}

/// 板块，自选列表按此分组。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Board {
    Cpu,
    Gpu,
    Mem,
    Power,
}

impl Board {
    pub fn title(self) -> &'static str {
        match self {
            Board::Cpu => "CPU",
            Board::Gpu => "GPU",
            Board::Mem => "内存",
            Board::Power => "功耗",
        }
    }
}

/// 指标标识。`Core` 是逐核频率，运行时按实际核心数展开。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MetricId {
    CpuActive,
    CpuFreq,
    CpuScaled,
    GpuActive,
    GpuFreq,
    GpuScaled,
    CpuPower,
    GpuPower,
    AnePower,
    RamPower,
    PackagePower,
    SysPower,
    CpuTemp,
    GpuTemp,
    MemUsed,
    MemRatio,
    SwapUsed,
    Fan(u8),
    Core(usize),
}

/// 指标定义。code 是交易所风格的六位代码，界面各处统一用它标识品种。
pub struct MetricDef {
    pub id: MetricId,
    pub code: &'static str,
    pub name: &'static str,
    pub unit: Unit,
    /// 小数位数。
    pub precision: usize,
    pub board: Board,
    /// 告警阈值，超过即产生逐笔成交提示。
    pub alarm: Option<f64>,
}

/// 指数条固定的八格，顺序即显示顺序。
pub const INDEX_BAR: [MetricId; 8] = [
    MetricId::CpuActive,
    MetricId::CpuFreq,
    MetricId::GpuActive,
    MetricId::GpuFreq,
    MetricId::MemRatio,
    MetricId::CpuTemp,
    MetricId::PackagePower,
    MetricId::GpuTemp,
];

/// 自选列表的静态部分，逐核频率由 `dynamic` 在运行时补齐。
const STATIC: &[MetricDef] = &[
    MetricDef { id: MetricId::CpuActive, code: "000001", name: "CPU 活跃", unit: Unit::Percent, precision: 1, board: Board::Cpu, alarm: Some(88.0) },
    MetricDef { id: MetricId::CpuFreq, code: "000002", name: "CPU 频率", unit: Unit::MHz, precision: 0, board: Board::Cpu, alarm: None },
    MetricDef { id: MetricId::CpuScaled, code: "000003", name: "CPU 频宽", unit: Unit::Percent, precision: 1, board: Board::Cpu, alarm: None },
    MetricDef { id: MetricId::CpuPower, code: "000004", name: "CPU 功耗", unit: Unit::Watt, precision: 2, board: Board::Cpu, alarm: None },
    MetricDef { id: MetricId::CpuTemp, code: "000005", name: "市场温度(CPU)", unit: Unit::Celsius, precision: 1, board: Board::Cpu, alarm: Some(82.0) },
    MetricDef { id: MetricId::GpuActive, code: "000101", name: "GPU 活跃", unit: Unit::Percent, precision: 1, board: Board::Gpu, alarm: Some(92.0) },
    MetricDef { id: MetricId::GpuFreq, code: "000102", name: "GPU 频率", unit: Unit::MHz, precision: 0, board: Board::Gpu, alarm: None },
    MetricDef { id: MetricId::GpuScaled, code: "000103", name: "GPU 频宽", unit: Unit::Percent, precision: 1, board: Board::Gpu, alarm: None },
    MetricDef { id: MetricId::GpuPower, code: "000104", name: "GPU 功耗", unit: Unit::Watt, precision: 2, board: Board::Gpu, alarm: None },
    MetricDef { id: MetricId::GpuTemp, code: "000105", name: "市场温度(GPU)", unit: Unit::Celsius, precision: 1, board: Board::Gpu, alarm: Some(82.0) },
    MetricDef { id: MetricId::MemUsed, code: "000201", name: "内存已用", unit: Unit::GB, precision: 1, board: Board::Mem, alarm: None },
    MetricDef { id: MetricId::MemRatio, code: "000202", name: "内存占比", unit: Unit::Percent, precision: 1, board: Board::Mem, alarm: Some(90.0) },
    MetricDef { id: MetricId::SwapUsed, code: "000203", name: "交换内存", unit: Unit::GB, precision: 2, board: Board::Mem, alarm: None },
    MetricDef { id: MetricId::RamPower, code: "000204", name: "内存功耗", unit: Unit::Watt, precision: 2, board: Board::Mem, alarm: None },
    MetricDef { id: MetricId::PackagePower, code: "000301", name: "封装功耗", unit: Unit::Watt, precision: 2, board: Board::Power, alarm: None },
    MetricDef { id: MetricId::SysPower, code: "000302", name: "整机功耗", unit: Unit::Watt, precision: 2, board: Board::Power, alarm: None },
    MetricDef { id: MetricId::AnePower, code: "000303", name: "神经引擎", unit: Unit::Watt, precision: 2, board: Board::Power, alarm: None },
];

pub fn def(id: MetricId) -> &'static MetricDef {
    match id {
        MetricId::CpuActive => &STATIC[0],
        MetricId::CpuFreq => &STATIC[1],
        MetricId::CpuScaled => &STATIC[2],
        MetricId::CpuPower => &STATIC[3],
        MetricId::CpuTemp => &STATIC[4],
        MetricId::GpuActive => &STATIC[5],
        MetricId::GpuFreq => &STATIC[6],
        MetricId::GpuScaled => &STATIC[7],
        MetricId::GpuPower => &STATIC[8],
        MetricId::GpuTemp => &STATIC[9],
        MetricId::MemUsed => &STATIC[10],
        MetricId::MemRatio => &STATIC[11],
        MetricId::SwapUsed => &STATIC[12],
        MetricId::RamPower => &STATIC[13],
        MetricId::PackagePower => &STATIC[14],
        MetricId::SysPower => &STATIC[15],
        MetricId::AnePower => &STATIC[16],
        MetricId::Fan(_) => &FAN_DEF,
        MetricId::Core(_) => &CORE_DEF,
    }
}

/// 动态指标共用一份定义，名称与代码由 `Catalog` 按序号补齐。
static CORE_DEF: MetricDef = MetricDef {
    id: MetricId::Core(0), code: "0000xx", name: "核心频率",
    unit: Unit::MHz, precision: 0, board: Board::Cpu, alarm: None,
};
static FAN_DEF: MetricDef = MetricDef {
    id: MetricId::Fan(0), code: "0004xx", name: "风扇转速",
    unit: Unit::Rpm, precision: 0, board: Board::Power, alarm: None,
};

/// 该指标的成交量口径。
///
/// 系统指标不像股票那样天然带成交量，这里只给找得到流量语义的配一个：
/// 占用率配对应功耗（产能对能耗），功耗配对应占用率（互为对照），
/// 内存类配换页速率（内存压力直接驱动换页）。
/// 频率、温度、逐核、风扇没有流量概念，返回 None，副图留空——不硬凑。
pub fn volume_of(id: MetricId, s: &Snapshot) -> Option<f64> {
    Some(match id {
        MetricId::CpuActive | MetricId::CpuScaled => return power(s, s.power.cpu),
        MetricId::GpuActive | MetricId::GpuScaled => return gpu(s, s.power.gpu),
        MetricId::CpuPower => s.cpu.active_ratio * 100.0,
        MetricId::GpuPower => s.gpu.active_ratio * 100.0,
        MetricId::PackagePower | MetricId::SysPower => {
            (s.cpu.active_ratio + s.gpu.active_ratio) * 50.0
        }
        MetricId::MemUsed
        | MetricId::MemRatio
        | MetricId::SwapUsed
        | MetricId::RamPower => s.faults_per_sec,
        _ => return None,
    })
}

/// 平台拿不到 GPU 读数时返回 None，界面显示占位符而不是 0。
fn gpu(s: &Snapshot, v: f64) -> Option<f64> {
    if s.has_gpu { Some(v) } else { None }
}

/// 平台拿不到功耗读数时返回 None。RAPL 在容器与虚拟机里通常不可读。
fn power(s: &Snapshot, v: f64) -> Option<f64> {
    if s.has_power { Some(v) } else { None }
}

/// 取一次采样中该指标的读数。平台给不出时返回 None，界面显示占位符。
pub fn value(id: MetricId, s: &Snapshot) -> Option<f64> {
    Some(match id {
        MetricId::CpuActive => s.cpu.active_ratio * 100.0,
        MetricId::CpuFreq => s.cpu.freq_mhz,
        MetricId::CpuScaled => s.cpu.scaled_ratio * 100.0,
        MetricId::CpuPower => return power(s, s.power.cpu),
        MetricId::CpuTemp => return s.temp.cpu,
        MetricId::GpuActive => return gpu(s, s.gpu.active_ratio * 100.0),
        MetricId::GpuFreq => return gpu(s, s.gpu.freq_mhz),
        MetricId::GpuScaled => return gpu(s, s.gpu.scaled_ratio * 100.0),
        MetricId::GpuPower => return gpu(s, s.power.gpu),
        MetricId::GpuTemp => return s.temp.gpu,
        MetricId::MemUsed => s.mem_used_gb(),
        MetricId::MemRatio => s.mem.ratio() * 100.0,
        MetricId::SwapUsed => s.swap_used_gb(),
        MetricId::RamPower => return power(s, s.power.ram),
        MetricId::PackagePower => return power(s, s.power.package),
        MetricId::SysPower => return power(s, s.power.sys),
        MetricId::AnePower => return power(s, s.power.ane),
        MetricId::Fan(i) => return s.fans.get(i as usize).map(|f| f64::from(f.rpm)),
        MetricId::Core(i) => return s.cpu.cores.get(i).map(|c| c.freq_mhz),
    })
}

/// 运行时目录：静态指标加上按实际硬件展开的逐核与风扇。
#[derive(Clone, Debug)]
pub struct Catalog {
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: MetricId,
    pub code: String,
    pub name: String,
    pub unit: Unit,
    pub precision: usize,
    pub board: Board,
}

impl Catalog {
    pub fn build(s: &Snapshot) -> Self {
        let mut entries: Vec<Entry> = STATIC.iter().map(|d| Entry {
            id: d.id,
            code: d.code.to_string(),
            name: d.name.to_string(),
            unit: d.unit,
            precision: d.precision,
            board: d.board,
        }).collect();

        for (i, c) in s.cpu.cores.iter().enumerate() {
            entries.push(Entry {
                id: MetricId::Core(i),
                code: format!("{:06}", 100_000 + i),
                name: format!("{} 频率", c.label),
                unit: Unit::MHz,
                precision: 0,
                board: Board::Cpu,
            });
        }

        for i in 0..s.fans.len() {
            entries.push(Entry {
                id: MetricId::Fan(i as u8),
                code: format!("{:06}", 400_000 + i),
                name: format!("风扇 {}", i),
                unit: Unit::Rpm,
                precision: 0,
                board: Board::Power,
            });
        }

        Self { entries }
    }

    pub fn find(&self, id: MetricId) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// 按精度与量纲把数值格式化成带后缀的字符串。
pub fn format(v: f64, unit: Unit, precision: usize) -> String {
    format!("{:.*}{}", precision, v, unit.suffix())
}
