use std::io::{self, Write};
use std::net::IpAddr;

use nysm_core::Status;
use nysm_core::sockets::{Protocol, SocketEntry, SocketFilter};
use nysm_engine::{Engine, EngineConfig};
use serde::Serialize;

use crate::{Ctx, ProtoArg, exit, fmt};

#[derive(Serialize)]
struct Output<'a> {
    schema_version: u32,
    producer: &'a str,
    scope: &'static str,
    sockets: Vec<&'a SocketEntry>,
}

fn endpoint(a: &IpAddr, port: u16) -> String {
    match a {
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        IpAddr::V4(v4) => format!("{v4}:{port}"),
    }
}

pub fn run(
    ctx: &Ctx,
    port: Option<u16>,
    proto: Option<ProtoArg>,
    all: bool,
    json: bool,
) -> io::Result<u8> {
    let mut engine = Engine::new(EngineConfig {
        filesystems: false,
        ..Default::default()
    });
    let mut sockets = match engine.sockets(true) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("nysm: socket tables unavailable: {e}");
            return Ok(exit::FAILURE);
        }
    };
    let filter = SocketFilter {
        port,
        protocol: proto.map(|p| match p {
            ProtoArg::Tcp => Protocol::Tcp,
            ProtoArg::Udp => Protocol::Udp,
        }),
        listening_only: !all && port.is_none(),
    };
    sockets.retain(|s| filter.matches(s));
    sockets.sort_by(|a, b| {
        (
            b.is_listening(),
            a.local_port,
            a.protocol as u8,
            a.remote_port,
        )
            .cmp(&(
                a.is_listening(),
                b.local_port,
                b.protocol as u8,
                b.remote_port,
            ))
    });
    let code = if port.is_some() && sockets.is_empty() {
        exit::NOT_FOUND
    } else {
        exit::OK
    };
    let mut out = io::stdout().lock();
    if json {
        let o = Output {
            schema_version: nysm_core::SCHEMA_VERSION,
            producer: nysm_core::brand::PRODUCER,
            scope: "sockets in this process's network namespace",
            sockets: sockets.iter().collect(),
        };
        serde_json::to_writer_pretty(&mut out, &o)?;
        writeln!(out)?;
        return Ok(code);
    }
    let st = &ctx.style;
    if sockets.is_empty() {
        writeln!(
            out,
            "{}",
            st.dim("no matching sockets in this network namespace")
        )?;
        return Ok(code);
    }
    writeln!(
        out,
        "{}",
        st.bold(&format!(
            "{:<5} {:<12} {:<28} {:<28} {:<8} {}",
            "PROTO", "STATE", "LOCAL", "REMOTE", "USER", "PROCESS"
        ))
    )?;
    let mut denied = 0;
    for s in &sockets {
        let remote = if s.is_listening() {
            "*".to_string()
        } else {
            endpoint(&s.remote_addr, s.remote_port)
        };
        let owner = if s.owners.is_empty() {
            if s.owner_status == Status::PermissionDenied {
                denied += 1;
            }
            st.dim(&format!("— ({})", s.owner_status.label()))
        } else {
            s.owners
                .iter()
                .map(|o| format!("{} {}", o.id.pid, fmt::safe(&o.name)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let user = engine.user_name(s.uid).unwrap_or_else(|| s.uid.to_string());
        let proto = match s.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        };
        writeln!(
            out,
            "{:<5} {:<12} {:<28} {:<28} {:<8} {owner}",
            proto,
            s.state.label(),
            endpoint(&s.local_addr, s.local_port),
            remote,
            fmt::truncate(&fmt::safe(&user), 8)
        )?;
    }
    if denied > 0 {
        writeln!(
            out,
            "{}",
            st.dim(&format!("{denied} sockets belong to other users; their processes are visible only to root or CAP_SYS_PTRACE"))
        )?;
    }
    Ok(code)
}
