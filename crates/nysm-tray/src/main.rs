//! `nysm-tray`: StatusNotifierItem/AppIndicator tray indicator.
//!
//! By default shows one top-bar item per metric (CPU, memory, network,
//! disk), each a symbolic icon with its value as text next to it (Ayatana
//! label; Ubuntu). `--meter` shows a single CPU/memory meter icon instead.
//! Exact values are in the tooltip and menu. Reads from the per-user collector (`nysm service run`) with a lite
//! subscription, or collects locally when no service is running. Works on
//! any desktop with a StatusNotifier host (Ubuntu GNOME, KDE, XFCE, …).

mod glyphs;
mod icon;
mod strip;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip};
use nysm_core::alerts::AlertState;
use nysm_core::sanitize::for_terminal;
use nysm_core::snapshot::{Pressure, Snapshot};
use nysm_core::units::{self, RateUnit};
use nysm_core::{Reading, Status};
use nysm_ipc::source::{Attach, Source};

/// What one top-bar item shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    /// Single item: CPU/memory meter icon, all values in the label.
    Meter,
    /// Single item: one wide image with all chosen parts (GNOME/Ubuntu).
    Strip,
    Cpu,
    Memory,
    Network,
    /// How full the root filesystem is (% used, like df).
    Storage,
    /// Busiest disk's activity (% of time busy).
    Disk,
    /// Disk read and write rates.
    DiskIo,
}

/// Items offered in the "Show in top bar" menu, in display order.
const CHOICES: [Item; 6] = [
    Item::Cpu,
    Item::Memory,
    Item::Network,
    Item::Storage,
    Item::Disk,
    Item::DiskIo,
];

impl Item {
    fn parse(s: &str) -> Option<Item> {
        Some(match s {
            "cpu" => Item::Cpu,
            "mem" | "memory" => Item::Memory,
            "net" | "network" => Item::Network,
            "storage" => Item::Storage,
            "disk" => Item::Disk,
            "diskio" => Item::DiskIo,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Item::Meter => "meter",
            Item::Strip => "strip",
            Item::Cpu => "cpu",
            Item::Memory => "mem",
            Item::Network => "net",
            Item::Storage => "storage",
            Item::Disk => "disk",
            Item::DiskIo => "diskio",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            Item::Meter => "Meter",
            Item::Strip => "Strip",
            Item::Cpu => "CPU usage",
            Item::Memory => "Memory used",
            Item::Network => "Network download / upload",
            Item::Storage => "Storage used (%)",
            Item::Disk => "Disk activity (%)",
            Item::DiskIo => "Disk read / write",
        }
    }

    /// Short name shown before the value when names are on.
    fn short_name(self) -> Option<&'static str> {
        match self {
            Item::Cpu => Some("CPU"),
            Item::Memory => Some("RAM"),
            Item::Storage => Some("Disk"),
            Item::Disk => Some("I/O"),
            Item::Meter | Item::Strip | Item::Network | Item::DiskIo => None,
        }
    }

    fn parse_list(list: &str) -> Option<Vec<Item>> {
        let v: Option<Vec<Item>> = list
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(Item::parse)
            .collect();
        v.filter(|v| !v.is_empty())
    }

    fn id(self) -> String {
        let base = nysm_core::brand::COMMAND_NAME;
        match self {
            Item::Meter => base.into(),
            Item::Strip => format!("{base}-strip"),
            Item::Cpu => format!("{base}-cpu"),
            Item::Memory => format!("{base}-memory"),
            Item::Network => format!("{base}-network"),
            Item::Storage => format!("{base}-storage"),
            Item::Disk => format!("{base}-disk"),
            Item::DiskIo => format!("{base}-diskio"),
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            Item::Meter | Item::Strip => "",
            Item::Cpu => glyphs::CPU,
            Item::Memory => glyphs::MEMORY,
            Item::Network => glyphs::NETWORK,
            Item::Storage => glyphs::STORAGE,
            Item::Disk | Item::DiskIo => glyphs::DISK,
        }
    }

    fn guide(self) -> &'static str {
        match self {
            Item::Meter => "100% · 999G · ↓999M ↑999M",
            Item::Strip => "",
            Item::Cpu => "100%",
            Item::Memory => "999G",
            Item::Network => "↓999M ↑999M",
            Item::Storage | Item::Disk => "100%",
            Item::DiskIo => "R999M W999M",
        }
    }
}

struct NysmTray {
    item: Item,
    /// Directory holding the symbolic icons (`IconThemePath`).
    icon_dir: String,
    snap: Option<Arc<Snapshot>>,
    firing: Vec<String>,
    stale: bool,
    source: String,
    rate: RateUnit,
    /// Show the text label next to the icon (Ayatana hosts, e.g. Ubuntu).
    show_label: bool,
    /// Short names before values; shared by all items, set from the menu
    /// or the config file.
    show_names: Arc<AtomicBool>,
    /// Icons before values (at least one of names/icons is on).
    show_icons: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    /// Set when the tray host comes back (e.g. after a screen lock), so the
    /// main loop can re-register all items in a stable order.
    host_back: Arc<AtomicBool>,
    /// Strip layout: writes the image this item shows.
    strip: Option<strip::Writer>,
    /// Text scale for the strip (GNOME text-scaling-factor).
    text_scale: f64,
    /// Items chosen in the menu; `reselect` asks the main loop to apply.
    selection: Arc<std::sync::Mutex<Vec<Item>>>,
    reselect: Arc<AtomicBool>,
}

