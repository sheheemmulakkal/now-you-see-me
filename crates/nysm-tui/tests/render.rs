//! Headless rendering tests using ratatui's TestBackend.

use std::sync::Arc;
use std::time::Duration;

use nysm_engine::{Engine, EngineConfig};
use nysm_tui::app::{App, Tab};
use nysm_tui::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn text(t: &Terminal<TestBackend>) -> String {
    let buf = t.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    text(&t)
}

fn app_from(mut e: Engine, samples: usize) -> App {
    let mut app = App::new(false, false, Default::default());
    for i in 0..samples {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(150));
        }
        app.live = Some(Arc::new(e.sample()));
    }
    app.live_history = e.history().iter().copied().collect();
    app
}

fn cfg() -> EngineConfig {
    EngineConfig {
        processes: true,
        process_interval: Duration::ZERO,
        filesystems: false,
        ..Default::default()
    }
}

#[test]
fn unsupported_platform_renders_reasons_not_zeros() {
    let mut app = app_from(Engine::with_platform(nysm_collect::unsupported(), cfg()), 2);
    for tab in Tab::ALL {
        app.tab = tab;
        let s = render(&mut app, 100, 30);
        assert!(!s.contains("panicked"));
        if tab == Tab::Overview {
            assert!(s.contains("unsupported"), "{s}");
            assert!(
                !s.contains("0.0%"),
                "missing CPU must not render as 0.0%:\n{s}"
            );
        }
    }
}

#[test]
fn tiny_terminals_degrade_to_summary() {
    let mut app = app_from(Engine::with_platform(nysm_collect::unsupported(), cfg()), 1);
    let s = render(&mut app, 30, 8);
    assert!(s.contains("CPU"));
    assert!(s.contains("q quit"));
    // Absurdly small must not panic.
    render(&mut app, 1, 1);
    render(&mut app, 0, 0);
}

#[test]
fn no_data_yet_is_explicit() {
    let mut app = App::new(true, false, Default::default());
    assert!(render(&mut app, 80, 24).contains("Collecting first sample"));
}

#[cfg(target_os = "linux")]
#[test]
fn real_linux_data_all_tabs_at_80x24_and_large() {
    let mut app = app_from(Engine::new(cfg()), 3);
    for (w, h) in [(80, 24), (200, 60)] {
        for tab in Tab::ALL {
            app.tab = tab;
            let s = render(&mut app, w, h);
            assert!(s.contains("1 Overview"), "{s}");
        }
    }
    app.tab = Tab::Overview;
    let s = render(&mut app, 80, 24);
    assert!(
        s.contains("CPU") && s.contains("Memory") && s.contains("Network") && s.contains("Disk"),
        "{s}"
    );
    // Pause banner, help overlay and selection.
    app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(render(&mut app, 80, 24).contains("PAUSED"));
    app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(render(&mut app, 80, 24).contains("Help"));
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.selected.is_some());
    app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    render(&mut app, 80, 24);
    // ASCII mode renders without box-drawing characters.
    app.ascii = true;
    app.help = false;
    app.tab = Tab::Overview;
    let s = render(&mut app, 80, 24);
    assert!(!s.contains('│') && !s.contains('█'), "{s}");
}

#[test]
fn hostile_process_names_are_escaped() {
    use nysm_core::sanitize::for_terminal;
    assert!(!for_terminal("\u{1b}[2J").contains('\u{1b}'));
}

#[test]
fn firing_alerts_show_badge_and_overview_line() {
    use nysm_core::alerts::{ActiveAlert, AlertMetric, AlertState};
    let mut app = app_from(Engine::with_platform(nysm_collect::unsupported(), cfg()), 2);
    app.alerts = vec![ActiveAlert {
        rule: "memory-pressure".into(),
        metric: AlertMetric::MemoryPressurePct,
        target: None,
        state: AlertState::Firing {
            since_s: 1.0,
            since_ms: 0,
            notified: true,
            data_missing: false,
        },
        value: Some(23.4),
    }];
    let s = render(&mut app, 100, 30);
    assert!(s.contains("1 alert"), "{s}");
    assert!(
        s.contains("ALERT") && s.contains("memory pressure 23.4%"),
        "{s}"
    );
    // Pending alerts are not shown as firing.
    app.alerts[0].state = AlertState::Pending { since_s: 0.0 };
    assert!(!render(&mut app, 100, 30).contains("1 alert"));
}

#[cfg(target_os = "linux")]
#[test]
fn pinned_process_details_show_trends() {
    use nysm_tui::app::Action;
    let mut e = Engine::new(cfg());
    let mut app = App::new(false, false, Default::default());
    app.live = Some(Arc::new(e.sample()));
    app.tab = Tab::Processes;
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let id = app.selected.clone().unwrap();
    let Action::Pin(pid, name) = app.on_key(KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE))
    else {
        panic!("expected a pin action")
    };
    e.pin(pid, name).unwrap();
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(150));
        app.live = Some(Arc::new(e.sample()));
    }
    app.pinned = e.pinned().to_vec();
    assert!(app.is_pinned(&id));
    assert!(matches!(
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Action::RequestDetails(_)
    ));
    let s = render(&mut app, 120, 40);
    assert!(s.contains("pinned CPU") && s.contains("pinned RSS"), "{s}");
    assert!(s.contains("* "), "pinned marker in table");
    assert!(matches!(
        app.on_key(KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE)),
        Action::Unpin(_)
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn groups_view_renders_real_cgroups() {
    let mut cfg = cfg();
    cfg.cgroups = true;
    let mut app = app_from(Engine::new(cfg), 2);
    app.tab = Tab::Groups;
    let s = render(&mut app, 120, 30);
    assert!(s.contains("Groups") || s.contains("cgroup"), "{s}");
    let s = render(&mut app, 80, 24);
    assert!(s.contains("7 Groups"), "{s}");
}
