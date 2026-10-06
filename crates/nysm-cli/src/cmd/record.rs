//! `nysm record`: record a time window or an explicitly launched command.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nysm_core::query::{self, ProcessSort};
use nysm_core::raw::ProcessId;
use nysm_core::snapshot::{ProcessTable, Snapshot};
use nysm_core::units;
use nysm_engine::Engine;
use nysm_record::writer::{RecordingWriter, WriteOutcome};
use nysm_record::*;

use crate::{Ctx, exit, fmt};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

fn install_sigint() {
    #[cfg(unix)]
    // SAFETY: the handler only stores to an atomic (async-signal-safe).
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct Args<'a> {
    pub output: &'a Path,
    pub duration: Option<Duration>,
    pub interval: Duration,
    pub max_bytes: u64,
    pub force: bool,
    pub label: Option<String>,
    pub store_args: bool,
    pub command: Vec<String>,
}

/// Child process handle with exact accounting where the OS provides it.
struct Child {
    pid: u32,
    started: Instant,
    #[cfg(not(unix))]
    inner: std::process::Child,
}

struct Exit {
    code: Option<i32>,
    signal: Option<i32>,
    user_s: Option<f64>,
    sys_s: Option<f64>,
    max_rss: Option<u64>,
}

impl Child {
    fn spawn(cmd: &[String]) -> io::Result<Self> {
        let child = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .spawn()?;
        let pid = child.id();
        #[cfg(unix)]
        {
            // We reap with wait4 ourselves to obtain rusage; dropping the
            // std handle does not wait or kill.
            drop(child);
            Ok(Child {
                pid,
                started: Instant::now(),
            })
        }
        #[cfg(not(unix))]
        Ok(Child {
            pid,
            started: Instant::now(),
            inner: child,
        })
    }

    /// Non-blocking reap.
    fn try_exit(&mut self) -> io::Result<Option<Exit>> {
        #[cfg(unix)]
        {
            let mut status = 0;
            let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
            // SAFETY: valid pointers; pid is our direct child.
            let r = unsafe {
                libc::wait4(self.pid as libc::pid_t, &mut status, libc::WNOHANG, &mut ru)
            };
            if r == 0 {
                return Ok(None);
            }
            if r < 0 {
                return Err(io::Error::last_os_error());
            }
            let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
            // ru_maxrss is KiB on Linux, bytes on macOS.
            let rss_unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
            Ok(Some(Exit {
                code: libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status)),
                signal: libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status)),
                user_s: Some(tv(ru.ru_utime)),
                sys_s: Some(tv(ru.ru_stime)),
                max_rss: Some(ru.ru_maxrss.max(0) as u64 * rss_unit),
            }))
        }
        #[cfg(not(unix))]
        {
            Ok(self.inner.try_wait()?.map(|s| Exit {
                code: s.code(),
                signal: None,
                user_s: None,
                sys_s: None,
                max_rss: None,
            }))
        }
    }
}

