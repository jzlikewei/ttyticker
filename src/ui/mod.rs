//! 界面层：全景终端布局。
//!
//! 六区同屏——标题栏、指数条、自选、主图、盘口与逐笔、进程表、状态栏。
//! 每个面板只读 App 的状态，不反向写，绘制完成后再由 App 回填视口尺寸。

pub mod chart;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::candle::PERIODS;
use crate::catalog::{self, Board, MetricId};
use crate::theme::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MIN_WIDTH: u16 = 100;

/// 按显示宽度截断，中文按两列计。
fn clip(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut acc = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + cw > w {
            break;
        }
        out.push(ch);
        acc += cw;
    }
    out
}

/// 按显示宽度左对齐补空格。
fn pad_right(s: &str, w: usize) -> String {
    let s = clip(s, w);
    let cur = UnicodeWidthStr::width(s.as_str());
    if cur >= w { s } else { format!("{}{}", s, " ".repeat(w - cur)) }
}

/// 按显示宽度右对齐补空格。
fn pad_left(s: &str, w: usize) -> String {
    let s = clip(s, w);
    let cur = UnicodeWidthStr::width(s.as_str());
    if cur >= w { s } else { format!("{}{}", " ".repeat(w - cur), s) }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < MIN_WIDTH {
        let msg = Paragraph::new(vec![
            Line::from("终端过窄").centered(),
            Line::from(format!("需要 {MIN_WIDTH} 列，当前 {} 列", area.width)).centered(),
        ])
        .style(Style::default().fg(app.theme.dim));
        f.render_widget(msg, area);
        return;
    }

    // 底部留给龙虎榜与个股列表：十条席位加边框要十一行。
    let [header, index, body, bottom, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Min(10),
        Constraint::Length(11),
        Constraint::Length(1),
    ])
    .areas(area);

    // 龙虎榜与个股列表并排：席位是「谁在占资源」，个股是「哪个进程在占」，
    // 两者是下钻关系，贴在一起选中联动才不用移动视线。
    let [tiger, procs] =
        Layout::horizontal([Constraint::Length(30), Constraint::Min(60)]).areas(bottom);

    let [watch, center, right] = Layout::horizontal([
        Constraint::Length(24),
        Constraint::Min(44),
        Constraint::Length(30),
    ])
    .areas(body);

    let [quote, chart] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(6)]).areas(center);

    let [book, tape] =
        Layout::vertical([Constraint::Length(11), Constraint::Min(5)]).areas(right);

    header_bar(f, app, header);
    index_bar(f, app, index);
    watch_list(f, app, watch);
    quote_head(f, app, quote);
    main_chart(f, app, chart);
    order_book(f, app, book);
    tape_panel(f, app, tape);
    dragon_tiger(f, app, tiger);
    proc_table(f, app, procs);
    status_bar(f, app, status);
}

fn panel<'a>(title: &'a str, t: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.line))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(t.dim),
        ))
}

/// 标题栏：品牌、硬件、采样状态、配色与时钟。
fn header_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let live = if app.paused {
        Span::styled("● 暂停", Style::default().fg(t.alert))
    } else {
        Span::styled("● 实时", Style::default().fg(t.up))
    };
    let colors = if t.intl { "绿涨红跌" } else { "红涨绿跌" };
    // 内存与 GPU 直接摆在标题栏：这两个是「一眼要知道」的状态，
    // 不该藏在左边的板块列表里翻。GPU 读不到时整段省略，不显示 0%。
    let mem = app.snap().map(|s| {
        format!(
            "内存 {:.1}G/{:.0}G",
            s.mem_used_gb(),
            s.mem.total as f64 / 1_073_741_824.0
        )
    });
    let gpu = app
        .snap()
        .filter(|s| s.has_gpu)
        .map(|s| format!("GPU {:.0}%", s.gpu.active_ratio * 100.0));
    let sep = || Span::styled("  │  ", Style::default().fg(t.line));
    let mut spans = vec![
        Span::styled(" ttyticker ", Style::default().fg(t.bg).bg(t.accent).bold()),
        Span::styled(" 老实看监控，很枯燥吧？ ", Style::default().fg(t.dim)),
        Span::styled(format!("{} · {} 核", app.chip, app.cores), Style::default().fg(t.text)),
        sep(),
        Span::styled(mem.unwrap_or_else(|| "内存 —".into()), Style::default().fg(t.text)),
    ];
    // GPU 读不到时整段省略，连分隔符一起省，不留一段空白。
    if let Some(g) = gpu {
        spans.push(sep());
        spans.push(Span::styled(g, Style::default().fg(t.text)));
    }
    spans.push(sep());
    spans.push(Span::styled(
        format!("采样 {:.1}s", app.interval.as_secs_f64()),
        Style::default().fg(t.dim),
    ));
    spans.push(Span::raw("  "));
    spans.push(live);
    spans.push(sep());
    spans.push(Span::styled(colors, Style::default().fg(t.dim)));
    let line = Line::from(spans);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(t.panel)), area);
}

