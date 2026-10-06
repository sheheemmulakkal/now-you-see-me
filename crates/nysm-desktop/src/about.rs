//! About page: what the app is, how it collects and keeps data, what it
//! costs (measured live from its own processes), privacy, files, licence.

use std::time::Duration;

use gtk::glib::markup_escape_text as esc;
use gtk::prelude::*;
use nysm_core::snapshot::Snapshot;
use nysm_core::units;

pub struct About {
    running: gtk::Label,
    cost: gtk::Label,
}

fn section(parent: &gtk::Box, title: &str) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 6);
    c.add_css_class("card");
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    t.add_css_class("card-title");
    c.append(&t);
    parent.append(&c);
    c
}

fn text(parent: &gtk::Box, markup: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(markup);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_selectable(true);
    l.set_max_width_chars(100);
    parent.append(&l);
    l
}

/// Build the page into `page` (a vertical box from the page stack).
pub fn build(page: &gtk::Box, history: Duration, interval: Duration) -> About {
    let version = env!("CARGO_PKG_VERSION");
    let s = section(page, "What it is");
    text(
        &s,
        &format!(
            "<b>{}</b> {version} shows what this computer is doing: how busy CPU, memory, \
             disks and network are, what changed in the last minutes, <i>which</i> process or \
             container is responsible, and whether work is <i>waiting</i> on a resource \
             (pressure). Values that cannot be measured are shown with the reason, never as 0.",
            nysm_core::brand::PRODUCT_NAME
        ),
    );

    let s = section(page, "The parts, and what is running now");
    text(
        &s,
        "<b>Top bar</b> (nysm-tray): CPU, memory, network, storage and disk at a glance. \
         <b>This window</b> (nysm-desktop): charts, processes, containers. \
         <b>Terminal</b>: <tt>nysm</tt> commands and <tt>nysm tui</tt>. \
         <b>Collector service</b> (<tt>nysm service run</tt>, optional): samples the machine once \
         for all of them and keeps the shared history; without it each part collects for itself.",
    );
    let running = text(&s, "…");

    let s = section(page, "How the numbers are collected");
    text(
        &s,
        &format!(
            "Read directly from the Linux kernel (<tt>/proc</tt>, <tt>/sys</tt>) as a normal \
             user, every {} (processes every 2 s, filesystems every 15 s). Rates (CPU %, \
             bytes/s) are the change between two samples. Other users' processes show \
             \"permission denied\" for details the kernel only gives their owner; nothing is \
             guessed. <b>Disk</b> in the top bar is how full <tt>/</tt> is (like <tt>df</tt>); \
             <b>I/O</b> is how busy the busiest disk was; <b>R/W</b> are bytes read and written \
             per second.",
            units::duration_s(interval.as_secs_f64())
        ),
    );

    let s = section(page, "History");
    text(
        &s,
        &format!(
            "The last <b>{}</b> are kept in memory by the collector (or by this window when no \
             collector runs), so charts show the recent past as soon as a window opens. History \
             is lost when the collector stops. Change the length in Settings (Ctrl+,), which \
             shows the memory it needs. For long periods record to a file: \
             <tt>nysm record -o day.nysm --duration 24h --interval 10s</tt>, then \
             <tt>nysm compare day.nysm</tt>.",
            human_span(history)
        ),
    );

    let s = section(page, "What it costs");
    text(
        &s,
        "Measured right now from this machine's process table (CPU as a share of all cores; \
         memory resident):",
    );
    let cost = text(&s, "collecting…");
    text(
        &s,
        "<small>Reference (Intel i5-7500, release builds): collector about 0.2 % of one core and \
         3 MiB; tray 0.03–0.2 % and 4 MiB; this window about 2 % while visible and nothing \
         while minimised or hidden; GNOME Shell spends about 1 % of one core more to redraw \
         the top bar each second. Pages that are not shown are not updated.</small>",
    );

    let s = section(page, "Privacy and safety");
    text(
        &s,
        "No network access except what you ask for (<tt>nysm net check</tt>, remote monitoring \
         over your own SSH). No accounts, no telemetry. It never asks for root. Command lines \
         are hidden unless you ask (<tt>nysm inspect --show-args</tt>) because they can hold \
         passwords. Container names are opt-in because the Docker socket is root-equivalent. \
         Files it writes are readable only by you.",
    );

    let s = section(page, "Files");
    let cfg = nysm_config::default_path().map_or("—".into(), |p| p.display().to_string());
    let state = nysm_config::default_incident_dir()
        .and_then(|p| p.parent().map(|d| d.display().to_string()))
        .unwrap_or_else(|| "—".into());
    text(
        &s,
        &format!(
            "Settings: <tt>{}</tt>\nSaved data (incident captures): <tt>{}</tt>\n\
             Runtime (socket, tray images; cleared at logout): <tt>$XDG_RUNTIME_DIR/nysm</tt>\n\
             Uninstall: <tt>~/.local/share/nysm/uninstall.sh</tt> (<tt>--purge</tt> also \
             removes settings and saved data).",
            esc(&cfg),
            esc(&state)
        ),
    );

    let s = section(page, "Licence");
    text(
        &s,
        "MIT OR Apache-2.0, at your option. Third-party components and their licences are \
         listed in THIRD-PARTY-LICENSES.txt. User guide: docs/guide.md.",
    );
    About { running, cost }
}

/// "10 minutes", "1.5 hours", "2 hours".
fn human_span(d: Duration) -> String {
    let m = d.as_secs_f64() / 60.0;
    if m < 1.0 {
        format!("{} seconds", d.as_secs())
    } else if m < 90.0 {
        format!(
            "{m:.0} minute{}",
            if (m - 1.0).abs() < 0.5 { "" } else { "s" }
        )
    } else {
        let h = m / 60.0;
        if (h - h.round()).abs() < 0.05 {
            format!("{h:.0} hours")
        } else {
            format!("{h:.1} hours")
        }
    }
}

impl About {
    /// Refresh the live parts (called while the page is shown).
    pub fn update(&self, s: &Snapshot, attached: bool) {
        let trays = crate::trayctl::running().len();
        self.running.set_markup(&format!(
            "<small>Now: data from {} · top bar {} · this window collects {}.</small>",
            if attached {
                "the collector service"
            } else {
                "this window (no collector service running)"
            },
            if trays > 0 { "running" } else { "not running" },
            if attached {
                "nothing itself"
            } else {
                "for itself"
            }
        ));
        let Some(t) = &s.processes else {
            return;
        };
        let cores = s.cpu.logical_cores.value.unwrap_or(1).max(1) as f64;
        let mut rows = Vec::new();
        let (mut cpu, mut rss) = (0.0, 0u64);
        for p in &t.entries {
            let role = match p.name.as_str() {
                "nysm" => "collector / terminal",
                "nysm-tray" => "top bar",
                "nysm-desktop" => "window",
                _ => continue,
            };
            let c = p.cpu_pct.live().copied();
            cpu += c.unwrap_or(0.0);
            rss += p.rss_bytes;
            rows.push(format!(
                "{role} (PID {}): CPU {} ({} of one core) · memory {}",
                p.id.pid,
                c.map_or("—".into(), |c| format!("{c:.2}%")),
                c.map_or("—".into(), |c| format!("{:.1}%", c * cores)),
                units::bytes(p.rss_bytes as f64)
            ));
        }
        rows.push(format!(
            "<b>Total: CPU {cpu:.2}% of all cores ({:.1}% of one core) · memory {}</b>",
            cpu * cores,
            units::bytes(rss as f64)
        ));
        self.cost.set_markup(&rows.join("\n"));
    }
}
