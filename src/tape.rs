//! 逐笔成交：把采样之间的差异翻译成行情事件流水。
//!
//! 事件有三个来源——阈值告警、指标跳变、进程启停。
//! 每条事件带方向，界面据此着色，涨为红、跌为绿。

use std::collections::VecDeque;

use crate::catalog::{self, MetricId};
use crate::snapshot::Snapshot;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Up,
    Down,
    Alert,
}

#[derive(Clone, Debug)]
pub struct TapeEvent {
    pub at: std::time::Instant,
    pub code: String,
    pub text: String,
    pub side: Side,
}

/// 事件流，保留最近若干条。
pub struct Tape {
    events: VecDeque<TapeEvent>,
    cap: usize,
    /// 上一次的进程 PID 集合，用于识别启停。
    seen_pids: std::collections::HashSet<u32>,
    /// 上一次的指标读数，用于识别跳变。
    prev: std::collections::HashMap<MetricId, f64>,
    /// 已经报过警的指标，避免持续超阈值刷屏。
    firing: std::collections::HashSet<MetricId>,
}

impl Tape {
    pub fn new(cap: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(cap),
            cap,
            seen_pids: Default::default(),
            prev: Default::default(),
            firing: Default::default(),
        }
    }

    pub fn events(&self) -> &VecDeque<TapeEvent> {
        &self.events
    }

    pub fn push(&mut self, code: String, text: String, side: Side) {
        if self.events.len() == self.cap {
            self.events.pop_back();
        }
        self.events.push_front(TapeEvent { at: std::time::Instant::now(), code, text, side });
    }

    /// 对比上一帧，产出这一帧的全部事件。
    pub fn scan(&mut self, s: &Snapshot, cat: &catalog::Catalog) {
        self.scan_alarms(s, cat);
        self.scan_jumps(s, cat);
        self.scan_procs(s);
    }

    /// 阈值告警。越过阈值报一次，回落到阈值以下八成才解除，可反复触发。
    fn scan_alarms(&mut self, s: &Snapshot, cat: &catalog::Catalog) {
        for d in cat.entries.iter() {
            let Some(limit) = catalog::def(d.id).alarm else { continue };
            let Some(v) = catalog::value(d.id, s) else { continue };
            if v >= limit {
                if self.firing.insert(d.id) {
                    let txt = format!("{} 触及 {}，现报 {}",
                        d.name, catalog::format(limit, d.unit, d.precision),
                        catalog::format(v, d.unit, d.precision));
                    self.push(d.code.clone(), txt, Side::Alert);
                }
            } else if v < limit * 0.8 {
                self.firing.remove(&d.id);
            }
        }
    }

    /// 频率与活跃度的显著跳变，视作一笔成交。
    fn scan_jumps(&mut self, s: &Snapshot, cat: &catalog::Catalog) {
        for d in cat.entries.iter() {
            let Some(v) = catalog::value(d.id, s) else { continue };
            let Some(&prev) = self.prev.get(&d.id) else {
                self.prev.insert(d.id, v);
                continue;
            };
            self.prev.insert(d.id, v);

            // 逐核频率抖动频繁，只对整机指标报跳变。
            if matches!(d.id, MetricId::Core(_) | MetricId::Fan(_)) {
                continue;
            }
            let delta = v - prev;
            // 跳变门限按量纲取整：百分比 8 点、频率 300MHz、功耗 3W、温度 5°C。
            let gate = match d.unit {
                catalog::Unit::Percent => 8.0,
                catalog::Unit::MHz => 300.0,
                catalog::Unit::Watt => 3.0,
                catalog::Unit::Celsius => 5.0,
                catalog::Unit::GB => 2.0,
                catalog::Unit::Rpm => 800.0,
            };
            if delta.abs() < gate {
                continue;
            }
            let side = if delta > 0.0 { Side::Up } else { Side::Down };
            let sign = if delta > 0.0 { "+" } else { "" };
            let txt = format!("{} {}{}", d.name, sign, catalog::format(delta, d.unit, d.precision));
            self.push(d.code.clone(), txt, side);
        }
    }

    /// 进程启停。
    fn scan_procs(&mut self, s: &Snapshot) {
        let mut now = std::collections::HashSet::new();
        for p in &s.procs {
            now.insert(p.pid);
            if !self.seen_pids.is_empty() && !self.seen_pids.contains(&p.pid) {
                self.push(format!("{:06}", p.pid % 1_000_000), format!("{} 上市", p.name), Side::Up);
            }
        }
        if !self.seen_pids.is_empty() {
            let gone: Vec<u32> = self.seen_pids.difference(&now).copied().take(3).collect();
            for pid in gone {
                self.push(format!("{:06}", pid % 1_000_000), format!("PID {pid} 退市"), Side::Down);
            }
        }
        self.seen_pids = now;
    }
}
