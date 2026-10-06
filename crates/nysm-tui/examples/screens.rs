//! Print every TUI view to stdout as plain text (for review/docs):
//! `cargo run -p nysm-tui --example screens -- 80 24`
//!
//! `--cells` prints one JSON object per view instead, with each cell's
//! symbol, colours and bold flag (for rendering exact images, e.g. demo
//! videos). `--warm N` samples for N seconds first so charts have history.
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
    let cells = std::env::args().any(|a| a == "--cells");
    let warm: u64 = std::env::args()
        .skip_while(|a| a != "--warm")
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut e = Engine::new(EngineConfig {
        cgroups: true,
        processes: true,
        process_interval: Duration::ZERO,
        ..Default::default()
    });
    let mut app = App::new(ascii, cells, Default::default());
    let rounds = if warm > 0 { warm } else { 3 } * 2;
    for _ in 0..rounds {
        app.live = Some(Arc::new(e.sample()));
        std::thread::sleep(Duration::from_millis(if warm > 0 { 500 } else { 400 }));
    }
    e.wait_slow_providers(Duration::from_secs(2));
    app.live = Some(Arc::new(e.sample()));
    app.live_history = e.history().iter().copied().collect();
    for tab in Tab::ALL {
        app.tab = tab;
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| nysm_tui::ui::draw(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        if cells {
            let rows: Vec<Vec<serde_json::Value>> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| {
                            let c = &buf[(x, y)];
                            serde_json::json!([
                                c.symbol(),
                                format!("{:?}", c.fg),
                                format!("{:?}", c.bg),
                                c.modifier.contains(ratatui::style::Modifier::BOLD),
                                c.modifier.contains(ratatui::style::Modifier::DIM),
                                c.modifier.contains(ratatui::style::Modifier::REVERSED)
                            ])
                        })
                        .collect()
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({"view": tab.title(), "w": w, "h": h, "cells": rows})
            );
            continue;
        }
        println!("===== {} ({w}x{h}) =====", tab.title());
        for y in 0..h {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{}", line.trim_end());
        }
    }
}
