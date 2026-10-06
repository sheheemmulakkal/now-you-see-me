//! Settings dialog: a few common options, written to the config file with
//! `nysm_config::set_many` (the file's comments are kept; an invalid file is
//! never overwritten). Changes apply the next time the app starts.

use std::time::Duration;

use gtk::prelude::*;
use nysm_config::Settings;
use nysm_core::units::RateUnit;

/// One dropdown: (label, config value). A `None` value means "keep the
/// current, custom value" and writes nothing.
struct Choice {
    key: &'static str,
    title: &'static str,
    options: Vec<(String, Option<&'static str>)>,
    selected: u32,
}

fn duration_choice(
    key: &'static str,
    title: &'static str,
    current: Duration,
    options: &[(&'static str, &'static str, Duration)],
) -> Choice {
    let mut opts: Vec<(String, Option<&'static str>)> = options
        .iter()
        .map(|(label, v, _)| (label.to_string(), Some(*v)))
        .collect();
    let selected = match options.iter().position(|(_, _, d)| *d == current) {
        Some(i) => i as u32,
        None => {
            opts.insert(
                0,
                (
                    format!(
                        "{} (current)",
                        nysm_core::units::duration_s(current.as_secs_f64())
                    ),
                    None,
                ),
            );
            0
        }
    };
    Choice {
        key,
        title,
        options: opts,
        selected,
    }
}

fn choices(s: &Settings) -> Vec<Choice> {
    let secs = Duration::from_secs;
    vec![
        Choice {
            key: "display.rate_unit",
            title: "Network rates",
            options: vec![
                ("Bytes per second (KiB/s, MiB/s)".into(), Some("bytes")),
                ("Bits per second (kb/s, Mb/s)".into(), Some("bits")),
            ],
            selected: (s.rate_unit == RateUnit::Bits) as u32,
        },
        duration_choice(
            "sampling.interval",
            "Sample every",
            s.interval,
            &[
                ("1 second", "1s", secs(1)),
                ("2 seconds", "2s", secs(2)),
                ("5 seconds", "5s", secs(5)),
            ],
        ),
        duration_choice(
            "sampling.history",
            "Keep history for",
            s.history,
            &[
                ("10 minutes", "10m", secs(600)),
                ("30 minutes", "30m", secs(1800)),
                ("1 hour", "1h", secs(3600)),
            ],
        ),
    ]
}

pub fn open(parent: &gtk::ApplicationWindow, current: &Settings, attached: bool) {
    let win = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body.set_margin_top(18);
    body.set_margin_bottom(18);
    body.set_margin_start(18);
    body.set_margin_end(18);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(10);
    grid.set_column_spacing(16);
    let choices = choices(current);
    let mut dropdowns = Vec::new();
    for (row, c) in choices.iter().enumerate() {
        let l = gtk::Label::new(Some(c.title));
        l.set_xalign(0.0);
        let labels: Vec<&str> = c.options.iter().map(|(l, _)| l.as_str()).collect();
        let dd = gtk::DropDown::from_strings(&labels);
        dd.set_selected(c.selected);
        dd.set_hexpand(true);
        l.set_mnemonic_widget(Some(&dd));
        grid.attach(&l, 0, row as i32, 1, 1);
        grid.attach(&dd, 1, row as i32, 1, 1);
        dropdowns.push(dd);
    }
    body.append(&grid);
    body.append(&tray_section());

    let path = nysm_config::default_path();
    let mut note = String::from(
        "Sampling and units apply the next time Now You See Me starts; top-bar settings apply at once.",
    );
    if attached {
        note.push_str(
            " Sampling comes from the collector service while attached; restart it \
             (nysm service stop, then nysm service run) to use new sampling settings.",
        );
    }
    if let Some(p) = &path {
        note.push_str(&format!("\nSaved in {}", p.display()));
    }
    let note_l = gtk::Label::new(Some(&note));
    note_l.set_xalign(0.0);
    note_l.set_wrap(true);
    note_l.set_max_width_chars(52);
    note_l.add_css_class("dim-label");
    note_l.set_selectable(true);
    body.append(&note_l);
    // Space is reserved so the result message does not resize the dialog.
    let status = gtk::Label::new(Some(" "));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_max_width_chars(52);
    body.append(&status);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    body.append(&buttons);
    win.set_child(Some(&body));
    win.set_default_widget(Some(&save));

    let w = win.clone();
    cancel.connect_clicked(move |_| w.close());
    let esc = gtk::ShortcutController::new();
    esc.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    win.add_controller(esc);
    let cancel_b = cancel.clone();
    save.connect_clicked(move |save| {
        let Some(path) = &path else {
            status.set_text("No configuration location ($HOME is not set).");
            return;
        };
        let values: Vec<(&str, &str)> = choices
            .iter()
            .zip(&dropdowns)
            .filter(|(c, dd)| dd.selected() != c.selected)
            .filter_map(|(c, dd)| {
                c.options
                    .get(dd.selected() as usize)
                    .and_then(|(_, v)| *v)
                    .map(|v| (c.key, v))
            })
            .collect();
        if values.is_empty() {
            status.set_text("Nothing changed.");
            return;
        }
        match nysm_config::set_many(path, &values) {
            Ok(_) => {
                status.set_text("Saved. Restart Now You See Me to apply.");
                save.set_sensitive(false);
                cancel_b.set_label("Close");
            }
            Err(e) => status.set_text(&format!("Not saved: {e}")),
        }
    });
    win.present();
}

/// Remember the header theme choice (best effort; a broken config file is
/// left alone and only reported).
pub fn save_theme(theme: nysm_config::Theme) {
    let value = match theme {
        nysm_config::Theme::System => "system",
        nysm_config::Theme::Light => "light",
        nysm_config::Theme::Dark => "dark",
    };
    if let Some(p) = nysm_config::default_path()
        && let Err(e) = nysm_config::set_many(&p, &[("display.theme", value)])
    {
        eprintln!("nysm-desktop: theme not saved: {e}");
    }
}

/// Tray items offered here, in top-bar order: (config key, label).
const TRAY_ITEMS: [(&str, &str); 6] = [
    ("cpu", "CPU usage"),
    ("mem", "Memory used"),
    ("net", "Network"),
    ("storage", "Storage used %"),
    ("disk", "Disk activity %"),
    ("diskio", "Disk read / write"),
];

/// "Top bar" controls: run the tray, start it at login, choose its items.
/// All apply immediately.
fn tray_section() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let title = small_label("Top bar (tray)");
    title.add_css_class("card-title");
    b.append(&title);
    let status = small_label("");
    status.add_css_class("dim-label");
    status.set_wrap(true);
    status.set_max_width_chars(52);

    let row = |text: &str, tip: &str, on: bool| {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let l = small_label(text);
        l.set_hexpand(true);
        l.set_tooltip_text(Some(tip));
        let sw = gtk::Switch::new();
        sw.set_active(on);
        sw.set_valign(gtk::Align::Center);
        sw.set_tooltip_text(Some(tip));
        r.append(&l);
        r.append(&sw);
        (r, sw)
    };
    let (r1, show) = row(
        "Show in top bar",
        "Start or stop the tray indicator now",
        !crate::trayctl::running().is_empty(),
    );
    let (r2, login) = row(
        "Start at login",
        "Add or remove ~/.config/autostart/nysm-tray.desktop",
        crate::trayctl::autostart_enabled(),
    );
    b.append(&r1);
    b.append(&r2);

    let st = status.clone();
    show.connect_active_notify(move |sw| {
        if sw.is_active() {
            match crate::trayctl::start() {
                Ok(()) => st.set_text("Tray started."),
                Err(e) => {
                    st.set_text(&e);
                    sw.set_active(false);
                }
            }
        } else {
            crate::trayctl::stop();
            st.set_text("Tray stopped.");
        }
    });
    let st = status.clone();
    login.connect_active_notify(
        move |sw| match crate::trayctl::set_autostart(sw.is_active()) {
            Ok(()) => st.set_text(if sw.is_active() {
                "The tray will start when you log in."
            } else {
                "The tray will no longer start at login."
            }),
            Err(e) => st.set_text(&format!("Not changed: {e}")),
        },
    );

    // Items: read the saved choice now (it may have changed in the tray).
    let path = nysm_config::default_path();
    let (cur, _) = nysm_config::load_or_default(path.as_deref());
    let chosen: Vec<String> = cur
        .tray_items
        .split(',')
        .map(|x| x.trim().to_string())
        .collect();
    let items_l = small_label("Shown in the top bar");
    items_l.add_css_class("dim-label");
    b.append(&items_l);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_max_children_per_line(3);
    flow.set_homogeneous(true);
    let checks: Vec<gtk::CheckButton> = TRAY_ITEMS
        .iter()
        .map(|(key, label)| {
            let c = gtk::CheckButton::with_label(label);
            let on = chosen.iter().any(|x| {
                x == key || (*key == "mem" && x == "memory") || (*key == "net" && x == "network")
            });
            c.set_active(on);
            flow.insert(&c, -1);
            c
        })
        .collect();
    b.append(&flow);
    // Icons, names, or both (at least one stays on).
    let style = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icons = gtk::CheckButton::with_label("Show icons");
    icons.set_active(cur.tray_icons);
    let names = gtk::CheckButton::with_label("Show names (CPU, RAM, Disk, I/O)");
    names.set_active(cur.tray_names || !cur.tray_icons);
    style.append(&icons);
    style.append(&names);
    for (this, other, key) in [
        (icons.clone(), names.clone(), "display.tray_icons"),
        (names.clone(), icons.clone(), "display.tray_names"),
    ] {
        let st = status.clone();
        this.connect_toggled(move |c| {
            if !c.is_active() && !other.is_active() {
                c.set_active(true);
                st.set_text("Icons, names, or both — at least one is shown.");
                return;
            }
            let v = if c.is_active() { "true" } else { "false" };
            match nysm_config::default_path()
                .ok_or_else(|| "no configuration location".to_string())
                .and_then(|p| nysm_config::set_many(&p, &[(key, v)]).map_err(|e| e.to_string()))
            {
                Ok(_) => st.set_text("Saved; the running tray updates within a few seconds."),
                Err(e) => st.set_text(&format!("Not saved: {e}")),
            }
        });
    }
    b.append(&style);
    let checks = std::rc::Rc::new(checks);
    for c in checks.iter() {
        let (all, st) = (checks.clone(), status.clone());
        c.connect_toggled(move |c| {
            let keys: Vec<&str> = TRAY_ITEMS
                .iter()
                .zip(all.iter())
                .filter(|(_, c)| c.is_active())
                .map(|((k, _), _)| *k)
                .collect();
            if keys.is_empty() {
                // At least one item stays.
                c.set_active(true);
                st.set_text("At least one item is always shown.");
                return;
            }
            match nysm_config::default_path()
                .ok_or_else(|| "no configuration location".to_string())
                .and_then(|p| {
                    nysm_config::set_many(&p, &[("display.tray_items", &keys.join(","))])
                        .map_err(|e| e.to_string())
                }) {
                Ok(_) => st.set_text("Saved; the running tray updates within a few seconds."),
                Err(e) => st.set_text(&format!("Not saved: {e}")),
            }
        });
    }
    b.append(&status);
    b
}

fn small_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l
}
