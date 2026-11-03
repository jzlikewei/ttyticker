//! 应用状态与事件循环。
//!
//! 采样跑在独立线程，通过 channel 把 Snapshot 交给主线程。
//! 主线程只做两件事：把新快照喂给各指标的 Series 与事件流，然后重绘。
//! 采集失败不会打断界面，最近一次成功的快照继续显示。

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::candle::{self, Series, PERIODS};
use crate::catalog::{self, Catalog, MetricId};
use crate::probe::Probe;
use crate::snapshot::Snapshot;
use crate::tape::Tape;
use crate::theme::Theme;

/// K 线保留根数，超出后丢弃最旧的。
const SERIES_CAP: usize = 240;
/// 逐笔流水保留条数。
const TAPE_CAP: usize = 200;
/// 每个进程保留的 CPU 历史长度。
///
/// 按最长的 K 线周期反推：绘图区容得下四十二根蜡烛，1 分 K 需要 42 分钟，
/// 1 秒采样就是 2520 个点，取 2700 留余量。
/// 之前设 300 只够 5 秒 K 填满，切到 10 秒 K 或更长就立刻见底——
/// 这正是主图大片留白的来源。
///
/// 内存：2700 × 8 字节 × 五百进程约 10 MB，可接受。
const PROC_HISTORY: usize = 2700;

/// 均线显示档位。终端的色带天生比 1px 均线粗十倍，
/// 三条同屏在密集区会叠成色斑，所以留一档只留快线、一档全关。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaMode {
    All,
    Fast,
    Off,
}

impl MaMode {
    pub fn next(self) -> Self {
        match self {
            MaMode::All => MaMode::Fast,
            MaMode::Fast => MaMode::Off,
            MaMode::Off => MaMode::All,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            MaMode::All => "MA5 MA10 MA20",
            MaMode::Fast => "MA5",
            MaMode::Off => "均线 关",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Watch,
    Seats,
    Procs,
}

/// 龙虎榜的排序维度。
///
/// 只有 CPU 与内存两项：席位要按 GPU 排需要 per-process 的 GPU 占用，
/// 而 macmon 的 lib 不暴露进程级数据（进程列表在 app feature 里），
/// Linux 侧更是只有 NVIDIA 的 NVML accounting 有。数据拿不到就不给这一档。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeatSort {
    Cpu,
    Mem,
}

impl SeatSort {
    pub fn next(self) -> Self {
        match self {
            SeatSort::Cpu => SeatSort::Mem,
            SeatSort::Mem => SeatSort::Cpu,
        }
    }

    /// 维度名，保持两字以内，状态栏与标题栏共用。
    pub fn title(self) -> &'static str {
        match self {
            SeatSort::Cpu => "CPU",
            SeatSort::Mem => "内存",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcSort {
    Cpu,
    Mem,
    Threads,
    Pid,
}

impl ProcSort {
    pub fn title(self) -> &'static str {
        match self {
            ProcSort::Cpu => "CPU",
            ProcSort::Mem => "内存",
            ProcSort::Threads => "线程",
            ProcSort::Pid => "PID",
        }
    }
}

pub struct App {
    rx: Receiver<Snapshot>,
    snap: Option<Snapshot>,
    catalog: Option<Catalog>,
    series: HashMap<MetricId, Series>,
    pub selected: MetricId,
    pub period_idx: usize,
    pub interval: Duration,
    pub paused: bool,
    pub theme: Theme,
    pub ma_mode: MaMode,
    pub tape: Tape,
    pub focus: Focus,
    /// 龙虎榜里选中行的下标。
    pub seat_selected: usize,
    pub seat_sort: SeatSort,
    /// 进程表的席位过滤，Some(uid) 表示只看该属主的进程。
    pub seat_filter: Option<u32>,
    pub watch_scroll: usize,
    viewport: usize,
    pub proc_scroll: usize,
    /// 进程表里选中行的下标，落在排序后的列表上。
    pub proc_selected: usize,
    /// 主图当前锁定的个股 PID，None 表示看系统指标。
    pub chart_proc: Option<u32>,
    proc_viewport: usize,
    /// 每个进程的整机 CPU 占比历史，用来画迷你走势。
    proc_history: HashMap<u32, std::collections::VecDeque<f64>>,
    pub proc_sort: ProcSort,
    /// 上一帧各进程的整机 CPU 占比，用于算涨跌。
    prev_proc_cpu: HashMap<u32, f64>,
    /// 本帧相对上一帧的占用变化。
    proc_delta: HashMap<u32, f64>,
    pub search: Option<String>,
    /// 正在接收搜索输入，此时按键优先喂给搜索框。
    pub searching: bool,
    pub started: Instant,
    pub samples: u64,
    pub running: bool,
    pub chip: String,
    pub model: String,
    pub cores: usize,
}

impl App {
    pub fn new(interval: Duration) -> Result<Self> {
        let (info_rx, rx) = spawn_probe(interval);
        let info = info_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow::anyhow!("采集器启动超时"))?
            .map_err(|e| anyhow::anyhow!(e))
            .context("无法初始化采集器")?;
        let chip = info.chip;
        let model = info.model;
        let cores = info.cores;

