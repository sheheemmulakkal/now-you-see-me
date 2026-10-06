//! `nysm groups|containers|services`: cgroup v2 accounting per group.

use std::io::{self, Write};
use std::time::Duration;

use nysm_core::query;
use nysm_core::raw::CgroupKind;
use nysm_core::snapshot::CgroupSnapshot;
use nysm_core::units;
use nysm_engine::Engine;
use serde::Serialize;

use crate::{Ctx, GroupKindArg, SortKey, exit, fmt};

#[derive(Serialize)]
struct SystemdInfo {
    status: nysm_core::Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
struct Output<'a> {
    schema_version: u32,
    producer: &'a str,
    timestamp_ms: i64,
    interval_ms: Option<u64>,
    logical_cores: u32,
    cpu_scale: &'static str,
    memory_note: &'static str,
    total_groups: usize,
    truncated: u32,
    groups: Vec<&'a CgroupSnapshot>,
    /// cgroup path → runtime container info (only with --names).
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    container_names: std::collections::HashMap<String, nysm_collect::runtime::ContainerInfo>,
    /// systemd unit state per service group (services only; optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    systemd: Option<SystemdInfo>,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    unit_states: std::collections::HashMap<String, nysm_collect::systemd::UnitState>,
}

fn to_kind(k: GroupKindArg) -> CgroupKind {
    match k {
        GroupKindArg::Container => CgroupKind::Container,
        GroupKindArg::Service => CgroupKind::Service,
        GroupKindArg::App => CgroupKind::UserApp,
        GroupKindArg::Machine => CgroupKind::Machine,
        GroupKindArg::Other => CgroupKind::Other,
    }
}

fn of_limit(v: Option<u64>, max: Option<u64>) -> String {
    match (v, max) {
        (Some(v), Some(m)) => format!("{} / {}", units::bytes(v as f64), units::bytes(m as f64)),
        (Some(v), None) => units::bytes(v as f64),
        _ => "—".into(),
    }
}

/// Full container id from a cgroup path like `…/docker-<id>.scope`.
use nysm_collect::runtime::container_id;

pub fn run(
    ctx: &Ctx,
    kind: Option<GroupKindArg>,
    sort: SortKey,
    limit: usize,
    json: bool,
    warmup: Duration,
    names: bool,
) -> io::Result<u8> {
    let mut cfg = ctx.engine_config(Some(warmup), false);
    cfg.cgroups = true;
    cfg.process_interval = Duration::ZERO;
    cfg.filesystems = false;
    let mut engine = Engine::new(cfg);
    engine.sample();
    std::thread::sleep(warmup);
    let snap = engine.sample();
    let Some(t) = snap.cgroups.clone() else {
        eprintln!(
            "nysm: cgroup accounting is not available here (needs the cgroup v2 unified hierarchy)"
        );
        return Ok(exit::FAILURE);
    };
    let want = kind.map(to_kind);
    // Opt-in: ask the container runtime for names (single read-only GET).
    let runtime = if names {
        match nysm_collect::runtime::container_names(Duration::from_secs(3)) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("nysm: container names unavailable: {e}");
                None
            }
        }
    } else {
        None
    };
    let runtime_name = |g: &CgroupSnapshot| -> Option<&nysm_collect::runtime::ContainerInfo> {
        runtime.as_ref()?.get(container_id(&g.path)?)
    };
    // Optional systemd states for services (bounded, on demand).
    let (states, systemd_info) = if want == Some(CgroupKind::Service) {
        match nysm_collect::systemd::service_units(false, Duration::from_secs(3)) {
            Ok(units) => (
                units
                    .into_iter()
                    .map(|u| (u.unit.trim_end_matches(".service").to_string(), u))
                    .collect(),
                Some(SystemdInfo {
                    status: nysm_core::Status::Available,
                    reason: None,
                }),
            ),
            Err(e) => (
                std::collections::HashMap::new(),
                Some(SystemdInfo {
                    status: e.status(),
                    reason: Some(e.reason()),
                }),
            ),
        }
    } else {
        (std::collections::HashMap::new(), None)
    };
    let mut rows: Vec<&CgroupSnapshot> = t
        .groups
        .iter()
        .filter(|g| want.is_none_or(|k| g.kind == k))
        .collect();
    query::sort_groups(&mut rows, crate::cmd::processes::to_sort(sort));
    let total = rows.len();
    rows.truncate(limit);
    let mut out = io::stdout().lock();
    if json {
        let o = Output {
            schema_version: nysm_core::SCHEMA_VERSION,
            producer: nysm_core::brand::PRODUCER,
            timestamp_ms: t.timestamp_ms,
            interval_ms: t.interval_ms,
            logical_cores: t.logical_cores,
            cpu_scale: "cpu_pct is a share of all logical cores (0-100); cpu_limit_cores is the cpu.max quota in cores",
            memory_note: "memory_bytes is cgroup-charged memory and includes page cache charged to the group",
            total_groups: total,
            truncated: t.truncated,
            container_names: rows
                .iter()
                .filter_map(|g| runtime_name(g).map(|i| (g.path.clone(), i.clone())))
                .collect(),
            unit_states: rows
                .iter()
                .filter_map(|g| states.get(&g.name).map(|u| (g.name.clone(), u.clone())))
                .collect(),
            groups: rows,
            systemd: systemd_info,
        };
        serde_json::to_writer_pretty(&mut out, &o)?;
        writeln!(out)?;
        return Ok(exit::OK);
    }
    let st = &ctx.style;
    if rows.is_empty() {
        writeln!(out, "{}", st.dim("no matching groups"))?;
        return Ok(if kind.is_some() {
            exit::NOT_FOUND
        } else {
            exit::OK
        });
    }
    let display_name = |g: &CgroupSnapshot| -> String {
        fmt::safe(&match (runtime_name(g), container_id(&g.path)) {
            (Some(i), Some(id)) => nysm_collect::runtime::label(&i.name, id),
            _ => g.name.clone(),
        })
    };
    // Name column: as wide as the longest name, within 28..=48.
    let nw = rows
        .iter()
        .map(|g| display_name(g).chars().count())
        .max()
        .unwrap_or(0)
        .clamp(28, 48);
    writeln!(
        out,
        "{}",
        st.bold(&format!(
            "{:<9} {:<nw$} {:>7} {:>8} {:>21} {:>6} {:>10} {:>10} {:>7}  {}",
            "KIND",
            "NAME",
            "CPU%",
            "LIMIT",
            "MEMORY / LIMIT",
            "PIDS",
            "READ/s",
            "WRITE/s",
            "MEM PSI",
            if states.is_empty() { "MAIN" } else { "STATE" }
        ))
    )?;
    for g in &rows {
        let cpu = g.cpu_pct.live().map_or("—".into(), |c| format!("{c:.1}"));
        let lim = g
            .cpu_limit_cores
            .map_or("none".into(), |c| format!("{c:.2}c"));
        let (r, w) = match g.disk_io.live() {
            Some(d) => (
                units::rate(d.read_bytes_per_s, units::RateUnit::Bytes),
                units::rate(d.write_bytes_per_s, units::RateUnit::Bytes),
            ),
            None => ("—".into(), "—".into()),
        };
        let psi = g
            .memory_pressure_pct
            .live()
            .map_or("—".into(), |v| format!("{v:.1}%"));
        let near_limit = matches!((g.memory_bytes, g.memory_max_bytes), (Some(v), Some(m)) if m > 0 && v as f64 / m as f64 >= 0.9);
        let mem = format!("{:>21}", of_limit(g.memory_bytes, g.memory_max_bytes));
        let mem = if near_limit { st.warn(&mem) } else { mem };
        writeln!(
            out,
            "{:<9} {:<nw$} {:>7} {:>8} {} {:>6} {:>10} {:>10} {:>7}  {}",
            g.kind.label(),
            fmt::truncate(&display_name(g), nw),
            cpu,
            lim,
            mem,
            g.pids.map_or("—".into(), |p| p.to_string()),
            r,
            w,
            psi,
            match states.get(&g.name) {
                Some(u) => format!("{}/{}", u.active, u.sub),
                None => fmt::safe(g.main_process.as_deref().unwrap_or("")),
            }
        )?;
    }
    writeln!(
        out,
        "{}",
        st.dim(&format!(
            "{} of {total} groups · CPU% = share of all {} cores · memory includes page cache charged to the group{}",
            rows.len(),
            t.logical_cores,
            if t.truncated > 0 { format!(" · {} groups beyond the scan bound", t.truncated) } else { String::new() }
        ))
    )?;
    Ok(exit::OK)
}