/// The root filesystem (`/`), if reported.
fn root_fs(s: &Snapshot) -> Option<&nysm_core::snapshot::FilesystemSnapshot> {
    s.storage
        .filesystems
        .value
        .as_ref()?
        .iter()
        .find(|f| f.mount_point == "/")
}

/// Activity of the busiest whole disk (the total carries it, so it is
/// available on the service's lite feed too).
fn disk_busy(s: &Snapshot) -> Option<f64> {
    s.storage.total_io.live().and_then(|d| d.busy_pct)
}

/// U+2007 FIGURE SPACE: as wide as a digit, so padded numbers keep a
/// constant width in the panel font.
const FIG: char = '\u{2007}';

/// U+2008 PUNCTUATION SPACE: as wide as a period.
const PUNCT: char = '\u{2008}';

/// Filler that brings a number up to the widest form it can take:
/// `max_digits` digits (a "9.9"-style value is narrower: a period is
/// narrower than a digit). Figure and punctuation spaces are by definition
/// as wide as a digit and a period, and UI fonts have tabular digits, so
/// every number then has the same pixel width.
fn number_fill(num: &str, max_digits: usize) -> String {
    let digits = num.chars().filter(char::is_ascii_digit).count();
    let dot = num.contains('.');
    // Widths in periods: digit = 2, period = 1.
    // The widest form is `max_digits` digits or "9.9" (two digits and a
    // period), whichever is wider.
    let want = (max_digits * 2).max(5);
    let mut need = want.saturating_sub(digits * 2 + dot as usize);
    let mut f = String::new();
    while need >= 2 {
        f.push(FIG);
        need -= 2;
    }
    if need == 1 {
        f.push(PUNCT);
    }
    f
}

/// Filler that makes a narrow unit letter as wide as "M" (K, B, k and b
/// are about a period narrower in common UI fonts).
fn unit_fill(unit: &str) -> String {
    if matches!(unit, "K" | "B" | "k" | "b") {
        PUNCT.to_string()
    } else {
        String::new()
    }
}

/// Compact value and its filler: "64K" + spaces so that every value of
/// this kind is equally wide. The filler is placed at the end of the item
/// so values stay next to their icons.
fn fixed(v: f64, base: f64, units: &[&str]) -> (String, String) {
    fixed_digits(v, base, units, 3)
}

fn fixed_digits(v: f64, base: f64, units: &[&str], max_digits: usize) -> (String, String) {
    let text = compact(v, base, units);
    let unit_len = text
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .len();
    let (num, unit) = text.split_at(text.len() - unit_len);
    let fill = number_fill(num, max_digits) + &unit_fill(unit);
    (text, fill)
}

/// Compact value for the top bar: at most three digits and a one-letter
/// unit, no padding: "64K", "1.2M", "0B".
fn compact(v: f64, base: f64, units: &[&str]) -> String {
    if !v.is_finite() || v < 0.0 {
        return "—".into();
    }
    let mut x = v;
    let mut i = 0;
    while x >= 999.5 && i < units.len() - 1 {
        x /= base;
        i += 1;
    }
    if i > 0 && x < 9.95 {
        format!("{x:.1}{}", units[i])
    } else {
        format!("{x:.0}{}", units[i])
    }
}

/// Memory used, always in GiB ("0.5G", "9.7G", "15G"), filled only up to
/// the widest value possible on this machine (e.g. "15G" with 15.6 GiB)
/// rather than "999G". Machines with 1000 GiB or more use T/G as needed.
fn fixed_bytes(n: f64, total: f64) -> (String, String) {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let (v, max) = (n / GIB, total / GIB);
    if max >= 999.5 {
        return fixed(n, 1024.0, &["B", "K", "M", "G", "T"]);
    }
    let num = if v < 9.95 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    };
    let max_digits = format!("{max:.0}").len().max(2);
    let fill = number_fill(&num, max_digits);
    (format!("{num}G"), fill)
}

fn fixed_rate(bytes_per_s: f64, unit: RateUnit) -> (String, String) {
    match unit {
        RateUnit::Bytes => fixed(bytes_per_s, 1024.0, &["B", "K", "M", "G"]),
        RateUnit::Bits => fixed(bytes_per_s * 8.0, 1000.0, &["b", "k", "M", "G"]),
    }
}

/// "35%" plus filler to the width of "100%".
fn fixed_pct(v: f64) -> (String, String) {
    let n = format!("{:.0}", v.clamp(0.0, 100.0));
    let fill = std::iter::repeat_n(FIG, 3usize.saturating_sub(n.len())).collect();
    (format!("{n}%"), fill)
}

/// Menu labels treat `_` as a mnemonic marker; show it literally.
fn menu_text(s: &str) -> String {
    for_terminal(s).replace('_', "__")
}

