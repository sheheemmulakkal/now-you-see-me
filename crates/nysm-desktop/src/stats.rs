//! Structured stat tiles ("caption / value / hint") laid out in a wrapping
//! grid, and per-core tiles with a usage sparkline.

use std::collections::VecDeque;

use gtk::prelude::*;

use crate::chart::{self, Chart, Series};

pub struct Tile {
    value: gtk::Label,
    hint: gtk::Label,
}

impl Tile {
    pub fn set(&self, value: &str, hint: &str) {
        if self.value.text() != value {
            self.value.set_text(value);
        }
        if self.hint.text() != hint {
            self.hint.set_text(hint);
        }
    }
}

fn small(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A wrapping grid of tiles; `items` are (caption, tooltip explaining it).
pub fn grid(items: &[(&str, &str)]) -> (gtk::FlowBox, Vec<Tile>) {
    let fb = gtk::FlowBox::new();
    fb.set_selection_mode(gtk::SelectionMode::None);
    fb.set_homogeneous(true);
    fb.set_min_children_per_line(2);
    fb.set_max_children_per_line(6);
    fb.set_row_spacing(8);
    fb.set_column_spacing(8);
    fb.set_can_focus(false);
    let mut tiles = Vec::new();
    for (caption, tip) in items {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.add_css_class("stat");
        if !tip.is_empty() {
            b.set_tooltip_text(Some(tip));
        }
        b.append(&small(caption, &["caption", "dim-label"]));
        let value = small("—", &["stat-value", "numeric"]);
        let hint = small("", &["caption", "dim-label", "numeric"]);
        b.append(&value);
        b.append(&hint);
        fb.insert(&b, -1);
        tiles.push(Tile { value, hint });
    }
    (fb, tiles)
}

/// Samples of per-core history kept by the desktop (the shared history
/// holds totals only).
pub const CORE_HISTORY: usize = 120;

pub struct CoreTile {
    title: gtk::Label,
    freq: gtk::Label,
    value: gtk::Label,
    chart: Chart,
    history: VecDeque<Option<f64>>,
}

impl CoreTile {
    pub fn new() -> (gtk::Box, Self) {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        b.add_css_class("stat");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = small("", &["caption", "dim-label"]);
        title.set_hexpand(true);
        let freq = small("", &["caption", "dim-label", "numeric"]);
        freq.set_xalign(1.0);
        top.append(&title);
        top.append(&freq);
        let value = small("—", &["stat-value", "numeric"]);
        let chart = Chart::mini(40, Some(100.0), |v| format!("{v:.0}%"));
        b.append(&top);
        b.append(&value);
        b.append(&chart.area);
        (
            b,
            CoreTile {
                title,
                freq,
                value,
                chart,
                history: VecDeque::with_capacity(CORE_HISTORY),
            },
        )
    }

    /// Record one sample (called for every new snapshot, visible or not).
    pub fn push(&mut self, usage: Option<f64>) {
        if self.history.len() == CORE_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(usage);
    }

    pub fn show(&self, id: u32, usage: Option<f64>, freq_mhz: Option<f64>, span_s: f64) {
        self.title.set_text(&format!("Core {id}"));
        self.freq
            .set_text(&freq_mhz.map_or(String::new(), |f| format!("{:.2} GHz", f / 1000.0)));
        self.value
            .set_text(&usage.map_or("—".into(), |v| format!("{v:.0}%")));
        self.chart.set(
            vec![Series {
                color: chart::CPU,
                // Left-padded so a short history grows in from the right.
                values: std::iter::repeat_n(None, CORE_HISTORY - self.history.len())
                    .chain(self.history.iter().copied())
                    .collect(),
            }],
            Vec::new(),
            span_s,
        );
        self.chart.set_summary(&format!(
            "Core {id} usage, last {} samples",
            self.history.len()
        ));
    }
}

/// A watched (pinned) process: name, live CPU and memory with history.
pub struct WatchTile {
    pub root: gtk::Box,
    title: gtk::Label,
    status: gtk::Label,
    cpu: Chart,
    mem: Chart,
    pub stop: gtk::Button,
}

impl WatchTile {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.add_css_class("stat");
        root.set_width_request(260);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = small("", &["caption"]);
        title.set_hexpand(true);
        let stop = gtk::Button::from_icon_name("window-close-symbolic");
        stop.add_css_class("flat");
        stop.set_tooltip_text(Some("Stop watching"));
        top.append(&title);
        top.append(&stop);
        let status = small("", &["numeric"]);
        let cpu_l = small("CPU, % of all cores", &["caption", "dim-label"]);
        let cpu = Chart::mini(36, None, |v| format!("{v:.1}%"));
        let mem_l = small("Memory (RSS)", &["caption", "dim-label"]);
        let mem = Chart::mini(36, None, bytes);
        root.append(&top);
        root.append(&status);
        root.append(&cpu_l);
        root.append(&cpu.area);
        root.append(&mem_l);
        root.append(&mem.area);
        WatchTile {
            root,
            title,
            status,
            cpu,
            mem,
            stop,
        }
    }

    pub fn show(&self, p: &nysm_core::history::PinnedProcess) {
        self.title.set_text(&format!(
            "{} · PID {}",
            nysm_core::sanitize::for_terminal(&p.name),
            p.id.pid
        ));
        let last = p.points.back();
        let status = match (p.exited, last) {
            (true, _) => "exited — history shown for 15 minutes, then removed".to_string(),
            (false, Some(pt)) => format!(
                "CPU {} · memory {}",
                pt.cpu_pct.map_or("—".into(), |c| format!("{c:.1}%")),
                bytes(pt.rss_bytes as f64)
            ),
            (false, None) => "collecting…".into(),
        };
        self.status.set_text(&status);
        if p.exited {
            self.root.add_css_class("dim-label");
        }
        let span = match (p.points.front(), last) {
            (Some(a), Some(b)) => (b.timestamp_ms - a.timestamp_ms) as f64 / 1000.0,
            _ => 0.0,
        };
        self.cpu.set(
            vec![Series {
                color: chart::CPU,
                values: p.points.iter().map(|x| x.cpu_pct.map(f64::from)).collect(),
            }],
            Vec::new(),
            span,
        );
        self.mem.set(
            vec![Series {
                color: chart::MEM,
                values: p.points.iter().map(|x| Some(x.rss_bytes as f64)).collect(),
            }],
            Vec::new(),
            span,
        );
        self.cpu
            .set_summary(&format!("CPU history of PID {}", p.id.pid));
        self.mem
            .set_summary(&format!("Memory history of PID {}", p.id.pid));
    }
}

fn bytes(v: f64) -> String {
    nysm_core::units::bytes(v)
}
