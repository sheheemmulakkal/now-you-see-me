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
use ksni::menu::StandardItem;
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
    Disk,
}

impl Item {
    fn parse(s: &str) -> Option<Item> {
        Some(match s {
            "cpu" => Item::Cpu,
            "mem" | "memory" => Item::Memory,
            "net" | "network" => Item::Network,
            "disk" => Item::Disk,
            _ => return None,
        })
    }

    fn id(self) -> String {
        let base = nysm_core::brand::COMMAND_NAME;
        match self {
            Item::Meter => base.into(),
            Item::Cpu => format!("{base}-cpu"),
            Item::Memory => format!("{base}-memory"),
            Item::Network => format!("{base}-network"),
            Item::Disk => format!("{base}-disk"),
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            Item::Meter => "",
            Item::Cpu => glyphs::CPU,
            Item::Memory => glyphs::MEMORY,
            Item::Network => glyphs::NETWORK,
            Item::Disk => glyphs::DISK,
        }
    }

    fn guide(self) -> &'static str {
        match self {
            Item::Meter => "100% · 99.9G · ↓999 MiB/s ↑999 MiB/s",
            Item::Cpu => "100%",
            Item::Memory => "99.9G",
            Item::Network => "↓999 MiB/s ↑999 MiB/s",
            Item::Disk => "R 999 MiB/s W 999 MiB/s",
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
    quit: Arc<AtomicBool>,
}

/// Compact size for the top bar: "6.2G", "512M".
fn short_bytes(n: f64) -> String {
    let s = units::bytes(n);
    match s.split_once(' ') {
        Some((v, unit)) => format!("{v}{}", &unit[..1]),
        None => s,
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
            .map_or("—".into(), |c| format!("{:.0}%", c.total_pct));
        let mem = s
            .memory
            .usage
            .live()
            .map_or("—".into(), |m| short_bytes(m.used_bytes as f64));
        let (rx, tx) = match s.network.total.live() {
            Some(n) => (
                units::rate(n.rx_bytes_per_s, self.rate),
                units::rate(n.tx_bytes_per_s, self.rate),
            ),
            None => ("—".into(), "—".into()),
        };
        match self.item {
            Item::Meter => format!("{cpu} · {mem} · ↓{rx} ↑{tx}"),
            Item::Cpu => cpu,
            Item::Memory => mem,
            Item::Network => format!("↓{rx} ↑{tx}"),
            Item::Disk => match s.storage.total_io.live() {
                Some(d) => format!(
                    "R {} W {}",
                    units::rate(d.read_bytes_per_s, RateUnit::Bytes),
                    units::rate(d.write_bytes_per_s, RateUnit::Bytes)
                ),
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
            format!(
                "Disk read {}  write {}",
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
                    ))
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
        if self.show_label {
            self.label_text()
        } else {
            String::new()
        }
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
        let mut lines = self.lines();
        lines.truncate(3);
        ToolTip {
            title: nysm_core::brand::PRODUCT_NAME.into(),
            description: lines.join("\n"),
            ..Default::default()
        }
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

fn main() {
    const USAGE: &str = "usage: nysm-tray [--attach auto|never|require] [--items cpu,mem,net,disk] [--meter] [--no-label]";
    let mut attach = Attach::Auto;
    let mut show_label = true;
    let mut items = vec![Item::Cpu, Item::Memory, Item::Network, Item::Disk];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-label" => {
                show_label = false;
                continue;
            }
            "--meter" => {
                items = vec![Item::Meter];
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
            ("--items", Some(list)) => {
                let parsed: Option<Vec<Item>> =
                    list.split(',').map(|x| Item::parse(x.trim())).collect();
                match parsed {
                    Some(v) if !v.is_empty() => items = v,
                    _ => {
                        eprintln!("nysm-tray: --items takes cpu,mem,net,disk\n{USAGE}");
                        std::process::exit(2);
                    }
                }
            }
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
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
    let path = nysm_config::default_path();
    let (settings, err) = nysm_config::load_or_default(path.as_deref());
    if let Some(e) = err {
        eprintln!("nysm-tray: warning: ignoring invalid configuration, using defaults: {e}");
    }
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
    let mut handles = Vec::new();
    for item in &items {
        let tray = NysmTray {
            item: *item,
            icon_dir: icon_dir.clone(),
            snap: None,
            firing: Vec::new(),
            stale: false,
            source: describe(&source),
            rate: settings.rate_unit,
            show_label,
            quit: quit.clone(),
        };
        // Wait for a tray host instead of failing: at login the tray can
        // start before the panel, and GNOME removes the host while the
        // screen is locked. Items re-register when it comes back.
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
        // Hosts list items in registration order; give each a moment.
        std::thread::sleep(Duration::from_millis(150));
    }
    let update_all = |f: &dyn Fn(&mut NysmTray)| {
        for h in &handles {
            h.update(|t| f(t));
        }
    };

    let mut last_seq = 0;
    let mut last_at = Instant::now();
    let mut was_stale = false;
    let mut last_attach_try = Instant::now();
    while !quit.load(Ordering::SeqCst) && !handles.iter().any(|h| h.is_closed()) {
        std::thread::sleep(Duration::from_millis(500));
        if !source.is_connected() {
            source = Source::local(engine_config(&settings), settings.rules.clone());
            last_seq = 0;
            let d = describe(&source);
            update_all(&|t| t.source = format!("{d} — service disconnected"));
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
                let d = describe(&source);
                update_all(&|t| t.source = d.clone());
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
            update_all(&|t| {
                t.snap = Some(s.clone());
                t.stale = false;
                t.firing = firing.clone();
            });
        } else {
            update_all(&|t| t.stale = stale);
        }
    }
    for h in handles {
        h.shutdown().wait();
    }
}