fn psi(r: &Reading<Pressure>) -> String {
    match r.live().and_then(|p| p.some.interval_pct) {
        Some(v) => format!("{v:.1}%"),
        None if r.status == Status::Unsupported => "n/a".into(),
        None => "—".into(),
    }
}

impl NysmTray {
    /// Names on/off (menu): applies to all items on their next update and
    /// is saved as `display.tray_names`.
    fn toggle_names(&self) {
        self.toggle_style(&self.show_names, &self.show_icons, "display.tray_names");
    }

    fn toggle_icons(&self) {
        self.toggle_style(&self.show_icons, &self.show_names, "display.tray_icons");
    }

    /// Flip names or icons; the other must stay on, so turning off the
    /// last one is refused. Saved to the config; items are re-created so
    /// every one updates at once.
    fn toggle_style(&self, flag: &AtomicBool, other: &AtomicBool, key: &str) {
        let on = !flag.load(Ordering::Relaxed);
        if !on && !other.load(Ordering::Relaxed) {
            return;
        }
        flag.store(on, Ordering::Relaxed);
        let value = if on { "true" } else { "false" };
        if let Some(p) = nysm_config::default_path()
            && let Err(e) = nysm_config::set_many(&p, &[(key, value)])
        {
            eprintln!("nysm-tray: choice not saved: {e}");
        }
        self.reselect.store(true, Ordering::SeqCst);
    }

    /// Show or hide an item (menu). The last item cannot be removed. The
    /// choice is saved as `display.tray_items` and applied by the main loop.
    fn toggle(&self, item: Item) {
        let Ok(mut sel) = self.selection.lock() else {
            return;
        };
        let mut next = sel.clone();
        if let Some(i) = next.iter().position(|x| *x == item) {
            if next.len() == 1 {
                return; // keep at least one item
            }
            next.remove(i);
        } else {
            // Insert before the first shown item that comes later in the
            // menu order, so the bar keeps the menu's order.
            let rank = |x: &Item| CHOICES.iter().position(|c| c == x).unwrap_or(usize::MAX);
            let at = next
                .iter()
                .position(|x| rank(x) > rank(&item))
                .unwrap_or(next.len());
            next.insert(at, item);
        }
        *sel = next.clone();
        let value: Vec<&str> = next.iter().map(|i| i.key()).collect();
        if let Some(p) = nysm_config::default_path()
            && let Err(e) = nysm_config::set_many(&p, &[("display.tray_items", &value.join(","))])
        {
            eprintln!("nysm-tray: choice not saved: {e}");
        }
        self.reselect.store(true, Ordering::SeqCst);
    }

    /// Label for this item; "24% · 6.2G · ↓1.8 MiB/s ↑240 KiB/s" for the
    /// meter. Dashes for values not yet known.
    fn label_text(&self) -> String {
        let Some(s) = &self.snap else {
            return "…".into();
        };
        let alert = if self.firing.is_empty() || self.item != self.first_item() {
            ""
        } else {
            "⚠ "
        };
        let text = self.value_text(s);
        let name = self
            .item
            .short_name()
            .filter(|_| self.show_names.load(Ordering::Relaxed))
            .map_or(String::new(), |n| format!("{n} "));
        format!("{alert}{name}{text}")
    }

    /// Strip layout: draw the chosen parts into the image file.
    fn render_strip(&mut self) {
        if self.item != Item::Strip {
            return;
        }
        let chosen = self.selection.lock().map(|v| v.clone()).unwrap_or_default();
        let s = self.snap.clone();
        let dash = || "—".to_string();
        let rate = |v: Option<f64>, unit: RateUnit| v.map_or_else(dash, |v| fixed_rate(v, unit).0);
        let parts: Vec<strip::Part> = chosen
            .iter()
            .filter_map(|item| {
                let s = s.as_deref();
                let icon = glyphs::body(item.icon_name());
                let name = item.short_name();
                let one = |v: String, widest: &'static str| vec![("", v, widest)];
                let fields = match item {
                    Item::Cpu => one(
                        s.and_then(|s| s.cpu.usage.live().map(|c| fixed_pct(c.total_pct).0))
                            .unwrap_or_else(dash),
                        "100%",
                    ),
                    Item::Memory => one(
                        s.and_then(|s| {
                            s.memory
                                .usage
                                .live()
                                .map(|m| fixed_bytes(m.used_bytes as f64, m.total_bytes as f64).0)
                        })
                        .unwrap_or_else(dash),
                        "99.9G",
                    ),
                    Item::Network => {
                        let n = s.and_then(|s| s.network.total.live().copied());
                        vec![
                            ("↓", rate(n.map(|n| n.rx_bytes_per_s), self.rate), "999M"),
                            ("↑", rate(n.map(|n| n.tx_bytes_per_s), self.rate), "999M"),
                        ]
                    }
                    Item::Storage => one(
                        s.and_then(root_fs)
                            .map_or_else(dash, |f| fixed_pct(f.used_pct).0),
                        "100%",
                    ),
                    Item::Disk => one(
                        s.and_then(disk_busy)
                            .map_or_else(dash, |b| fixed_pct(b.min(100.0)).0),
                        "100%",
                    ),
                    Item::DiskIo => {
                        let d = s.and_then(|s| s.storage.total_io.live().copied());
                        vec![
                            (
                                "R ",
                                rate(d.map(|d| d.read_bytes_per_s), RateUnit::Bytes),
                                "999M",
                            ),
                            (
                                "W ",
                                rate(d.map(|d| d.write_bytes_per_s), RateUnit::Bytes),
                                "999M",
                            ),
                        ]
                    }
                    Item::Meter | Item::Strip => return None,
                };
                Some(strip::Part { icon, name, fields })
            })
            .collect();
        let style = strip::Style {
            icons: self.show_icons.load(Ordering::Relaxed),
            names: self.show_names.load(Ordering::Relaxed),
            stale: self.stale || self.snap.is_none(),
            alert: !self.firing.is_empty(),
            scale: self.text_scale,
        };
        let svg = strip::svg(&parts, &style);
        if let Some(w) = self.strip.as_mut() {
            w.write(&svg);
        }
    }