/// 指数条：八格，每格名称、现价、涨跌与迷你走势。
fn index_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = panel("指数", t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cells = catalog::INDEX_BAR.len() as u16;
    let w = inner.width / cells;
    for (i, id) in catalog::INDEX_BAR.iter().enumerate() {
        let x = inner.x + i as u16 * w;
        let cw = if i == cells as usize - 1 { inner.width - i as u16 * w } else { w };
        let cell = Rect { x, y: inner.y, width: cw, height: inner.height };
        index_cell(f, app, cell, *id);
    }
}

fn index_cell(f: &mut Frame, app: &App, area: Rect, id: MetricId) {
    let t = &app.theme;
    let d = catalog::def(id);
    let Some(snap) = app.snap() else { return };
    let v = catalog::value(id, snap);
    let ser = app.series(id);

    let (price, delta) = match (v, ser) {
        (Some(v), _) => (catalog::format(v, d.unit, d.precision), ser.and_then(|s| s.change())),
        _ => ("—".to_string(), None),
    };
    let up = delta.unwrap_or(0.0) >= 0.0;
    let color = if delta.is_none() { t.dim } else { t.of(up) };

    let mut lines = vec![
        Line::from(Span::styled(d.name, Style::default().fg(t.dim))),
        Line::from(Span::styled(price, Style::default().fg(color).bold())),
    ];
    let chg = match delta {
        Some(x) => {
            let sign = if x >= 0.0 { "+" } else { "" };
            format!("{sign}{}", catalog::format(x, d.unit, d.precision))
        }
        None => "—".to_string(),
    };
    lines.push(Line::from(Span::styled(chg, Style::default().fg(color))));

    // 迷你走势用八个高度档位表现近段收盘价。
    let spark = ser.map(|s| sparkline(s, area.width as usize)).unwrap_or_default();
    if !spark.is_empty() {
        lines.push(Line::from(Span::styled(spark, Style::default().fg(color))));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// 用块元素的八档高度画迷你走势，输入任意数值序列。
fn spark<I: Iterator<Item = f64>>(vals: I, width: usize) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let v: Vec<f64> = vals.collect();
    if v.is_empty() || width == 0 {
        return String::new();
    }
    let seg = &v[v.len().saturating_sub(width)..];
    let lo = seg.iter().copied().fold(f64::MAX, f64::min);
    let hi = seg.iter().copied().fold(f64::MIN, f64::max);
    let span = if (hi - lo).abs() < f64::EPSILON { 1.0 } else { hi - lo };
    seg.iter()
        .map(|x| BLOCKS[(((x - lo) / span) * 7.0).round().clamp(0.0, 7.0) as usize])
        .collect()
}

/// 用块元素的八档高度画迷你走势。
fn sparkline(s: &crate::candle::Series, width: usize) -> String {
    let cs = s.candles();
    if cs.is_empty() || width == 0 {
        return String::new();
    }
    spark(cs.iter().map(|c| c.close), width)
}

/// 自选行情：按板块分组，带迷你走势与涨跌。
fn watch_list(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let title = match app.catalog() {
        Some(c) => format!("大盘/板块行情 {} 项", c.len()),
        None => "大盘/板块行情".to_string(),
    };
    let block = panel(&title, &t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // 每项占两行，板块标题占一行。先回填视口，再借用目录，避免可变借用冲突。
    let rows = (inner.height as usize).saturating_sub(1) / 2;
    app.set_watch_viewport(rows.max(1));

    let Some(cat) = app.catalog() else { return };

    let mut y = inner.y;
    let mut board: Option<Board> = None;
    let skip = app.watch_scroll;
    for e in cat.entries.iter().skip(skip) {
        if y + 1 >= inner.y + inner.height {
            break;
        }
        if board != Some(e.board) {
            if y >= inner.y + inner.height {
                break;
            }
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {} ", e.board.title()),
                    Style::default().fg(t.bg).bg(t.line),
                ))),
                Rect { x: inner.x, y, width: inner.width, height: 1 },
            );
            y += 1;
            board = Some(e.board);
            if y + 1 >= inner.y + inner.height {
                break;
            }
        }

        let sel = e.id == app.selected;
        let v = app.snap().and_then(|s| catalog::value(e.id, s));
        let ser = app.series(e.id);
        let delta = ser.and_then(|s| s.change());
        let color = if sel { t.accent } else { t.of(delta.unwrap_or(0.0) >= 0.0) };

        let col = inner.width.saturating_sub(8) as usize;
        let name = pad_right(&e.name, col);
        let price = match v {
            Some(v) => pad_left(&format!("{:.*}", e.precision, v), 7),
            None => pad_left("—", 7),
        };
        let name_style = if sel {
            Style::default().fg(t.bg).bg(t.accent).bold()
        } else {
            Style::default().fg(t.text)
        };

        let head = Line::from(vec![
            Span::styled(name, name_style),
            Span::styled(price, Style::default().fg(color)),
        ]);
        f.render_widget(
            Paragraph::new(head),
            Rect { x: inner.x, y, width: inner.width, height: 1 },
        );

        let chg = match delta {
            Some(x) => pad_left(&format!("{:+.*}", e.precision, x), 7),
            None => pad_left("—", 7),
        };
        let sub = Line::from(vec![
            Span::styled(pad_right(&e.code, col), Style::default().fg(t.line)),
            Span::styled(chg, Style::default().fg(color)),
        ]);
        f.render_widget(
            Paragraph::new(sub),
            Rect { x: inner.x, y: y + 1, width: inner.width, height: 1 },
        );
        y += 2;
    }
}

