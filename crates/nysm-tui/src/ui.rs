//! Rendering. Pure function of `App`; never performs I/O.

use nysm_core::history::HistoryPoint;
use nysm_core::raw::DiskKind;
use nysm_core::sanitize::for_terminal;
use nysm_core::snapshot::{Pressure, Snapshot};
use nysm_core::units::{self, RateUnit};
use nysm_core::{Reading, Status};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Tabs, Wrap};

use crate::app::{App, Tab};

struct Theme {
    color: bool,
    ascii: bool,
}

impl Theme {
    fn fg(&self, c: Color) -> Style {
        if self.color {
            Style::default().fg(c)
        } else {
            Style::default()
        }
    }
    fn cpu(&self) -> Style {
        self.fg(Color::Cyan)
    }
    fn mem(&self) -> Style {
        self.fg(Color::Magenta)
    }
    fn net(&self) -> Style {
        self.fg(Color::Green)
    }
    fn disk(&self) -> Style {
        self.fg(Color::Yellow)
    }
    fn up(&self) -> Style {
        self.fg(Color::Blue)
    }
    fn write(&self) -> Style {
        self.fg(Color::LightRed)
    }
    /// Secondary text: mid grey, readable on dark and light terminals
    /// (DarkGray nearly vanishes on many dark themes).
    fn dim(&self) -> Style {
        if self.color {
            Style::default().fg(Color::Indexed(245))
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }
    fn border(&self) -> Style {
        if self.color {
            Style::default().fg(Color::Indexed(240))
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }
    /// Heat colour for a 0-100 share: plain, then yellow, then red.
    fn heat(&self, pct: f64) -> Style {
        match pct {
            p if p >= 50.0 => self.fg(Color::LightRed).add_modifier(Modifier::BOLD),
            p if p >= 15.0 => self.fg(Color::Yellow),
            _ => Style::default(),
        }
    }
    fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    fn alert(&self) -> Style {
        if self.color {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }
    fn danger(&self) -> Style {
        if self.color {
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }
    fn selected(&self) -> Style {
        if self.color {
            Style::default()
                .bg(Color::Indexed(24))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        }
    }
    fn tab_selected(&self) -> Style {
        if self.color {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        }
    }
    fn borders(&self) -> symbols::border::Set<'static> {
        if self.ascii {
            symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            }
        } else {
            symbols::border::ROUNDED
        }
    }
    fn block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_set(self.borders())
            .border_style(self.border())
            .title(Span::styled(title, self.bold()))
    }
}

fn safe(s: &str) -> String {
    for_terminal(s).into_owned()
}

fn trunc(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(w.saturating_sub(1)).collect();
        o.push('~');
        o
    }
}

/// Rows needed to show `lines` wrapped at `width`.
fn wrapped_height(lines: &[Line], width: u16) -> u16 {
    let w = width.max(1) as usize;
    lines
        .iter()
        .map(|l| l.width().max(1).div_ceil(w) as u16)
        .sum()
}

fn missing<T>(r: &Reading<T>) -> String {
    format!("— ({})", r.status.label())
}

fn pressure_short(r: &Reading<Pressure>) -> String {
    match r.live() {
        Some(p) => match p.some.interval_pct {
            Some(i) => format!("{i:.1}%"),
            None => format!("avg10 {:.1}%", p.some.avg10_pct),
        },
        None => match r.status {
            Status::Unsupported => "n/a".into(),
            s => s.label().into(),
        },
    }
}

fn firing_alerts(app: &App) -> impl Iterator<Item = &nysm_core::alerts::ActiveAlert> {
    app.alerts
        .iter()
        .filter(|a| matches!(a.state, nysm_core::alerts::AlertState::Firing { .. }))
}

/// One-line summary of firing alerts, built from the rules' own values.
fn alert_line(app: &App, t: &Theme) -> Option<Line<'static>> {
    let parts: Vec<String> = firing_alerts(app)
        .map(|a| {
            let what = match &a.target {
                Some(m) => format!("{} {}", a.metric.label(), safe(m)),
                None => a.metric.label().to_string(),
            };
            let missing = matches!(
                a.state,
                nysm_core::alerts::AlertState::Firing {
                    data_missing: true,
                    ..
                }
            );
            match (a.value, missing) {
                (_, true) => format!("{what} (data missing)"),
                (Some(v), _) => format!("{what} {v:.1}{}", a.metric.unit()),
                (None, _) => what,
            }
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(Line::from(vec![
        Span::styled(" ALERT ", t.danger()),
        Span::raw(format!(" {}", parts.join(" · "))),
    ]))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = Theme {
        color: app.color,
        ascii: app.ascii,
    };
    let area = f.area();
    let Some(snap) = app.shown().cloned() else {
        f.render_widget(Paragraph::new("Collecting first sample…"), area);
        return;
    };
    if area.width < 50 || area.height < 12 {
        draw_tiny(f, area, app, &snap, &t);
        return;
    }
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, header, app, &snap, &t);
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, tab)| Line::from(format!("{} {}", i + 1, tab.title())))
        .collect();
    let idx = Tab::ALL.iter().position(|x| *x == app.tab).unwrap_or(0);
    f.render_widget(
        Tabs::new(titles)
            .select(idx)
            .style(t.dim())
            .highlight_style(t.tab_selected())
            .divider(Span::styled(if t.ascii { "|" } else { "│" }, t.border())),
        tabs,
    );
    match app.tab {
        Tab::Overview => draw_overview(f, body, app, &snap, &t),
        Tab::Processes => draw_processes(f, body, app, &snap, &t),
        Tab::Cpu => draw_cpu(f, body, app, &snap, &t),
        Tab::Memory => draw_memory(f, body, app, &snap, &t),
        Tab::Network => draw_network(f, body, app, &snap, &t),
        Tab::Disk => draw_disk(f, body, app, &snap, &t),
        Tab::Groups => draw_groups(f, body, app, &snap, &t),
    }
    draw_footer(f, footer, app, &t);
    if app.help {
        draw_help(f, area, &t);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App, s: &Snapshot, t: &Theme) {
    let host = safe(s.host.hostname.as_deref().unwrap_or("?"));
    let mut left = vec![
        Span::styled(
            nysm_core::brand::COMMAND_NAME,
            t.cpu().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::raw(host),
    ];
    if let Some(u) = s.host.uptime_s {
        left.push(Span::styled(
            format!(" · up {}", units::duration_s(u)),
            t.dim(),
        ));
    }
    if let Some(ms) = s.interval_ms {
        left.push(Span::styled(
            format!(" · {:.1}s samples", ms as f64 / 1000.0),
            t.dim(),
        ));
    }
    if app.source.starts_with("attached") {
        left.push(Span::styled(" · via service", t.dim()));
    }
    if s.host.scope != nysm_core::snapshot::MeasurementScope::Host {
        left.push(Span::styled(
            format!(" · scope: {:?}", s.host.scope).to_lowercase(),
            t.alert(),
        ));
    }
    let status = if app.paused.is_some() {
        Span::styled(" PAUSED · display frozen, collection continues ", t.alert())
    } else if app.cursor.is_some() {
        Span::styled(" TIMELINE ", t.alert())
    } else {
        Span::styled(
            if t.ascii { " LIVE " } else { " ● LIVE " },
            t.fg(Color::Green),
        )
    };
    let mut right = Vec::new();
    let firing = firing_alerts(app).count();
    if firing > 0 {
        let mark = if t.ascii { "!" } else { "⚠" };
        right.push(Span::styled(
            format!(
                " {mark} {firing} alert{} ",
                if firing == 1 { "" } else { "s" }
            ),
            t.danger(),
        ));
        right.push(Span::raw(" "));
    }
    right.push(status);
    let w = right.iter().map(|s| s.width()).sum::<usize>() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(w)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(Line::from(right)), r);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, t: &Theme) {
    let (lr, ud) = if t.ascii {
        ("Left/Right", "Up/Down")
    } else {
        ("←→", "↑↓")
    };
    let text = if app.editing_filter {
        format!("Filter: {}_   Enter apply · Esc clear", safe(&app.filter))
    } else {
        match app.tab {
            Tab::Processes => format!(
                "{ud} Enter:details *:pin c/m/d/n/P:sort t:tree /:filter Space:pause ?:help q:quit"
            ),
            Tab::Groups => {
                format!("{ud}:scroll k:kind c/m/d/n:sort 1-7:views Space:pause ?:help q:quit")
            }
            _ => format!("{lr}:timeline End:live 1-7:views Space:pause /:search ?:help q:quit"),
        }
    };
    if let Some(n) = &app.notice {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {n} "), t.alert()))),
            area,
        );
        return;
    }
    let filter = if !app.filter.is_empty() && !app.editing_filter {
        Span::styled(format!(" filter: {} ", safe(&app.filter)), t.alert())
    } else {
        Span::raw("")
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![filter, Span::styled(text, t.dim())])),
        area,
    );
}

