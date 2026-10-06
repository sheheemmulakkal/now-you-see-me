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
