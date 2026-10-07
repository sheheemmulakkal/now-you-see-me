//! About page: what the app is, what is running and what it costs right
//! now (measured live from its own processes), how it collects and keeps
//! data, the top-bar labels, privacy, files and licence.

use std::time::Duration;

use gtk::glib::markup_escape_text as esc;
use gtk::prelude::*;
use nysm_core::snapshot::Snapshot;
use nysm_core::units;

use crate::stats::{self, Tile};

pub struct About {
    status: Vec<Tile>,
    totals: Vec<Tile>,
    procs: gtk::Grid,
    history: Duration,
    interval: Duration,
}

fn label(markup: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(markup);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn card(title: &str) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 8);
    c.add_css_class("card");
    c.append(&label(&esc(title), &["card-title"]));
    c
}

/// A card with a symbolic icon next to its title, for the info grid.
fn info_card(icon: &str, title: &str, markup: &str) -> gtk::Box {
    let c = gtk::Box::new(gtk::Orientation::Vertical, 8);
    c.add_css_class("card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(18);
    head.append(&img);
    head.append(&label(&esc(title), &["card-title"]));
    c.append(&head);
    let body = label(markup, &[]);
    body.set_selectable(true);
    c.append(&body);
    c
}

/// "10 minutes", "1.5 hours", "2 hours".
fn human_span(d: Duration) -> String {
    let m = d.as_secs_f64() / 60.0;
    if m < 1.0 {
        format!("{} s", d.as_secs())
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

/// Build the page into `page` (a vertical box from the page stack).
pub fn build(page: &gtk::Box, history: Duration, interval: Duration) -> About {
    // Header: icon, name, version, licence, one-line summary.
    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    hero.add_css_class("card");
    // The app's own icon when installed, else a generic one.
    let has_own = gtk::gdk::Display::default()
        .map(|d| gtk::IconTheme::for_display(&d).has_icon(nysm_core::brand::APP_ID))
        .unwrap_or(false);
    let icon = gtk::Image::from_icon_name(if has_own {
        nysm_core::brand::APP_ID
    } else {
        "utilities-system-monitor"
    });
    icon.set_pixel_size(64);
    icon.set_valign(gtk::Align::Center);
    hero.append(&icon);
    let hv = gtk::Box::new(gtk::Orientation::Vertical, 4);
    hv.set_valign(gtk::Align::Center);
    hv.append(&label(&esc(nysm_core::brand::PRODUCT_NAME), &["mid-value"]));
    hv.append(&label(
        &format!(
            "Version {} · MIT OR Apache-2.0 · runs as you, offline, no telemetry",
            env!("CARGO_PKG_VERSION")
        ),
        &["dim-label", "caption"],
    ));
    hv.append(&label(
        "See what this computer is doing, what changed in the last minutes, <i>which</i> \
         process or container is responsible, and whether work is <i>waiting</i>. \
         Values that cannot be measured show why, never a fake 0.",
        &[],
    ));
    hero.append(&hv);
    page.append(&hero);

    // Right now.
    let now = card("Right now");
    let (grid, status) = stats::grid(&[
        (
            "Data comes from",
            "The collector service samples once for every window and the top bar",
        ),
        (
            "Top bar",
            "nysm-tray; turn it on or off in Settings (Ctrl+,)",
        ),
        ("Sampling", "How often values are read from the kernel"),
        (
            "History kept",
            "Shown in charts; change it in Settings (Ctrl+,)",
        ),
    ]);
    now.append(&grid);
    page.append(&now);

    // Cost.
    let cost = card("What it costs right now");
    let (tgrid, totals) = stats::grid(&[
        (
            "CPU, all parts",
            "Sum over the collector, the top bar and every open window",
        ),
        ("Memory, all parts", "Resident memory of those processes"),
    ]);
    cost.append(&tgrid);
    let procs = gtk::Grid::new();
    procs.set_column_spacing(24);
    procs.set_row_spacing(4);
    cost.append(&procs);
    cost.append(&label(
        "<small>CPU is a share of one core. Reference (Intel i5-7500): collector 0.2–0.4 % and \
         3 MiB (about 0.8 % while a process list is open), top bar 0.03–0.2 % and 5 MiB, a window ~2 % while visible and nothing while \
         minimised; GNOME Shell spends ~1 % more to redraw the top bar. Pages you are not \
         looking at are not updated.</small>",
        &["dim-label"],
    ));
    page.append(&cost);

    // Info cards, two per row.
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(2);
    flow.set_row_spacing(14);
    flow.set_column_spacing(14);
    let cfg = nysm_config::default_path().map_or("—".into(), |p| p.display().to_string());
    let state = nysm_config::default_incident_dir()
        .and_then(|p| p.parent().map(|d| d.display().to_string()))
        .unwrap_or_else(|| "—".into());
    let files = format!(
        "Settings <tt>{}</tt>\nSaved data <tt>{}</tt>\nRuntime (cleared at logout) \
         <tt>$XDG_RUNTIME_DIR/nysm</tt>\nUninstall <tt>~/.local/share/nysm/uninstall.sh</tt> \
         (<tt>--purge</tt> also removes settings and saved data)",
        esc(&cfg),
        esc(&state)
    );
    for c in [
        info_card(
            "utilities-system-monitor-symbolic",
            "How the numbers are collected",
            "Read directly from the Linux kernel (<tt>/proc</tt>, <tt>/sys</tt>) as a normal \
             user. Rates such as CPU % and bytes/s are the change between two samples. Other \
             users' processes show \"permission denied\" where the kernel only answers their \
             owner; nothing is guessed.",
        ),
        info_card(
            "document-open-recent-symbolic",
            "History",
            "Recent history lives in memory in the collector, so charts fill in as soon as a \
             window opens. It is lost when the collector stops. For days of data record to a \
             file: <tt>nysm record -o day.nysm --duration 24h --interval 10s</tt>, then \
             <tt>nysm compare day.nysm</tt>.",
        ),
        info_card(
            "view-grid-symbolic",
            "Top-bar labels",
            "<b>CPU</b> usage of all cores · <b>RAM</b> memory in use · <b>↓ ↑</b> network \
             download / upload · <b>Disk</b> storage used on <tt>/</tt> · <b>I/O</b> how busy \
             the disk is · <b>R / W</b> disk read / write per second. Click the top bar to \
             open this window or choose what it shows (Top bar menu).",
        ),
        info_card(
            "security-high-symbolic",
            "Privacy and safety",
            "No network access unless you ask (<tt>nysm net check</tt>, remote over your own \
             SSH). No accounts, no telemetry, never root. Command lines are hidden unless you \
             ask, because they can hold passwords. Container names are opt-in: the Docker \
             socket is root-equivalent. Its files are readable only by you.",
        ),
        info_card("folder-symbolic", "Files", &files),
        info_card(
            "text-x-generic-symbolic",
            "Licence and help",
            "MIT OR Apache-2.0, at your option. Third-party components and their licences: \
             <tt>THIRD-PARTY-LICENSES.txt</tt>. User guide: <tt>docs/guide.md</tt>. Every \
             command has <tt>--help</tt>; <tt>nysm doctor</tt> checks this machine.",
        ),
    ] {
        flow.insert(&c, -1);
    }
    page.append(&flow);
    About {
        status,
        totals,
        procs,
        history,
        interval,
    }
}

impl About {
    /// Refresh the live parts (called while the page is shown).
    pub fn update(&self, s: &Snapshot, attached: bool) {
        let trays = crate::trayctl::running().len();
        let t = &self.status;
        if attached {
            t[0].set("Collector service", "shared by all windows and the top bar");
        } else {
            t[0].set("This window", "no collector service running");
        }
        t[1].set(
            if trays > 0 { "Running" } else { "Off" },
            if trays > 0 {
                "nysm-tray"
            } else {
                "turn it on in Settings"
            },
        );
        t[2].set(
            &format!("Every {}", human_span(self.interval)),
            "processes every 2 s, disks every 15 s",
        );
        t[3].set(&human_span(self.history), "in memory");

        let Some(table) = &s.processes else {
            return;
        };
        let cores = s.cpu.logical_cores.value.unwrap_or(1).max(1) as f64;
        while let Some(c) = self.procs.first_child() {
            self.procs.remove(&c);
        }
        let cell = |text: &str, col: i32, row: i32, xalign: f32, classes: &[&str]| {
            let l = gtk::Label::new(Some(text));
            l.set_xalign(xalign);
            l.add_css_class("numeric");
            for c in classes {
                l.add_css_class(c);
            }
            self.procs.attach(&l, col, row, 1, 1);
        };
        for (i, h) in ["Part", "PID", "CPU (one core)", "Memory"]
            .iter()
            .enumerate()
        {
            let x = if i == 0 { 0.0 } else { 1.0 };
            cell(h, i as i32, 0, x, &["dim-label", "caption"]);
        }
        let (mut cpu, mut rss, mut row) = (0.0, 0u64, 1);
        for p in &table.entries {
            if p.state == 'Z' {
                continue; // exited, not yet reaped by its parent
            }
            let part = match p.name.as_str() {
                "nysm" => "Collector / terminal",
                "nysm-tray" => "Top bar",
                "nysm-desktop" => "Window",
                _ => continue,
            };
            let c = p.cpu_pct.live().copied().map(|c| c * cores);
            cpu += c.unwrap_or(0.0);
            rss += p.rss_bytes;
            cell(part, 0, row, 0.0, &[]);
            cell(&p.id.pid.to_string(), 1, row, 1.0, &["dim-label"]);
            cell(
                &c.map_or("—".into(), |c| format!("{c:.1}%")),
                2,
                row,
                1.0,
                &[],
            );
            cell(&units::bytes(p.rss_bytes as f64), 3, row, 1.0, &[]);
            row += 1;
        }
        self.totals[0].set(&format!("{cpu:.1}%"), "of one core");
        self.totals[1].set(&units::bytes(rss as f64), &format!("{} processes", row - 1));
    }
}