fn draw_help(f: &mut Frame, area: Rect, t: &Theme) {
    let w = area.width.min(64);
    let h = area.height.min(22);
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let lines = [
        ("1-7, Tab", "switch view (7: containers/services/apps)"),
        ("Space / p", "pause display (collection keeps running)"),
        (
            "Left/Right, h/l",
            "move timeline cursor; End or Esc returns live",
        ),
        ("Up/Down, j/k", "select process; PgUp/PgDn, g/G jump"),
        ("Enter", "process details (exe, cwd, cgroup)"),
        ("*", "pin/unpin process: keeps its CPU/memory history"),
        ("/", "filter processes by name, PID or user"),
        ("c m d n P", "sort by CPU, memory, disk I/O, name, PID"),
        ("t", "toggle process tree"),
        ("Esc", "close details / leave timeline / clear filter"),
        ("q, Ctrl-C", "quit"),
        ("", ""),
        ("CPU %", "share of all logical cores (0-100)"),
        ("Memory used", "total - MemAvailable (cache that can be"),
        ("", "reclaimed counts as available)"),
        ("Pressure", "PSI: % of time tasks stalled, not usage"),
        ("·  in charts", "no data (warm-up, gap or unsupported)"),
    ];
    let text: Vec<Line> = lines
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<16} "), t.bold()),
                Span::raw(*v),
            ])
        })
        .collect();
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(text)
            .block(t.block(" Help · any key closes "))
            .wrap(Wrap { trim: false }),
        r,
    );
}

/// Recent samples of one metric, newest last. `None` = no data (warm-up,
/// gap or unsupported), drawn blank so it never looks like zero.
struct Series {
    vals: Vec<Option<f64>>,
    max: f64,
    /// Index in `vals` of the timeline cursor.
    cursor: Option<usize>,
}

/// The most recent `n` samples; a gap (suspend, stall) adds one blank.
fn series(
    h: &[HistoryPoint],
    n: usize,
    cursor: Option<usize>,
    get: impl Fn(&HistoryPoint) -> Option<f32>,
) -> Series {
    let len = h.len();
    let start = len.saturating_sub(n);
    let mut vals = Vec::with_capacity(n + 4);
    let (mut max, mut cur) = (0.0f64, None);
    for (i, p) in h[start..].iter().enumerate() {
        if p.gap_before && i > 0 {
            vals.push(None);
        }
        let v = get(p)
            .filter(|v| v.is_finite())
            .map(|v| (v as f64).max(0.0));
        if let Some(v) = v {
            max = max.max(v);
        }
        if cursor.is_some_and(|c| start + i + 1 + c == len) {
            cur = Some(vals.len());
        }
        vals.push(v);
    }
    if vals.len() > n {
        let d = vals.len() - n;
        vals.drain(..d);
        cur = cur.and_then(|c| c.checked_sub(d));
    }
    Series {
        vals,
        max,
        cursor: cur,
    }
}

/// A chart inside a titled box.
#[allow(clippy::too_many_arguments)]
fn boxed_chart(
    f: &mut Frame,
    area: Rect,
    t: &Theme,
    title: String,
    s: &Series,
    scale: f64,
    grid: bool,
    style: Style,
) {
    let block = t.block("").title(Span::styled(title, t.bold()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    thin_bars(f, inner, t, s, scale, grid, style);
}

/// Samples that fit a box of `area` (borders excluded).
fn boxed_samples(area: Rect) -> usize {
    area.width.saturating_sub(2) as usize
}

/// Colour for how full something is: green, yellow from 60%, red from 85%.
fn level_style(t: &Theme, pct: f64) -> Style {
    match pct {
        p if p >= 85.0 => t.fg(Color::LightRed),
        p if p >= 60.0 => t.fg(Color::Yellow),
        _ => t.fg(Color::Green),
    }
}

/// `▕██████░░░░░░▏` filled to `pct`; `None` draws an empty bar.
fn gauge(t: &Theme, pct: Option<f64>, width: usize) -> Line<'static> {
    let inner = width.saturating_sub(2);
    let filled = pct.map_or(0, |p| {
        ((p / 100.0) * inner as f64)
            .round()
            .clamp(0.0, inner as f64) as usize
    });
    let (on, off) = if t.ascii { ("=", "-") } else { ("━", "─") };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            on.repeat(filled),
            pct.map_or(t.dim(), |p| level_style(t, p).add_modifier(Modifier::BOLD)),
        ),
        Span::styled(off.repeat(inner - filled), t.border()),
        Span::raw(" "),
    ])
}

fn history_span(h: &[HistoryPoint], width: usize) -> String {
    let n = h.len().min(width);
    if n < 2 {
        return String::new();
    }
    let first = h[h.len() - n].timestamp_ms;
    let last = h[h.len() - 1].timestamp_ms;
    format!("last {}", units::duration_s((last - first) as f64 / 1000.0))
}

fn opt_rate(v: Option<f32>, unit: RateUnit) -> String {
    v.map_or("—".into(), |v| units::rate(v as f64, unit))
}

fn draw_overview(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    overview_boxes(f, area, app, s, t);
}

