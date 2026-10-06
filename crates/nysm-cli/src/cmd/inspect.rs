use std::io::{self, Write};
use std::time::Duration;

use nysm_collect::CResult;
use nysm_core::snapshot::ProcessSnapshot;
use nysm_core::units;
use serde::Serialize;

use crate::{Ctx, exit, fmt};

#[derive(Serialize)]
struct Field<T: Serialize> {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<T>,
    status: nysm_core::Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

fn field<T: Serialize>(r: CResult<T>) -> Field<T> {
    match r {
        Ok(v) => Field {
            value: Some(v),
            status: nysm_core::Status::Available,
            reason: None,
        },
        Err(e) => Field {
            value: None,
            status: e.status(),
            reason: Some(e.reason()),
        },
    }
}

#[derive(Serialize)]
struct Output<'a> {
    schema_version: u32,
    producer: &'a str,
    boot_id: Option<&'a str>,
    logical_cores: u32,
    process: &'a ProcessSnapshot,
    parent: Option<ParentRef<'a>>,
    children: Vec<ParentRef<'a>>,
    exe: Field<String>,
    cwd: Field<String>,
    cgroup: Field<String>,
    open_fds: Field<u32>,
    /// Soft open-files limit; null value with status when unknown.
    /// `18446744073709551615` (u64::MAX) means unlimited.
    fd_limit: Field<u64>,
    swap_bytes: Field<u64>,
    /// Service/container/app the process belongs to (from its cgroup).
    #[serde(skip_serializing_if = "Option::is_none")]
    belongs_to: Option<BelongsTo>,
    /// Sockets this process holds (visible ones; this network namespace).
    sockets: Field<Vec<nysm_core::sockets::SocketEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cmdline: Option<Field<Vec<String>>>,
}

#[derive(Serialize)]
struct BelongsTo {
    kind: nysm_core::raw::CgroupKind,
    name: String,
}

#[derive(Serialize)]
struct ParentRef<'a> {
    pid: u32,
    name: &'a str,
}

