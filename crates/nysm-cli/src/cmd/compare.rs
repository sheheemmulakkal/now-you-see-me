//! `nysm compare A [B]`: summarise one recording or compare two.

use std::io::{self, Write};
use std::path::Path;

use nysm_core::units;
use nysm_record::compare::{self, Kind, Summarizer, Summary};
use nysm_record::reader;
use serde::Serialize;

use crate::{Ctx, exit, fmt};

fn load(path: &Path) -> Result<Summary, String> {
    let mut s = Summarizer::default();
    let c = reader::read(path, reader::DEFAULT_READ_LIMIT, |x| s.add(x))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let h = c.header.clone();
    Ok(s.finish(&h, &c))
}

fn value(kind: Kind, v: Option<f64>) -> String {
    match v {
        None => "—".into(),
        Some(v) => match kind {
            Kind::Percent => format!("{v:.1}%"),
            Kind::Bytes => units::bytes(v),
            Kind::Seconds => format!("{v:.2} s"),
            Kind::Count => format!("{v:.0}"),
        },
    }
}

#[derive(Serialize)]
struct Output<'a> {
    schema_version: u32,
    producer: &'a str,
    first: &'a Summary,
    #[serde(skip_serializing_if = "Option::is_none")]
    second: Option<&'a Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison: Option<compare::Comparison>,
}