    /// What this item shows, for the top of its menu.
    fn heading(&self) -> String {
        let s = self.snap.as_deref();
        match self.item {
            Item::Meter => nysm_core::brand::PRODUCT_NAME.into(),
            Item::Strip => format!(
                "{}: CPU usage · RAM in use · network ↓↑ · Disk = storage used on / · I/O = disk activity · R/W = disk read/write",
                nysm_core::brand::PRODUCT_NAME
            ),
            Item::Cpu => "CPU: usage of all cores".into(),
            Item::Memory => "RAM: memory in use".into(),
            Item::Network => "Network: ↓ download  ↑ upload, per second".into(),
            Item::Storage => match s.and_then(root_fs) {
                Some(f) => format!(
                    "Disk: storage used on / — {} of {}, {} free",
                    units::bytes(f.used_bytes as f64),
                    units::bytes(f.total_bytes as f64),
                    units::bytes(f.available_bytes as f64)
                ),
                None => "Disk: storage used on /".into(),
            },
            Item::Disk => "I/O: disk activity (time the busiest disk was busy)".into(),
            Item::DiskIo => "Disk read (R) and write (W), per second".into(),
        }
    }

    /// The item that carries the alert marker (leftmost).
    fn first_item(&self) -> Item {
        match self.item {
            Item::Meter | Item::Strip => self.item,
            _ => Item::Cpu,
        }
    }

    /// Label text with its filler appended, so each item keeps exactly the
    /// same width whatever the values (see `fixed`).
    fn value_text(&self, s: &Snapshot) -> String {
        let dash = || ("—".to_string(), String::new());
        let cpu = s
            .cpu
            .usage
            .live()
            .map_or_else(dash, |c| fixed_pct(c.total_pct));
        let mem = s.memory.usage.live().map_or_else(dash, |m| {
            fixed_bytes(m.used_bytes as f64, m.total_bytes as f64)
        });
        let (rx, tx) = match s.network.total.live() {
            Some(n) => (
                fixed_rate(n.rx_bytes_per_s, self.rate),
                fixed_rate(n.tx_bytes_per_s, self.rate),
            ),
            None => (dash(), dash()),
        };
        match self.item {
            Item::Meter => format!(
                "{}{} · {}{} · ↓{}{} ↑{}{}",
                cpu.0, cpu.1, mem.0, mem.1, rx.0, rx.1, tx.0, tx.1
            ),
            Item::Strip => String::new(),
            Item::Cpu => cpu.0 + &cpu.1,
            Item::Memory => mem.0 + &mem.1,
            // Fill at the end: the item's width is constant; only the
            // upload value may shift slightly inside it.
            Item::Network => format!("↓{} ↑{}{}{}", rx.0, tx.0, rx.1, tx.1),
            Item::Storage => {
                let d = root_fs(s).map_or_else(dash, |f| fixed_pct(f.used_pct));
                d.0 + &d.1
            }
            Item::Disk => {
                let d = disk_busy(s).map_or_else(dash, |b| fixed_pct(b.min(100.0)));
                d.0 + &d.1
            }
            Item::DiskIo => match s.storage.total_io.live() {
                Some(d) => {
                    let r = fixed_rate(d.read_bytes_per_s, RateUnit::Bytes);
                    let w = fixed_rate(d.write_bytes_per_s, RateUnit::Bytes);
                    format!("R {} W {}{}{}", r.0, w.0, r.1, w.1)
                }
                None => "R — W —".into(),
            },
        }
    }