/// Thin vertical bars, one sample per column (`┃` full cell, `╻` half),
/// newest at the right. Zero draws nothing; a missing sample a dim dot on
/// the baseline, so the two never look alike.
/// `grid` draws a dotted line at the top of the scale (100%) where no bar
/// reaches, so a level reads against its ceiling.
fn thin_bars(
    f: &mut Frame,
    area: Rect,
    t: &Theme,
    s: &Series,
    scale: f64,
    grid: bool,
    style: Style,
) {
    let (w, rows) = (area.width as usize, area.height as usize);
    if w == 0 || rows == 0 {
        return;
    }
    let (full, half, none) = if t.ascii {
        ("|", ".", ".")
    } else {
        ("┃", "╻", "·")
    };
    let pad = w.saturating_sub(s.vals.len());
    let halves = rows * 2;
    let cursor_cell = s.cursor.map(|c| c + pad);
    let lines: Vec<Line> = (0..rows)
        .map(|row| {
            let from_bottom = rows - 1 - row;
            let spans: Vec<Span> = (0..w)
                .map(|col| {
                    let v = col
                        .checked_sub(pad)
                        .and_then(|i| s.vals.get(i).copied())
                        .flatten();
                    let in_range = col >= pad;
                    let gridline = if grid && row == 0 {
                        (if t.ascii { "." } else { "┈" }, t.border())
                    } else {
                        (" ", style)
                    };
                    let (sym, st) = match v {
                        Some(v) => {
                            let l = if scale > 0.0 {
                                ((v / scale) * halves as f64).round() as usize
                            } else {
                                0
                            };
                            let l = if v > 0.0 { l.clamp(1, halves) } else { 0 };
                            match l.saturating_sub(from_bottom * 2).min(2) {
                                2 => (full, style),
                                1 => (half, style),
                                _ => gridline,
                            }
                        }
                        None if in_range && from_bottom == 0 => (none, t.border()),
                        None => gridline,
                    };
                    if Some(col) == cursor_cell {
                        (
                            if sym == " " { "│" } else { sym },
                            st.add_modifier(Modifier::REVERSED),
                        )
                    } else {
                        (sym, st)
                    }
                })
                .map(|(sym, st)| Span::styled(sym, st))
                .collect();
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

/// One resource for the panel layouts.
struct Panel {
    title: &'static str,
    style: Style,
    value: Vec<Span<'static>>,
    detail: Vec<Span<'static>>,
    /// Fill level for a gauge, when the resource has one.
    pct: Option<Option<f64>>,
    /// One chart, or two (download/upload, read/write) on one scale.
    charts: Vec<(Series, Style)>,
    scale: f64,
}

fn panels(app: &App, s: &Snapshot, t: &Theme, h: &[HistoryPoint], n: usize) -> Vec<Panel> {
    let cursor = app.cursor;
    let point = app.cursor_point().copied();
    let b = |v: u64| units::bytes(v as f64);
    let mut out = Vec::new();

    let cpu = match &point {
        Some(p) => p.cpu_pct.map(f64::from),
        None => s.cpu.usage.live().map(|c| c.total_pct),
    };
    let cores = s.cpu.logical_cores.value.unwrap_or(0);
    let detail = match (&point, s.cpu.usage.live()) {
        (None, Some(c)) => format!(
            "user {:.0}% · system {:.0}% · iowait {:.0}% · load {} on {cores} cores",
            c.user_pct + c.nice_pct,
            c.system_pct,
            c.iowait_pct,
            s.cpu
                .load
                .live()
                .map_or("—".into(), |l| format!("{:.2}", l.one)),
        ),
        (None, None) => missing(&s.cpu.usage),
        _ => String::new(),
    };
    out.push(Panel {
        title: "CPU",
        style: t.cpu(),
        value: vec![Span::styled(
            cpu.map_or("—".into(), |c| format!("{c:.1}%")),
            t.cpu().add_modifier(Modifier::BOLD),
        )],
        detail: vec![Span::styled(detail, t.dim())],
        pct: Some(cpu),
        charts: vec![(series(h, n, cursor, |p| p.cpu_pct), t.cpu())],
        scale: 100.0,
    });

    let live_mem = s.memory.usage.live();
    let mem = match &point {
        Some(p) => p.mem_used_pct.map(f64::from),
        None => live_mem.map(|m| m.used_pct),
    };
    let (value, detail) = match (&point, live_mem) {
        (None, Some(m)) => (
            format!(
                "{} / {}  ({:.0}%)",
                b(m.used_bytes),
                b(m.total_bytes),
                m.used_pct
            ),
            format!(
                "{} available{}",
                b(m.available_bytes),
                s.memory.swap.live().map_or(String::new(), |w| {
                    if w.total_bytes == 0 {
                        " · no swap".into()
                    } else {
                        format!(" · swap {} / {}", b(w.used_bytes), b(w.total_bytes))
                    }
                })
            ),
        ),
        (None, None) => ("—".into(), missing(&s.memory.usage)),
        (Some(_), _) => (
            mem.map_or("—".into(), |m| format!("{m:.1}% used")),
            String::new(),
        ),
    };
    out.push(Panel {
        title: "Memory",
        style: t.mem(),
        value: vec![Span::styled(value, t.mem().add_modifier(Modifier::BOLD))],
        detail: vec![Span::styled(detail, t.dim())],
        pct: Some(mem),
        charts: vec![(series(h, n, cursor, |p| p.mem_used_pct), t.mem())],
        scale: 100.0,
    });

    let (rx, tx) = match &point {
        Some(p) => (p.net_rx_bytes_per_s, p.net_tx_bytes_per_s),
        None => {
            let v = s.network.total.live();
            (
                v.map(|v| v.rx_bytes_per_s as f32),
                v.map(|v| v.tx_bytes_per_s as f32),
            )
        }
    };
    let rs = series(h, n, cursor, |p| p.net_rx_bytes_per_s);
    let ts = series(h, n, cursor, |p| p.net_tx_bytes_per_s);
    let peak = rs.max.max(ts.max).max(1.0);
    let (dn, upa) = if t.ascii {
        ("down ", "up ")
    } else {
        ("↓ ", "↑ ")
    };
    out.push(Panel {
        title: "Network",
        style: t.net(),
        value: vec![
            Span::styled(
                format!("{dn}{}", opt_rate(rx, app.rate)),
                t.net().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                format!("{upa}{}", opt_rate(tx, app.rate)),
                t.up().add_modifier(Modifier::BOLD),
            ),
        ],
        detail: vec![Span::styled(
            if point.is_none() && !s.network.total.is_available() {
                missing(&s.network.total)
            } else {
                format!(
                    "download / upload · chart peak {}",
                    units::rate(peak, app.rate)
                )
            },
            t.dim(),
        )],
        pct: None,
        charts: vec![(rs, t.net()), (ts, t.up())],
        scale: peak,
    });

    let (rd, wr) = match &point {
        Some(p) => (p.disk_read_bytes_per_s, p.disk_write_bytes_per_s),
        None => {
            let d = s.storage.total_io.live();
            (
                d.map(|d| d.read_bytes_per_s as f32),
                d.map(|d| d.write_bytes_per_s as f32),
            )
        }
    };
    let rs = series(h, n, cursor, |p| p.disk_read_bytes_per_s);
    let ws = series(h, n, cursor, |p| p.disk_write_bytes_per_s);
    let peak = rs.max.max(ws.max).max(1.0);
    let root = s
        .storage
        .filesystems
        .value
        .as_ref()
        .and_then(|v| v.iter().find(|fs| fs.mount_point == "/"));
    let mut detail = Vec::new();
    if point.is_none() {
        if let Some(fs) = root {
            detail.push(format!(
                "/ {:.0}% full ({} free)",
                fs.used_pct,
                b(fs.available_bytes)
            ));
        }
        if let Some(busy) = s.storage.total_io.live().and_then(|d| d.busy_pct) {
            detail.push(format!("busy {busy:.0}%"));
        }
    }
    detail.push(format!("chart peak {}", units::rate(peak, RateUnit::Bytes)));
    out.push(Panel {
        title: "Disk",
        style: t.disk(),
        value: vec![
            Span::styled(
                format!("read {}", opt_rate(rd, RateUnit::Bytes)),
                t.disk().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                format!("write {}", opt_rate(wr, RateUnit::Bytes)),
                t.write().add_modifier(Modifier::BOLD),
            ),
        ],
        detail: vec![Span::styled(detail.join(" · "), t.dim())],
        pct: None,
        charts: vec![(rs, t.disk()), (ws, t.write())],
        scale: peak,
    });
    out
}

/// Charts of a panel stacked in `area` (two charts split it).
fn panel_charts(f: &mut Frame, area: Rect, t: &Theme, p: &Panel) {
    // A gutter on the left labels the scale or the series.
    let gw = 5u16.min(area.width);
    let [gutter, area] =
        Layout::horizontal([Constraint::Length(gw), Constraint::Min(0)]).areas(area);
    let label = |f: &mut Frame, y: u16, text: &str, st: Style| {
        if y < gutter.y + gutter.height {
            f.render_widget(
                Paragraph::new(Span::styled(format!("{text:>4}"), st)),
                Rect {
                    y,
                    height: 1,
                    ..gutter
                },
            );
        }
    };
    match p.charts.as_slice() {
        [(a, sa)] => {
            // The 100% line gets its own row above the bars, so a high but
            // not full level (89%) still shows a gap below it.
            label(f, gutter.y, "100%", t.dim());
            label(f, gutter.y + gutter.height.saturating_sub(1), "0%", t.dim());
            let line = Rect { height: 1, ..area };
            f.render_widget(
                Paragraph::new(Span::styled(
                    (if t.ascii { "." } else { "┈" }).repeat(area.width as usize),
                    t.border(),
                )),
                line,
            );
            let bars = below(area, 1);
            if bars.height >= 4 {
                label(f, bars.y + bars.height / 2, "50%", t.dim());
            }
            thin_bars(f, bars, t, a, p.scale, false, *sa);
        }
        [(a, sa), (b, sb)] => {
            let top = area.height.div_ceil(2);
            let [x, y] =
                Layout::vertical([Constraint::Length(top), Constraint::Min(0)]).areas(area);
            let names = if p.title == "Network" {
                if t.ascii {
                    ["down", "up"]
                } else {
                    ["↓", "↑"]
                }
            } else {
                ["R", "W"]
            };
            label(f, x.y + x.height.saturating_sub(1), names[0], *sa);
            label(f, y.y + y.height.saturating_sub(1), names[1], *sb);
            thin_bars(f, x, t, a, p.scale, false, *sa);
            thin_bars(f, y, t, b, p.scale, false, *sb);
        }
        _ => {}
    }
}

/// A bordered box per resource, two per row, then the process table.
fn overview_boxes(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    let h = app.history().to_vec();
    let two = area.width >= 80;
    let box_w = if two { area.width / 2 } else { area.width };
    let n = box_w.saturating_sub(2 + 5) as usize; // borders, scale gutter
    let ps = panels(app, s, t, &h, n);
    // Taller boxes on tall terminals give the charts finer steps.
    // Boxes shrink on short terminals so the process table keeps ~8 rows.
    let rows = if two { 2 } else { 4 };
    let want = if area.height >= 44 { 11u16 } else { 9 };
    let box_h = want.min(area.height.saturating_sub(1 + 8) / rows).max(5);
    let [head, grid, procs] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(box_h * rows),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(overview_head(app, s, t, &h, n)), head);
    for (i, p) in ps.iter().enumerate() {
        let (cx, cy) = if two { (i % 2, i / 2) } else { (0, i) };
        let r = Rect {
            x: grid.x + cx as u16 * box_w,
            y: grid.y + cy as u16 * box_h,
            width: if two && cx == 1 {
                grid.width - box_w
            } else {
                box_w
            },
            height: box_h,
        };
        let inner = card(f, r, t, p.title, p.style);
        let mut lines = vec![Line::from(p.value.clone()), Line::from(p.detail.clone())];
        // The gauge repeats the chart's latest level: drop it when space is short.
        if let Some(pct) = p.pct
            && inner.height >= 7
        {
            lines.push(gauge(t, pct, inner.width as usize));
        }
        let text_h = lines.len() as u16;
        f.render_widget(
            Paragraph::new(lines),
            Rect {
                height: text_h,
                ..inner
            },
        );
        panel_charts(f, below(inner, text_h), t, p);
    }
    if procs.height >= 5 {
        process_table(f, procs, app, s, t, false);
    }
}

/// A titled card with rounded borders; returns its inner area.
fn card(f: &mut Frame, area: Rect, t: &Theme, title: &str, style: Style) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(t.borders())
        .border_style(t.border())
        .title(Span::styled(
            format!(" {title} "),
            style.add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// The area under the first `skip` lines.
fn below(area: Rect, skip: u16) -> Rect {
    let skip = skip.min(area.height);
    Rect {
        y: area.y + skip,
        height: area.height - skip,
        ..area
    }
}

/// Status line above the overview: timeline cursor, alerts or the clock.
fn overview_head(
    app: &App,
    s: &Snapshot,
    t: &Theme,
    h: &[HistoryPoint],
    n: usize,
) -> Line<'static> {
    let interval_ms = s.interval_ms.unwrap_or(1000) as i64;
    let event_at = |ts: i64| {
        app.alert_events
            .iter()
            .rev()
            .find(|e| e.timestamp_ms > ts - interval_ms && e.timestamp_ms <= ts + interval_ms / 2)
    };
    match app.cursor_point() {
        Some(p) => {
            let mut spans = vec![Span::styled(
                format!(
                    " timeline: {} ({}s ago) ",
                    clock(p.timestamp_ms),
                    (s.timestamp_ms - p.timestamp_ms) / 1000
                ),
                t.alert(),
            )];
            match event_at(p.timestamp_ms) {
                Some(e) => spans.push(Span::styled(
                    format!("  event: {} {}", safe(&e.rule), safe(&e.describe())),
                    t.bold(),
                )),
                None if p.gap_before => spans.push(Span::styled(
                    "  gap before this sample (suspend or stall)",
                    t.dim(),
                )),
                None => spans.push(Span::styled(
                    "  values below are from the cursor; Esc returns to live",
                    t.dim(),
                )),
            }
            Line::from(spans)
        }
        None => alert_line(app, t).unwrap_or_else(|| {
            let last = app.alert_events.last().map_or(String::new(), |e| {
                format!(
                    " · last alert event {}s ago: {}",
                    (s.timestamp_ms - e.timestamp_ms).max(0) / 1000,
                    safe(&e.rule)
                )
            });
            let span = history_span(h, n);
            let span = if span.is_empty() {
                String::new()
            } else {
                format!(" · charts: {span}")
            };
            Line::from(Span::styled(
                format!("now {}{span}{last}", clock(s.timestamp_ms)),
                t.dim(),
            ))
        }),
    }
}

fn clock(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    #[cfg(unix)]
    {
        let tt = s;
        if let Some((h, m, sec)) = libc_time::local(tt) {
            return format!("{h:02}:{m:02}:{sec:02}");
        }
    }
    let d = s.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}Z", d / 3600, d / 60 % 60, d % 60)
}

#[cfg(unix)]
mod libc_time {
    /// Local hour/minute/second for a Unix timestamp.
    pub fn local(t: i64) -> Option<(i32, i32, i32)> {
        // `as _` lets the platform's time_t width be inferred (musl/glibc).
        let t = t as _;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: valid pointers; localtime_r is thread-safe.
        let ok = !unsafe { libc::localtime_r(&t, &mut tm) }.is_null();
        ok.then_some((tm.tm_hour, tm.tm_min, tm.tm_sec))
    }
}

fn process_table(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme, full: bool) {
    let Some(table) = &s.processes else {
        f.render_widget(
            Paragraph::new("process list not available").style(t.dim()),
            area,
        );
        return;
    };
    let rows = app.rows();
    let title = format!(
        " Processes · {} shown of {} · sort {}{} · CPU% of all {} cores ",
        rows.len(),
        table.entries.len(),
        app.sort.label(),
        if app.tree && app.filter.is_empty() {
            " · tree"
        } else {
            ""
        },
        table.logical_cores
    );
    let block = t.block("").title(Span::styled(title, t.bold()));
    let inner_h = area.height.saturating_sub(3) as usize; // borders + header
    app.page_rows = inner_h;
    let sel = app.selected_index(&rows);
    // Keep selection visible.
    if let Some(sel) = sel {
        if sel < app.scroll {
            app.scroll = sel;
        } else if sel >= app.scroll + inner_h {
            app.scroll = sel + 1 - inner_h.max(1);
        }
    }
    app.scroll = app.scroll.min(rows.len().saturating_sub(inner_h));
    let wide = area.width >= 100;
    let total_mem = s
        .memory
        .usage
        .live()
        .map(|m| m.total_bytes as f64)
        .filter(|t| *t > 0.0);
    // (title, width, right-aligned)
    let cols: &[(&str, u16, bool)] = if wide {
        &[
            ("PID", 8, true),
            ("USER", 10, false),
            ("CPU%", 6, true),
            ("MEM%", 6, true),
            ("RSS", 9, true),
            ("DISK R/s", 10, true),
            ("DISK W/s", 10, true),
            ("THR", 4, true),
            ("S", 1, false),
            ("NAME", 0, false),
        ]
    } else {
        &[
            ("PID", 8, true),
            ("CPU%", 6, true),
            ("MEM%", 6, true),
            ("RSS", 9, true),
            ("S", 1, false),
            ("NAME", 0, false),
        ]
    };
    let align = |text: String, right: bool| {
        let l = Line::from(text);
        if right { l.right_aligned() } else { l }
    };
    let header = Row::new(cols.iter().map(|(h, _, r)| {
        Cell::from(align(h.to_string(), *r)).style(t.bold().add_modifier(Modifier::UNDERLINED))
    }));
    let body: Vec<Row> = rows
        .iter()
        .enumerate()
        .skip(app.scroll)
        .take(inner_h)
        .map(|(ri, (i, depth))| {
            let p = &table.entries[*i];
            let cpu_v = p.cpu_pct.live().copied();
            // "…" while the first rate is measured, "—" when unavailable.
            let pending = |st: Status| {
                if st == Status::WarmingUp {
                    "…"
                } else {
                    "—"
                }
            };
            let cpu = Cell::from(align(
                cpu_v.map_or(pending(p.cpu_pct.status).into(), |c| format!("{c:.1}")),
                true,
            ))
            .style(cpu_v.map_or(t.dim(), |c| t.heat(c)));
            let mem_v = total_mem.map(|tm| p.rss_bytes as f64 / tm * 100.0);
            let mem = Cell::from(align(mem_v.map_or("—".into(), |m| format!("{m:.1}")), true))
                .style(mem_v.map_or(t.dim(), |m| t.heat(m)));
            let indent = if *depth > 0 {
                format!(
                    "{}{}",
                    "  ".repeat((*depth - 1).min(12)),
                    if t.ascii { "`-" } else { "└─" }
                )
            } else {
                String::new()
            };
            let pin = if app.is_pinned(&p.id) { "* " } else { "" };
            let name = Cell::from(Line::from(vec![
                Span::styled(indent, t.dim()),
                Span::styled(pin, t.alert()),
                Span::styled(safe(&p.name), t.bold()),
            ]));
            let io = |v: Option<f64>| match (v, p.disk_io.status) {
                (Some(v), _) => Cell::from(align(units::rate(v, RateUnit::Bytes), true)),
                (None, Status::PermissionDenied) => {
                    Cell::from(align("denied".into(), true)).style(t.dim())
                }
                (None, st) => Cell::from(align(pending(st).into(), true)).style(t.dim()),
            };
            let d = p.disk_io.live();
            let (r, w) = (
                io(d.map(|d| d.read_bytes_per_s)),
                io(d.map(|d| d.write_bytes_per_s)),
            );
            let user = p
                .user
                .clone()
                .unwrap_or_else(|| p.uid.map_or("?".into(), |u| u.to_string()));
            let pid = Cell::from(align(p.id.pid.to_string(), true)).style(t.dim());
            let rss = Cell::from(align(units::bytes(p.rss_bytes as f64), true));
            let state = Cell::from(p.state.to_string()).style(if p.state == 'R' {
                t.net()
            } else {
                t.dim()
            });
            let cells: Vec<Cell> = if wide {
                vec![
                    pid,
                    Cell::from(trunc(&safe(&user), 10)).style(t.dim()),
                    cpu,
                    mem,
                    rss,
                    r,
                    w,
                    Cell::from(align(p.threads.to_string(), true)).style(t.dim()),
                    state,
                    name,
                ]
            } else {
                vec![pid, cpu, mem, rss, state, name]
            };
            let row = Row::new(cells);
            if Some(ri) == sel && (full || app.selected.is_some()) {
                row.style(t.selected())
            } else {
                row
            }
        })
        .collect();
    let widths: Vec<Constraint> = cols
        .iter()
        .map(|(_, w, _)| {
            if *w == 0 {
                Constraint::Min(10)
            } else {
                Constraint::Length(*w)
            }
        })
        .collect();
    f.render_widget(
        Table::new(body, widths)
            .header(header)
            .block(block)
            .column_spacing(2),
        area,
    );
}

fn draw_processes(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    if app.details.is_none() {
        process_table(f, area, app, s, t, true);
        return;
    }
    let (a, b) = if area.width >= 120 {
        let [a, b] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
            .areas(area);
        (a, b)
    } else {
        let [a, b] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
        (a, b)
    };
    process_table(f, a, app, s, t, true);
    draw_details(f, b, app, s, t);
}

fn draw_details(f: &mut Frame, area: Rect, app: &App, s: &Snapshot, t: &Theme) {
    let Some(d) = &app.details else { return };
    let entry = s.processes.as_ref().and_then(|tb| {
        tb.entries
            .iter()
            .find(|p| p.id == d.id)
            .map(|p| (p.clone(), tb.logical_cores))
    });
    let mut lines: Vec<Line> = Vec::new();
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<12}"), t.bold()),
            Span::raw(v),
        ])
    };
    match &entry {
        Some((p, cores)) => {
            lines.push(Line::from(Span::styled(
                format!("{} (pid {})", safe(&p.name), p.id.pid),
                t.bold(),
            )));
            lines.push(kv(
                "user",
                safe(
                    &p.user
                        .clone()
                        .unwrap_or_else(|| p.uid.map_or("?".into(), |u| u.to_string())),
                ),
            ));
            lines.push(kv("parent", p.ppid.to_string()));
            lines.push(kv(
                "cpu",
                p.cpu_pct.live().map_or_else(
                    || missing(&p.cpu_pct),
                    |c| format!("{c:.1}% machine · {:.1}% one core", c * *cores as f64),
                ),
            ));
            lines.push(kv("cpu time", units::duration_s(p.cpu_time_s)));
            lines.push(kv("rss", units::bytes(p.rss_bytes as f64)));
            lines.push(kv("virtual", units::bytes(p.virtual_bytes as f64)));
            lines.push(kv("threads", p.threads.to_string()));
        }
        None => lines.push(Line::from(Span::styled("process has exited", t.alert()))),
    }
    match &d.result {
        None => lines.push(Line::from(Span::styled("loading details…", t.dim()))),
        Some(Err(e)) => lines.push(kv("details", safe(&e.to_string()))),
        Some(Ok(x)) => {
            let show = |r: &nysm_collect::CResult<String>| match r {
                Ok(v) => safe(v),
                Err(e) => format!("— ({})", e.status().label()),
            };
            lines.push(kv("exe", show(&x.exe)));
            lines.push(kv("cwd", show(&x.cwd)));
            if let Ok(c) = &x.cgroup
                && !c.is_empty()
                && c != "/"
            {
                let (kind, name) =
                    nysm_collect::linux::parse::cgroup_classify(c.trim_start_matches('/'));
                lines.push(kv(
                    "belongs to",
                    format!("{} {}", kind.label(), safe(&name)),
                ));
            }
            lines.push(kv("cgroup", show(&x.cgroup)));
            lines.push(kv(
                "open fds",
                x.open_fds
                    .as_ref()
                    .map_or_else(|e| format!("— ({})", e.status().label()), |n| n.to_string()),
            ));
            lines.push(Line::from(Span::styled(
                "command line hidden by default (nysm inspect --show-args)",
                t.dim(),
            )));
        }
    }
    // Pinned processes get trend charts under the details text.
    let pinned = app.pinned.iter().find(|p| p.id == d.id);
    let (text_area, charts) = match pinned {
        Some(_) if area.height >= 14 => {
            let [a, b] = Layout::vertical([Constraint::Min(0), Constraint::Length(6)]).areas(area);
            (a, Some(b))
        }
        _ => (area, None),
    };
    if pinned.is_none() {
        lines.push(Line::from(Span::styled(
            "press * to pin and keep CPU/memory history",
            t.dim(),
        )));
    }
    if let (Some(p), Some(c)) = (pinned, charts) {
        let n = boxed_samples(c);
        let pts: Vec<_> = p.points.iter().rev().take(n).rev().collect();
        let cpu = Series {
            vals: pts.iter().map(|x| x.cpu_pct.map(|v| v as f64)).collect(),
            max: 100.0,
            cursor: None,
        };
        let rss_max = pts.iter().map(|x| x.rss_bytes).max().unwrap_or(1).max(1);
        let rss = Series {
            vals: pts.iter().map(|x| Some(x.rss_bytes as f64)).collect(),
            max: rss_max as f64,
            cursor: None,
        };
        let [c1, c2] = Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(c);
        let cpu_max = pts.iter().filter_map(|x| x.cpu_pct).fold(0.0f32, f32::max);
        let exited = if p.exited { " · exited" } else { "" };
        boxed_chart(
            f,
            c1,
            t,
            format!(" pinned CPU % of machine, 0-100 (max {cpu_max:.1}){exited} "),
            &cpu,
            100.0,
            true,
            t.cpu(),
        );
        boxed_chart(
            f,
            c2,
            t,
            format!(" pinned RSS (max {}) ", units::bytes(rss_max as f64)),
            &rss,
            rss_max as f64,
            false,
            t.mem(),
        );
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(t.block(" Details · Esc closes "))
            .wrap(Wrap { trim: false }),
        text_area,
    );
}