/// 品种头：代码、名称、现价、涨跌、今开与最高最低。
fn quote_head(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = panel("", &t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // 数据来源：主图锁定的个股优先，否则是大盘/板块里选中的指标。
    // 个股的量纲固定是整机 CPU 占比。
    let owned;
    let (code, name, unit, precision, v, ser) = match app.chart_proc_series() {
        Some((pname, pid, s)) => {
            owned = s;
            let v = app
                .snap()
                .and_then(|sn| sn.procs.iter().find(|p| p.pid == pid))
                .map(|p| p.cpu_pct);
            (format!("{pid:06}"), pname, catalog::Unit::Percent, 1usize, v, Some(&owned))
        }
        None => {
            let d = catalog::def(app.selected);
            let name = app.catalog().and_then(|c| c.find(app.selected)).map(|e| e.name.clone())
                .unwrap_or_else(|| d.name.to_string());
            let code = app.catalog().and_then(|c| c.find(app.selected)).map(|e| e.code.clone())
                .unwrap_or_else(|| d.code.to_string());
            let v = app.snap().and_then(|s| catalog::value(app.selected, s));
            (code, name, d.unit, d.precision, v, app.series(app.selected))
        }
    };
    let delta = ser.and_then(|s| s.change());
    let pct = ser.and_then(|s| s.change_pct());
    let color = t.of(delta.unwrap_or(0.0) >= 0.0);

    let price = v.map(|v| catalog::format(v, unit, precision)).unwrap_or_else(|| "—".into());
    let chg = delta.map(|x| {
        let sign = if x >= 0.0 { "+" } else { "" };
        match pct {
            Some(p) => format!("{sign}{}  {:+.2}%", catalog::format(x, unit, precision), p),
            None => format!("{sign}{}", catalog::format(x, unit, precision)),
        }
    }).unwrap_or_else(|| "—".into());

    let head = Line::from(vec![
        Span::styled(format!(" {code} "), Style::default().fg(t.dim)),
        Span::styled(name, Style::default().fg(t.text).bold()),
        Span::raw("   "),
        Span::styled(price, Style::default().fg(color).bold()),
        Span::raw("  "),
        Span::styled(chg, Style::default().fg(color)),
    ]);

    let mut sub_spans = vec![Span::styled(
        format!(" 今开 {}", ser.and_then(|s| s.candles().front().map(|c| catalog::format(c.open, unit, precision))).unwrap_or_else(|| "—".into())),
        Style::default().fg(t.dim),
    )];
    sub_spans.push(Span::styled(
        format!("  最高 {}", ser.and_then(|s| s.high()).map(|v| catalog::format(v, unit, precision)).unwrap_or_else(|| "—".into())),
        Style::default().fg(t.up),
    ));
    sub_spans.push(Span::styled(
        format!("  最低 {}", ser.and_then(|s| s.low()).map(|v| catalog::format(v, unit, precision)).unwrap_or_else(|| "—".into())),
        Style::default().fg(t.down),
    ));
    if app.chart_proc.is_some() {
        sub_spans.push(Span::styled(
            "  回车返回板块",
            Style::default().fg(t.accent),
        ));
    }

    // K 线周期页签，当前档高亮，低于采样间隔两倍的档位不可用。
    let floor = crate::candle::min_period_index(app.interval);
    let mut tabs = vec![Span::raw("  ")];
    for (i, p) in PERIODS.iter().enumerate() {
        let style = if i == app.period_idx {
            Style::default().fg(t.bg).bg(t.accent).bold()
        } else if i < floor {
            Style::default().fg(t.line)
        } else {
            Style::default().fg(t.dim)
        };
        tabs.push(Span::styled(format!(" {} ", p.1), style));
    }

    f.render_widget(Paragraph::new(vec![head, Line::from(sub_spans), Line::from(tabs)]), inner);
}

/// 主图：K 线加均线，底部一条量柱。
/// 主图区去掉边框与价格轴后的可用列数。
fn inner_w_of(area: Rect) -> u16 {
    area.width.saturating_sub(2 + 9)
}

fn main_chart(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    // 图例压在边框线上，不占用绘图区，否则会盖住蜡烛顶部。
    let legend = match app.ma_mode {
        crate::app::MaMode::All => Line::from(vec![
            Span::styled("─ MA5 ", Style::default().fg(t.ma[0])),
            Span::styled("─ MA10 ", Style::default().fg(t.ma[1])),
            Span::styled("─ MA20 ", Style::default().fg(t.ma[2])),
        ]),
        crate::app::MaMode::Fast => Line::from(Span::styled("─ MA5", Style::default().fg(t.ma[0]))),
        crate::app::MaMode::Off => Line::from(Span::styled("均线 关", Style::default().fg(t.line))),
    }
    .right_aligned();
    // 锁定个股时画它的走势，否则画选中的板块指标。
    let owned;
    let (ser, precision) = match app.chart_proc_series() {
        Some((_, _, s)) => {
            owned = s;
            (Some(&owned), 1usize)
        }
        None => {
            let p = app.catalog().and_then(|c| c.find(app.selected)).map(|e| e.precision)
                .unwrap_or(catalog::def(app.selected).precision);
            (app.series(app.selected), p)
        }
    };

    // 标题带上蜡烛根数与容量：数据没攒够时留白是正常的，让它可见比让人猜要好。
    let cap = ((inner_w_of(area) as usize) * 2 / 4).max(1);
    let have = ser.map(|s| s.candles().len()).unwrap_or(0);
    let title = if app.chart_proc.is_some() {
        format!("K 线 · 个股 {have}/{cap}")
    } else {
        format!("K 线 {have}/{cap}")
    };
    let block = panel(&title, &t).title_top(legend);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(ser) = ser else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("等待采样…", Style::default().fg(t.dim)))),
            inner,
        );
        return;
    };
    let [price, vol] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(3)]).areas(inner);
    chart::render_canvas(price, f.buffer_mut(), ser, &t, precision, app.ma_mode);
    chart::render_volume_canvas(vol, f.buffer_mut(), ser, &t);
}

