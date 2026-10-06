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
    Cpu,
    Memory,
    Network,
    /// Busiest disk's activity (% of time busy).
    Disk,
    /// Disk read and write rates.
    DiskIo,
}

/// Items offered in the "Show in top bar" menu, in display order.
const CHOICES: [Item; 5] = [
    Item::Cpu,
    Item::Memory,
    Item::Network,
    Item::Disk,
    Item::DiskIo,
];

impl Item {
    fn parse(s: &str) -> Option<Item> {
        Some(match s {
            "cpu" => Item::Cpu,
            "mem" | "memory" => Item::Memory,
            "net" | "network" => Item::Network,
            "disk" => Item::Disk,
            "diskio" => Item::DiskIo,
            _ => return None,
        })
    }

    fn key(self) -> &'static str {
        match self {
            Item::Meter => "meter",
            Item::Cpu => "cpu",
            Item::Memory => "mem",
            Item::Network => "net",
            Item::Disk => "disk",
            Item::DiskIo => "diskio",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            Item::Meter => "Meter",
            Item::Cpu => "CPU usage",
            Item::Memory => "Memory used",
            Item::Network => "Network download / upload",
            Item::Disk => "Disk activity (%)",
            Item::DiskIo => "Disk read / write",
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
            Item::Cpu => format!("{base}-cpu"),
            Item::Memory => format!("{base}-memory"),
            Item::Network => format!("{base}-network"),
            Item::Disk => format!("{base}-disk"),
            Item::DiskIo => format!("{base}-diskio"),
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            Item::Meter => "",
            Item::Cpu => glyphs::CPU,
            Item::Memory => glyphs::MEMORY,
            Item::Network => glyphs::NETWORK,
            Item::Disk | Item::DiskIo => glyphs::DISK,
        }
    }

    fn guide(self) -> &'static str {
        match self {
            Item::Meter => "100% · 999G · ↓999M ↑999M",
            Item::Cpu => "100%",
            Item::Memory => "999G",
            Item::Network => "↓999M ↑999M",
            Item::Disk => "100%",
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
    /// Widest label shown so far (characters): labels are padded to it so
    /// items never shrink and shift their neighbours ("sticky" width).
    widest: std::cell::Cell<usize>,
    quit: Arc<AtomicBool>,
    /// Set when the tray host comes back (e.g. after a screen lock), so the
    /// main loop can re-register all items in a stable order.
    host_back: Arc<AtomicBool>,
    /// Items chosen in the menu; `reselect` asks the main loop to apply.
    selection: Arc<std::sync::Mutex<Vec<Item>>>,
    reselect: Arc<AtomicBool>,
}

/// Activity of the busiest whole disk (the total carries it, so it is
/// available on the service's lite feed too).
fn disk_busy(s: &Snapshot) -> Option<f64> {
    s.storage.total_io.live().and_then(|d| d.busy_pct)
}

/// U+2007 FIGURE SPACE: as wide as a digit, so padded numbers keep a
/// constant width in the panel font.
const FIG: char = '\u{2007}';

/// Left-pad with figure spaces to `width` characters.
fn pad(s: &str, width: usize) -> String {
    let n = s.chars().count();
    let mut out: String = std::iter::repeat_n(FIG, width.saturating_sub(n)).collect();
    out.push_str(s);
    out
}

/// U+2008 PUNCTUATION SPACE: as wide as a period.
const PUNCT: char = '\u{2008}';

/// Fixed-width compact value for the top bar: the width of three digits
/// and a period, plus a one-letter unit, e.g. " 64K", "1.2M", "  0B".
/// Whole numbers get a punctuation space so they are exactly as wide as
/// "9.9"-style values.
fn compact(v: f64, base: f64, units: &[&str]) -> String {
    if !v.is_finite() || v < 0.0 {
        return pad("—", 5);
    }
    let mut x = v;
    let mut i = 0;
    while x >= 999.5 && i < units.len() - 1 {
        x /= base;
        i += 1;
    }
    if i > 0 && x < 9.95 {
        format!("{FIG}{x:.1}{}", units[i])
    } else {
        format!("{PUNCT}{}{}", pad(&format!("{x:.0}"), 3), units[i])
    }
}

/// Bytes ("6.2G", " 15G") for memory.
fn short_bytes(n: f64) -> String {
    compact(n, 1024.0, &["B", "K", "M", "G", "T"])
}

