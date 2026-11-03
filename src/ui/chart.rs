//! K 线主图，走 ratatui 的 Canvas，几何完全照搬 tickrs。
//!
//! Canvas 给一个浮点逻辑坐标系，图元丢进去由它栅格化到字符网格。
//! 每根蜡烛占四个逻辑单位：实体占逻辑 1 到 3（宽 2），影线落在 2.0，
//! 两侧各留一个单位当间隙。
//!
//! 这组参数是为 Braille 调的——Canvas 默认 marker 就是它，
//! 每格两列四行，实体那两个逻辑单位正好落在同一格之内，影线视觉上居中。
//! Block 每格只有一个点，照搬会让影线落到实体右边缘，所以另给一组参数。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Rectangle};
use ratatui::widgets::Widget;

use crate::app::MaMode;
use crate::candle::{Candle, Series};
use crate::theme::Theme;

/// 右侧价格轴宽度。
const AXIS_W: u16 = 9;
/// 价格刻度的行间隔。
const GRID_ROWS: usize = 4;

/// Canvas 的栅格化 marker，默认 Braille，可用 PT_MARKER 覆盖。
fn marker() -> Marker {
    match std::env::var("PT_MARKER").as_deref() {
        Ok("block") => Marker::Block,
        Ok("dot") => Marker::Dot,
        Ok("bar") => Marker::Bar,
        Ok("halfblock") => Marker::HalfBlock,
        _ => Marker::Braille,
    }
}

/// 一套几何参数，与 marker 的分辨率绑定。
#[derive(Clone, Copy)]
struct Geometry {
    /// 每根蜡烛占的逻辑单位数，两端各留一个单位作间隙。
    slot: f64,
    /// 实体起点与宽度。
    body_x: f64,
    body_w: f64,
    /// 影线所在的逻辑横坐标，落在实体正中。
    wick_x: f64,
    /// 每个单元格的水平点数，用来算可视蜡烛根数。
    dots_x: usize,
    /// 能否在 Canvas 里画网格。Block 会把细线图元栅格化成整行满格字符。
    grid: bool,
}

/// 按 marker 取几何。
///
/// Braille 那组是 tickrs 的原值：实体两个逻辑单位落在一格之内，影线居中，
/// 网格线也能画成点阵细线。Block 那组把实体加宽到三个单位、影线放在 2.5，
/// 映射到实体的中间那一格，同时放弃网格线。
fn geometry() -> Geometry {
    match marker() {
        // tickrs 的原值，实测正确。探针（--grid-test）确认：
        // Rectangle 的 width 是闭区间，width=2 覆盖三个点，Line 占一个点，
        // 一个逻辑单位对应一个 braille 点。
        //
        // 于是实体覆盖点 base+1..base+3，影线在 base+2 正是这三个点的正中；
        // base 是 4 的倍数即偶数，整根蜡烛的落点不会随位置漂移。
        Marker::Braille => Geometry {
            slot: 4.0, body_x: 1.0, body_w: 2.0, wick_x: 2.0, dots_x: 2, grid: true,
        },
        // Block 一个逻辑单位就是一格，width=1 覆盖两格，实体是两格实心；
        // 影线取中间偏右那一格。网格线放弃：Block 会把细线图元画成整行满格字符。
        _ => Geometry {
            slot: 5.0, body_x: 1.0, body_w: 1.0, wick_x: 2.0, dots_x: 1, grid: false,
        },
    }
}

fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, fg: Color, bg: Color) {
    buf.set_string(x, y, s, Style::default().fg(fg).bg(bg));
}

fn put(buf: &mut Buffer, x: u16, y: u16, sym: &str, fg: Color, bg: Color) {
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_symbol(sym);
        c.set_fg(fg);
        c.set_bg(bg);
    }
}

/// 纵轴范围：可见蜡烛的极值外加一成余量，让影线不贴边。
fn bounds(view: &[&Candle]) -> (f64, f64) {
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for c in view {
        lo = lo.min(c.low);
        hi = hi.max(c.high);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0);
    }
    if (hi - lo).abs() < f64::EPSILON {
        hi = lo + 1.0;
    }
    let pad = (hi - lo) * 0.08;
    (lo - pad, hi + pad)
}