fn bar_text(t: &Theme, pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    let (on, off) = if t.ascii { ("#", ".") } else { ("█", "░") };
    format!("{}{}", on.repeat(filled), off.repeat(width - filled))
}

fn draw_cpu(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    let c = &s.cpu;
    let mut lines = vec![];
    match c.usage.live() {
        Some(u) => {
            lines.push(Line::from(vec![
                Span::styled("Total ", t.cpu().add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:.1}%", u.total_pct), t.bold()),
                Span::raw(format!(
                    "  usr {:.1} nice {:.1} sys {:.1} irq {:.1} iowait {:.1} steal {:.1}",
                    u.user_pct, u.nice_pct, u.system_pct, u.irq_pct, u.iowait_pct, u.steal_pct
                )),
            ]));
        }
        None => lines.push(Line::from(format!("Total {}", missing(&c.usage)))),
    }
    let cores = c.logical_cores.value.unwrap_or(0);
    lines.push(Line::from(match c.load.live() {
        Some(l) => format!(
            "Load  {:.2} {:.2} {:.2} (1/5/15 min) on {cores} cores · a queue length, not %",
            l.one, l.five, l.fifteen
        ),
        None => format!("Load  {}", missing(&c.load)),
    }));
    lines.push(Line::from(match c.pressure.live() {
        Some(p) => format!(
            "PSI   some now {} · avg10 {:.1}% · avg60 {:.1}% · avg300 {:.1}%",
            p.some
                .interval_pct
                .map_or("—".into(), |v| format!("{v:.1}%")),
            p.some.avg10_pct,
            p.some.avg60_pct,
            p.some.avg300_pct
        ),
        None => format!("PSI   {}", missing(&c.pressure)),
    }));
    lines.push(Line::from(match s.sensors.value.as_ref() {
        Some(v) if !v.temperatures.is_empty() => {
            let parts: Vec<String> = v
                .summary()
                .iter()
                .map(|(c, x)| format!("{} {x:.0}°C", c.label()))
                .collect();
            let high = v
                .cpu_temperature()
                .and_then(|t| t.high_celsius)
                .map_or(String::new(), |h| format!(" (CPU high {h:.0}°C)"));
            format!("Temp  {}{high}", parts.join(" · "))
        }
        _ => format!("Temp  {}", missing(&s.sensors)),
    }));
    lines.push(Line::from(Span::styled(
        "PSI = time runnable tasks waited for a CPU. iowait is idle time, not CPU work.",
        t.dim(),
    )));
    let head_h = wrapped_height(&lines, area.width) + 1;
    let col_w = 30u16;
    let cols = (area.width.saturating_sub(2) / col_w).max(1) as usize;
    let core_rows = c.per_core.len().div_ceil(cols) as u16;
    let [top, cores_area, chart_area] = Layout::vertical([
        Constraint::Length(head_h),
        Constraint::Length(core_rows + 2),
        Constraint::Min(6),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), top);

    let mut core_lines: Vec<Line> = Vec::new();
    for chunk in c.per_core.chunks(cols) {
        let spans: Vec<Span> = chunk
            .iter()
            .flat_map(|core| {
                let pct = core.usage_pct.live().copied();
                let freq = core
                    .frequency_mhz
                    .live()
                    .map_or("".into(), |m| format!("{:.1}G", m / 1000.0));
                vec![
                    Span::styled(format!("{:>4} ", format!("{}", core.id)), t.dim()),
                    Span::styled(
                        bar_text(t, pct.unwrap_or(0.0), 10),
                        if pct.is_some() { t.cpu() } else { t.dim() },
                    ),
                    Span::raw(format!(
                        " {:>5} {:<5}  ",
                        pct.map_or("—".into(), |p| format!("{p:.0}%")),
                        freq
                    )),
                ]
            })
            .collect();
        core_lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(core_lines).block(t.block(" Logical cores · usage · current frequency ")),
        cores_area,
    );
    let h = app.history().to_vec();
    let cs = series(&h, boxed_samples(chart_area), app.cursor, |p| p.cpu_pct);
    boxed_chart(
        f,
        chart_area,
        t,
        " CPU total, 0-100% ".into(),
        &cs,
        100.0,
        true,
        t.cpu(),
    );
}