        // 默认 5 秒 K：2 秒周期把每次占用跳变都记成一根长实体，曲线像噪音；
        // 聚合窗口拉到 5 秒后形状才读得出结构。仍受采样间隔两倍的下限约束。
        const DEFAULT_PERIOD: usize = 1;
        let period_idx = candle::min_period_index(interval).max(DEFAULT_PERIOD);
        Ok(Self {
            rx,
            snap: None,
            catalog: None,
            series: HashMap::new(),
            selected: MetricId::CpuActive,
            period_idx,
            interval,
            paused: false,
            theme: Theme::new(false),
            ma_mode: MaMode::All,
            tape: Tape::new(TAPE_CAP),
            focus: Focus::Watch,
            seat_selected: 0,
            seat_sort: SeatSort::Cpu,
            seat_filter: None,
            watch_scroll: 0,
            viewport: 1,
            proc_scroll: 0,
            proc_selected: 0,
            chart_proc: None,
            proc_viewport: 1,
            proc_history: HashMap::new(),
            proc_sort: ProcSort::Cpu,
            prev_proc_cpu: HashMap::new(),
            proc_delta: HashMap::new(),
            search: None,
            searching: false,
            started: Instant::now(),
            samples: 0,
            running: true,
            chip,
            model,
            cores,
        })
    }

    /// 取走通道里最新的快照。暂停时丢弃，行情与营收一并冻结。
    pub fn tick(&mut self) {
        let mut latest = None;
        while let Ok(s) = self.rx.try_recv() {
            latest = Some(s);
        }
        let Some(s) = latest else { return };
        if self.paused {
            self.snap = Some(s);
            return;
        }

        // 指标目录只在首次和硬件变化时重建。
        let known = self.catalog.as_ref().is_some_and(|c| c.len() > 0);
        let cat = if known { self.catalog.clone().unwrap() } else { Catalog::build(&s) };

        // 每个指标喂一根 tick，成交量按品种各自的口径取；
        // 没有口径的记 NaN，渲染时整块跳过而不是画成零。
        for e in &cat.entries {
            if let Some(v) = catalog::value(e.id, &s) {
                let volume = catalog::volume_of(e.id, &s).unwrap_or(f64::NAN);
                self.series
                    .entry(e.id)
                    .or_insert_with(|| Series::new(PERIODS[self.period_idx].0, SERIES_CAP))
                    .push(v, volume, s.at);
            }
        }

        // 进程涨跌：本帧减上一帧，新进程没有基准，记为持平。
        self.proc_delta = s.procs.iter()
            .filter_map(|p| self.prev_proc_cpu.get(&p.pid).map(|prev| (p.pid, p.cpu_pct - prev)))
            .collect();
        self.prev_proc_cpu = s.procs.iter().map(|p| (p.pid, p.cpu_pct)).collect();

        // 每个进程留一段 CPU 历史供迷你走势使用，并清掉已退市的条目。
        for p in &s.procs {
            let h = self.proc_history.entry(p.pid).or_default();
            if h.len() == PROC_HISTORY {
                h.pop_front();
            }
            h.push_back(p.cpu_pct);
        }
        let alive: std::collections::HashSet<u32> = s.procs.iter().map(|p| p.pid).collect();
        self.proc_history.retain(|pid, _| alive.contains(pid));

        self.tape.scan(&s, &cat);
        self.catalog = Some(cat);
        self.snap = Some(s);
        self.samples += 1;
    }