    fn lines(&self) -> Vec<String> {
        let Some(s) = &self.snap else {
            return vec!["Collecting…".into()];
        };
        let rate = |v: f64| units::rate(v, self.rate);
        let cores = s.cpu.logical_cores.value.unwrap_or(0);
        let mut out = vec![
            format!(
                "CPU {}",
                s.cpu.usage.live().map_or("—".into(), |c| format!(
                    "{:.1}% of {cores} cores",
                    c.total_pct
                ))
            ),
            format!(
                "Memory {}",
                s.memory.usage.live().map_or("—".into(), |m| format!(
                    "{} of {} ({:.0}%), {} available",
                    units::bytes(m.used_bytes as f64),
                    units::bytes(m.total_bytes as f64),
                    m.used_pct,
                    units::bytes(m.available_bytes as f64)
                ))
            ),
            format!(
                "Network ↓ {}  ↑ {}",
                s.network
                    .total
                    .live()
                    .map_or("—".into(), |n| rate(n.rx_bytes_per_s)),
                s.network
                    .total
                    .live()
                    .map_or("—".into(), |n| rate(n.tx_bytes_per_s))
            ),
            match root_fs(s) {
                Some(f) => format!(
                    "Storage / {} of {} ({:.0}%), {} free{}",
                    units::bytes(f.used_bytes as f64),
                    units::bytes(f.total_bytes as f64),
                    f.used_pct,
                    units::bytes(f.available_bytes as f64),
                    match (f.growth_bytes_per_hour, f.full_in_hours) {
                        (Some(g), Some(h)) if g > 1024.0 * 1024.0 && h < 24.0 * 14.0 =>
                            format!(" · full in ~{} at this rate", units::duration_s(h * 3600.0)),
                        _ => String::new(),
                    }
                ),
                None => "Storage / —".into(),
            },
            format!(
                "Disk read {}  write {}{}",
                s.storage
                    .total_io
                    .live()
                    .map_or("—".into(), |d| units::rate(
                        d.read_bytes_per_s,
                        RateUnit::Bytes
                    )),
                s.storage
                    .total_io
                    .live()
                    .map_or("—".into(), |d| units::rate(
                        d.write_bytes_per_s,
                        RateUnit::Bytes
                    )),
                disk_busy(s).map_or(String::new(), |b| format!(" · {b:.0}% active"))
            ),
            format!(
                "Load {} · pressure cpu {} mem {} io {}",
                s.cpu
                    .load
                    .live()
                    .map_or("—".into(), |l| format!("{:.2}", l.one)),
                psi(&s.cpu.pressure),
                psi(&s.memory.pressure),
                psi(&s.storage.io_pressure)
            ),
        ];
        if let Some(v) = s.sensors.value.as_ref()
            && !v.temperatures.is_empty()
        {
            let parts: Vec<String> = v
                .summary()
                .iter()
                .map(|(c, t)| format!("{} {t:.0} °C", c.label()))
                .collect();
            out.push(format!("Temperature {}", parts.join(" · ")));
        }
        if self.stale {
            out.push("Not updating — values are stale".into());
        }
        out
    }
}

impl ksni::Tray for NysmTray {
    fn id(&self) -> String {
        self.item.id()
    }

    fn icon_name(&self) -> String {
        if let Some(w) = &self.strip {
            return w.path.clone();
        }
        if self.show_icons.load(Ordering::Relaxed) {
            self.item.icon_name().into()
        } else {
            String::new()
        }
    }

    fn icon_theme_path(&self) -> String {
        if self.item == Item::Meter {
            String::new()
        } else {
            self.icon_dir.clone()
        }
    }

    fn title(&self) -> String {
        nysm_core::brand::PRODUCT_NAME.into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::SystemServices
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.item == Item::Strip {
            return Vec::new();
        }
        if self.item != Item::Meter {
            if self.show_icons.load(Ordering::Relaxed) {
                return Vec::new(); // themed icon via icon_name
            }
            // Names only: a transparent image, so the host does not draw
            // its "missing icon" placeholder.
            return vec![ksni::Icon {
                width: 1,
                height: 1,
                data: vec![0, 0, 0, 0],
            }];
        }
        let m = icon::Meter {
            cpu: self
                .snap
                .as_ref()
                .and_then(|s| s.cpu.usage.live().map(|c| c.total_pct)),
            mem: self
                .snap
                .as_ref()
                .and_then(|s| s.memory.usage.live().map(|m| m.used_pct)),
            stale: self.stale || self.snap.is_none(),
            alert: !self.firing.is_empty(),
        };
        vec![icon::render(22, m), icon::render(44, m)]
    }

    fn label(&self) -> String {
        if !self.show_label || self.item == Item::Strip {
            return String::new();
        }
        self.label_text()
    }

    fn label_guide(&self) -> String {
        // Widest typical label, so the panel reserves a stable width.
        if self.show_label {
            self.item.guide().into()
        } else {
            String::new()
        }
    }

    fn tool_tip(&self) -> ToolTip {
        // The same full summary on every item.
        let mut lines = self.lines();
        lines.extend(self.firing.iter().map(|f| format!("⚠ {f}")));
        ToolTip {
            title: nysm_core::brand::PRODUCT_NAME.into(),
            description: lines.join("\n"),
            ..Default::default()
        }
    }

