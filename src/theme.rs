//! 视觉系统，取 TradingView Lightweight Charts 的暗色默认样式。
//!
//! 涨跌配色沿用它的 #26A69A 与 #EF5350，背景、网格、坐标轴文字同样照搬，
//! 于是终端里的行情与看惯的图表保持同一套观感。
//! A 股习惯的红涨绿跌按 C 切换，切换只对调 up 与 down。

use ratatui::style::Color;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub up: Color,
    pub down: Color,
    pub bg: Color,
    pub panel: Color,
    pub line: Color,
    pub grid: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub alert: Color,
    pub crosshair: Color,
    pub ma: [Color; 3],
    pub intl: bool,
}

/// Lightweight Charts 暗色主题的涨跌色。
const TV_UP: Color = Color::Rgb(38, 166, 154);
const TV_DOWN: Color = Color::Rgb(239, 83, 80);

impl Theme {
    /// `intl` 为假时红涨绿跌，为真时沿用 TradingView 的绿涨红跌。
    pub fn new(intl: bool) -> Self {
        let (up, down) = if intl { (TV_UP, TV_DOWN) } else { (TV_DOWN, TV_UP) };
        Self {
            up,
            down,
            // layout.backgroundColor
            bg: Color::Rgb(19, 23, 34),
            panel: Color::Rgb(23, 27, 39),
            // priceScale 与 timeScale 的 borderColor
            line: Color::Rgb(43, 43, 67),
            // grid.vertLines / horzLines
            grid: Color::Rgb(30, 34, 45),
            // layout.textColor
            text: Color::Rgb(209, 212, 220),
            dim: Color::Rgb(120, 123, 134),
            accent: Color::Rgb(41, 98, 255),
            alert: Color::Rgb(255, 152, 0),
            // crosshair.color
            crosshair: Color::Rgb(117, 134, 150),
            ma: [
                Color::Rgb(41, 98, 255),
                Color::Rgb(255, 109, 0),
                Color::Rgb(171, 71, 188),
            ],
            intl,
        }
    }

    /// 按涨跌方向取色。
    pub fn of(&self, up: bool) -> Color {
        if up { self.up } else { self.down }
    }
}