    pub fn snap(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    pub fn catalog(&self) -> Option<&Catalog> {
        self.catalog.as_ref()
    }

    pub fn series(&self, id: MetricId) -> Option<&Series> {
        self.series.get(&id)
    }

    pub fn period(&self) -> candle::Period {
        PERIODS[self.period_idx]
    }

    /// 切换 K 线周期，所有序列一起重建。
    pub fn set_period(&mut self, idx: usize) {
        let floor = candle::min_period_index(self.interval);
        let idx = idx.max(floor);
        if idx == self.period_idx {
            return;
        }
        self.period_idx = idx;
        let p = PERIODS[idx].0;
        for s in self.series.values_mut() {
            s.set_period(p);
        }
        if let Some(snap) = self.snap.clone() {
            self.replay(&snap);
        }
    }

    /// 换周期后用当前快照的现值重新起一根，避免主图空白。
    fn replay(&mut self, s: &Snapshot) {
        if let Some(cat) = &self.catalog {
            for e in &cat.entries {
                if let (Some(v), Some(ser)) = (catalog::value(e.id, s), self.series.get_mut(&e.id)) {
                    let volume = catalog::volume_of(e.id, s).unwrap_or(f64::NAN);
                    ser.push(v, volume, s.at);
                }
            }
        }
    }

    /// 在自选列表里上下移动选中品种。
    pub fn move_selection(&mut self, delta: isize) {
        let Some(cat) = &self.catalog else { return };
        if cat.entries.is_empty() {
            return;
        }
        let cur = cat.entries.iter().position(|e| e.id == self.selected).unwrap_or(0) as isize;
        let n = cat.entries.len() as isize;
        let next = ((cur + delta) % n + n) % n;
        self.selected = cat.entries[next as usize].id;
        // 主动切品种就是想看别的，主图的个股锁定跟着解除。
        self.chart_proc = None;
        self.keep_selection_visible();
    }

    /// 选中项滚出视野时把滚动位置拉回来。viewport 由界面每帧回填。
    pub fn watch_viewport(&self) -> usize {
        self.viewport
    }

    pub fn set_watch_viewport(&mut self, h: usize) {
        self.viewport = h.max(1);
        self.keep_selection_visible();
    }

    fn keep_selection_visible(&mut self) {
        let idx = match &self.catalog {
            Some(cat) => cat.entries.iter().position(|e| e.id == self.selected),
            None => None,
        };
        let Some(idx) = idx else { return };
        let vp = self.viewport.max(1);
        if idx < self.watch_scroll {
            self.watch_scroll = idx;
        } else if idx >= self.watch_scroll + vp {
            self.watch_scroll = idx + 1 - vp;
        }
    }

    pub fn toggle_theme(&mut self) {
        self.theme = Theme::new(!self.theme.intl);
    }

    /// 龙虎榜的数据源，采集层给的原始顺序（按 CPU）。
    pub fn seats(&self) -> &[crate::snapshot::Seat] {
        self.snap.as_ref().map(|s| s.seats.as_slice()).unwrap_or(&[])
    }

    /// 按当前排序维度重排的席位。
    pub fn seats_sorted(&self) -> Vec<&crate::snapshot::Seat> {
        let mut v: Vec<&crate::snapshot::Seat> = self.seats().iter().collect();
        match self.seat_sort {
            SeatSort::Cpu => v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
            SeatSort::Mem => v.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes)),
        }
        v
    }