    fn watcher_online(&self) {
        self.host_back.store(true, Ordering::SeqCst);
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_monitor();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let info = |label: String| {
            StandardItem {
                label: menu_text(&label),
                enabled: false,
                ..Default::default()
            }
            .into()
        };
        // Ubuntu shows no tooltips, so the menu says which item this is.
        let mut items: Vec<MenuItem<Self>> = vec![info(self.heading()), MenuItem::Separator];
        items.extend(self.lines().into_iter().map(info));
        if !self.firing.is_empty() {
            items.push(MenuItem::Separator);
            for f in &self.firing {
                items.push(info(format!("⚠ {f}")));
            }
        }
        items.push(MenuItem::Separator);
        let chosen = self.selection.lock().map(|v| v.clone()).unwrap_or_default();
        items.push(
            SubMenu {
                label: "Show in top bar".into(),
                submenu: CHOICES
                    .iter()
                    .map(|&c| {
                        // The last shown item cannot be turned off, so the
                        // tray never disappears; say so instead of a dead click.
                        let only = chosen.len() == 1 && chosen.contains(&c);
                        CheckmarkItem {
                            label: if only {
                                format!("{} (always one shown)", c.menu_label())
                            } else {
                                c.menu_label().into()
                            },
                            enabled: !only,
                            checked: chosen.contains(&c),
                            activate: Box::new(move |t: &mut Self| t.toggle(c)),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect(),
                ..Default::default()
            }
            .into(),
        );
        let (names, icons) = (
            self.show_names.load(Ordering::Relaxed),
            self.show_icons.load(Ordering::Relaxed),
        );
        items.push(
            CheckmarkItem {
                label: "Show icons".into(),
                checked: icons,
                // The last of names/icons cannot be turned off.
                enabled: names || !icons,
                activate: Box::new(|t: &mut Self| t.toggle_icons()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            CheckmarkItem {
                label: "Show names (CPU, RAM, Disk, I/O)".into(),
                checked: names,
                enabled: icons || !names,
                activate: Box::new(|t: &mut Self| t.toggle_names()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            CheckmarkItem {
                label: "One compact strip (GNOME)".into(),
                checked: self.item == Item::Strip,
                activate: Box::new(|t: &mut Self| {
                    let v = if t.item == Item::Strip {
                        "items"
                    } else {
                        "strip"
                    };
                    if let Some(p) = nysm_config::default_path()
                        && let Err(e) = nysm_config::set_many(&p, &[("display.tray_layout", v)])
                    {
                        eprintln!("nysm-tray: choice not saved: {e}");
                    }
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(info(self.source.clone()));
        items.push(
            StandardItem {
                label: "Open monitor".into(),
                activate: Box::new(|_: &mut Self| open_monitor()),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| t.quit.store(true, Ordering::SeqCst)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

fn open_monitor() {
    // Prefer a nysm-desktop next to this binary, then PATH.
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("nysm-desktop")));
    let cmd = sibling
        .filter(|p| p.exists())
        .map_or_else(|| "nysm-desktop".into(), |p| p.into_os_string());
    if let Err(e) = std::process::Command::new(&cmd).spawn() {
        eprintln!("nysm-tray: cannot start {}: {e}", cmd.to_string_lossy());
    }
}

fn engine_config(s: &nysm_config::Settings) -> nysm_engine::EngineConfig {
    nysm_engine::EngineConfig {
        interval: s.interval,
        processes: false,
        process_interval: s.process_interval,
        cgroups: false,
        // For the "storage used" item (statvfs on a worker every 15 s).
        filesystems: true,
        filesystem_interval: s.filesystem_interval,
        frequency: false,
        sensors: true,
        sensor_interval: Duration::from_secs(5),
        history_samples: 2,
        history_bytes: 4096,
    }
}

fn describe(source: &Source) -> String {
    if source.is_remote() {
        "Data: collector service".into()
    } else {
        "Data: collected by the tray (no service running)".into()
    }
}

/// Whether to draw all parts as one image: "strip", "items", or "auto"
/// (strip on GNOME, whose AppIndicator host shows wide images at width).
fn strip_layout(cfg: &str) -> bool {
    match cfg {
        "strip" => true,
        "items" => false,
        _ => std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|d| d.split(':').any(|x| x.eq_ignore_ascii_case("GNOME"))),
    }
}

/// GNOME's text-scaling-factor (large text), best effort; 1.0 otherwise.
fn gnome_text_scale() -> f64 {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
        })
        .filter(|v| (0.5..=3.0).contains(v))
        .unwrap_or(1.0)
}

fn update_all(handles: &[ksni::blocking::Handle<NysmTray>], f: &dyn Fn(&mut NysmTray)) {
    for h in handles {
        h.update(|t| {
            f(t);
            t.render_strip();
        });
    }
}

fn main() {
    const USAGE: &str = "usage: nysm-tray [--attach auto|never|require] [--items cpu,mem,net,disk,diskio] [--meter] [--no-label]";
    let mut attach = Attach::Auto;
    let mut show_label = true;
    let mut items: Option<Vec<Item>> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-label" => {
                show_label = false;
                continue;
            }
            "--meter" => {
                items = Some(vec![Item::Meter]);
                continue;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => {}
        }
        match (a.as_str(), args.next().as_deref()) {
            ("--attach", Some("auto")) => attach = Attach::Auto,
            ("--attach", Some("never")) => attach = Attach::Never,
            ("--attach", Some("require")) => attach = Attach::Require,
            ("--items", Some(list)) => match Item::parse_list(list) {
                Some(v) => items = Some(v),
                None => {
                    eprintln!("nysm-tray: --items takes cpu,mem,net,disk,diskio\n{USAGE}");
                    std::process::exit(2);
                }
            },
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let path = nysm_config::default_path();
    let (settings, err) = nysm_config::load_or_default(path.as_deref());
    if let Some(e) = err {
        eprintln!("nysm-tray: warning: ignoring invalid configuration, using defaults: {e}");
    }
    // Command line first, then the saved choice, then everything but diskio.
    let mut items = items
        .or_else(|| Item::parse_list(&settings.tray_items))
        .unwrap_or_else(|| {
            vec![
                Item::Cpu,
                Item::Memory,
                Item::Network,
                Item::Storage,
                Item::Disk,
            ]
        });
    let icon_dir = if items.iter().any(|i| *i != Item::Meter) {
        match glyphs::install() {
            Some(d) => d.to_string_lossy().into_owned(),
            None => {
                eprintln!("nysm-tray: cannot write icons; showing the meter instead");
                items = vec![Item::Meter];
                String::new()
            }
        }
    } else {
        String::new()
    };
    let client = concat!("nysm-tray/", env!("CARGO_PKG_VERSION"));
    let mut source = match Source::open_with(
        attach,
        client,
        engine_config(&settings),
        settings.rules.clone(),
        true,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("nysm-tray: cannot attach to the collector service: {e}");
            std::process::exit(1);
        }
    };
    let quit = Arc::new(AtomicBool::new(false));
    let host_back = Arc::new(AtomicBool::new(false));
    let reselect = Arc::new(AtomicBool::new(false));
    let strip_mode = Arc::new(AtomicBool::new(strip_layout(&settings.tray_layout)));
    let text_scale = gnome_text_scale();
    // At least one of names/icons is shown.
    let show_names = Arc::new(AtomicBool::new(settings.tray_names || !settings.tray_icons));
    let show_icons = Arc::new(AtomicBool::new(settings.tray_icons));
    let selection = Arc::new(std::sync::Mutex::new(items.clone()));
    let spawn_all = |source_text: &str| -> Vec<ksni::blocking::Handle<NysmTray>> {
        let mut handles = Vec::new();
        let chosen = selection.lock().map(|v| v.clone()).unwrap_or_default();
        let items = if strip_mode.load(Ordering::SeqCst) && !chosen.contains(&Item::Meter) {
            vec![Item::Strip]
        } else {
            chosen
        };
        // Ubuntu's host inserts each new item to the left of the previous
        // one, so register right-to-left to read cpu, mem, net, disk.
        for item in items.iter().rev() {
            let mut tray = NysmTray {
                item: *item,
                icon_dir: icon_dir.clone(),
                snap: None,
                firing: Vec::new(),
                stale: false,
                source: source_text.to_string(),
                rate: settings.rate_unit,
                show_label,
                show_names: show_names.clone(),
                show_icons: show_icons.clone(),
                quit: quit.clone(),
                host_back: host_back.clone(),
                selection: selection.clone(),
                reselect: reselect.clone(),
                strip: (*item == Item::Strip).then(|| strip::Writer::new(Path::new(&icon_dir))),
                text_scale,
            };
            tray.render_strip();
            // Wait for a tray host instead of failing: at login the tray can
            // start before the panel, and GNOME removes the host while the
            // screen is locked.
            match tray.assume_sni_available(true).spawn() {
                Ok(h) => handles.push(h),
                Err(e) => {
                    eprintln!(
                        "nysm-tray: cannot start the tray service ({e}).\n\
                         GNOME needs AppIndicator support (Ubuntu has it built in; elsewhere install the\n\
                         'AppIndicator and KStatusNotifierItem Support' extension), or use the optional\n\
                         GNOME Shell extension, `nysm tui`, or `nysm-desktop`."
                    );
                    std::process::exit(1);
                }
            }
            // Let the host add this item before the next one.
            std::thread::sleep(Duration::from_millis(200));
        }
        handles
    };
    let mut source_text = describe(&source);
    let mut handles = spawn_all(&source_text);
    let mut last_snap: Option<Arc<Snapshot>> = None;
    let mut last_firing: Vec<String> = Vec::new();

    let mut last_seq = 0;
    let mut last_at = Instant::now();
    let mut was_stale = false;
    let mut last_attach_try = Instant::now();
    // Follow item choices made elsewhere (the desktop app's Settings).
    let mtime = |p: &Option<std::path::PathBuf>| {
        p.as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
    };
    let mut config_mtime = mtime(&path);
    let mut last_config_check = Instant::now();
    while !quit.load(Ordering::SeqCst) && !handles.iter().any(|h| h.is_closed()) {
        std::thread::sleep(Duration::from_millis(500));
        if last_config_check.elapsed() >= Duration::from_secs(2) {
            last_config_check = Instant::now();
            let m = mtime(&path);
            if m != config_mtime {
                config_mtime = m;
                let (cfg, _) = nysm_config::load_or_default(path.as_deref());
                let strip = strip_layout(&cfg.tray_layout);
                if strip_mode.swap(strip, Ordering::SeqCst) != strip {
                    reselect.store(true, Ordering::SeqCst);
                }
                let names = cfg.tray_names || !cfg.tray_icons;
                if show_names.swap(names, Ordering::SeqCst) != names
                    || show_icons.swap(cfg.tray_icons, Ordering::SeqCst) != cfg.tray_icons
                {
                    reselect.store(true, Ordering::SeqCst);
                }
                if let Some(want) = Item::parse_list(&cfg.tray_items)
                    && let Ok(mut sel) = selection.lock()
                    && *sel != want
                    && !sel.contains(&Item::Meter)
                {
                    *sel = want;
                    reselect.store(true, Ordering::SeqCst);
                }
            }
        }
        let host_returned = host_back.swap(false, Ordering::SeqCst);
        if host_returned || reselect.swap(false, Ordering::SeqCst) {
            // After the host returns, items re-registered all at once in
            // arbitrary order; after a menu change the set differs. Either
            // way, re-create them one by one so the order is stable.
            if host_returned {
                std::thread::sleep(Duration::from_secs(1));
                host_back.store(false, Ordering::SeqCst);
            }
            for h in std::mem::take(&mut handles) {
                h.shutdown().wait();
            }
            handles = spawn_all(&source_text);
            let (snap, firing, src) = (last_snap.clone(), last_firing.clone(), source_text.clone());
            update_all(&handles, &|t| {
                t.snap = snap.clone();
                t.firing = firing.clone();
                t.source = src.clone();
            });
        }
        if !source.is_connected() {
            source = Source::local(engine_config(&settings), settings.rules.clone());
            last_seq = 0;
            source_text = format!("{} — service disconnected", describe(&source));
            let d = source_text.clone();
            update_all(&handles, &|t| t.source = d.clone());
        }
        // While collecting locally, periodically try to (re)attach to a
        // service so a long-running tray does not duplicate sampling.
        if attach == Attach::Auto
            && !source.is_remote()
            && last_attach_try.elapsed() >= Duration::from_secs(30)
        {
            last_attach_try = Instant::now();
            if let Ok(s) = Source::open_with(
                Attach::Require,
                client,
                engine_config(&settings),
                settings.rules.clone(),
                true,
            ) {
                source = s;
                last_seq = 0;
                source_text = describe(&source);
                let d = source_text.clone();
                update_all(&handles, &|t| t.source = d.clone());
            }
        }
        let stale = last_at.elapsed() > settings.interval * 3 + Duration::from_secs(1);
        let fresh = source.latest().filter(|s| s.seq != last_seq);
        if fresh.is_none() && stale == was_stale {
            continue;
        }
        was_stale = stale;
        let firing: Vec<String> = source.with_state(|st| {
            st.alerts
                .iter()
                .filter(|a| matches!(a.state, AlertState::Firing { .. }))
                .map(|a| match (&a.target, a.value) {
                    (Some(t), Some(v)) => {
                        format!("{} {t} {v:.1}{}", a.metric.label(), a.metric.unit())
                    }
                    (None, Some(v)) => format!("{} {v:.1}{}", a.metric.label(), a.metric.unit()),
                    _ => a.metric.label().into(),
                })
                .collect()
        });
        if let Some(s) = fresh {
            last_seq = s.seq;
            last_at = Instant::now();
            last_snap = Some(s.clone());
            last_firing = firing.clone();
            update_all(&handles, &|t| {
                t.snap = Some(s.clone());
                t.stale = false;
                t.firing = firing.clone();
            });
        } else {
            update_all(&handles, &|t| t.stale = stale);
        }
    }
    for h in handles {
        h.shutdown().wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_have_constant_pixel_width() {
        // Width in periods (digit = 2, period = 1) of value + filler.
        let width = |(t, f): (String, String)| -> usize {
            (t + &f)
                .chars()
                .map(|c| match c {
                    '0'..='9' | FIG => 2,
                    '.' | PUNCT => 1,
                    _ => 0,
                })
                .sum()
        };
        // Rates: three digits; K/B carry one extra period to match M.
        for v in [0.0, 5.0, 64.0, 999.0, 1536.0, 9.0e4, 1.2e6, 5.0e8] {
            let (t, f) = fixed_rate(v, RateUnit::Bytes);
            let narrow = !t.ends_with('M') as usize;
            assert_eq!(width((t, f)), 6 + narrow, "{v}");
        }
        // Memory on a 15.6 GiB machine: as wide as "9.9".
        let gib = 1024.0 * 1024.0 * 1024.0;
        for g in [0.5, 9.7, 10.0, 15.0] {
            assert_eq!(width(fixed_bytes(g * gib, 15.6 * gib)), 5, "{g}");
        }
        assert_eq!(fixed_rate(1536.0, RateUnit::Bytes).0, "1.5K");
        assert_eq!(fixed_rate(64.0 * 1024.0, RateUnit::Bytes).0, "64K");
        assert_eq!(width(fixed_pct(5.0)), width(fixed_pct(100.0)));
    }
}