fn draw_memory(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    let m = &s.memory;
    let mut lines = vec![];
    match m.usage.live() {
        Some(u) => {
            let b = |v: u64| units::bytes(v as f64);
            let o = |v: Option<u64>| v.map_or("—".into(), b);
            lines.push(Line::from(vec![
                Span::styled("Used      ", t.mem().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!(
                        "{} of {} ({:.1}%)",
                        b(u.used_bytes),
                        b(u.total_bytes),
                        u.used_pct
                    ),
                    t.bold(),
                ),
                Span::styled("   used = total - available", t.dim()),
            ]));
            lines.push(Line::from(format!(
                "Available {}   (kernel MemAvailable: free + reclaimable cache estimate)",
                b(u.available_bytes)
            )));
            lines.push(Line::from(format!(
                "Cache     {} · slab reclaimable {} · shmem {} · dirty {} · free {}",
                o(u.cache_bytes),
                o(u.reclaimable_slab_bytes),
                o(u.shared_bytes),
                o(u.dirty_bytes),
                o(u.free_bytes)
            )));
        }
        None => lines.push(Line::from(format!("Memory {}", missing(&m.usage)))),
    }
    lines.push(Line::from(match m.swap.live() {
        Some(w) if w.total_bytes == 0 => "Swap      none configured".to_string(),
        Some(w) => format!(
            "Swap      {} used of {}",
            units::bytes(w.used_bytes as f64),
            units::bytes(w.total_bytes as f64)
        ),
        None => format!("Swap      {}", missing(&m.swap)),
    }));
    lines.push(Line::from(match m.swap_activity.live() {
        Some(a) => format!(
            "Swapping  in {} · out {}",
            units::rate(a.in_bytes_per_s, RateUnit::Bytes),
            units::rate(a.out_bytes_per_s, RateUnit::Bytes)
        ),
        None => format!("Swapping  {}", missing(&m.swap_activity)),
    }));
    lines.push(Line::from(match m.pressure.live() {
        Some(p) => {
            let w = |x: &nysm_core::snapshot::PressureWindow| {
                format!(
                    "now {} avg10 {:.1}% avg60 {:.1}%",
                    x.interval_pct.map_or("—".into(), |v| format!("{v:.1}%")),
                    x.avg10_pct,
                    x.avg60_pct
                )
            };
            format!(
                "Pressure  some: {} · full: {}",
                w(&p.some),
                p.full.as_ref().map_or("—".into(), w)
            )
        }
        None => format!("Pressure  {}", missing(&m.pressure)),
    }));
    lines.push(Line::from(Span::styled(
        "Pressure = time tasks stalled waiting for memory, not how full RAM is.",
        t.dim(),
    )));
    let [top, c1, c2] = Layout::vertical([
        Constraint::Length(wrapped_height(&lines, area.width) + 1),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), top);
    let h = app.history().to_vec();
    let n = boxed_samples(c1);
    let ms = series(&h, n, app.cursor, |p| p.mem_used_pct);
    boxed_chart(
        f,
        c1,
        t,
        " Memory used, 0-100% ".into(),
        &ms,
        100.0,
        true,
        t.mem(),
    );
    let ps = series(&h, n, app.cursor, |p| p.mem_pressure_some_pct);
    // Pressure is usually near zero: scale to its peak, at least 10%.
    let scale = ps.max.max(10.0);
    boxed_chart(
        f,
        c2,
        t,
        format!(" Memory pressure (PSI some, per interval) · scale 0-{scale:.0}% "),
        &ps,
        scale,
        false,
        t.mem(),
    );
}