    /// 龙虎榜最多展示的席位数量。
    pub const SEAT_ROWS: usize = 10;

    /// 龙虎榜上下键移动选中行，只在可见的十条之内循环。
    ///
    /// 行号必须对着 `seats_sorted()` 算——界面渲染的是那份重排后的列表，
    /// 拿原始顺序按同一个行号取会选到另一个人。
    pub fn move_seat_selection(&mut self, delta: isize) {
        let n = self.seats_sorted().len().min(Self::SEAT_ROWS);
        if n == 0 {
            return;
        }
        let cur = self.seat_selected.min(n - 1) as isize;
        let n = n as isize;
        self.seat_selected = (((cur + delta) % n + n) % n) as usize;
    }

    /// 回车在龙虎榜切换席位过滤：选中则只看该属主的进程，再按一次放开。
    pub fn toggle_seat_filter(&mut self) {
        if self.seat_filter.take().is_some() {
            self.proc_selected = 0;
            self.proc_scroll = 0;
            return;
        }
        // 同样对着重排后的列表取，否则按 S 换过排序之后会过滤错人。
        let uid = self.seats_sorted().get(self.seat_selected).map(|s| s.uid);
        self.seat_filter = uid;
        self.proc_selected = 0;
        self.proc_scroll = 0;
    }

    /// 当前生效的过滤与锁定，逐层退出时按这个顺序拆。
    ///
    /// 从最临时的往外拆：搜索 → 个股锁定 → 席位过滤。
    /// 三层都没有时返回 false，调用方据此决定是否退出程序。
    pub fn pop_layer(&mut self) -> bool {
        if self.search.take().is_some() {
            return true;
        }
        if self.chart_proc.take().is_some() {
            return true;
        }
        if self.seat_filter.take().is_some() {
            self.proc_selected = 0;
            self.proc_scroll = 0;
            return true;
        }
        false
    }

    /// 注入样例进程，用于验证多用户场景下的席位显示。
    ///
    /// 真实环境里一台机器上通常只有一两个属主，样例数据能一次覆盖
    /// 「真实用户名」「未知 UID 回退」「多席位混排」三种情况。
    pub fn set_demo_procs(&mut self, procs: Vec<crate::snapshot::Proc>) {
        let seats = crate::probe::aggregate_seats(&procs);
        if let Some(s) = &mut self.snap {
            s.procs = procs;
            s.seats = seats;
        }
    }

    /// 某个进程的 CPU 历史，用于表格里的迷你走势。
    pub fn proc_history(&self, pid: u32) -> Option<&std::collections::VecDeque<f64>> {
        self.proc_history.get(&pid)
    }

    /// 主图锁定的个股：返回名称与从 tick 历史临时聚合出的 K 线。
    ///
    /// 进程历史只存采样值不存时间戳，聚合时按采样间隔倒推出一串递增时刻；
    /// 时间必须递增，Series 用 duration_since 判断新蜡烛，倒流会 panic。
    pub fn chart_proc_series(&self) -> Option<(String, u32, Series)> {
        let pid = self.chart_proc?;
        let ticks = self.proc_history.get(&pid)?;
        if ticks.is_empty() {
            return None;
        }
        let name = self
            .snap()?
            .procs
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("PID {pid}"));

