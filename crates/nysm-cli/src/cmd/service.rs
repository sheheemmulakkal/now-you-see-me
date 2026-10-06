//! `nysm service run|status|stop|unit`.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crate::{Ctx, ServiceAction, exit};

#[cfg(unix)]
pub fn run(ctx: &Ctx, action: ServiceAction) -> io::Result<u8> {
    use nysm_ipc::{client::RemoteLive, paths, protocol::Subscribe, server};
    let dir = paths::runtime_dir();
    let mut out = io::stdout().lock();
    match action {
        ServiceAction::Run {
            idle_exit,
            interval,
        } => {
            let opts = server::ServerOptions {
                engine: ctx.engine_config(interval, false),
                rules: ctx.settings.rules.clone(),
                idle_exit,
                max_clients: 32,
                log_alerts: true,
                handle_signals: true,
                stop: Default::default(),
                incidents: incident_config(ctx, interval),
            };
            if let Some(c) = &opts.incidents {
                eprintln!(
                    "nysm service: incident snapshots on ({} before / {} after) in {}",
                    nysm_core::units::duration_s(c.pre.as_secs_f64()),
                    nysm_core::units::duration_s(c.post.as_secs_f64()),
                    c.dir.display()
                );
            }
            match server::run(opts, &dir) {
                Ok(()) => Ok(exit::OK),
                Err(e @ server::ServeError::AlreadyRunning(_)) => {
                    eprintln!("nysm: {e}");
                    Ok(exit::FAILURE)
                }
                Err(e) => Err(io::Error::other(e.to_string())),
            }
        }
        ServiceAction::Stdio => {
            if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                eprintln!(
                    "nysm: `service stdio` speaks a binary protocol on stdout; it is meant to be run by `nysm tui --remote`"
                );
                return Ok(exit::USAGE);
            }
            server::serve_stdio(ctx.engine_config(None, false), ctx.settings.rules.clone())?;
            Ok(exit::OK)
        }
        ServiceAction::Status => {
            match RemoteLive::connect(&dir, "nysm-status", Subscribe::default()) {
                Ok(r) => {
                    let pid = read_pid(&dir).map_or("?".into(), |p| p.to_string());
                    writeln!(
                        out,
                        "running: {} (pid {pid}), {} ms interval",
                        r.producer, r.interval_ms
                    )?;
                    writeln!(out, "socket:  {}", paths::socket_path(&dir).display())?;
                    Ok(exit::OK)
                }
                Err(e) => {
                    writeln!(out, "not running ({e})")?;
                    writeln!(out, "socket:  {}", paths::socket_path(&dir).display())?;
                    Ok(exit::NOT_FOUND)
                }
            }
        }
        ServiceAction::Stop => {
            let Some(pid) = read_pid(&dir) else {
                eprintln!(
                    "nysm: no collector pid recorded in {}",
                    paths::lock_path(&dir).display()
                );
                return Ok(exit::NOT_FOUND);
            };
            if RemoteLive::connect(&dir, "nysm-stop", Subscribe::default()).is_err() {
                eprintln!("nysm: collector is not running");
                return Ok(exit::NOT_FOUND);
            }
            if !is_our_collector(pid) {
                eprintln!("nysm: pid {pid} is not a nysm process owned by you; not signalling it");
                return Ok(exit::FAILURE);
            }
            // SAFETY: plain kill(2) on a verified pid.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            let t = Instant::now();
            while paths::socket_path(&dir).exists() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(50));
            }
            writeln!(out, "stopped")?;
            Ok(exit::OK)
        }
        ServiceAction::Unit => {
            let exe = std::env::current_exe()?;
            write!(
                out,
                "\
# Systemd user unit for the Now You See Me collector. Install with:
#   nysm service unit > ~/.config/systemd/user/nysm.service
#   systemctl --user daemon-reload
#   systemctl --user start nysm        # or `enable --now` for every login (opt-in)
[Unit]
Description=Now You See Me resource collector (per-user)

[Service]
ExecStart={} service run
Restart=on-failure
# The collector needs no privileges or network access.
NoNewPrivileges=yes
PrivateNetwork=yes
RestrictAddressFamilies=AF_UNIX

[Install]
WantedBy=default.target
",
                exe.display()
            )?;
            Ok(exit::OK)
        }
    }
}

#[cfg(unix)]
fn incident_config(
    ctx: &Ctx,
    interval: Option<Duration>,
) -> Option<nysm_record::incident::IncidentConfig> {
    let i = &ctx.settings.incidents;
    if !i.enabled {
        return None;
    }
    let Some(dir) = i.dir.clone() else {
        eprintln!("nysm service: incidents enabled but no state directory (set incidents.dir)");
        return None;
    };
    Some(nysm_record::incident::IncidentConfig {
        dir,
        pre: i.pre,
        post: i.post,
        interval: interval.unwrap_or(ctx.settings.interval),
        max_files: i.max_files,
        max_total_bytes: i.max_total_bytes,
    })
}

#[cfg(unix)]
fn read_pid(dir: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(nysm_ipc::paths::lock_path(dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The pid must belong to us and be a `nysm` binary (guards against reuse).
#[cfg(unix)]
fn is_our_collector(pid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let ours = std::fs::metadata(format!("/proc/{pid}"))
        .map(|m| m.uid() == unsafe { libc::geteuid() })
        .unwrap_or(false);
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    ours && comm.trim() == nysm_core::brand::COMMAND_NAME
}

#[cfg(not(unix))]
pub fn run(_ctx: &Ctx, _action: ServiceAction) -> io::Result<u8> {
    eprintln!("nysm: the collector service is not supported on this platform yet");
    Ok(exit::FAILURE)
}