fn descendants(t: &ProcessTable, root: &ProcessId) -> Vec<usize> {
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut root_idx = None;
    for (i, p) in t.entries.iter().enumerate() {
        kids.entry(p.ppid).or_default().push(i);
        if &p.id == root {
            root_idx = Some(i);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<usize> = match root_idx {
        Some(i) => vec![i],
        // Root exited: its children were re-parented and are lost to us.
        None => return out,
    };
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        out.push(i);
        if let Some(k) = kids.get(&t.entries[i].id.pid) {
            stack.extend(k);
        }
    }
    out
}

fn tree_sample(t: &ProcessTable, root: &ProcessId) -> TreeSample {
    let members = descendants(t, root);
    let mut cpu = None::<f64>;
    let (mut rss, mut rd, mut wr, mut io_any) = (0u64, 0.0, 0.0, false);
    for &i in &members {
        let p = &t.entries[i];
        if let Some(c) = p.cpu_pct.live() {
            *cpu.get_or_insert(0.0) += c;
        }
        rss += p.rss_bytes;
        if let Some(d) = p.disk_io.live() {
            rd += d.read_bytes_per_s;
            wr += d.write_bytes_per_s;
            io_any = true;
        }
    }
    TreeSample {
        processes: members.len() as u32,
        cpu_pct: cpu,
        rss_bytes: rss,
        read_bytes_per_s: io_any.then_some(rd),
        write_bytes_per_s: io_any.then_some(wr),
    }
}

fn to_sample(mut snap: Snapshot, root: Option<&ProcessId>) -> Sample {
    let table = snap.processes.take();
    let (top, tree) = match &table {
        Some(t) => {
            let top = query::view(&t.entries, ProcessSort::Cpu, "")
                .into_iter()
                .take(5)
                .map(|i| {
                    let p = &t.entries[i];
                    ProcessBrief {
                        id: p.id.clone(),
                        name: p.name.clone(),
                        cpu_pct: p.cpu_pct.live().copied(),
                        rss_bytes: p.rss_bytes,
                    }
                })
                .collect();
            (top, root.map(|r| tree_sample(t, r)))
        }
        None => (Vec::new(), None),
    };
    compact_snapshot(&mut snap);
    Sample {
        snapshot: snap,
        top,
        tree,
    }
}

pub fn run(ctx: &Ctx, a: Args) -> io::Result<u8> {
    let command_mode = !a.command.is_empty();
    let duration = match (a.duration, command_mode) {
        (Some(d), _) => Some(d),
        (None, true) => None,
        (None, false) => Some(Duration::from_secs(60)),
    };
    let mut cfg = ctx.engine_config(Some(a.interval), true);
    if command_mode {
        // Track the command tree at the full sampling rate.
        cfg.process_interval = a.interval;
    }
    let mut engine = Engine::new(cfg);
    let baseline = engine.sample();

    install_sigint();
    let mut child = if command_mode {
        match Child::spawn(&a.command) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("nysm: cannot start {}: {e}", fmt::safe(&a.command[0]));
                return Ok(exit::NOT_FOUND);
            }
        }
    } else {
        None
    };
    let target = match &child {
        Some(c) => Target::Command {
            program: Path::new(&a.command[0])
                .file_name()
                .map_or(a.command[0].clone(), |s| s.to_string_lossy().into_owned()),
            args: a.store_args.then(|| a.command[1..].to_vec()),
            root_pid: c.pid,
        },
        None => Target::Window,
    };
    let header = Header {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        producer: nysm_core::brand::PRODUCER.into(),
        schema_version: nysm_core::SCHEMA_VERSION,
        created_ms: now_ms(),
        host: baseline.host.clone(),
        interval_ms: a.interval.as_millis() as u64,
        target,
        label: a.label.clone(),
    };
    let mut w = match RecordingWriter::create(a.output, &header, a.max_bytes, a.force) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "nysm: cannot create {}: {e}{}",
                a.output.display(),
                if e.kind() == io::ErrorKind::AlreadyExists {
                    " (use --force to overwrite)"
                } else {
                    ""
                }
            );
            return Ok(exit::FAILURE);
        }
    };
    let what = match &child {
        Some(c) => format!("command (pid {}) until it exits", c.pid),
        None => format!(
            "system for {}",
            units::duration_s(duration.unwrap_or_default().as_secs_f64())
        ),
    };
    eprintln!(
        "nysm: recording {what} every {} ms to {}",
        a.interval.as_millis(),
        a.output.display()
    );
    if child.is_some() {
        w.event(Event {
            timestamp_ms: now_ms(),
            kind: EventKind::CommandStarted,
        })?;
    }

    // The root identity is learnt from the first process table that has it.
    let mut root: Option<ProcessId> = None;
    let start = Instant::now();
    let mut next = start + a.interval;
    let mut exit_info: Option<Exit> = None;
    let mut truncated = false;
    loop {
        let now = Instant::now();
        if next > now {
            // Sleep in short slices so command exit and Ctrl-C are noticed promptly.
            std::thread::sleep((next - now).min(Duration::from_millis(50)));
            if let Some(c) = child.as_mut()
                && exit_info.is_none()
            {
                exit_info = c.try_exit()?;
            }
            if exit_info.is_none() && !(INTERRUPTED.load(Ordering::SeqCst) && child.is_none()) {
                continue;
            }
        } else {
            // Woke late: account for every scheduled sample we missed.
            let missed = ((now - next).as_secs_f64() / a.interval.as_secs_f64()).floor() as u64;
            w.dropped += missed;
            next += a.interval * missed as u32;
        }
        next += a.interval;
        let snap = engine.sample();
        if let (Some(c), None) = (&child, &root) {
            root = snap.processes.as_ref().and_then(|t| {
                t.entries
                    .iter()
                    .find(|p| p.id.pid == c.pid)
                    .map(|p| p.id.clone())
            });
        }
        if let WriteOutcome::LimitReached = w.sample(to_sample(snap, root.as_ref()))? {
            if !truncated {
                eprintln!("nysm: size limit reached; no further samples are written");
                w.event(Event {
                    timestamp_ms: now_ms(),
                    kind: EventKind::SizeLimitReached,
                })?;
                truncated = true;
            }
            if child.is_none() {
                break;
            }
        }
        if let Some(c) = child.as_mut() {
            if exit_info.is_none() {
                exit_info = c.try_exit()?;
            }
            if exit_info.is_some() {
                break;
            }
        }
        if child.is_none() && INTERRUPTED.load(Ordering::SeqCst) {
            w.event(Event {
                timestamp_ms: now_ms(),
                kind: EventKind::Interrupted,
            })?;
            truncated = true;
            break;
        }
        if duration.is_some_and(|d| start.elapsed() >= d) {
            if child.is_some() {
                eprintln!(
                    "nysm: --duration reached; the command keeps running but is no longer recorded"
                );
                truncated = true;
            }
            break;
        }
    }

    let command = match (&child, &exit_info) {
        (Some(c), Some(e)) => {
            w.event(Event {
                timestamp_ms: now_ms(),
                kind: EventKind::CommandExited {
                    exit_code: e.code,
                    signal: e.signal,
                },
            })?;
            Some(CommandResult {
                exit_code: e.code,
                signal: e.signal,
                wall_s: c.started.elapsed().as_secs_f64(),
                user_cpu_s: e.user_s,
                system_cpu_s: e.sys_s,
                max_single_process_rss_bytes: e.max_rss,
            })
        }
        _ => None,
    };
    let samples = w.samples;
    let dropped = w.dropped;
    let bytes = w.finish(End {
        timestamp_ms: now_ms(),
        samples: 0,
        dropped_samples: 0,
        truncated,
        command: command.clone(),
    })?;
    eprintln!(
        "nysm: wrote {samples} samples ({}) to {}{}{}",
        units::bytes(bytes as f64),
        a.output.display(),
        if dropped > 0 {
            format!(", {dropped} dropped")
        } else {
            String::new()
        },
        if truncated { ", truncated" } else { "" }
    );
    eprintln!("nysm: summarise with `nysm compare {}`", a.output.display());
    Ok(match command {
        // Command mode propagates the command's status for scripting.
        Some(c) => match (c.exit_code, c.signal) {
            (Some(code), _) => code.clamp(0, 255) as u8,
            (None, Some(sig)) => (128 + sig).clamp(0, 255) as u8,
            _ => exit::FAILURE,
        },
        None => exit::OK,
    })
}