fn draw_network(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    let [note, tbl, c1, c2] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(7),
        Constraint::Length(7),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Total = {}.", s.network.total_scope)),
            Line::from(Span::styled("Interface throughput includes LAN traffic; it is not internet speed or connection capacity.", t.dim())),
        ])
        .wrap(Wrap { trim: true }),
        note,
    );
    let wide = tbl.width >= 100;
    let mut ifs: Vec<_> = s
        .network
        .interfaces
        .live()
        .map(|v| v.iter().collect())
        .unwrap_or_default();
    // Counted interfaces first, then active ones, then idle/down.
    ifs.sort_by_key(|i| {
        let active = i
            .rates
            .live()
            .is_some_and(|r| r.rx_bytes_per_s + r.tx_bytes_per_s > 0.0);
        (
            !i.counted_in_total,
            !active,
            i.up != Some(true),
            i.name.clone(),
        )
    });
    let rows: Vec<Row> = Some(ifs)
        .map(|v| {
            v.into_iter()
                .map(|i| {
                    let (rx, tx) = i.rates.live().map_or(("—".into(), "—".into()), |r| {
                        (
                            units::rate(r.rx_bytes_per_s, app.rate),
                            units::rate(r.tx_bytes_per_s, app.rate),
                        )
                    });
                    let errs = i.rates.live().map_or(String::new(), |r| {
                        if r.rx_errors_dropped + r.tx_errors_dropped > 0 {
                            format!("{}", r.rx_errors_dropped + r.tx_errors_dropped)
                        } else {
                            String::new()
                        }
                    });
                    let mut cells = vec![
                        Cell::from(trunc(&safe(&i.name), 15)),
                        Cell::from(i.kind.label()),
                        Cell::from(match i.up {
                            Some(true) => "up",
                            Some(false) => "down",
                            None => "?",
                        }),
                        Cell::from(if i.counted_in_total { "yes" } else { "" }),
                        Cell::from(rx),
                        Cell::from(tx),
                    ];
                    if wide {
                        cells.push(Cell::from(units::bytes(i.rx_total_bytes as f64)));
                        cells.push(Cell::from(units::bytes(i.tx_total_bytes as f64)));
                    }
                    cells.push(Cell::from(errs));
                    Row::new(cells).style(if i.counted_in_total {
                        Style::default()
                    } else {
                        t.dim()
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut head = vec!["INTERFACE", "KIND", "STATE", "TOTAL", "RX/s", "TX/s"];
    let mut widths = vec![
        Constraint::Length(15),
        Constraint::Length(10),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(11),
        Constraint::Length(11),
    ];
    if wide {
        head.extend(["RX total", "TX total"]);
        widths.extend([Constraint::Length(10), Constraint::Length(10)]);
    }
    head.push("ERR+DROP");
    widths.push(Constraint::Length(8));
    let header = Row::new(head).style(t.bold());
    f.render_widget(
        Table::new(rows, widths)
            .header(header)
            .block(t.block(" Interfaces (counters since boot or interface creation) ")),
        tbl,
    );
    let h = app.history().to_vec();
    let n = boxed_samples(c1);
    let rs = series(&h, n, app.cursor, |p| p.net_rx_bytes_per_s);
    let ts = series(&h, n, app.cursor, |p| p.net_tx_bytes_per_s);
    let title1 = format!(" Total receive · peak {} ", units::rate(rs.max, app.rate));
    let title2 = format!(" Total transmit · peak {} ", units::rate(ts.max, app.rate));
    boxed_chart(f, c1, t, title1, &rs, rs.max.max(1.0), false, t.net());
    boxed_chart(f, c2, t, title2, &ts, ts.max.max(1.0), false, t.up());
}

fn draw_groups(f: &mut Frame, area: Rect, app: &mut App, s: &Snapshot, t: &Theme) {
    let Some(table) = &s.cgroups else {
        f.render_widget(
            Paragraph::new("Collecting per-container / service / app accounting (cgroup v2)…")
                .style(t.dim()),
            area,
        );
        return;
    };
    let mut rows: Vec<&nysm_core::snapshot::CgroupSnapshot> = table
        .groups
        .iter()
        .filter(|g| app.group_kind.is_none_or(|k| g.kind == k))
        .collect();
    nysm_core::query::sort_groups(&mut rows, app.sort);
    let inner_h = area.height.saturating_sub(3) as usize;
    app.page_rows = inner_h;
    app.group_scroll = app.group_scroll.min(rows.len().saturating_sub(inner_h));
    let wide = area.width >= 110;
    let body: Vec<Row> = rows
        .iter()
        .skip(app.group_scroll)
        .take(inner_h)
        .map(|g| {
            let mem = match (g.memory_bytes, g.memory_max_bytes) {
                (Some(v), Some(m)) => format!("{} / {}", units::bytes(v as f64), units::bytes(m as f64)),
                (Some(v), None) => units::bytes(v as f64),
                _ => "—".into(),
            };
            let near = matches!((g.memory_bytes, g.memory_max_bytes), (Some(v), Some(m)) if m > 0 && v as f64 / m as f64 >= 0.9);
            let mut cells = vec![
                Cell::from(g.kind.label()),
                Cell::from(safe(&g.name)),
                Cell::from(g.cpu_pct.live().map_or("—".into(), |c| format!("{c:.1}"))),
                Cell::from(g.cpu_limit_cores.map_or("".into(), |c| format!("{c:.2}c"))),
                Cell::from(mem).style(if near { t.alert() } else { Style::default() }),
                Cell::from(g.pids.map_or("—".into(), |p| p.to_string())),
            ];
            if wide {
                let (r, w) = g.disk_io.live().map_or(("—".into(), "—".into()), |d| {
                    (units::rate(d.read_bytes_per_s, RateUnit::Bytes), units::rate(d.write_bytes_per_s, RateUnit::Bytes))
                });
                cells.push(Cell::from(r));
                cells.push(Cell::from(w));
                cells.push(Cell::from(g.memory_pressure_pct.live().map_or("—".into(), |v| format!("{v:.1}%"))));
                cells.push(Cell::from(safe(g.main_process.as_deref().unwrap_or(""))));
            }
            Row::new(cells)
        })
        .collect();
    let mut head = vec!["KIND", "NAME", "CPU%", "LIMIT", "MEMORY / LIMIT", "PIDS"];
    // Name and main-process columns flex; numbers keep fixed widths.
    let mut widths = vec![
        Constraint::Length(9),
        Constraint::Min(18),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(20),
        Constraint::Length(5),
    ];
    if wide {
        head.extend(["READ/s", "WRITE/s", "MEM PSI", "MAIN"]);
        widths.extend([
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(15),
        ]);
    }
    let kind = app.group_kind.map_or("all", |k| k.label());
    let title = format!(
        " Groups ({kind}) · {} · sort {} · k: kind · CPU% of all {} cores · memory incl. page cache ",
        rows.len(),
        app.sort.label(),
        table.logical_cores
    );
    f.render_widget(
        Table::new(body, widths)
            .header(Row::new(head).style(t.bold()))
            .block(t.block("").title(Span::styled(title, t.bold()))),
        area,
    );
}

fn draw_disk(f: &mut Frame, area: Rect, _app: &mut App, s: &Snapshot, t: &Theme) {
    let devs: Vec<_> = s
        .storage
        .devices
        .live()
        .map(|v| {
            v.iter()
                .filter(|d| match d.kind {
                    DiskKind::Loop | DiskKind::Memory => {
                        d.io.live()
                            .is_some_and(|io| io.read_ops_per_s + io.write_ops_per_s > 0.0)
                    }
                    _ => true,
                })
                .collect()
        })
        .unwrap_or_default();
    let fss = s.storage.filesystems.value.clone().unwrap_or_default();
    let dev_h = (devs.len() as u16 + 3).min(area.height / 2).max(4);
    let [note, dt, ft] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(dev_h),
        Constraint::Min(3),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "I/O pressure (PSI): {}",
                pressure_short(&s.storage.io_pressure)
            )),
            Line::from(Span::styled(
                "Totals: whole disks only. In-flight = time with I/O pending, not saturation.",
                t.dim(),
            )),
        ])
        .wrap(Wrap { trim: true }),
        note,
    );
    let ms = |v: Option<f64>| {
        v.map_or("—".into(), |v| {
            if v < 0.005 {
                "<0.01".into()
            } else {
                format!("{v:.2}")
            }
        })
    };
    let rows: Vec<Row> = devs
        .iter()
        .map(|d| {
            let k = format!("{:?}", d.kind).to_lowercase();
            match d.io.live() {
                Some(io) => Row::new(vec![
                    Cell::from(trunc(&safe(&d.name), 14)),
                    Cell::from(k),
                    Cell::from(units::rate(io.read_bytes_per_s, RateUnit::Bytes)),
                    Cell::from(units::rate(io.write_bytes_per_s, RateUnit::Bytes)),
                    Cell::from(format!(
                        "{:.0}/{:.0}",
                        io.read_ops_per_s, io.write_ops_per_s
                    )),
                    Cell::from(format!(
                        "{}/{}",
                        ms(io.read_latency_ms),
                        ms(io.write_latency_ms)
                    )),
                    Cell::from(io.busy_pct.map_or("—".into(), |b| format!("{b:.0}%"))),
                ]),
                None => Row::new(vec![
                    Cell::from(trunc(&safe(&d.name), 14)),
                    Cell::from(k),
                    Cell::from(missing(&d.io)),
                ]),
            }
            .style(if d.counted_in_total {
                Style::default()
            } else {
                t.dim()
            })
        })
        .collect();
    let header = Row::new([
        "DEVICE",
        "KIND",
        "READ/s",
        "WRITE/s",
        "IOPS r/w",
        "LAT ms r/w",
        "IN-FLIGHT",
    ])
    .style(t.bold());
    let widths = [
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(9),
    ];
    f.render_widget(
        Table::new(rows, widths)
            .header(header)
            .block(t.block(" Block devices ")),
        dt,
    );
    let stale = if s.storage.filesystems.status == Status::Stale {
        " (stale)"
    } else {
        ""
    };
    let rows: Vec<Row> = fss
        .iter()
        .map(|fs| {
            let style = if fs.used_pct >= 90.0 {
                t.alert()
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(trunc(&safe(&fs.mount_point), 20)),
                Cell::from(trunc(&safe(&fs.fs_type), 6)),
                Cell::from(format!(
                    "{} {:>5.1}%",
                    bar_text(t, fs.used_pct, 6),
                    fs.used_pct
                ))
                .style(style),
                Cell::from(units::bytes(fs.used_bytes as f64)),
                Cell::from(units::bytes(fs.available_bytes as f64)),
                Cell::from(units::bytes(fs.total_bytes as f64)),
                Cell::from(if fs.read_only { "ro" } else { "" }),
                Cell::from(trend_text(fs)),
            ])
        })
        .collect();
    let header = Row::new([
        "MOUNT", "TYPE", "USED %", "USED", "AVAIL", "SIZE", "", "TREND",
    ])
    .style(t.bold());
    let widths = [
        Constraint::Length(20),
        Constraint::Length(6),
        Constraint::Length(13),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(2),
        Constraint::Min(10),
    ];
    let title =
        format!(" Filesystems{stale} · available = space a normal user can still allocate ");
    if fss.is_empty() {
        f.render_widget(
            Paragraph::new(missing(&s.storage.filesystems)).block(t.block("").title(title)),
            ft,
        );
    } else {
        f.render_widget(
            Table::new(rows, widths)
                .header(header)
                .block(t.block("").title(title)),
            ft,
        );
    }
}

