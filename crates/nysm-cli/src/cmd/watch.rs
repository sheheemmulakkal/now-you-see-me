use std::io::{self, Write};
use std::time::{Duration, Instant};

use nysm_core::units::{self, RateUnit};
use nysm_engine::Engine;

use crate::fmt::reading;
use crate::{Ctx, WatchFormat};

pub fn run(
    ctx: &Ctx,
    interval: Option<Duration>,
    format: WatchFormat,
    count: Option<u64>,
    processes: bool,
    cgroups: bool,
) -> io::Result<u8> {
    let mut cfg = ctx.engine_config(interval, processes);
    cfg.cgroups = cgroups;
    let interval = cfg.interval;
    let mut engine = Engine::new(cfg);
    let st = &ctx.style;
    let mut out = io::stdout().lock();
    // Baseline sample (rates need two), then one line per interval.
    engine.sample();
    let mut next = Instant::now() + interval;
    let mut emitted = 0u64;
    let mut code = crate::exit::OK;
    if format == WatchFormat::Text {
        writeln!(
            out,
            "{}",
            st.dim(&format!(
                "{:<8} {:>6} {:>6} {:>12} {:>12} {:>12} {:>12}",
                "time", "cpu%", "mem%", "net rx", "net tx", "disk read", "disk write"
            ))
        )?;
    }
    loop {
        if count.is_some_and(|c| emitted >= c) {
            return Ok(code);
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += interval;
        if next < Instant::now() {
            next = Instant::now() + interval;
        }
        let s = engine.sample();
        code = code.max(super::exit_code_for(&s));
        match format {
            WatchFormat::Jsonl => {
                serde_json::to_writer(&mut out, &s)?;
                writeln!(out)?;
            }
            WatchFormat::Text => {
                let time = crate::fmt::clock(s.timestamp_ms);
                let w = |x: String, n: usize| format!("{x:>n$}");
                let gap = if s.gap_before {
                    st.warn(" (gap before this sample)")
                } else {
                    String::new()
                };
                writeln!(
                    out,
                    "{:<8} {} {} {} {} {} {}{gap}",
                    time,
                    w(
                        reading(st, &s.cpu.usage, |c| format!("{:.1}", c.total_pct)),
                        6
                    ),
                    w(
                        reading(st, &s.memory.usage, |m| format!("{:.1}", m.used_pct)),
                        6
                    ),
                    w(
                        reading(st, &s.network.total, |n| units::rate(
                            n.rx_bytes_per_s,
                            ctx.rate
                        )),
                        12
                    ),
                    w(
                        reading(st, &s.network.total, |n| units::rate(
                            n.tx_bytes_per_s,
                            ctx.rate
                        )),
                        12
                    ),
                    w(
                        reading(st, &s.storage.total_io, |d| units::rate(
                            d.read_bytes_per_s,
                            RateUnit::Bytes
                        )),
                        12
                    ),
                    w(
                        reading(st, &s.storage.total_io, |d| units::rate(
                            d.write_bytes_per_s,
                            RateUnit::Bytes
                        )),
                        12
                    ),
                )?;
            }
        }
        out.flush()?;
        emitted += 1;
    }
}