pub fn run(ctx: &Ctx, first: &Path, second: Option<&Path>, json: bool) -> io::Result<u8> {
    let a = match load(first) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("nysm: {e}");
            return Ok(exit::FAILURE);
        }
    };
    let b = match second.map(load).transpose() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("nysm: {e}");
            return Ok(exit::FAILURE);
        }
    };
    let cmp = b.as_ref().map(|b| compare::compare(&a, b));
    let mut out = io::stdout().lock();
    if json {
        let o = Output {
            schema_version: nysm_core::SCHEMA_VERSION,
            producer: nysm_core::brand::PRODUCER,
            first: &a,
            second: b.as_ref(),
            comparison: cmp,
        };
        serde_json::to_writer_pretty(&mut out, &o)?;
        writeln!(out)?;
        return Ok(exit::OK);
    }
    let st = &ctx.style;
    let describe = |s: &Summary| {
        format!(
            "{} on {} · {} samples every {} ms · {:.1} s{}{}",
            s.target,
            fmt::safe(s.hostname.as_deref().unwrap_or("?")),
            s.samples,
            s.interval_ms,
            s.duration_s,
            if s.dropped_samples > 0 {
                format!(" · {} dropped", s.dropped_samples)
            } else {
                String::new()
            },
            if s.truncated { " · truncated" } else { "" }
        )
    };
    writeln!(out, "{} {}", st.bold("first: "), describe(&a))?;
    if let Some(b) = &b {
        writeln!(out, "{} {}", st.bold("second:"), describe(b))?;
    }
    writeln!(out)?;
    match (&b, cmp) {
        (Some(_), Some(c)) => {
            writeln!(
                out,
                "{}",
                st.bold(&format!(
                    "{:<36} {:>12} {:>12} {:>8}  {}",
                    "metric", "first", "second", "change", "source"
                ))
            )?;
            for r in &c.rows {
                let change = r.change_pct.map_or("—".into(), |c| format!("{c:+.0}%"));
                let change = match r.change_pct {
                    Some(c) if c.abs() >= compare::NOISE_PCT => st.bold(&format!("{change:>8}")),
                    _ => format!("{change:>8}"),
                };
                let src = match (r.exact, r.note.is_empty()) {
                    (true, _) => format!("exact · {}", r.note),
                    (false, true) => st.dim("sampled"),
                    (false, false) => st.dim(&format!("sampled · {}", r.note)),
                };
                writeln!(
                    out,
                    "{:<36} {:>12} {:>12} {change}  {src}",
                    r.metric,
                    value(r.kind, r.before),
                    value(r.kind, r.after)
                )?;
            }
            writeln!(out)?;
            writeln!(out, "{}", st.bold("Observations"))?;
            for o in &c.observations {
                writeln!(out, "  - {o}")?;
            }
            writeln!(out, "{}", st.bold("Caveats"))?;
            for o in &c.caveats {
                writeln!(out, "  - {}", st.dim(o))?;
            }
        }
        _ => {
            let row =
                |out: &mut io::StdoutLock, k: &str, v: String| writeln!(out, "  {:<34} {v}", k);
            if let Some(c) = &a.command {
                let code = match (c.exit_code, c.signal) {
                    (Some(x), _) => format!("exit {x}"),
                    (None, Some(s)) => format!("signal {s}"),
                    _ => "unknown".into(),
                };
                row(&mut out, "command result", code)?;
                row(&mut out, "wall time (exact)", format!("{:.2} s", c.wall_s))?;
                if let (Some(u), Some(s)) = (c.user_cpu_s, c.system_cpu_s) {
                    row(
                        &mut out,
                        "CPU time user+sys (exact)",
                        format!("{:.2} s ({u:.2} + {s:.2})", u + s),
                    )?;
                }
                if let Some(m) = c.max_single_process_rss_bytes {
                    row(
                        &mut out,
                        "largest process peak RSS (exact)",
                        units::bytes(m as f64),
                    )?;
                }
            }
            if let Some(t) = &a.tree {
                row(
                    &mut out,
                    "tree peak RSS (sampled, sum)",
                    format!(
                        "{} (lower bound)",
                        units::bytes(t.sampled_peak_rss_bytes as f64)
                    ),
                )?;
                row(
                    &mut out,
                    "tree mean CPU (sampled)",
                    value(Kind::Percent, t.cpu_pct.mean),
                )?;
                row(
                    &mut out,
                    "tree max processes (sampled)",
                    t.max_processes.to_string(),
                )?;
            }
            row(
                &mut out,
                "system CPU mean / p95 / max",
                format!(
                    "{} / {} / {}",
                    value(Kind::Percent, a.cpu_pct.mean),
                    value(Kind::Percent, a.cpu_pct.p95),
                    value(Kind::Percent, a.cpu_pct.max)
                ),
            )?;
            row(
                &mut out,
                "memory used max",
                value(Kind::Bytes, a.mem_used_bytes.max),
            )?;
            row(
                &mut out,
                "pressure mean cpu / mem / io",
                format!(
                    "{} / {} / {}",
                    value(Kind::Percent, a.cpu_pressure_pct.mean),
                    value(Kind::Percent, a.mem_pressure_pct.mean),
                    value(Kind::Percent, a.io_pressure_pct.mean)
                ),
            )?;
            row(
                &mut out,
                "network received / sent",
                format!(
                    "{} / {}",
                    units::bytes(a.net_rx.bytes),
                    units::bytes(a.net_tx.bytes)
                ),
            )?;
            row(
                &mut out,
                "disk read / written",
                format!(
                    "{} / {}",
                    units::bytes(a.disk_read.bytes),
                    units::bytes(a.disk_write.bytes)
                ),
            )?;
            if !a.top_processes.is_empty() {
                writeln!(
                    out,
                    "  {}",
                    st.bold("busiest processes (sampled top-5 by CPU; % of all cores)")
                )?;
                for p in &a.top_processes {
                    writeln!(
                        out,
                        "    {:<20} pid {:<8} mean {:>6} max {:>6}  rss up to {:>9}  in {} samples",
                        fmt::truncate(&fmt::safe(&p.name), 20),
                        p.pid,
                        value(Kind::Percent, p.mean_cpu_pct),
                        value(Kind::Percent, p.max_cpu_pct),
                        units::bytes(p.max_rss_bytes as f64),
                        p.samples
                    )?;
                }
            }
        }
    }
    for w in a
        .warnings
        .iter()
        .chain(b.iter().flat_map(|b| b.warnings.iter()))
    {
        writeln!(out, "{}", st.warn(&format!("warning: {w}")))?;
    }
    Ok(exit::OK)
}
