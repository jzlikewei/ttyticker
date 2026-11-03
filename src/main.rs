mod app;
mod candle;
mod gridtest;
mod catalog;
mod probe;
mod snapshot;
mod tape;
mod theme;
mod ui;

use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::{App, Focus, ProcSort};

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--once") {
        return dump_once();
    }
    if args.iter().any(|a| a == "--grid-test") {
        return gridtest::run();
    }
    if args.iter().any(|a| a == "--demo") {
        return demo();
    }
    if args.iter().any(|a| a == "--faults-probe") {
        return faults_probe();
    }

    let mut app = App::new(Duration::from_secs(1))?;

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = run(&mut term, &mut app);

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

/// 非交互模式：累积一段真实采样后渲染一帧，把带颜色的 ANSI 打到标准输出。
///
/// 用于核对渲染效果与回归截图，不进入 raw 模式，重定向到文件也能拿到完整画面。
fn dump_once() -> Result<()> {
    let interval = Duration::from_millis(250);
    let mut app = App::new(interval)?;

    // 250ms 采样、2 秒 K 线，跑满 45 秒可得到二十余根蜡烛。
    let rounds: usize = std::env::var("PT_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    for _ in 0..rounds {
        app.tick();
        std::thread::sleep(Duration::from_millis(260));
    }

    let cols: u16 = std::env::var("PT_COLS").ok().and_then(|v| v.parse().ok()).unwrap_or(140);
    let rows: u16 = std::env::var("PT_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(42);

    // 导出时可选指定品种，用来核对不同品种的副图口径。
    if let Ok(code) = std::env::var("PT_SELECT") {
        let id = app
            .catalog()
            .and_then(|c| c.entries.iter().find(|e| e.code == code).map(|e| e.id));
        if let Some(id) = id {
            app.selected = id;
        }
    }

    // 导出时可选锁定个股，用来核对个股 K 线的布局。
    // PT_LOCK_SELF 锁本进程，它的 PID 从第一帧就在，历史是满的；
    // PT_LOCK_TOP 锁当前占用最高的那个，可能是中途才启动的，历史会短。
    if std::env::var("PT_LOCK_SELF").is_ok() {
        app.chart_proc = Some(std::process::id());
    } else if std::env::var("PT_LOCK_TOP").is_ok() {
        app.proc_selected = 0;
        app.toggle_chart_proc();
    }

    let mut term = Terminal::new(ratatui::backend::TestBackend::new(cols, rows))?;
    term.draw(|f| ui::draw(f, &mut app))?;
    print!("{}", ansi_dump(term.backend().buffer()));
    Ok(())
}

/// 把 Buffer 转成 24 位色的 ANSI 文本。
fn ansi_dump(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::Color;
    let rgb = |c: Color| -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Reset => (0, 0, 0),
            _ => (128, 128, 128),
        }
    };
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            let (fr, fg_, fb) = rgb(c.fg);
            let (br, bg_, bb) = rgb(c.bg);
            out.push_str(&format!(
                "\x1b[38;2;{fr};{fg_};{fb}m\x1b[48;2;{br};{bg_};{bb}m{}",
                c.symbol()
            ));
        }
        out.push_str("\x1b[0m\r\n");
    }
    out
}

/// 缺页速率诊断：连续采样，核对数值非零、且与 CPU 占用不同步。
///
/// 成交量取自缺页速率的意义就在于它与占用率不同步；
/// 两者高度相关说明这个口径没带来新信息，不如不换。
fn faults_probe() -> Result<()> {
    let n: usize = std::env::var("PT_N").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let mut probe = probe::Probe::new()?;
    let mut cpu = Vec::with_capacity(n);
    let mut faults = Vec::with_capacity(n);
    let mut mem = Vec::with_capacity(n);

    for i in 0..n {
        let s = probe.sample(1000)?;
        cpu.push(s.cpu.active_ratio * 100.0);
        faults.push(s.faults_per_sec);
        mem.push(s.mem_used_gb());
        if (i + 1) % 10 == 0 {
            eprintln!("采样 {}/{}", i + 1, n);
        }
    }

    println!("idx,cpu_pct,faults_per_sec");
    for i in 0..n {
        println!("{i},{:.2},{:.0}", cpu[i], faults[i]);
    }

    let mean = faults.iter().sum::<f64>() / n as f64;
    let max = faults.iter().copied().fold(0.0_f64, f64::max);
    let min = faults.iter().copied().fold(f64::MAX, f64::min);
    let nonzero = faults.iter().filter(|v| **v > 0.0).count();
    let r = pearson(&cpu, &faults);

    eprintln!("--- 统计 ---");
    eprintln!("faults/s  均值 {mean:.0}  最小 {min:.0}  最大 {max:.0}");
    eprintln!("非零帧    {nonzero}/{n}");
    eprintln!("与 CPU 占用相关系数 {r:.3}");

    // 内存占用：副图的成交量口径，看它波动够不够撑起量柱。
    let mmean = mem.iter().sum::<f64>() / n as f64;
    let mmin = mem.iter().copied().fold(f64::MAX, f64::min);
    let mmax = mem.iter().copied().fold(0.0_f64, f64::max);
    let swing: Vec<f64> = mem.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let smean = swing.iter().sum::<f64>() / swing.len().max(1) as f64;
    let smax = swing.iter().copied().fold(0.0_f64, f64::max);
    eprintln!("--- 内存占用 (GB) ---");
    eprintln!("占用  均值 {mmean:.2}  最小 {mmin:.2}  最大 {mmax:.2}  跨度 {:.2}", mmax - mmin);
    eprintln!("帧间变化  均值 {smean:.3}  最大 {smax:.3}");
    eprintln!("与 CPU 占用相关系数 {:.3}", pearson(&cpu, &mem));
    Ok(())
}