/// 主力动向：当前维度下占用最高的十个进程。
fn order_book(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let d = catalog::def(app.selected);
    let title = format!("主力动向 · 按 {}", if d.board == Board::Mem { "内存" } else { "CPU" });
    let block = panel(&title, &t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let procs = app.sorted_procs();
    let peak = procs.iter().map(|p| p.cpu_pct).fold(0.0_f64, f64::max).max(1.0);
    let name_w = inner.width.saturating_sub(16) as usize;

    for (i, p) in procs.iter().take(inner.height as usize).enumerate() {
        let y = inner.y + i as u16;
        let badge = Span::styled(format!("{:>2} ", i + 1), Style::default().fg(t.accent));
        let name = pad_right(&p.name, name_w);
        let val = format!("{:>6.1}%", p.cpu_pct);

        // 热度条长度按与榜首的比值。
        let bar_len = ((p.cpu_pct / peak) * 8.0).round() as usize;
        let bar: String = "▮".repeat(bar_len.clamp(1, 8));
        let color = t.of(app.proc_delta(p.pid).unwrap_or(0.0) >= 0.0);

        let line = Line::from(vec![
            badge,
            Span::styled(name, Style::default().fg(t.text)),
            Span::styled(val, Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(color)),
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y, width: inner.width, height: 1 });
    }
}

/// 逐笔成交：事件流水，最新在上。
fn tape_panel(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = panel("逐笔成交", &t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    for (i, e) in app.tape.events().iter().take(inner.height as usize).enumerate() {
        let y = inner.y + i as u16;
        let color = match e.side {
            crate::tape::Side::Up => t.up,
            crate::tape::Side::Down => t.down,
            crate::tape::Side::Alert => t.alert,
        };
        let arrow = match e.side {
            crate::tape::Side::Up => "▲",
            crate::tape::Side::Down => "▼",
            crate::tape::Side::Alert => "!",
        };
        let secs = e.at.elapsed().as_secs();
        let text: String = e.text.chars().take(inner.width.saturating_sub(12) as usize).collect();
        let line = Line::from(vec![
            Span::styled(format!("{secs:>3}s "), Style::default().fg(t.line)),
            Span::styled(format!("{arrow} "), Style::default().fg(color)),
            Span::styled(text, Style::default().fg(color)),
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y, width: inner.width, height: 1 });
    }
}

/// 龙虎榜：资源占用前十的席位，与个股列表并排。
///
/// 席位回答「谁在占资源」，个股回答「哪个进程在占」，两者是下钻关系。
/// 回车选中席位后，下方的个股列表立刻收窄到该属主。
fn dragon_tiger(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let title = match app.seat_filter {
        Some(uid) => format!(
            "龙虎榜 · 按 {} · {}",
            app.seat_sort.title(),
            crate::probe::username(uid)
        ),
        None => format!("龙虎榜 · 按 {}", app.seat_sort.title()),
    };
    let block = panel(&title, &t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let seats = app.seats_sorted();
    if seats.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("等待采样…", Style::default().fg(t.dim)))),
            inner,
        );
        return;
    }

    let focused = app.focus == crate::app::Focus::Seats;
    for (i, s) in seats.iter().take(App::SEAT_ROWS).enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let y = inner.y + i as u16;
        // 选中行（焦点在本区）与已过滤行（正在生效的席位）用两套标记。
        let sel = focused && i == app.seat_selected;
        let active = app.seat_filter == Some(s.uid);

        let bg = if sel { t.accent } else { t.bg };
        let fg_name = if sel { t.bg } else if active { t.accent } else { t.text };
        let fg_num = if sel { t.bg } else { t.dim };

        let mem_gb = s.mem_bytes as f64 / 1_073_741_824.0;
        let line = Line::from(vec![
            Span::styled(format!("{:>2} ", i + 1), Style::default().fg(fg_num).bg(bg)),
            Span::styled(pad_right(&s.name, 10), Style::default().fg(fg_name).bg(bg)),
            Span::styled(
                pad_left(&format!("{:.1}%", s.cpu_pct), 7),
                Style::default().fg(fg_name).bg(bg),
            ),
            Span::styled(
                pad_left(&format!("{mem_gb:.1}G"), 7),
                Style::default().fg(fg_num).bg(bg),
            ),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(bg)),
            Rect { x: inner.x, y, width: inner.width, height: 1 },
        );
    }
}

