//! Print every TUI view to stdout as plain text (for review/docs):
//! `cargo run -p nysm-tui --example screens -- 80 24`
use std::sync::Arc;
use std::time::Duration;

use nysm_engine::{Engine, EngineConfig};
use nysm_tui::app::{App, Tab};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn main() {
    let args: Vec<u16> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let (w, h) = (
        args.first().copied().unwrap_or(80),
        args.get(1).copied().unwrap_or(24),
    );
    let ascii = std::env::args().any(|a| a == "--ascii");
    let mut e = Engine::new(EngineConfig {
        cgroups: true,
        processes: true,
        process_interval: Duration::ZERO,
        ..Default::default()
    });
    let mut app = App::new(ascii, false, Default::default());
    for _ in 0..6 {
        app.live = Some(Arc::new(e.sample()));
        std::thread::sleep(Duration::from_millis(400));
    }
    e.wait_slow_providers(Duration::from_secs(2));
    app.live = Some(Arc::new(e.sample()));
    app.live_history = e.history().iter().copied().collect();
    for tab in Tab::ALL {
        app.tab = tab;
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| nysm_tui::ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        println!("===== {} ({w}x{h}) =====", tab.title());
        for y in 0..h {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{}", line.trim_end());
        }
    }
}