/// 样例数据模式：真实采样几帧后注入多属主的进程，验证席位列显示。
///
/// 覆盖三种情况——真实存在的用户（能查到名字）、不存在的 UID（回退成 uidNNN）、
/// 多个席位混排。真实机器上通常只有一两个属主，跑不出这种组合。
fn demo() -> Result<()> {
    use snapshot::Proc;

    let mut app = App::new(Duration::from_millis(200))?;
    for _ in 0..12 {
        app.tick();
        std::thread::sleep(Duration::from_millis(220));
    }

    let specs: [(u32, &[&str]); 4] = [
        (501, &["rustc", "cargo", "node", "python3", "clang"]),
        (0, &["kernel_task", "launchd", "WindowServer", "sshd"]),
        (502, &["docker", "postgres"]),
        (9001, &["ci-runner", "gitlab-runner", "build-agent"]),
    ];
    let mut procs = Vec::new();
    let mut pid = 1000u32;
    for (uid, names) in specs {
        for (i, n) in names.iter().enumerate() {
            pid += 7;
            procs.push(Proc {
                pid,
                name: (*n).to_string(),
                cpu_pct: (18.0 - i as f64 * 3.4).max(0.4),
                mem_bytes: 2_000_000_000u64 >> (i % 3),
                threads: 4 + i * 3,
                uid: Some(uid),
            });
        }
    }
    procs.sort_by(|a, b| b.cpu_pct.total_cmp(&a.cpu_pct));
    app.set_demo_procs(procs);

    let cols: u16 = std::env::var("PT_COLS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    let rows: u16 = std::env::var("PT_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(42);
    let mut term = Terminal::new(ratatui::backend::TestBackend::new(cols, rows))?;
    term.draw(|f| ui::draw(f, &mut app))?;
    print!("{}", ansi_dump(term.backend().buffer()));
    Ok(())
}

/// 皮尔逊相关系数。
fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    if n < 2.0 || xs.len() != ys.len() {
        return 0.0;
    }
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let cov: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let sx = xs.iter().map(|x| (x - mx).powi(2)).sum::<f64>().sqrt();
    let sy = ys.iter().map(|y| (y - my).powi(2)).sum::<f64>().sqrt();
    if sx * sy == 0.0 { 0.0 } else { cov / (sx * sy) }
}

fn run(term: &mut Tui, app: &mut App) -> Result<()> {
    loop {
        app.tick();
        term.draw(|f| ui::draw(f, app))?;

        // 100ms 一次轮询，采样节奏由采集线程自己控制。
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    on_key(app, k);
                }
            }
        }
        if !app.running {
            return Ok(());
        }
    }
}

fn on_key(app: &mut App, k: KeyEvent) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
        app.running = false;
        return;
    }

    // 搜索框吞掉按键，除退出与提交外不回落到快捷键。
    if app.searching {
        match k.code {
            KeyCode::Esc => {
                app.searching = false;
                app.search = None;
            }
            KeyCode::Enter => app.searching = false,
            KeyCode::Backspace => {
                if let Some(q) = &mut app.search {
                    q.pop();
                }
            }
            KeyCode::Char(c) => {
                app.search.get_or_insert_with(String::new).push(c);
            }
            _ => {}
        }
        return;
    }

    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => app.running = false,
        KeyCode::Char(' ') => app.paused = !app.paused,
        KeyCode::Char('c') => app.toggle_theme(),
        KeyCode::Char('m') => app.ma_mode = app.ma_mode.next(),
        KeyCode::Char('j') | KeyCode::Down => match app.focus {
            Focus::Watch => app.move_selection(1),
            Focus::Seats => app.move_seat_selection(1),
            Focus::Procs => app.move_proc_selection(1),
        },
        KeyCode::Char('k') | KeyCode::Up => match app.focus {
            Focus::Watch => app.move_selection(-1),
            Focus::Seats => app.move_seat_selection(-1),
            Focus::Procs => app.move_proc_selection(-1),
        },
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Watch => Focus::Seats,
                Focus::Seats => Focus::Procs,
                Focus::Procs => Focus::Watch,
            };
        }
        KeyCode::Enter => match app.focus {
            // 龙虎榜里回车按席位过滤进程表，个股区回车把主图锁到该进程。
            Focus::Seats => app.toggle_seat_filter(),
            Focus::Procs => app.toggle_chart_proc(),
            Focus::Watch => {}
        },
        KeyCode::Char('/') => {
            app.searching = true;
            app.search = Some(app.search.clone().unwrap_or_default());
        }
        KeyCode::Char('s') => {
            app.proc_sort = match app.proc_sort {
                ProcSort::Cpu => ProcSort::Mem,
                ProcSort::Mem => ProcSort::Threads,
                ProcSort::Threads => ProcSort::Pid,
                ProcSort::Pid => ProcSort::Cpu,
            };
        }
        KeyCode::Char(c @ '1'..='5') => {
            app.set_period(c as usize - '1' as usize);
        }
        _ => {}
    }
}
