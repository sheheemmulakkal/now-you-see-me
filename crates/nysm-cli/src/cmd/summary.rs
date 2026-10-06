use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use nysm_core::query::{self, ProcessSort};
use nysm_core::raw::DiskKind;
use nysm_core::snapshot::{ProcessTable, Snapshot};
use nysm_core::units;

use crate::Ctx;
use crate::fmt::{self, reading};

pub fn run(ctx: &Ctx, json: bool, warmup: Duration, top: usize) -> io::Result<u8> {
    let (_engine, mut snap) = fmt::one_shot(warmup, top > 0);
    // Keep only the top-N processes in the output.
    if let Some(t) = snap.processes.take() {
        let order = query::view(&t.entries, ProcessSort::Cpu, "");
        let entries = order
            .into_iter()
            .take(top)
            .map(|i| t.entries[i].clone())
            .collect();
        snap.processes = Some(Arc::new(ProcessTable {
            entries,
            ..(*t).clone()
        }));
    }
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &snap)?;
        writeln!(out)?;
    } else {
        render(ctx, &snap, &mut out)?;
    }
    Ok(super::exit_code_for(&snap))
}

pub fn render(ctx: &Ctx, s: &Snapshot, out: &mut impl Write) -> io::Result<()> {
    let st = &ctx.style;
    let label = |l: &str| st.bold(&format!("{l:<9}"));
    writeln!(out, "{}", fmt::host_line(st, s))?;
    if let Some(ms) = s.interval_ms {
        writeln!(
            out,
            "{}",
            st.dim(&format!(
                "rates averaged over the last {:.2} s",
                ms as f64 / 1000.0
            ))
        )?;
    }
    writeln!(out)?;

    // CPU
    let cores = s.cpu.logical_cores.value.unwrap_or(0);
    let usage = reading(st, &s.cpu.usage, |c| {
        format!(
            "{}  user {:.1}  system {:.1}  irq {:.1}  iowait {:.1}  steal {:.1}",
            st.bold(&units::percent(c.total_pct)),
            c.user_pct + c.nice_pct,
            c.system_pct,
            c.irq_pct,
            c.iowait_pct,
            c.steal_pct
        )
    });
    writeln!(out, "{}{usage}", label("CPU"))?;
    let load = reading(st, &s.cpu.load, |l| {
        format!(
            "load {:.2} {:.2} {:.2} (1/5/15 min) on {cores} logical cores",
            l.one, l.five, l.fifteen
        )
    });
    writeln!(out, "{:9}{load}", "")?;
    writeln!(
        out,
        "{:9}pressure {}",
        "",
        fmt::pressure_text(st, &s.cpu.pressure)
    )?;

    let temps = reading(st, &s.sensors, |v| {
        let parts: Vec<String> = v
            .summary()
            .iter()
            .map(|(c, t)| format!("{} {t:.0} °C", c.label()))
            .collect();
        let mut out = if parts.is_empty() {
            "no temperature sensors".to_string()
        } else {
            parts.join(" · ")
        };
        if let Some(t) = v.cpu_temperature()
            && let Some(h) = t.high_celsius
        {
            out += &format!("  (CPU high {h:.0} °C)");
        }
        for b in &v.batteries {
            out += &format!(
                "  battery {} {}",
                b.capacity_pct.map_or("?".into(), |c| format!("{c:.0}%")),
                b.status
            );
        }
        out
    });
    writeln!(out, "{:9}temps {temps}", "")?;
    if let Some(v) = s.sensors.value.as_ref() {
        for g in &v.gpus {
            let mut parts = vec![format!("{} ({})", g.card, fmt::safe(&g.driver))];
            if let Some(b) = g.busy_pct {
                parts.push(format!("busy {b:.0}%"));
            }
            match (g.frequency_mhz, g.max_frequency_mhz) {
                (Some(f), Some(m)) => parts.push(format!("{f:.0} of {m:.0} MHz")),
                (Some(f), None) => parts.push(format!("{f:.0} MHz")),
                _ => {}
            }
            if let (Some(u), Some(t)) = (g.vram_used_bytes, g.vram_total_bytes) {
                parts.push(format!(
                    "VRAM {} of {}",
                    units::bytes(u as f64),
                    units::bytes(t as f64)
                ));
            }
            if g.busy_pct.is_none() {
                parts.push(st.dim("utilisation not exposed by this driver"));
            }
            writeln!(out, "{}{}", label("GPU"), parts.join(" · "))?;
        }
    }

    if let Some(l) = s.limits.live() {
        let mem = match (l.memory_bytes, l.memory_max_bytes) {
            (Some(u), Some(m)) => format!(
                "memory {} of {} ({:.0}%)",
                units::bytes(u as f64),
                units::bytes(m as f64),
                u as f64 / m.max(1) as f64 * 100.0
            ),
            (Some(u), None) => format!("memory {} (no limit)", units::bytes(u as f64)),
            _ => "memory —".into(),
        };
        let cpu = l
            .cpu_limit_cores
            .map_or("CPU unlimited".to_string(), |c| format!("CPU {c:.2} cores"));
        let pids = match (l.pids, l.pids_max) {
            (Some(p), Some(m)) => format!("{p} of {m} tasks"),
            (Some(p), None) => format!("{p} tasks (no limit)"),
            _ => String::new(),
        };
        writeln!(
            out,
            "{}{}",
            label("Limits"),
            st.bold(&format!("{mem} · {cpu} · {pids}"))
        )?;
        writeln!(
            out,
            "{:9}{}",
            "",
            st.dim("this process's cgroup (container); host-wide values below may exceed it")
        )?;
    }

    // Memory
    let mem = reading(st, &s.memory.usage, |m| {
        let mut t = format!(
            "{} used of {} ({})  available {}",
            st.bold(&units::bytes(m.used_bytes as f64)),
            units::bytes(m.total_bytes as f64),
            units::percent(m.used_pct),
            units::bytes(m.available_bytes as f64)
        );
        if let Some(c) = m.cache_bytes {
            t += &format!("  cache {}", units::bytes(c as f64));
        }
        t
    });
    writeln!(out, "{}{mem}", label("Memory"))?;
    let swap = reading(st, &s.memory.swap, |w| {
        if w.total_bytes == 0 {
            "none configured".into()
        } else {
            format!(
                "{} used of {}",
                units::bytes(w.used_bytes as f64),
                units::bytes(w.total_bytes as f64)
            )
        }
    });
    let swap_act = reading(st, &s.memory.swap_activity, |a| {
        format!(
            "in {} out {}",
            units::rate(a.in_bytes_per_s, units::RateUnit::Bytes),
            units::rate(a.out_bytes_per_s, units::RateUnit::Bytes)
        )
    });
    writeln!(out, "{}{swap}  {swap_act}", label("Swap"))?;
    writeln!(
        out,
        "{:9}pressure {}",
        "",
        fmt::pressure_text(st, &s.memory.pressure)
    )?;

    // Network
    let counted: Vec<String> = s
        .network
        .interfaces
        .live()
        .map(|v| {
            v.iter()
                .filter(|i| i.counted_in_total)
                .map(|i| fmt::safe(&i.name))
                .collect()
        })
        .unwrap_or_default();
    let net = reading(st, &s.network.total, |n| {
        format!(
            "rx {}  tx {}",
            st.bold(&fmt::rate(n.rx_bytes_per_s, ctx.rate)),
            st.bold(&fmt::rate(n.tx_bytes_per_s, ctx.rate))
        )
    });
    writeln!(
        out,
        "{}{net}  {}",
        label("Network"),
        st.dim(&format!("[{}]", counted.join(", ")))
    )?;
    if let Some(ifs) = s.network.interfaces.live() {
        let others: Vec<String> = ifs
            .iter()
            .filter(|i| {
                !i.counted_in_total
                    && i.rates
                        .live()
                        .is_some_and(|r| r.rx_bytes_per_s + r.tx_bytes_per_s > 0.0)
            })
            .map(|i| {
                let r = i.rates.live().unwrap();
                format!(
                    "{} ({}) rx {} tx {}",
                    fmt::safe(&i.name),
                    i.kind.label(),
                    fmt::rate(r.rx_bytes_per_s, ctx.rate),
                    fmt::rate(r.tx_bytes_per_s, ctx.rate)
                )
            })
            .collect();
        if !others.is_empty() {
            writeln!(
                out,
                "{:9}{}",
                "",
                st.dim(&format!("not in total: {}", others.join("; ")))
            )?;
        }
    }

    // Disk
    let disks: Vec<String> = s
        .storage
        .devices
        .live()
        .map(|v| {
            v.iter()
                .filter(|d| d.counted_in_total)
                .map(|d| fmt::safe(&d.name))
                .collect()
        })
        .unwrap_or_default();
    let io = reading(st, &s.storage.total_io, |d| {
        format!(
            "read {}  write {}  ({:.0} r/s, {:.0} w/s)",
            st.bold(&units::rate(d.read_bytes_per_s, units::RateUnit::Bytes)),
            st.bold(&units::rate(d.write_bytes_per_s, units::RateUnit::Bytes)),
            d.read_ops_per_s,
            d.write_ops_per_s
        )
    });
    writeln!(
        out,
        "{}{io}  {}",
        label("Disk I/O"),
        st.dim(&format!("[{}]", disks.join(", ")))
    )?;
    if let Some(devs) = s.storage.devices.live() {
        for d in devs.iter().filter(|d| d.kind == DiskKind::Disk) {
            if let Some(io) = d.io.live() {
                let lat = match (io.read_latency_ms, io.write_latency_ms) {
                    (None, None) => "no completed I/O".to_string(),
                    (r, w) => format!(
                        "latency r {} w {}",
                        r.map_or("—".into(), |v| format!("{v:.2} ms")),
                        w.map_or("—".into(), |v| format!("{v:.2} ms"))
                    ),
                };
                let busy = io
                    .busy_pct
                    .map_or(String::new(), |b| format!("  in-flight time {b:.0}%"));
                writeln!(
                    out,
                    "{:9}{}",
                    "",
                    st.dim(&format!("{}: {lat}{busy}", fmt::safe(&d.name)))
                )?;
            }
        }
    }
    writeln!(
        out,
        "{:9}pressure {}",
        "",
        fmt::pressure_text(st, &s.storage.io_pressure)
    )?;

    // Filesystems
    writeln!(out)?;
    match (&s.storage.filesystems.value, s.storage.filesystems.status) {
        (Some(fss), status) => {
            let stale = if status == nysm_core::Status::Stale {
                st.warn(" (stale)")
            } else {
                String::new()
            };
            writeln!(out, "{}{stale}", st.bold("Filesystems"))?;
            for f in fss {
                let ro = if f.read_only { " ro" } else { "" };
                let pct = units::percent(f.used_pct);
                let pct = if f.used_pct >= 90.0 {
                    st.bad(&pct)
                } else {
                    pct
                };
                writeln!(
                    out,
                    "  {:<24} {:<6} {:>9} used of {:>9} ({pct})  {:>9} available{ro}",
                    fmt::truncate(&fmt::safe(&f.mount_point), 24),
                    fmt::truncate(&fmt::safe(&f.fs_type), 6),
                    units::bytes(f.used_bytes as f64),
                    units::bytes(f.total_bytes as f64),
                    units::bytes(f.available_bytes as f64)
                )?;
            }
        }
        _ => writeln!(
            out,
            "{} {}",
            st.bold("Filesystems"),
            reading(st, &s.storage.filesystems, |_| String::new())
        )?,
    }

    // Processes
    if let Some(t) = &s.processes {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            st.bold("Top processes by CPU (% of whole machine)")
        )?;
        writeln!(out, "{}", st.dim(&fmt::process_header(false)))?;
        for p in &t.entries {
            writeln!(out, "{}", fmt::process_row(st, p, t.logical_cores, false))?;
        }
    }
    Ok(())
}