/// 个股列表：全量进程表，每行一条迷你走势。
fn proc_table(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let sort = app.proc_sort.title();
    let seat = match app.seat_filter {
        Some(uid) => format!(" · 席位 {}", crate::probe::username(uid)),
        None => String::new(),
    };
    let title = match &app.search {
        Some(q) if !q.is_empty() => format!("个股列表 · {sort}{seat} · 搜索 {q}"),
        _ => format!("个股列表 · {sort}{seat}"),
    };
    let block = panel(&title, &t);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let head = Line::from(vec![
        Span::styled(
            format!("{}{}{}{}{}{}  {}",
                pad_right("代码", 8), pad_right("席位", 10), pad_right("名称", 20),
                pad_left("现价", 8), pad_left("涨跌", 10), pad_left("内存", 7), "走势"),
            Style::default().fg(t.dim),
        ),
    ]);
    f.render_widget(Paragraph::new(head), Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });

    let rows = inner.height.saturating_sub(1) as usize;
    app.set_proc_viewport(rows);
    let procs = app.sorted_procs();
    // 走势列吃掉剩余宽度。固定列合计 63 列加两列分隔。
    let spark_w = inner.width.saturating_sub(65) as usize;

    for (i, p) in procs.iter().skip(app.proc_scroll).take(rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        let delta = app.proc_delta(p.pid);
        let color = t.of(delta.unwrap_or(0.0) >= 0.0);
        // 选中行整行反色，与自选列表的选中态一致。
        let sel = app.proc_scroll + i == app.proc_selected;
        // 代码列原先用边框色，那是描边用的，当文字读不出来。
        let (fg_text, fg_dim) = if sel { (t.bg, t.bg) } else { (t.text, t.dim) };
        let row_bg = if sel { t.accent } else { t.bg };
        let spark = match app.proc_history(p.pid) {
            Some(h) => spark(h.iter().copied(), spark_w),
            None => String::new(),
        };
        // 席位列：进程属主的用户名，查不到时显示 uid 数字。
        let seat = match p.uid {
            Some(uid) => crate::probe::username(uid),
            None => "—".to_string(),
        };

        let line = Line::from(vec![
            Span::styled(pad_right(&p.pid.to_string(), 8), Style::default().fg(fg_dim).bg(row_bg)),
            Span::styled(pad_right(&seat, 10), Style::default().fg(fg_dim).bg(row_bg)),
            Span::styled(pad_right(&p.name, 20), Style::default().fg(fg_text).bg(row_bg)),
            Span::styled(format!("{:>8.1}", p.cpu_pct), Style::default().fg(color).bg(row_bg)),
            Span::styled(
                match delta {
                    Some(d) => pad_left(&format!("{:+.1}", d), 10),
                    None => pad_left("—", 10),
                },
                Style::default().fg(color).bg(row_bg),
            ),
            Span::styled(
                format!("{:>7.1}G", p.mem_bytes as f64 / 1073741824.0),
                Style::default().fg(if sel { t.bg } else { t.dim }).bg(row_bg),
            ),
            Span::styled(format!("  {spark}"), Style::default().fg(color).bg(row_bg)),
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y, width: inner.width, height: 1 });
    }
}