/// `nysm services --failed`: failed systemd units (system and user).
pub fn failed_units(ctx: &Ctx, json: bool) -> io::Result<u8> {
    let mut out = io::stdout().lock();
    let mut all = Vec::new();
    let mut errors = Vec::new();
    for (scope, user) in [("system", false), ("user", true)] {
        match nysm_collect::systemd::service_units(user, Duration::from_secs(3)) {
            Ok(u) => all.extend(
                u.into_iter()
                    .filter(|u| u.active == "failed")
                    .map(|u| (scope, u)),
            ),
            Err(e) => errors.push(format!("{scope}: {e}")),
        }
    }
    if json {
        #[derive(Serialize)]
        struct F<'a> {
            scope: &'a str,
            #[serde(flatten)]
            unit: &'a nysm_collect::systemd::UnitState,
        }
        let v: Vec<F> = all.iter().map(|(s, u)| F { scope: s, unit: u }).collect();
        serde_json::to_writer_pretty(
            &mut out,
            &serde_json::json!({
                "schema_version": nysm_core::SCHEMA_VERSION,
                "failed": v,
                "errors": errors,
            }),
        )?;
        writeln!(out)?;
        return Ok(if errors.len() == 2 {
            exit::FAILURE
        } else {
            exit::OK
        });
    }
    let st = &ctx.style;
    if all.is_empty() {
        writeln!(out, "no failed services")?;
    }
    for (scope, u) in &all {
        writeln!(
            out,
            "{} {:<6} {:<40} {}",
            st.bad("failed"),
            scope,
            fmt::safe(&u.unit),
            st.dim(&fmt::safe(&u.description))
        )?;
    }
    for e in &errors {
        writeln!(out, "{}", st.dim(&format!("unavailable: {e}")))?;
    }
    if !all.is_empty() {
        writeln!(
            out,
            "{}",
            st.dim("details: systemctl [--user] status <unit>  ·  logs: journalctl [--user] -u <unit> -n 50")
        )?;
    }
    Ok(if errors.len() == 2 {
        exit::FAILURE
    } else {
        exit::OK
    })
}
