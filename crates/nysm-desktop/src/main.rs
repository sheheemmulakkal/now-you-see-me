//! `nysm-desktop`: native GTK4 front end. Uses the per-user collector
//! service when it is running, otherwise an embedded collector. All values
//! and formulas come from `nysm-core`; this crate only presents them.

mod chart;
mod pages;
mod proctable;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gio, glib};
use nysm_config::Theme;
use nysm_ipc::source::{Attach, Source};

/// Styling after the reference design: soft cards, large tabular values,
/// a warm light theme and a calm dark theme. Colours for series dots match
/// `chart.rs`.
const CSS: &str = "
.page-title { font-size: 1.45em; font-weight: 600; margin-bottom: 2px; }
.card { background-color: alpha(currentColor, 0.045); border: 1px solid alpha(currentColor, 0.07);
        border-radius: 12px; padding: 14px 16px; }
.card-title { font-weight: 600; }
.big-value { font-size: 2.1em; font-weight: 300; font-feature-settings: \"tnum\"; }
.mid-value { font-size: 1.35em; font-weight: 400; font-feature-settings: \"tnum\"; }
.numeric { font-feature-settings: \"tnum\"; }
.caption { font-size: 0.85em; }
.c-net-rx { color: #4f8cff; } .c-net-tx { color: #a66bff; }
.c-disk-r { color: #f5a623; } .c-disk-w { color: #ed7321; }
.alert-banner { background: #c01c28; color: white; padding: 8px 12px; border-radius: 8px; font-weight: bold; }
.status-live { color: #2ec4b5; } .status-stale { color: #e5a50a; }
.flat-list, .flat-list row { background: transparent; }
.proc-table > listview > row > cell { padding: 3px 6px; }
.flat-list row { border-bottom: 1px solid alpha(currentColor, 0.06); }
.nav { background: transparent; padding: 8px; }
.nav row { border-radius: 8px; padding: 8px 10px; margin: 1px 0; }
.nav row:selected { background-color: alpha(#7c6cff, 0.22); color: inherit; }
.sidebar { background-color: alpha(currentColor, 0.03); border-right: 1px solid alpha(currentColor, 0.07); }
levelbar.capacity block.filled { background-color: #6c5ce7; }
window.warm { background-color: #f7f4ef; }
window.warm .card { background-color: #ffffff; border-color: rgba(0, 0, 0, 0.07); }
window.warm .sidebar { background-color: #f1ede6; }
";

const PAGES: [(&str, &str, &str); 7] = [
    ("overview", "Overview", "view-grid-symbolic"),
    ("cpu", "CPU", "utilities-system-monitor-symbolic"),
    ("memory", "Memory", "media-flash-symbolic"),
    ("network", "Network", "network-wired-symbolic"),
    ("storage", "Storage", "drive-harddisk-symbolic"),
    (
        "groups",
        "Containers & services",
        "application-x-executable-symbolic",
    ),
    ("processes", "Processes", "view-list-symbolic"),
];

const RANGES: [(&str, f64); 3] = [
    ("Last 1 minute", 60.0),
    ("Last 5 minutes", 300.0),
    ("Last 10 minutes", 600.0),
];

struct Args {
    attach: Attach,
    screenshot: Option<std::path::PathBuf>,
    page: Option<String>,
    theme: Option<Theme>,
    delay: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        attach: Attach::Auto,
        screenshot: None,
        page: None,
        theme: None,
        delay: Duration::from_secs(4),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--attach" => {
                a.attach = match it.next().as_deref() {
                    Some("auto") => Attach::Auto,
                    Some("never") => Attach::Never,
                    Some("require") => Attach::Require,
                    other => return Err(format!("--attach auto|never|require (got {other:?})")),
                }
            }
            "--theme" => {
                a.theme = Some(match it.next().as_deref() {
                    Some("system") => Theme::System,
                    Some("light") => Theme::Light,
                    Some("dark") => Theme::Dark,
                    other => return Err(format!("--theme system|light|dark (got {other:?})")),
                })
            }
            // Development aid: render the window to a PNG and exit.
            "--screenshot" => a.screenshot = it.next().map(Into::into),
            "--page" => a.page = it.next(),
            "--delay" => {
                a.delay = it
                    .next()
                    .and_then(|d| nysm_core::units::parse_duration(&d))
                    .ok_or("--delay needs a duration such as 4s")?;
            }
            "-h" | "--help" => {
                println!(
                    "nysm-desktop [--attach auto|never|require] [--theme system|light|dark]\n             \
                     [--page overview|cpu|memory|network|storage|processes]\n\
                     \n  --screenshot FILE.png  render the window to FILE after --delay (default 4s) and exit"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }
    Ok(a)
}

/// GNOME's dark-style preference, if the schema is available.
fn system_prefers_dark() -> Option<bool> {
    let src = gio::SettingsSchemaSource::default()?;
    let schema = src.lookup("org.gnome.desktop.interface", true)?;
    if !schema.has_key("color-scheme") {
        return None;
    }
    Some(gio::Settings::new("org.gnome.desktop.interface").string("color-scheme") == "prefer-dark")
}

fn apply_theme(window: &gtk::ApplicationWindow, theme: Theme) {
    let dark = match theme {
        Theme::Dark => true,
        Theme::Light => false,
        Theme::System => system_prefers_dark().unwrap_or(false),
    };
    if let Some(gs) = gtk::Settings::default() {
        gs.set_gtk_application_prefer_dark_theme(dark);
    }
    if dark {
        window.remove_css_class("warm");
    } else {
        window.add_css_class("warm");
    }
}

fn main() -> glib::ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("nysm-desktop: {e}");
            return glib::ExitCode::from(2);
        }
    };
    let path = nysm_config::default_path();
    let (settings, err) = nysm_config::load_or_default(path.as_deref());
    if let Some(e) = err {
        eprintln!("nysm-desktop: warning: ignoring invalid configuration, using defaults: {e}");
    }
    let app = gtk::Application::builder()
        .application_id(nysm_core::brand::APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let args = Rc::new(args);
    let settings = Rc::new(settings);
    app.connect_activate(move |app| build(app, &args, &settings));
    // Our own arguments are parsed above; don't let GTK see them.
    app.run_with_args(&[] as &[&str])
}

fn engine_config(s: &nysm_config::Settings) -> nysm_engine::EngineConfig {
    let samples = (s.history.as_secs_f64() / s.interval.as_secs_f64())
        .ceil()
        .max(1.0) as usize;
    nysm_engine::EngineConfig {
        interval: s.interval,
        processes: true,
        process_interval: s.process_interval,
        cgroups: false,
        filesystems: true,
        filesystem_interval: s.filesystem_interval,
        frequency: s.cpu_frequency,
        sensors: true,
        sensor_interval: std::time::Duration::from_secs(5),
        history_samples: samples,
        history_bytes: (samples * 64).min(4 << 20),
    }
}

fn sidebar(stack: &gtk::Stack) -> gtk::Box {
    let list = gtk::ListBox::new();
    list.add_css_class("nav");
    list.set_selection_mode(gtk::SelectionMode::Single);
    for (_, title, icon) in PAGES {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.append(&gtk::Image::from_icon_name(icon));
        let l = gtk::Label::new(Some(title));
        l.set_xalign(0.0);
        b.append(&l);
        list.append(&b);
    }
    let s = stack.clone();
    list.connect_row_selected(move |_, row| {
        if let Some(r) = row
            && let Some((name, _, _)) = PAGES.get(r.index() as usize)
        {
            s.set_visible_child_name(name);
        }
    });
    // Keep the selection in sync when the page changes programmatically.
    let l = list.clone();
    stack.connect_visible_child_name_notify(move |st| {
        let name = st
            .visible_child_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        if let Some(i) = PAGES.iter().position(|(n, _, _)| *n == name)
            && l.selected_row().map(|r| r.index()) != Some(i as i32)
        {
            l.select_row(l.row_at_index(i as i32).as_ref());
        }
    });
    list.select_row(list.row_at_index(0).as_ref());
    let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side.add_css_class("sidebar");
    side.set_width_request(190);
    side.append(&list);
    side
}

fn build(app: &gtk::Application, args: &Rc<Args>, settings: &Rc<nysm_config::Settings>) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let source = match Source::open(
        args.attach,
        concat!("nysm-desktop/", env!("CARGO_PKG_VERSION")),
        engine_config(settings),
        settings.rules.clone(),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("nysm-desktop: cannot attach to the collector service: {e}");
            app.quit();
            return;
        }
    };

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(nysm_core::brand::PRODUCT_NAME)
        .default_width(1180)
        .default_height(820)
        .build();
    let theme = args.theme.unwrap_or(settings.theme);
    apply_theme(&window, theme);

    let header = gtk::HeaderBar::new();
    let status = gtk::Label::new(None);
    let alert_badge = gtk::Label::new(None);
    alert_badge.add_css_class("alert-banner");
    alert_badge.set_visible(false);
    let range = gtk::DropDown::from_strings(&RANGES.map(|(l, _)| l));
    range.set_selected(1);
    range.set_tooltip_text(Some("Time range shown in charts"));
    let theme_dd = gtk::DropDown::from_strings(&["System theme", "Light", "Dark"]);
    theme_dd.set_selected(match theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    });
    header.pack_end(&theme_dd);
    header.pack_end(&range);
    header.pack_end(&status);
    header.pack_end(&alert_badge);
    window.set_titlebar(Some(&header));
    {
        let w = window.clone();
        theme_dd.connect_selected_notify(move |dd| {
            let t = match dd.selected() {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::System,
            };
            apply_theme(&w, t);
        });
    }
    if theme == Theme::System
        && gio::SettingsSchemaSource::default()
            .and_then(|s| s.lookup("org.gnome.desktop.interface", true))
            .is_some()
    {
        // Follow live changes of the desktop preference.
        let gs = gio::Settings::new("org.gnome.desktop.interface");
        let (w, dd) = (window.clone(), theme_dd.clone());
        gs.connect_changed(Some("color-scheme"), move |_, _| {
            if dd.selected() == 0 {
                apply_theme(&w, Theme::System);
            }
        });
        // Keep the settings object (and its signal) alive with the window.
        window.connect_destroy(move |_| {
            let _ = &gs;
        });
    }

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None); // no motion
    stack.set_hexpand(true);
    let ui = pages::Pages::new(&stack, settings.rate_unit, source_label(&source));
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&sidebar(&stack));
    body.append(&stack);
    window.set_child(Some(&body));
    if let Some(p) = &args.page {
        stack.set_visible_child_name(p);
    }

    let state = Rc::new(RefCell::new(Loop {
        source,
        cgroups_on: false,
        last_seq: 0,
        last_at: Instant::now(),
        pending: None,
    }));
    let ui = Rc::new(ui);
    {
        let (state, ui) = (state.clone(), ui.clone());
        range.connect_selected_notify(move |dd| {
            ui.set_window_s(RANGES.get(dd.selected() as usize).map_or(300.0, |r| r.1));
            state.borrow_mut().last_seq = 0;
        });
    }
    {
        let (state, ui, window, status, alert_badge, settings) = (
            state.clone(),
            ui.clone(),
            window.clone(),
            status.clone(),
            alert_badge.clone(),
            settings.clone(),
        );
        glib::timeout_add_local(Duration::from_millis(250), move || {
            tick(&state, &ui, &window, &status, &alert_badge, &settings);
            glib::ControlFlow::Continue
        });
    }
    {
        let (state, ui) = (state.clone(), ui.clone());
        ui.on_details_requested(move |pid, start| {
            let mut st = state.borrow_mut();
            let rx = st.source.request_details(pid, Some(start));
            st.pending = Some(rx);
        });
    }
    {
        // Refresh a page as soon as it becomes visible (hidden pages are
        // not updated).
        let state = state.clone();
        stack.connect_visible_child_notify(move |st| {
            let mut s = state.borrow_mut();
            s.last_seq = 0;
            // Only collect cgroups while their page is open.
            let want = st.visible_child_name().is_some_and(|n| n == "groups");
            if want != s.cgroups_on {
                s.source.set_cgroups(want);
                s.cgroups_on = want;
            }
        });
    }
    if stack.visible_child_name().is_some_and(|n| n == "groups") {
        let mut s = state.borrow_mut();
        s.source.set_cgroups(true);
        s.cgroups_on = true;
    }
    {
        let (kind, sort) = ui.groups_controls();
        for dd in [kind, sort] {
            let state = state.clone();
            dd.connect_selected_notify(move |_| state.borrow_mut().last_seq = 0);
        }
    }
    window.present();

    if let Some(path) = args.screenshot.clone() {
        let window = window.clone();
        let app = app.clone();
        glib::timeout_add_local_once(args.delay, move || {
            match screenshot(&window, &path) {
                Ok(()) => eprintln!("nysm-desktop: wrote {}", path.display()),
                Err(e) => eprintln!("nysm-desktop: screenshot failed: {e}"),
            }
            app.quit();
        });
    }
}

fn source_label(s: &Source) -> String {
    if s.is_remote() {
        "Data: collector service".into()
    } else {
        "Data: embedded collector".into()
    }
}

struct Loop {
    source: Source,
    cgroups_on: bool,
    last_seq: u64,
    last_at: Instant,
    pending: Option<std::sync::mpsc::Receiver<nysm_collect::CResult<nysm_collect::ProcessDetails>>>,
}

fn window_hidden(window: &gtk::ApplicationWindow) -> bool {
    use gtk::gdk::prelude::ToplevelExt;
    if !window.is_visible() {
        return true;
    }
    window
        .surface()
        .and_then(|s| s.downcast::<gtk::gdk::Toplevel>().ok())
        .is_some_and(|t| t.state().contains(gtk::gdk::ToplevelState::MINIMIZED))
}

fn tick(
    state: &Rc<RefCell<Loop>>,
    ui: &pages::Pages,
    window: &gtk::ApplicationWindow,
    status: &gtk::Label,
    alert_badge: &gtk::Label,
    settings: &nysm_config::Settings,
) {
    let mut st = state.borrow_mut();
    if !st.source.is_connected() {
        st.source = Source::local(engine_config(settings), settings.rules.clone());
        st.last_seq = 0;
        if st.cgroups_on {
            st.source.set_cgroups(true);
        }
        ui.set_source(&format!(
            "{} (service disconnected)",
            source_label(&st.source)
        ));
    }
    if let Some(rx) = &st.pending
        && let Ok(r) = rx.try_recv()
    {
        ui.show_details(r);
        st.pending = None;
    }
    // Hidden or minimised windows do no rendering work at all.
    if window_hidden(window) {
        return;
    }
    let Some(snap) = st.source.latest() else {
        status.set_text("collecting…");
        return;
    };
    let stale = st.last_at.elapsed() > settings.interval * 3 + Duration::from_secs(1);
    status.set_text(if stale { "● stale" } else { "● live" });
    status.set_css_classes(&[if stale { "status-stale" } else { "status-live" }]);
    if snap.seq == st.last_seq {
        return;
    }
    st.last_seq = snap.seq;
    st.last_at = Instant::now();
    let (history, alerts) = st.source.with_state(|s| {
        (
            s.history.iter().copied().collect::<Vec<_>>(),
            s.alerts.clone(),
        )
    });
    let firing = alerts
        .iter()
        .filter(|a| matches!(a.state, nysm_core::alerts::AlertState::Firing { .. }))
        .count();
    alert_badge.set_visible(firing > 0);
    alert_badge.set_text(&format!(
        "{firing} alert{}",
        if firing == 1 { "" } else { "s" }
    ));
    let visible = ui.visible_page();
    ui.update(&snap, &history, &alerts, &visible);
}

fn screenshot(window: &gtk::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    // Snapshot the children directly instead of relying on the last painted
    // frame (headless backends may not paint without a viewer).
    let (w, h) = (window.width() as f32, window.height() as f32);
    let snapshot = gtk::Snapshot::new();
    let dark = gtk::Settings::default().is_some_and(|s| s.is_gtk_application_prefer_dark_theme());
    let bg = if dark {
        gtk::gdk::RGBA::new(0.14, 0.15, 0.17, 1.0)
    } else {
        gtk::gdk::RGBA::new(0.97, 0.957, 0.937, 1.0)
    };
    snapshot.append_color(&bg, &gtk::graphene::Rect::new(0.0, 0.0, w, h));
    // Lay out synchronously: headless backends may not run frames.
    let mut top = 0;
    if let Some(tb) = window.titlebar() {
        let (_, nat, _, _) = tb.measure(gtk::Orientation::Vertical, w as i32);
        tb.allocate(w as i32, nat, -1, None);
        window.snapshot_child(&tb, &snapshot);
        top = nat;
    }
    if let Some(child) = window.child() {
        let shift =
            gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(0.0, top as f32));
        child.allocate(w as i32, h as i32 - top, -1, Some(shift));
        window.snapshot_child(&child, &snapshot);
    }
    let node = snapshot.to_node().ok_or("nothing rendered")?;
    let renderer = window
        .native()
        .and_then(|n| n.renderer())
        .ok_or("no renderer")?;
    let texture = renderer.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, w, h)));
    texture.save_to_png(path).map_err(|e| e.to_string())
}
