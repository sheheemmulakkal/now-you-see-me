use std::io::{self, Write};
use std::time::Duration;

use nysm_core::query::{self, ProcessSort};
use serde::Serialize;

use crate::{Ctx, SortKey, fmt};

#[derive(Serialize)]
struct Output<'a> {
    schema_version: u32,
    producer: &'a str,
    timestamp_ms: i64,
    interval_ms: Option<u64>,
    logical_cores: u32,
    cpu_scale: &'static str,
    total_visible: usize,
    unreadable: u32,
    sort: &'static str,
    processes: Vec<&'a nysm_core::snapshot::ProcessSnapshot>,
}

pub fn to_sort(k: SortKey) -> ProcessSort {
    match k {
        SortKey::Cpu => ProcessSort::Cpu,
        SortKey::Mem => ProcessSort::Memory,
        SortKey::Io => ProcessSort::DiskIo,
        SortKey::Pid => ProcessSort::Pid,
        SortKey::Name => ProcessSort::Name,
    }
}

pub fn run(
    ctx: &Ctx,
    sort: SortKey,
    limit: usize,
    filter: Option<String>,
    json: bool,
    warmup: Duration,
    per_core: bool,
) -> io::Result<u8> {
    let (_e, snap) = fmt::one_shot(warmup, true);
    let Some(t) = &snap.processes else {
        eprintln!("nysm: process list unavailable on this platform");
        return Ok(crate::exit::FAILURE);
    };
    let key = to_sort(sort);
    let order = query::view(&t.entries, key, filter.as_deref().unwrap_or(""));
    let rows: Vec<_> = order.iter().take(limit).map(|&i| &t.entries[i]).collect();
    let mut out = io::stdout().lock();
    if json {
        let o = Output {
            schema_version: nysm_core::SCHEMA_VERSION,
            producer: nysm_core::brand::PRODUCER,
            timestamp_ms: t.timestamp_ms,
            interval_ms: t.interval_ms,
            logical_cores: t.logical_cores,
            cpu_scale: "cpu_pct is a share of all logical cores (0-100)",
            total_visible: t.entries.len(),
            unreadable: t.unreadable,
            sort: key.label(),
            processes: rows,
        };
        serde_json::to_writer_pretty(&mut out, &o)?;
        writeln!(out)?;
        return Ok(crate::exit::OK);
    }
    let st = &ctx.style;
    let scale = if per_core {
        "CPU%1c: 100% = one full core"
    } else {
        "CPU%: share of all logical cores"
    };
    writeln!(
        out,
        "{}",
        st.dim(&format!(
            "{} processes visible, sorted by {}; {scale}; {} logical cores",
            t.entries.len(),
            key.label(),
            t.logical_cores
        ))
    )?;
    writeln!(out, "{}", st.bold(&fmt::process_header(per_core)))?;
    for p in rows {
        writeln!(
            out,
            "{}",
            fmt::process_row(st, p, t.logical_cores, per_core)
        )?;
    }
    if t.unreadable > 0 {
        writeln!(
            out,
            "{}",
            st.dim(&format!("{} processes could not be read", t.unreadable))
        )?;
    }
    Ok(crate::exit::OK)
}