/// 主图：网格、K 线、均线与右侧价格轴。
pub fn render_canvas(
    area: Rect,
    buf: &mut Buffer,
    s: &Series,
    t: &Theme,
    precision: usize,
    ma: MaMode,
) {
    if area.height < 3 || area.width < AXIS_W + 8 {
        return;
    }
    let cs = s.candles();
    if cs.is_empty() {
        return;
    }

    let g = geometry();
    let plot = Rect { width: area.width - AXIS_W, ..area };
    // slots 是绘图区能容纳的蜡烛根数，与实际有多少根无关。
    //
    // x_bounds 必须按容量给，不能按实际根数：数据少的时候 Canvas 会把少量蜡烛
    // 拉伸铺满整个宽度，一根能撑到五六个 cell 宽。容量固定之后蜡烛宽度恒定，
    // 数据不足时左侧留白，新数据从右边长出来，与行情软件一致。
    let slots = ((plot.width as usize) * g.dots_x / g.slot as usize).max(1);
    let start = cs.len().saturating_sub(slots);
    let view: Vec<(f64, f64, f64, f64)> = cs
        .iter()
        .skip(start)
        .map(|c| (c.open, c.high, c.low, c.close))
        .collect();
    if view.is_empty() {
        return;
    }
    let refs: Vec<&Candle> = cs.iter().skip(start).collect();
    let (lo, hi) = bounds(&refs);

    let (periods, colors): (Vec<usize>, Vec<Color>) = match ma {
        MaMode::All => (vec![20, 10, 5], vec![t.ma[2], t.ma[1], t.ma[0]]),
        MaMode::Fast => (vec![5], vec![t.ma[0]]),
        MaMode::Off => (vec![], vec![]),
    };
    let mas: Vec<Vec<(f64, f64)>> = periods
        .iter()
        .map(|n| {
            s.ma(*n)
                .iter()
                .skip(start)
                .enumerate()
                .filter_map(|(i, v)| v.map(|v| (i as f64 * g.slot + g.wick_x, v)))
                .collect()
        })
        .collect();

    let n = view.len();
    // x_bounds 必须精确等于 Canvas 的水平点数，否则映射比例不是 1:1，
    // 实体那两个点会错开半个格、被劈进两个 cell 显示成虚线。
    let grid_w = plot.width as f64 * g.dots_x as f64;
    // 右对齐：最新一根蜡烛的右边界贴住绘图区右端。
    let offset = (grid_w - n as f64 * g.slot).max(0.0);
    // 一个字符行对应的逻辑单位。实体不足一行会被栅格化丢掉，
    // 只剩一根孤立影线，十字星和窄实体都会中招，用它兜底。
    let row_unit = (hi - lo) / plot.height.max(1) as f64;
    let (bg, grid, up, down) = (t.bg, t.grid, t.up, t.down);

    if std::env::var("PT_DEBUG_CHART").is_ok() {
        eprintln!(
            "[chart] plot.width={} dots_x={} grid_w={grid_w} slots={slots} start={start} n={n} slot={} offset={offset}",
            plot.width, g.dots_x, g.slot
        );
    }

    Canvas::default()
        .background_color(bg)
        .marker(marker())
        .x_bounds([0.0, grid_w])
        .y_bounds([lo, hi])
        .paint(move |ctx| {
            if g.grid {
                let rows = 6;
                for i in 0..=rows {
                    let y = lo + (hi - lo) * i as f64 / rows as f64;
                    ctx.draw(&CanvasLine {
                        x1: 0.0, y1: y, x2: grid_w, y2: y, color: grid,
                    });
                }
                ctx.layer();
            }
            for (i, c) in view.iter().enumerate() {
                let (open, high, low, close) = *c;
                let base = offset + i as f64 * g.slot;
                let color = if close >= open { up } else { down };
                let bottom = open.min(close);
                let height = (open.max(close) - bottom).max(row_unit);
                ctx.draw(&Rectangle {
                    x: base + g.body_x, y: bottom, width: g.body_w, height, color,
                });
                ctx.draw(&CanvasLine {
                    x1: base + g.wick_x, y1: low, x2: base + g.wick_x, y2: bottom, color,
                });
                ctx.draw(&CanvasLine {
                    x1: base + g.wick_x, y1: bottom + height, x2: base + g.wick_x, y2: high, color,
                });
            }
            for (line, color) in mas.iter().zip(colors.iter()) {
                for w in line.windows(2) {
                    let ((x1, y1), (x2, y2)) = (w[0], w[1]);
                    // 均线在 mas 里按 0 起算，这里补上右对齐偏移。
                    ctx.draw(&CanvasLine {
                        x1: x1 + offset, y1, x2: x2 + offset, y2, color: *color,
                    });
                }
            }
        })
        .render(plot, buf);

    let axis_x = plot.x + plot.width;
    let h = plot.height as usize;
    for row in (0..h).step_by(GRID_ROWS) {
        let v = hi - (row as f64 / (h.max(2) - 1) as f64) * (hi - lo);
        let label = format!("{:>width$.prec$}", v, width = AXIS_W as usize - 1, prec = precision);
        text(buf, axis_x, plot.y + row as u16, &label, t.dim, t.bg);
    }
    if let Some(last) = s.last() {
        let r = ((hi - last) / (hi - lo)).clamp(0.0, 1.0);
        let row = ((r * (h.max(2) - 1) as f64).round() as usize).min(h - 1) as u16;
        let color = s.change().map(|d| t.of(d >= 0.0)).unwrap_or(t.dim);
        let label = format!("{:>width$.prec$}", last, width = AXIS_W as usize - 1, prec = precision);
        let y = plot.y + row;
        for x in axis_x..area.x + area.width {
            put(buf, x, y, " ", t.bg, color);
        }
        text(buf, axis_x, y, &label, t.bg, color);
    }
}