/// 状态栏：采样计数与按键提示。
fn status_bar(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let up = app.uptime();
    let left = format!(
        " 采样 {:.1}s  运行 {:02}:{:02}:{:02}  样本 {}  进程 {} ",
        app.interval.as_secs_f64(),
        up.as_secs() / 3600,
        (up.as_secs() % 3600) / 60,
        up.as_secs() % 60,
        app.samples,
        app.snap().map(|s| s.procs.len()).unwrap_or(0),
    );
    // 提示下一步会切到哪个区，三区循环。
    let next = match app.focus {
        crate::app::Focus::Watch => "龙虎榜",
        crate::app::Focus::Seats => "个股",
        crate::app::Focus::Procs => "板块",
    };
    // 排序键跟着焦点走，提示里带上当前维度：按下去之前就知道在切什么、
    // 现在是什么，而不是等标题栏变了才反应过来。
    let sort_hint = match app.focus {
        crate::app::Focus::Seats => format!("S 席位排序·{}", app.seat_sort.title()),
        _ => format!("S 排序·{}", app.proc_sort.title()),
    };
    let keys = format!(
        " Tab 切到{next} · ↑↓ 选行 · 回车 展开 · 空格 暂停 · 1-5 周期 · C 配色 · M 均线 · {sort_hint} · / 搜索 · Q 退出 "
    );
    // 当前生效的层：席位过滤、个股锁定、搜索。
    //
    // 这三层都能用 Esc 逐层拆掉，但前提是用户知道它们正在生效——
    // 看不见的状态等于没有出口，之前「个股一直显示某个人却不知道怎么取消」
    // 就是因为过滤生效了却没有任何提示。
    let mut layers: Vec<String> = Vec::new();
    if let Some(uid) = app.seat_filter {
        layers.push(format!("席位 {}", crate::probe::username(uid)));
    }
    if let Some(pid) = app.chart_proc {
        let name = app
            .snap()
            .and_then(|s| s.procs.iter().find(|p| p.pid == pid))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("PID{pid}"));
        layers.push(format!("个股 {name}"));
    }
    if let Some(q) = &app.search {
        if !q.is_empty() {
            layers.push(format!("搜索 {q}"));
        }
    }

    let mut spans = vec![Span::styled(left, Style::default().fg(t.text))];
    if !layers.is_empty() {
        spans.push(Span::styled(
            format!("│ {} ", layers.join(" · ")),
            Style::default().fg(t.accent).bold(),
        ));
        spans.push(Span::styled("Esc 清除 ", Style::default().fg(t.text)));
    }
    // 按键提示原先用边框色，那是给描边用的，在黑底上读不出来。
    spans.push(Span::styled(keys, Style::default().fg(t.dim)));
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(t.panel)),
        area,
    );
}
