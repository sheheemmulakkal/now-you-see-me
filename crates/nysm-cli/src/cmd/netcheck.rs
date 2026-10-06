//! `nysm net check HOST[:PORT]`: active, on-demand reachability checks
//! against an explicit endpoint. Kept separate from passive counters.
//!
//! Measures DNS resolution time and TCP connect latency (handshake round
//! trip) with timeouts. No ICMP (raw sockets need privileges) and no data is
//! sent after the connection is established.

use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{Ctx, exit, fmt};

#[derive(Serialize)]
struct Attempt {
    #[serde(skip_serializing_if = "Option::is_none")]
    connect_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    target: String,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    dns_ms: Option<f64>,
    addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tested_address: Option<String>,
    attempts: Vec<Attempt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_ms: Option<f64>,
    failed: usize,
    note: &'static str,
}

/// Split "host:port", "[v6]:port" or bare host (default port).
pub fn split_target(t: &str, default_port: u16) -> Result<(String, u16), String> {
    if let Some(rest) = t.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or("missing ] in IPv6 address")?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().map_err(|_| format!("invalid port {p:?}"))?,
            None if after.is_empty() => default_port,
            None => return Err(format!("unexpected {after:?} after IPv6 address")),
        };
        return Ok((host.to_string(), port));
    }
    match t.rsplit_once(':') {
        // A bare IPv6 address has several colons and no port.
        Some((h, p)) if !h.contains(':') => Ok((
            h.to_string(),
            p.parse().map_err(|_| format!("invalid port {p:?}"))?,
        )),
        _ => Ok((t.to_string(), default_port)),
    }
}

/// DNS lookup with a timeout (std resolution itself has none, so it runs
/// on a helper thread that is abandoned if it overruns).
fn resolve(host: &str, port: u16, timeout: Duration) -> Result<(Vec<SocketAddr>, f64), String> {
    let (tx, rx) = mpsc::channel();
    let target = format!("{host}:{port}");
    let start = Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(target.to_socket_addrs().map(|a| a.collect::<Vec<_>>()));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(addrs)) if !addrs.is_empty() => Ok((addrs, start.elapsed().as_secs_f64() * 1000.0)),
        Ok(Ok(_)) => Err("no addresses".into()),
        Ok(Err(e)) => Err(format!("DNS: {e}")),
        Err(_) => Err(format!("DNS: no answer within {} s", timeout.as_secs())),
    }
}

pub fn run(
    ctx: &Ctx,
    target: &str,
    port: u16,
    count: u32,
    timeout: Duration,
    json: bool,
) -> io::Result<u8> {
    let (host, port) = match split_target(target, port) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("nysm: {e}");
            return Ok(exit::USAGE);
        }
    };
    let mut report = Report {
        schema_version: nysm_core::SCHEMA_VERSION,
        target: host.clone(),
        port,
        dns_ms: None,
        addresses: Vec::new(),
        tested_address: None,
        attempts: Vec::new(),
        min_ms: None,
        avg_ms: None,
        max_ms: None,
        failed: 0,
        note: "TCP connect time = handshake round trip incl. server accept; not ICMP ping, not bandwidth",
    };
    let resolved = resolve(&host, port, timeout);
    if let Ok((addrs, ms)) = &resolved {
        report.dns_ms = Some(*ms);
        report.addresses = addrs.iter().map(|a| a.ip().to_string()).collect();
        let addr = addrs[0];
        report.tested_address = Some(addr.to_string());
        for i in 0..count {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(200));
            }
            let t = Instant::now();
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(s) => {
                    report.attempts.push(Attempt {
                        connect_ms: Some(t.elapsed().as_secs_f64() * 1000.0),
                        error: None,
                    });
                    drop(s);
                }
                Err(e) => {
                    report.failed += 1;
                    report.attempts.push(Attempt {
                        connect_ms: None,
                        error: Some(e.to_string()),
                    });
                }
            }
        }
        let ok: Vec<f64> = report
            .attempts
            .iter()
            .filter_map(|a| a.connect_ms)
            .collect();
        if !ok.is_empty() {
            report.min_ms = ok.iter().copied().reduce(f64::min);
            report.max_ms = ok.iter().copied().reduce(f64::max);
            report.avg_ms = Some(ok.iter().sum::<f64>() / ok.len() as f64);
        }
    }
    let reachable = report.failed < report.attempts.len();
    let code = if resolved.is_err() || !reachable {
        exit::FAILURE
    } else {
        exit::OK
    };
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &report)?;
        writeln!(out)?;
        return Ok(code);
    }
    let st = &ctx.style;
    match &resolved {
        Err(e) => writeln!(out, "{} {}", st.bad("unreachable:"), fmt::safe(e))?,
        Ok((addrs, ms)) => {
            writeln!(
                out,
                "{} {} → {} ({ms:.1} ms)",
                st.bold("DNS"),
                fmt::safe(&host),
                addrs
                    .iter()
                    .map(|a| a.ip().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
            writeln!(
                out,
                "{} {} ",
                st.bold("TCP"),
                report.tested_address.as_deref().unwrap_or("?")
            )?;
            for (i, a) in report.attempts.iter().enumerate() {
                match (a.connect_ms, &a.error) {
                    (Some(ms), _) => writeln!(out, "  #{:<2} connected in {ms:.1} ms", i + 1)?,
                    (None, Some(e)) => writeln!(out, "  #{:<2} {}", i + 1, st.bad(&fmt::safe(e)))?,
                    _ => {}
                }
            }
            match (report.min_ms, report.avg_ms, report.max_ms) {
                (Some(mi), Some(av), Some(ma)) => writeln!(
                    out,
                    "  min/avg/max {mi:.1}/{av:.1}/{ma:.1} ms · {} of {} failed",
                    report.failed,
                    report.attempts.len()
                )?,
                _ => writeln!(out, "  {}", st.bad("all connection attempts failed"))?,
            }
        }
    }
    writeln!(out, "{}", st.dim(report.note))?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parsing() {
        assert_eq!(
            split_target("example.com", 443).unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            split_target("example.com:8080", 443).unwrap(),
            ("example.com".into(), 8080)
        );
        assert_eq!(split_target("[::1]:22", 443).unwrap(), ("::1".into(), 22));
        assert_eq!(split_target("[::1]", 443).unwrap(), ("::1".into(), 443));
        assert_eq!(split_target("::1", 443).unwrap(), ("::1".into(), 443));
        assert!(split_target("host:notaport", 443).is_err());
        assert!(split_target("[::1", 443).is_err());
    }
}
