//! Text rendering helpers shared by CLI commands.

use std::time::Duration;

use nysm_core::sanitize::for_terminal;
use nysm_core::snapshot::{Pressure, ProcessSnapshot, Snapshot};
use nysm_core::units::{self, RateUnit};
use nysm_core::{Reading, Status};
use nysm_engine::{Engine, EngineConfig};

use crate::style::Style;

/// Sample twice `warmup` apart so rates are real, never a fake zero.
pub fn one_shot(warmup: Duration, processes: bool) -> (Engine, Snapshot) {
    one_shot_with(warmup, processes, false)
}

/// One-shot sample that can also include cgroup accounting (diagnostics).
pub fn one_shot_with(warmup: Duration, processes: bool, cgroups: bool) -> (Engine, Snapshot) {
    let cfg = EngineConfig {
        processes,
        cgroups,
        process_interval: Duration::ZERO,
        interval: warmup,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg);
    engine.sample();
    std::thread::sleep(warmup);
    engine.wait_slow_providers(Duration::from_secs(2));
    let snap = engine.sample();
    (engine, snap)
}

/// Value text or a dim "— (reason)" for unavailable readings.
pub fn reading<T>(st: &Style, r: &Reading<T>, f: impl Fn(&T) -> String) -> String {
    match (&r.status, &r.value) {
        (Status::Available, Some(v)) => f(v),
        (Status::Stale, Some(v)) => format!("{} {}", f(v), st.warn("(stale)")),
        (s, _) => st.dim(&format!("— ({})", s.label())),
    }
}

pub fn pressure_text(st: &Style, r: &Reading<Pressure>) -> String {
    reading(st, r, |p| {
        let w = |x: &nysm_core::snapshot::PressureWindow| match x.interval_pct {
            Some(i) => format!("{i:.1}% now, avg10 {:.1}%", x.avg10_pct),
            None => format!("avg10 {:.1}%", x.avg10_pct),
        };
        match &p.full {
            Some(f) => format!("some {} · full {}", w(&p.some), w(f)),
            None => format!("some {}", w(&p.some)),
        }
    })
}

pub fn rate(r: f64, unit: RateUnit) -> String {
    units::rate(r, unit)
}

pub fn safe(s: &str) -> String {
    for_terminal(s).into_owned()
}

pub fn truncate(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub fn process_header(per_core: bool) -> String {
    format!(
        "{:>8} {:<10} {:>7} {:>9} {:>10} {:>10} {:>4} {} {}",
        "PID",
        "USER",
        if per_core { "CPU%1c" } else { "CPU%" },
        "RSS",
        "DISK R/s",
        "DISK W/s",
        "THR",
        "S",
        "NAME"
    )
}

pub fn process_row(st: &Style, p: &ProcessSnapshot, cores: u32, per_core: bool) -> String {
    let cpu = if per_core {
        p.cpu_one_core_pct(cores)
    } else {
        p.cpu_pct.live().copied()
    };
    let cpu = match cpu {
        Some(c) => format!("{c:>7.1}"),
        None => format!("{:>7}", "—"),
    };
    let (r, w) = match p.disk_io.live() {
        Some(d) => (
            units::rate(d.read_bytes_per_s, RateUnit::Bytes),
            units::rate(d.write_bytes_per_s, RateUnit::Bytes),
        ),
        None if p.disk_io.status == Status::PermissionDenied => ("denied".into(), "denied".into()),
        None => ("—".into(), "—".into()),
    };
    let user = p
        .user
        .clone()
        .unwrap_or_else(|| p.uid.map(|u| u.to_string()).unwrap_or_else(|| "?".into()));
    let line = format!(
        "{:>8} {:<10} {} {:>9} {:>10} {:>10} {:>4} {} {}",
        p.id.pid,
        truncate(&safe(&user), 10),
        cpu,
        units::bytes(p.rss_bytes as f64),
        r,
        w,
        p.threads,
        p.state,
        safe(&p.name)
    );
    if p.disk_io.status == Status::PermissionDenied {
        line.replace("denied", &st.dim("denied"))
    } else {
        line
    }
}

pub fn host_line(st: &Style, s: &Snapshot) -> String {
    let h = &s.host;
    let mut parts = vec![st.bold(&safe(h.hostname.as_deref().unwrap_or("unknown host")))];
    parts.push(safe(&h.os));
    if let Some(k) = &h.kernel {
        parts.push(format!("kernel {}", safe(k)));
    }
    parts.push(h.arch.clone());
    if let Some(u) = h.uptime_s {
        parts.push(format!("up {}", units::duration_s(u)));
    }
    parts.push(format!("scope: {}", h.scope.describe()));
    parts.join(" · ")
}

/// `HH:MM:SS` in local time (UTC with a `Z` suffix where unavailable).
pub fn clock(timestamp_ms: i64) -> String {
    let secs = timestamp_ms.div_euclid(1000);
    #[cfg(unix)]
    {
        // `as _` lets the platform's time_t width be inferred (musl/glibc).
        let t = secs as _;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: valid pointers; localtime_r is thread-safe.
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
        }
    }
    let s = secs.rem_euclid(86400);
    format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}
