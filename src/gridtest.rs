//! 栅格化探针：测 Canvas 在 Braille 下把图元画成几格。
//!
//! 实体宽度必须落在整数个 braille cell 上才显示成实心块。
//! 这个规则推算不出来——Rectangle 的 width 与实际覆盖的点数不是直觉关系，
//! 直接测一次比反复猜快。

use anyhow::Result;
use ratatui::Terminal;
use ratatui::style::Color;
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Rectangle};
use ratatui::widgets::Widget;

pub fn run() -> Result<()> {
    let cols = 44u16;
    let rows = 3u16;
    let mut term = Terminal::new(ratatui::backend::TestBackend::new(cols, rows))?;

    term.draw(|f| {
        let area = f.area();
        Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([0.0, (cols * 2) as f64])
            .y_bounds([0.0, 3.0])
            .paint(|ctx| {
                ctx.draw(&Rectangle { x: 4.0, y: 0.0, width: 2.0, height: 3.0, color: Color::Red });
                ctx.draw(&Rectangle { x: 12.0, y: 0.0, width: 1.0, height: 3.0, color: Color::Red });
                ctx.draw(&Rectangle { x: 18.0, y: 0.0, width: 3.0, height: 3.0, color: Color::Red });
                ctx.draw(&CanvasLine { x1: 28.0, y1: 0.0, x2: 28.0, y2: 3.0, color: Color::Red });
                ctx.draw(&CanvasLine { x1: 35.0, y1: 0.0, x2: 35.0, y2: 3.0, color: Color::Red });
            })
            .render(area, f.buffer_mut());
    })?;

    let buf = term.backend().buffer();
    for y in 0..rows {
        let mut line = String::new();
        for x in 0..cols {
            let sym = buf[(x, y)].symbol();
            line.push(if sym.trim().is_empty() { '.' } else { '#' });
        }
        println!("{line}");
    }
    println!("起点 4/12/18/28/35，宽度 2/1/3/线/线，grid 点数 {}", cols * 2);
    Ok(())
}