/// 成交量柱，横坐标与蜡烛严格对齐。
pub fn render_volume_canvas(area: Rect, buf: &mut Buffer, s: &Series, t: &Theme) {
    if area.height == 0 || area.width < AXIS_W + 8 {
        return;
    }
    let cs = s.candles();
    if cs.is_empty() {
        return;
    }
    let g = geometry();
    let plot = Rect { width: area.width - AXIS_W, ..area };
    let slots = ((plot.width as usize) * g.dots_x / g.slot as usize).max(1);
    let start = cs.len().saturating_sub(slots);
    let view: Vec<(bool, f64)> = cs.iter().skip(start).map(|c| (c.is_up(), c.volume)).collect();
    // 这个品种没配成交量口径，整块跳过而不是画一排零高度的柱。
    if view.iter().any(|v| v.1.is_nan()) {
        return;
    }
    // 成交量是内存占用这类存量，零不是有意义的基线：
    // 17 到 19 GB 按峰值归一化后高度只在 0.90 到 1.00 之间，量柱会拉成一条平线。
    // 改用可见区间的最小值当基线，把跨度铺满整个量柱区。
    let lo = view.iter().map(|v| v.1).fold(f64::MAX, f64::min);
    let hi = view.iter().map(|v| v.1).fold(f64::MIN, f64::max);
    let span = hi - lo;
    if !span.is_finite() || span <= f64::EPSILON {
        return;
    }
    let n = view.len();
    let grid_w = plot.width as f64 * g.dots_x as f64;
    let offset = (grid_w - n as f64 * g.slot).max(0.0);
    let (bg, up, down) = (t.bg, t.up, t.down);
    Canvas::default()
        .background_color(bg)
        .marker(marker())
        .x_bounds([0.0, grid_w])
        .y_bounds([0.0, 1.0])
        .paint(move |ctx| {
            for (i, (is_up, vol)) in view.iter().enumerate() {
                ctx.draw(&Rectangle {
                    x: offset + i as f64 * g.slot + g.body_x,
                    y: 0.0,
                    width: g.body_w,
                    height: ((vol - lo) / span).clamp(0.02, 1.0),
                    color: if *is_up { up } else { down },
                });
            }
        })
        .render(plot, buf);
}