        let n = ticks.len();
        let back = self.interval * (n as u32).saturating_sub(1);
        let base = Instant::now().checked_sub(back).unwrap_or_else(Instant::now);
        let mut s = Series::new(self.period().0, SERIES_CAP);
        for (i, v) in ticks.iter().enumerate() {
            // 进程历史只记了占用率，没有对应的流量量，副图留空。
            s.push(*v, f64::NAN, base + self.interval * i as u32);
        }
        Some((name, pid, s))
    }

    /// 回车在个股区切换主图锁定：锁上当前选中行，再按一次解锁。
    pub fn toggle_chart_proc(&mut self) {
        if self.chart_proc.take().is_some() {
            return;
        }
        let pid = {
            let procs = self.sorted_procs();
            procs.get(self.proc_selected).map(|p| p.pid)
        };
        self.chart_proc = pid;
    }

    /// 进程表的选中行随上下键移动，并保持落在可视区内。
    pub fn move_proc_selection(&mut self, delta: isize) {
        let n = self.sorted_procs().len();
        if n == 0 {
            return;
        }
        let cur = (self.proc_selected.min(n - 1)) as isize;
        let n = n as isize;
        self.proc_selected = (((cur + delta) % n + n) % n) as usize;

        let vp = self.proc_viewport.max(1);
        if self.proc_selected < self.proc_scroll {
            self.proc_scroll = self.proc_selected;
        } else if self.proc_selected >= self.proc_scroll + vp {
            self.proc_scroll = self.proc_selected + 1 - vp;
        }
    }

    /// 表格可视行数由界面回填。
    pub fn set_proc_viewport(&mut self, h: usize) {
        self.proc_viewport = h.max(1);
    }

    /// 进程相对上一帧的占用变化，新进程返回 None。
    pub fn proc_delta(&self, pid: u32) -> Option<f64> {
        self.proc_delta.get(&pid).copied()
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// 进程表按当前排序列出，受搜索过滤。
    pub fn sorted_procs(&self) -> Vec<&crate::snapshot::Proc> {
        let Some(s) = &self.snap else { return Vec::new() };
        let mut v: Vec<&crate::snapshot::Proc> = s.procs.iter()
            .filter(|p| match &self.search {
                Some(q) if !q.is_empty() => {
                    let q = q.to_lowercase();
                    p.name.to_lowercase().contains(&q) || p.pid.to_string().contains(&q)
                }
                _ => true,
            })
            // 席位过滤与搜索是叠加关系：先按席位收窄，再按关键词找。
            .filter(|p| match self.seat_filter {
                Some(uid) => p.uid == Some(uid),
                None => true,
            })
            .collect();
        match self.proc_sort {
            ProcSort::Cpu => v.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct)),
            ProcSort::Mem => v.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes)),
            ProcSort::Threads => v.sort_by(|a, b| b.threads.cmp(&a.threads)),
            ProcSort::Pid => v.sort_by(|a, b| a.pid.cmp(&b.pid)),
        }
        v
    }
}

/// 采样线程首次启动时回传的硬件信息。
pub struct ProbeInfo {
    pub chip: String,
    pub model: String,
    pub cores: usize,
}

/// 启动采样线程。
///
/// macmon 的 Sampler 内含 CFDictionary 与 IOReport 订阅裸指针，不是 Send，
/// 只能在线程内部创建与使用，所以这里把 Probe::new 也放进线程，
/// 启动结果通过一次性 channel 回传主线程。
fn spawn_probe(
    interval: Duration,
) -> (Receiver<Result<ProbeInfo, String>>, Receiver<Snapshot>) {
    let (info_tx, info_rx) = mpsc::channel();
    let (snap_tx, snap_rx) = mpsc::channel();
    let ms = interval.as_millis() as u32;

    std::thread::Builder::new()
        .name("probe".into())
        .spawn(move || {
            let mut probe = match Probe::new() {
                Ok(p) => p,
                Err(e) => {
                    let _ = info_tx.send(Err(format!("{e:#}")));
                    return;
                }
            };
            let info = ProbeInfo {
                chip: probe.chip_name().to_string(),
                model: probe.mac_model().to_string(),
                cores: probe.core_count(),
            };
            if info_tx.send(Ok(info)).is_err() {
                return;
            }

            loop {
                match probe.sample(ms) {
                    Ok(s) => {
                        if snap_tx.send(s).is_err() {
                            break;
                        }
                    }
                    // 单次采样失败不终止线程，下一轮继续。
                    Err(_) => std::thread::sleep(interval),
                }
            }
        })
        .expect("spawn probe thread");

    (info_rx, snap_rx)
}
