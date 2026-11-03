//! tick 到 K 线的聚合，以及均线。
//!
//! 采样是等间隔的 tick，K 线周期是它的整数倍。周期小于采样间隔两倍时
//! 会退化成只有一条线的伪 K 线，切换周期时由 `min_period` 挡住。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Candle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// 成交量。系统指标没有成交，这里用上下文切换数或采样活跃量代替。
    pub volume: f64,
    pub started: Instant,
}

impl Candle {
    pub fn new(v: f64, vol: f64, at: Instant) -> Self {
        Self { open: v, high: v, low: v, close: v, volume: vol, started: at }
    }

    pub fn is_up(&self) -> bool {
        self.close >= self.open
    }
}

/// 一条指标的时间序列：既保留原始 tick，也维护聚合好的 K 线。
pub struct Series {
    period: Duration,
    candles: VecDeque<Candle>,
    cap: usize,
    /// 最新价与本序列的今开，用于涨跌与涨跌幅。
    last: Option<f64>,
    open: Option<f64>,
    high: f64,
    low: f64,
}

impl Series {
    pub fn new(period: Duration, cap: usize) -> Self {
        Self {
            period,
            candles: VecDeque::with_capacity(cap),
            cap,
            last: None,
            open: None,
            high: f64::MIN,
            low: f64::MAX,
        }
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    /// 改变聚合周期，已有 K 线按新周期重建。
    pub fn set_period(&mut self, period: Duration) {
        if self.period == period {
            return;
        }
        self.period = period;
        // 旧 K 线的分辨率低于新周期，重建会得到错误的 OHLC，直接清空重新累积。
        self.candles.clear();
    }

    /// 送入一个采样点。
    pub fn push(&mut self, value: f64, volume: f64, at: Instant) {
        if self.open.is_none() {
            self.open = Some(value);
        }
        self.last = Some(value);
        self.high = self.high.max(value);
        self.low = self.low.min(value);

        match self.candles.back_mut() {
            Some(c) if at.duration_since(c.started) < self.period => {
                c.high = c.high.max(value);
                c.low = c.low.min(value);
                c.close = value;
                c.volume += volume;
            }
            _ => {
                if self.candles.len() == self.cap {
                    self.candles.pop_front();
                }
                self.candles.push_back(Candle::new(value, volume, at));
            }
        }
    }

    pub fn candles(&self) -> &VecDeque<Candle> {
        &self.candles
    }

    pub fn last(&self) -> Option<f64> {
        self.last
    }

    /// 相对今开的涨跌。
    pub fn change(&self) -> Option<f64> {
        match (self.last, self.open) {
            (Some(l), Some(o)) => Some(l - o),
            _ => None,
        }
    }

    /// 涨跌幅百分比。分母为零的指标（如温度起点为零）返回 None。
    pub fn change_pct(&self) -> Option<f64> {
        match (self.last, self.open) {
            (Some(l), Some(o)) if o.abs() > f64::EPSILON => Some((l - o) / o.abs() * 100.0),
            _ => None,
        }
    }

    pub fn high(&self) -> Option<f64> {
        if self.high == f64::MIN { None } else { Some(self.high) }
    }

    pub fn low(&self) -> Option<f64> {
        if self.low == f64::MAX { None } else { Some(self.low) }
    }

    /// n 日均线的收盘价序列，长度与 K 线一致，前 n-1 根为 None。
    pub fn ma(&self, n: usize) -> Vec<Option<f64>> {
        let closes: Vec<f64> = self.candles.iter().map(|c| c.close).collect();
        (0..closes.len())
            .map(|i| {
                if i + 1 < n {
                    None
                } else {
                    let w = &closes[i + 1 - n..=i];
                    Some(w.iter().sum::<f64>() / n as f64)
                }
            })
            .collect()
    }
}

/// K 线可选项。周期必须不低于采样间隔的两倍。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Period(pub Duration, pub &'static str);

pub const PERIODS: [Period; 5] = [
    Period(Duration::from_secs(2), "2秒K"),
    Period(Duration::from_secs(5), "5秒K"),
    Period(Duration::from_secs(10), "10秒K"),
    Period(Duration::from_secs(30), "30秒K"),
    Period(Duration::from_secs(60), "1分K"),
];

/// 采样间隔对应的最小可用 K 线周期下标。
pub fn min_period_index(interval: Duration) -> usize {
    let floor = interval * 2;
    PERIODS.iter().position(|p| p.0 >= floor).unwrap_or(PERIODS.len() - 1)
}
