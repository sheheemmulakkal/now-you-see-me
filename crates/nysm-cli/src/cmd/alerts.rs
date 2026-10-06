//! `nysm alerts`: evaluate sustained-threshold rules continuously.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use nysm_core::alerts::{AlertEngine, AlertEventKind, Direction};
use nysm_engine::Engine;
use serde::Serialize;

use crate::{Ctx, WatchFormat, exit, fmt};

#[derive(Serialize)]
struct JsonEvent<'a> {
    schema_version: u32,
    producer: &'a str,
    #[serde(flatten)]
    event: &'a nysm_core::alerts::AlertEvent,
    message: String,
}

pub fn run(
    ctx: &Ctx,
    list: bool,
    format: WatchFormat,
    interval: Option<Duration>,
    count: Option<u64>,
) -> io::Result<u8> {
    let rules = ctx.settings.rules.clone();
    let st = &ctx.style;
    let mut out = io::stdout().lock();
    if list {
        if rules.is_empty() {
            writeln!(out, "no alert rules enabled")?;
            return Ok(exit::OK);
        }
        writeln!(
            out,
            "{}",
            st.bold(&format!(
                "{:<24} {:<20} {:<14} {:>8} {:>10}",
                "RULE", "METRIC", "CONDITION", "FOR", "COOLDOWN"
            ))
        )?;
        for r in &rules {
            let op = match r.direction {
                Direction::Above => ">",
                Direction::Below => "<",
            };
            writeln!(
                out,
                "{:<24} {:<20} {:<14} {:>7.0}s {:>9.0}s",
                fmt::truncate(&fmt::safe(&r.id), 24),
                r.metric.label(),
                format!("{op} {} (clear {})", r.threshold, r.clear),
                r.for_s,
                r.cooldown_s
            )?;
        }
        if let Some(p) = &ctx.config_path {
            writeln!(out, "{}", st.dim(&format!("configure in {}", p.display())))?;
        }
        return Ok(exit::OK);
    }
    if rules.is_empty() {
        eprintln!("nysm: no alert rules enabled (see `nysm config show`)");
        return Ok(exit::USAGE);
    }
    let cfg = ctx.engine_config(interval, false);
    let interval = cfg.interval;
    let mut engine = Engine::new(cfg);
    let mut alerts = AlertEngine::new(rules);
    eprintln!(
        "nysm: watching {} alert rules every {} ms (Ctrl-C to stop)",
        alerts.rules().len(),
        interval.as_millis()
    );
    engine.sample();
    let mut next = Instant::now() + interval;
    let mut n = 0u64;
    while count.is_none_or(|c| n < c) {
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += interval;
        if next < Instant::now() {
            next = Instant::now() + interval;
        }
        let snap = engine.sample();
        for e in alerts.evaluate(&snap) {
            match format {
                WatchFormat::Jsonl => {
                    let j = JsonEvent {
                        schema_version: nysm_core::SCHEMA_VERSION,
                        producer: nysm_core::brand::PRODUCER,
                        event: &e,
                        message: e.describe(),
                    };
                    serde_json::to_writer(&mut out, &j)?;
                    writeln!(out)?;
                }
                WatchFormat::Text => {
                    let tag = match e.kind {
                        AlertEventKind::Fired { .. } => st.bad("FIRED   "),
                        AlertEventKind::Resolved { .. } => "RESOLVED".to_string(),
                        AlertEventKind::DataMissing { .. } => st.warn("NO DATA "),
                        AlertEventKind::DataRestored => "DATA OK ".to_string(),
                    };
                    writeln!(
                        out,
                        "{} {tag} {}: {}",
                        fmt::clock(e.timestamp_ms),
                        fmt::safe(&e.rule),
                        fmt::safe(&e.describe())
                    )?;
                }
            }
            out.flush()?;
        }
        n += 1;
    }
    Ok(exit::OK)
}
