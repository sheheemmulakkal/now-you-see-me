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

    let path = nysm_config::default_path();
    let mut note = String::from("Changes apply the next time Now You See Me starts.");
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
