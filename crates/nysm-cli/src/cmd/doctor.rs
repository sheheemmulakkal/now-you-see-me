//! Environment diagnostics. Explains problems without leaking secrets:
//! no command lines, environment values or personal paths are printed.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use nysm_core::Status;
use nysm_core::capabilities::report;

use crate::{Ctx, fmt};

enum Level {
    Ok,
    Info,
    Warn,
}

pub fn run(ctx: &Ctx) -> io::Result<u8> {
    let st = &ctx.style;
    let t0 = Instant::now();
    let (engine, snap) = fmt::one_shot_with(Duration::from_millis(500), true, true);
    let sample_time = t0.elapsed();
    let rep = report(&snap, &|id| engine.source(id));
    let mut checks: Vec<(Level, String, String)> = Vec::new();
    let mut add = |l: Level, k: &str, v: String| checks.push((l, k.to_string(), v));

    add(
        Level::Info,
        "platform",
        fmt::host_line(&nysm_core_style_off(), &snap),
    );
    add(
        if snap.host.scope == nysm_core::snapshot::MeasurementScope::Host {
            Level::Ok
        } else {
            Level::Warn
        },
        "measurement scope",
        snap.host.scope.describe().to_string(),
    );

    #[cfg(unix)]
    {
        // SAFETY: geteuid cannot fail.
        let root = unsafe { libc::geteuid() } == 0;
        add(
            if root { Level::Warn } else { Level::Ok },
            "privileges",
            if root {
                "running as root: not required; normal users get the core metrics".into()
            } else {
                "running as a normal user (sufficient for core metrics)".into()
            },
        );
    }

    if cfg!(target_os = "linux") {
        let proc_ok = Path::new("/proc/stat").exists();
        add(
            if proc_ok { Level::Ok } else { Level::Warn },
            "/proc",
            if proc_ok {
                "mounted".into()
            } else {
                "not mounted: Linux adapter cannot work".into()
            },
        );
        if let Ok(mi) = std::fs::read_to_string("/proc/self/mountinfo") {
            let hidepid = mi
                .lines()
                .filter(|l| l.contains(" - proc "))
                .any(|l| l.contains("hidepid=") && !l.contains("hidepid=0"));
            if hidepid {
                add(
                    Level::Warn,
                    "process visibility",
                    "/proc is mounted with hidepid: other users' processes are hidden".into(),
                );
            }
        }
    }

    for c in &rep.capabilities {
        let level = match c.status {
            Status::Available => Level::Ok,
            Status::Unsupported if c.source == "none" => Level::Info,
            _ => Level::Warn,
        };
        let mut v = format!("{} ({})", c.status.label(), c.source);
        if let Some(r) = &c.reason {
            v += &format!(": {r}");
        }
        add(level, c.id, v);
    }

    add(
        Level::Info,
        "sample cost",
        format!(
            "one-shot sampling took {:.0} ms including a 500 ms warm-up",
            sample_time.as_secs_f64() * 1000.0
        ),
    );
    let tty = io::stdout().is_terminal();
    add(
        Level::Info,
        "terminal",
        format!(
            "stdout {}; TERM {}; NO_COLOR {}; colour {}",
            if tty {
                "is a terminal"
            } else {
                "is redirected"
            },
            if std::env::var_os("TERM").is_some() {
                "set"
            } else {
                "unset"
            },
            if std::env::var_os("NO_COLOR").is_some() {
                "set"
            } else {
                "unset"
            },
            if st.color { "on" } else { "off" }
        ),
    );
    let gui =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    let dbus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
    let systemd = Path::new("/run/systemd/system").exists();
    add(
        Level::Info,
        "session",
        format!(
            "graphical session {}, session D-Bus {}, systemd {} (none are required for CLI/TUI)",
            yes(gui),
            yes(dbus),
            yes(systemd)
        ),
    );

    let mut out = io::stdout().lock();
    writeln!(
        out,
        "{}",
        st.bold(&format!("{} doctor", nysm_core::brand::PRODUCT_NAME))
    )?;
    for (level, k, v) in checks {
        let tag = match level {
            Level::Ok => "ok  ".to_string(),
            Level::Info => st.dim("info"),
            Level::Warn => st.warn("warn"),
        };
        writeln!(
            out,
            "  [{tag}] {} {}",
            st.bold(&format!("{k:<24}")),
            fmt::safe(&v)
        )?;
    }
    Ok(crate::exit::OK)
}

fn yes(b: bool) -> &'static str {
    if b { "present" } else { "absent" }
}

fn nysm_core_style_off() -> crate::style::Style {
    crate::style::Style { color: false }
}