/// Per-second rate without "/s" (the tooltip and menu spell it out):
/// bytes with binary prefixes, or bits with decimal prefixes.
fn short_rate(bytes_per_s: f64, unit: RateUnit) -> String {
    match unit {
        RateUnit::Bytes => compact(bytes_per_s, 1024.0, &["B", "K", "M", "G"]),
        RateUnit::Bits => compact(bytes_per_s * 8.0, 1000.0, &["b", "k", "M", "G"]),
    }
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
        format!("{alert}{text}")
    }

    /// The item that carries the alert marker (leftmost).
    fn first_item(&self) -> Item {
        match self.item {
            Item::Meter => Item::Meter,
            _ => Item::Cpu,
        }
    }

    fn value_text(&self, s: &Snapshot) -> String {
        let cpu = s
            .cpu
            .usage
            .live()
            .map_or(pad("—", 3), |c| pad(&format!("{:.0}", c.total_pct), 3))
            + "%";
        let mem = s
            .memory
            .usage
            .live()
            .map_or(pad("—", 5), |m| short_bytes(m.used_bytes as f64));
        let (rx, tx) = match s.network.total.live() {
            Some(n) => (
                short_rate(n.rx_bytes_per_s, self.rate),
                short_rate(n.tx_bytes_per_s, self.rate),
            ),
            None => (pad("—", 5), pad("—", 5)),
        };
        match self.item {
            Item::Meter => format!("{cpu} · {mem} · ↓{rx} ↑{tx}"),
            Item::Cpu => cpu,
            Item::Memory => mem,
            Item::Network => format!("↓{rx} ↑{tx}"),
            Item::Disk => {
                disk_busy(s).map_or(pad("—", 3), |b| {
                    pad(&format!("{b:.0}", b = b.min(100.0)), 3)
                }) + "%"
            }
            Item::DiskIo => match s.storage.total_io.live() {
                Some(d) => format!(
                    "R{} W{}",
                    short_rate(d.read_bytes_per_s, RateUnit::Bytes),
                    short_rate(d.write_bytes_per_s, RateUnit::Bytes)
                ),
                None => format!("R{} W{}", pad("—", 5), pad("—", 5)),
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
        self.item.icon_name().into()
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
        if self.item != Item::Meter {
            return Vec::new();
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
        if !self.show_label {
            return String::new();
        }
        let text = self.label_text();
        let n = text.chars().count();
        if n > self.widest.get() {
            self.widest.set(n);
        }
        pad(&text, self.widest.get())
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
        let mut items: Vec<MenuItem<Self>> = self.lines().into_iter().map(info).collect();
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
                        CheckmarkItem {
                            label: c.menu_label().into(),
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
        filesystems: false,
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

fn update_all(handles: &[ksni::blocking::Handle<NysmTray>], f: &dyn Fn(&mut NysmTray)) {
    for h in handles {
        h.update(|t| f(t));
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
        .unwrap_or_else(|| vec![Item::Cpu, Item::Memory, Item::Network, Item::Disk]);
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
    let selection = Arc::new(std::sync::Mutex::new(items.clone()));
    let spawn_all = |source_text: &str| -> Vec<ksni::blocking::Handle<NysmTray>> {
        let mut handles = Vec::new();
        let items = selection.lock().map(|v| v.clone()).unwrap_or_default();
        // Ubuntu's host inserts each new item to the left of the previous
        // one, so register right-to-left to read cpu, mem, net, disk.
        for item in items.iter().rev() {
            let tray = NysmTray {
                item: *item,
                icon_dir: icon_dir.clone(),
                snap: None,
                firing: Vec::new(),
                stale: false,
                source: source_text.to_string(),
                rate: settings.rate_unit,
                show_label,
                widest: std::cell::Cell::new(0),
                quit: quit.clone(),
                host_back: host_back.clone(),
                selection: selection.clone(),
                reselect: reselect.clone(),
            };
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
    while !quit.load(Ordering::SeqCst) && !handles.iter().any(|h| h.is_closed()) {
        std::thread::sleep(Duration::from_millis(500));
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

    fn w(s: &str) -> usize {
        s.chars().count()
    }

    #[test]
    fn compact_values_have_constant_width() {
        for v in [
            0.0, 5.0, 64.0, 999.0, 1000.0, 1536.0, 9.0e4, 1.2e6, 5.0e8, 3.0e9,
        ] {
            assert_eq!(w(&short_rate(v, RateUnit::Bytes)), 5, "{v}");
            assert_eq!(w(&short_rate(v, RateUnit::Bits)), 5, "{v}");
        }
        assert_eq!(short_rate(1536.0, RateUnit::Bytes), "\u{2007}1.5K");
        assert_eq!(
            short_rate(64.0 * 1024.0, RateUnit::Bytes),
            "\u{2008}\u{2007}64K"
        );
        assert_eq!(short_rate(1000.0 * 1024.0, RateUnit::Bytes), "\u{2007}1.0M");
        assert_eq!(short_bytes(9.6 * 1024.0 * 1024.0 * 1024.0), "\u{2007}9.6G");
        assert_eq!(pad("5", 3), "\u{2007}\u{2007}5");
    }
}
