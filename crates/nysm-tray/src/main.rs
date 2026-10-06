//! `nysm-tray`: StatusNotifierItem/AppIndicator tray indicator.
//!
//! Shows a live CPU/memory meter icon with exact values in the tooltip and
//! menu. Reads from the per-user collector (`nysm service run`) with a lite
//! subscription, or collects locally when no service is running. Works on
//! any desktop with a StatusNotifier host (Ubuntu GNOME, KDE, XFCE, …).

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

struct NysmTray {
    snap: Option<Arc<Snapshot>>,
    firing: Vec<String>,
    stale: bool,
    source: String,
    rate: RateUnit,
    quit: Arc<AtomicBool>,
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
        nysm_core::brand::COMMAND_NAME.into()
    }

    fn title(&self) -> String {
        nysm_core::brand::PRODUCT_NAME.into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::SystemServices
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
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
    let mut attach = Attach::Auto;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next().as_deref()) {
            ("--attach", Some("auto")) => attach = Attach::Auto,
            ("--attach", Some("never")) => attach = Attach::Never,
            ("--attach", Some("require")) => attach = Attach::Require,
            _ => {
                eprintln!("usage: nysm-tray [--attach auto|never|require]");
                std::process::exit(2);
            }
        }
    }
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
    let tray = NysmTray {
        snap: None,
        firing: Vec::new(),
        stale: false,
        source: describe(&source),
        rate: settings.rate_unit,
        quit: quit.clone(),
    };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(e) => {
            eprintln!(
                "nysm-tray: no system tray available ({e}).\n\
                 GNOME needs AppIndicator support (Ubuntu has it built in; elsewhere install the\n\
                 'AppIndicator and KStatusNotifierItem Support' extension), or use the optional\n\
                 GNOME Shell extension, `nysm tui`, or `nysm-desktop`."
            );
            std::process::exit(1);
        }
    };

    let mut last_seq = 0;
    let mut last_at = Instant::now();
    let mut was_stale = false;
    let mut last_attach_try = Instant::now();
    while !quit.load(Ordering::SeqCst) && !handle.is_closed() {
        std::thread::sleep(Duration::from_millis(500));
        if !source.is_connected() {
            source = Source::local(engine_config(&settings), settings.rules.clone());
            last_seq = 0;
            let d = describe(&source);
            handle.update(|t| t.source = format!("{d} — service disconnected"));
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
                handle.update(|t| t.source = d);
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
            handle.update(|t| {
                t.snap = Some(s);
                t.stale = false;
                t.firing = firing;
            });
        } else {
            handle.update(|t| t.stale = stale);
        }
    }
    handle.shutdown().wait();
}