/// "+120 MiB/h, full in ~3 h" (projection) or "stable" / "collecting…".
fn trend_text(fs: &nysm_core::snapshot::FilesystemSnapshot) -> String {
    match (fs.growth_bytes_per_hour, fs.full_in_hours) {
        (None, _) => "collecting…".into(),
        (Some(g), _) if g.abs() < 1024.0 * 1024.0 => "stable".into(),
        (Some(g), Some(h)) if h < 24.0 * 14.0 => format!(
            "+{}/h, full in ~{}",
            units::bytes(g),
            units::duration_s(h * 3600.0)
        ),
        (Some(g), _) if g > 0.0 => format!("+{}/h", units::bytes(g)),
        (Some(g), _) => format!("-{}/h", units::bytes(-g)),
    }
}

/// Usable summary for very small terminals.
fn draw_tiny(f: &mut Frame, area: Rect, app: &App, s: &Snapshot, t: &Theme) {
    let mut lines = vec![];
    if app.paused.is_some() {
        lines.push(Line::from(Span::styled("PAUSED", t.alert())));
    }
    lines.push(Line::from(format!(
        "CPU {}",
        s.cpu
            .usage
            .live()
            .map_or_else(|| missing(&s.cpu.usage), |c| format!("{:.1}%", c.total_pct))
    )));
    lines.push(Line::from(format!(
        "Mem {}",
        s.memory.usage.live().map_or_else(
            || missing(&s.memory.usage),
            |m| format!("{:.1}%", m.used_pct)
        )
    )));
    if let Some(n) = s.network.total.live() {
        lines.push(Line::from(format!(
            "Net ↓{} ↑{}",
            units::rate(n.rx_bytes_per_s, app.rate),
            units::rate(n.tx_bytes_per_s, app.rate)
        )));
    }
    if let Some(d) = s.storage.total_io.live() {
        lines.push(Line::from(format!(
            "Disk r {} w {}",
            units::rate(d.read_bytes_per_s, RateUnit::Bytes),
            units::rate(d.write_bytes_per_s, RateUnit::Bytes)
        )));
    }
    lines.push(Line::from(Span::styled(
        "enlarge for more · q quit",
        t.dim(),
    )));
    if t.ascii {
        for l in lines.iter_mut() {
            for sp in l.spans.iter_mut() {
                sp.content = sp.content.replace('↓', "rx ").replace('↑', "tx ").into();
            }
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}