pub fn run(ctx: &Ctx, pid: u32, show_args: bool, json: bool, warmup: Duration) -> io::Result<u8> {
    let (mut engine, snap) = fmt::one_shot(warmup, true);
    let Some(t) = &snap.processes else {
        eprintln!("nysm: process list unavailable on this platform");
        return Ok(exit::FAILURE);
    };
    let Some(p) = t.entries.iter().find(|p| p.id.pid == pid) else {
        eprintln!("nysm: no visible process with PID {pid}");
        return Ok(exit::NOT_FOUND);
    };
    let d = match engine.process_details(pid, Some(p.id.start_ticks), show_args) {
        Ok(d) => d,
        Err(nysm_collect::CollectError::Gone) => {
            eprintln!("nysm: process {pid} exited (or its PID was reused) during inspection");
            return Ok(exit::NOT_FOUND);
        }
        Err(e) => {
            eprintln!("nysm: {e}");
            return Ok(exit::FAILURE);
        }
    };
    let parent = t
        .entries
        .iter()
        .find(|x| x.id.pid == p.ppid)
        .map(|x| ParentRef {
            pid: x.id.pid,
            name: &x.name,
        });
    let children: Vec<_> = t
        .entries
        .iter()
        .filter(|x| x.ppid == pid)
        .map(|x| ParentRef {
            pid: x.id.pid,
            name: &x.name,
        })
        .collect();
    // Service/container association from the cgroup path (best effort).
    let belongs_to = d
        .cgroup
        .as_ref()
        .ok()
        .filter(|c| !c.is_empty() && c.as_str() != "/")
        .map(|c| {
            let (kind, name) =
                nysm_collect::linux::parse::cgroup_classify(c.trim_start_matches('/'));
            BelongsTo { kind, name }
        });
    // Sockets owned by exactly this process (PID + start time).
    let sockets = engine.sockets(true).map(|all| {
        let mut v: Vec<_> = all
            .into_iter()
            .filter(|s| {
                s.owners
                    .iter()
                    .any(|o| o.id.pid == pid && o.id.start_ticks == p.id.start_ticks)
            })
            .collect();
        v.sort_by_key(|s| (!s.is_listening(), s.local_port));
        v
    });
    let o = Output {
        schema_version: nysm_core::SCHEMA_VERSION,
        producer: nysm_core::brand::PRODUCER,
        boot_id: snap.host.boot_id.as_deref(),
        logical_cores: t.logical_cores,
        process: p,
        parent,
        children,
        exe: field(d.exe),
        cwd: field(d.cwd),
        cgroup: field(d.cgroup),
        open_fds: field(d.open_fds),
        fd_limit: field(d.fd_limit),
        swap_bytes: field(d.swap_bytes),
        belongs_to,
        sockets: field(sockets),
        cmdline: d.cmdline.map(field),
    };
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &o)?;
        writeln!(out)?;
        return Ok(exit::OK);
    }
    let st = &ctx.style;
    let row = |out: &mut io::StdoutLock, k: &str, v: String| {
        writeln!(out, "  {} {v}", st.bold(&format!("{k:<14}")))
    };
    let f = |x: &Field<String>| match &x.value {
        Some(v) => fmt::safe(v),
        None => st.dim(&format!(
            "— ({})",
            x.reason.as_deref().unwrap_or(x.status.label())
        )),
    };
    writeln!(
        out,
        "{} {}",
        st.bold(&fmt::safe(&p.name)),
        st.dim(&format!(
            "pid {} · start {} ticks after boot",
            p.id.pid, p.id.start_ticks
        ))
    )?;
    row(&mut out, "state", p.state.to_string())?;
    row(
        &mut out,
        "user",
        fmt::safe(
            &p.user
                .clone()
                .unwrap_or_else(|| p.uid.map_or("?".into(), |u| u.to_string())),
        ),
    )?;
    row(
        &mut out,
        "parent",
        o.parent
            .as_ref()
            .map_or(format!("{} (not visible)", p.ppid), |x| {
                format!("{} {}", x.pid, fmt::safe(x.name))
            }),
    )?;
    if !o.children.is_empty() {
        let kids: Vec<String> = o
            .children
            .iter()
            .take(12)
            .map(|c| format!("{} {}", c.pid, fmt::safe(c.name)))
            .collect();
        let more = if o.children.len() > 12 {
            format!(" … +{}", o.children.len() - 12)
        } else {
            String::new()
        };
        row(&mut out, "children", format!("{}{more}", kids.join(", ")))?;
    }
    row(
        &mut out,
        "cpu",
        match p.cpu_pct.live() {
            Some(c) => format!(
                "{c:.1}% of machine ({:.1}% of one core) · {} total CPU time",
                c * t.logical_cores as f64,
                units::duration_s(p.cpu_time_s)
            ),
            None => format!("— ({})", p.cpu_pct.status.label()),
        },
    )?;
    row(
        &mut out,
        "memory",
        format!(
            "RSS {} · virtual {}",
            units::bytes(p.rss_bytes as f64),
            units::bytes(p.virtual_bytes as f64)
        ),
    )?;
    row(&mut out, "threads", p.threads.to_string())?;
    row(
        &mut out,
        "disk I/O",
        match p.disk_io.live() {
            Some(d) => format!(
                "read {} · write {}",
                units::rate(d.read_bytes_per_s, units::RateUnit::Bytes),
                units::rate(d.write_bytes_per_s, units::RateUnit::Bytes)
            ),
            None => st.dim(&format!(
                "— ({})",
                p.disk_io
                    .reason
                    .as_deref()
                    .unwrap_or(p.disk_io.status.label())
            )),
        },
    )?;
    row(&mut out, "executable", f(&o.exe))?;
    row(&mut out, "working dir", f(&o.cwd))?;
    if let Some(b) = &o.belongs_to {
        row(
            &mut out,
            "belongs to",
            format!("{} {}", b.kind.label(), fmt::safe(&b.name)),
        )?;
    }
    row(&mut out, "cgroup", f(&o.cgroup))?;
    match &o.sockets.value {
        Some(v) => {
            let ep = |a: &std::net::IpAddr, port: u16| match a {
                std::net::IpAddr::V6(x) => format!("[{x}]:{port}"),
                std::net::IpAddr::V4(x) => format!("{x}:{port}"),
            };
            let proto = |p: nysm_core::sockets::Protocol| match p {
                nysm_core::sockets::Protocol::Tcp => "tcp",
                nysm_core::sockets::Protocol::Udp => "udp",
            };
            let listening: Vec<String> = v
                .iter()
                .filter(|s| s.is_listening())
                .map(|s| format!("{} {}", proto(s.protocol), ep(&s.local_addr, s.local_port)))
                .collect();
            let conns: Vec<&nysm_core::sockets::SocketEntry> =
                v.iter().filter(|s| !s.is_listening()).collect();
            row(
                &mut out,
                "listening",
                if listening.is_empty() {
                    st.dim("none")
                } else {
                    listening.join(", ")
                },
            )?;
            let mut text = format!("{}", conns.len());
            if !conns.is_empty() {
                let shown: Vec<String> = conns
                    .iter()
                    .take(6)
                    .map(|s| {
                        format!(
                            "{} {} → {} ({})",
                            proto(s.protocol),
                            ep(&s.local_addr, s.local_port),
                            ep(&s.remote_addr, s.remote_port),
                            s.state.label()
                        )
                    })
                    .collect();
                text += &format!(": {}", shown.join("; "));
                if conns.len() > 6 {
                    text += &format!(" … +{}", conns.len() - 6);
                }
            }
            row(&mut out, "connections", text)?;
        }
        None => row(
            &mut out,
            "sockets",
            st.dim(&format!("— ({})", o.sockets.status.label())),
        )?,
    }
    row(
        &mut out,
        "open fds",
        match (&o.open_fds.value, &o.fd_limit.value) {
            (Some(n), Some(u64::MAX)) => format!("{n} (no limit)"),
            (Some(n), Some(l)) => format!("{n} of {l} allowed"),
            (Some(n), None) => n.to_string(),
            (None, _) => st.dim(&format!("— ({})", o.open_fds.status.label())),
        },
    )?;
    row(
        &mut out,
        "swap used",
        match &o.swap_bytes.value {
            Some(b) => units::bytes(*b as f64),
            None => st.dim(&format!("— ({})", o.swap_bytes.status.label())),
        },
    )?;
    match &o.cmdline {
        Some(Field {
            value: Some(args), ..
        }) => row(
            &mut out,
            "command line",
            args.iter()
                .map(|a| fmt::safe(a))
                .collect::<Vec<_>>()
                .join(" "),
        )?,
        Some(x) => row(
            &mut out,
            "command line",
            st.dim(&format!("— ({})", x.status.label())),
        )?,
        None => row(
            &mut out,
            "command line",
            st.dim("hidden (may contain secrets; use --show-args)"),
        )?,
    }
    Ok(exit::OK)
}
