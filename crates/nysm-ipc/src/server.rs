//! `nysm service run`: one collector shared by local clients.
//!
//! Threads: the `Live` collector, an accept loop, and per client a writer
//! (blocks on the next snapshot, then on its own socket) and a reader.
//! A slow client only ever blocks its own writer; after a 5 s write
//! timeout it is disconnected. Collection is never delayed by clients.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nysm_core::alerts::Rule;
use nysm_engine::live::{Command, Live};
use nysm_engine::{Engine, EngineConfig};

use crate::paths;
use crate::protocol::*;

pub struct ServerOptions {
    pub engine: EngineConfig,
    pub rules: Vec<Rule>,
    /// Exit after this long with no clients (ignored while alert rules are
    /// configured, since alerts need continuous collection).
    pub idle_exit: Option<Duration>,
    pub max_clients: usize,
    /// Log alert events to stderr (journal when run under systemd --user).
    pub log_alerts: bool,
    /// Install SIGINT/SIGTERM handlers (off for embedding, e.g. tests).
    pub handle_signals: bool,
    /// Set to stop the server from another thread.
    pub stop: Arc<AtomicBool>,
    /// Opt-in incident snapshots around alert events.
    pub incidents: Option<nysm_record::incident::IncidentConfig>,
}

#[derive(Debug)]
pub enum ServeError {
    AlreadyRunning(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::AlreadyRunning(p) => write!(
                f,
                "a collector is already running (lock held on {})",
                p.display()
            ),
            ServeError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<io::Error> for ServeError {
    fn from(e: io::Error) -> Self {
        ServeError::Io(e)
    }
}

struct Shared {
    live: Live,
    clients: AtomicUsize,
    proc_subscribers: Mutex<usize>,
    cg_subscribers: Mutex<usize>,
    last_activity: Mutex<Instant>,
    stop: AtomicBool,
    interval_ms: u64,
}

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

/// Holds the single-instance lock for the lifetime of the server.
struct InstanceLock {
    _file: std::fs::File,
}

fn acquire_lock(path: &Path) -> Result<InstanceLock, ServeError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: valid fd. The lock is released when the file is closed,
    // including on crash, so a dead server never blocks a new one.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        return Err(if e.kind() == ErrorKind::WouldBlock {
            ServeError::AlreadyRunning(path.into())
        } else {
            e.into()
        });
    }
    // Record our PID for `nysm service stop`/`status`.
    use std::io::Write;
    let mut f = &file;
    let _ = file.set_len(0);
    let _ = writeln!(f, "{}", std::process::id());
    Ok(InstanceLock { _file: file })
}

/// Feed every sample and alert event to the incident recorder.
fn run_incidents(sh: Arc<Shared>, mut recorder: nysm_record::incident::IncidentRecorder) {
    let mut last_seq = 0;
    let mut seen = sh.live.with_state(|st| st.alert_events_total);
    while !sh.stop.load(Ordering::Relaxed) {
        let Some(snap) = sh.live.wait_newer(last_seq, Duration::from_millis(500)) else {
            continue;
        };
        last_seq = snap.seq;
        let (events, total) = sh.live.with_state(|st| st.events_since(seen));
        seen = total;
        match recorder.observe(&snap, &events) {
            Ok(done) => {
                for p in done {
                    eprintln!("nysm service: incident saved to {}", p.display());
                }
            }
            Err(e) => eprintln!("nysm service: incident capture failed: {e}"),
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if let Ok(done) = recorder.finish_all(now) {
        for p in done {
            eprintln!(
                "nysm service: incident saved (truncated at shutdown) to {}",
                p.display()
            );
        }
    }
}

fn set_cgroups(shared: &Shared, delta: isize) {
    let mut n = shared.cg_subscribers.lock().unwrap();
    let before = *n;
    *n = (*n as isize + delta).max(0) as usize;
    match (before, *n) {
        (0, 1..) => shared.live.send(Command::SetCgroups(true)),
        (1.., 0) => shared.live.send(Command::SetCgroups(false)),
        _ => {}
    }
}

fn set_processes(shared: &Shared, delta: isize) {
    let mut n = shared.proc_subscribers.lock().unwrap();
    let before = *n;
    *n = (*n as isize + delta).max(0) as usize;
    match (before, *n) {
        (0, 1..) => shared.live.send(Command::SetProcesses(true)),
        (1.., 0) => shared.live.send(Command::SetProcesses(false)),
        _ => {}
    }
}

fn new_shared(engine: &EngineConfig, rules: Vec<Rule>) -> Arc<Shared> {
    let mut engine_cfg = engine.clone();
    engine_cfg.processes = false; // enabled on demand by subscribers
    Arc::new(Shared {
        interval_ms: engine_cfg.interval.as_millis() as u64,
        live: Live::spawn_with_alerts(Engine::new(engine_cfg), rules),
        clients: AtomicUsize::new(0),
        proc_subscribers: Mutex::new(0),
        cg_subscribers: Mutex::new(0),
        last_activity: Mutex::new(Instant::now()),
        stop: AtomicBool::new(false),
    })
}

/// Serve exactly one client over stdin/stdout with an embedded collector,
/// e.g. `ssh host nysm service stdio`. No socket, lock or listener is
/// created; the process exits when the client disconnects. Transport
/// security and authentication are SSH's.
pub fn serve_stdio(engine: EngineConfig, rules: Vec<Rule>) -> io::Result<()> {
    let shared = new_shared(&engine, rules);
    let reader: Box<dyn Read + Send> = Box::new(io::stdin());
    let writer: Box<dyn Write + Send> = Box::new(io::stdout());
    let r = serve_conn(reader, writer, Box::new(|| {}), Box::new(|| {}), &shared);
    shared.stop.store(true, Ordering::SeqCst);
    match r {
        Err(e) if matches!(e.kind(), ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe) => Ok(()),
        other => other,
    }
}

/// Run the collector service until signalled, idle-exit, or `Shutdown`.
pub fn run(opts: ServerOptions, dir: &Path) -> Result<(), ServeError> {
    paths::check_socket_path(&paths::socket_path(dir))?;
    paths::ensure_private_dir(dir)?;
    let _lock = acquire_lock(&paths::lock_path(dir))?;
    let sock = paths::socket_path(dir);
    // We hold the lock, so any existing socket is stale.
    match std::fs::remove_file(&sock) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let listener = UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    if opts.handle_signals {
        // SAFETY: handlers only store to an atomic.
        unsafe {
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
    }

    let keep_alive_for_alerts = !opts.rules.is_empty();
    let shared = new_shared(&opts.engine, opts.rules.clone());
    let external_stop = opts.stop.clone();
    eprintln!(
        "nysm service: listening on {} (pid {})",
        sock.display(),
        std::process::id()
    );

    let incident_thread = match opts.incidents.clone() {
        Some(cfg) => {
            // Incidents should name the processes involved: keep the
            // process table on for as long as the service runs.
            set_processes(&shared, 1);
            let recorder = nysm_record::incident::IncidentRecorder::new(cfg)?;
            let sh = shared.clone();
            Some(
                thread::Builder::new()
                    .name("nysm-incidents".into())
                    .spawn(move || run_incidents(sh, recorder))?,
            )
        }
        None => None,
    };

    if opts.log_alerts {
        let sh = shared.clone();
        thread::Builder::new()
            .name("nysm-alert-log".into())
            .spawn(move || {
                let mut seen = 0;
                let mut last_seq = 0;
                while !sh.stop.load(Ordering::Relaxed) {
                    if let Some(s) = sh.live.wait_newer(last_seq, Duration::from_millis(500)) {
                        last_seq = s.seq;
                        let (events, total) = sh.live.with_state(|st| st.events_since(seen));
                        seen = total;
                        for e in events {
                            eprintln!(
                                "nysm service: alert {}: {}",
                                nysm_core::sanitize::for_terminal(&e.rule),
                                e.describe()
                            );
                        }
                    }
                }
            })?;
    }

    loop {
        if (opts.handle_signals && SIGNALLED.load(Ordering::SeqCst))
            || shared.stop.load(Ordering::SeqCst)
            || external_stop.load(Ordering::SeqCst)
        {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                *shared.last_activity.lock().unwrap() = Instant::now();
                let _ = stream.set_nonblocking(false);
                match paths::peer_uid(&stream) {
                    // SAFETY: geteuid cannot fail.
                    Ok(uid) if uid == unsafe { libc::geteuid() } => {}
                    _ => continue, // defence in depth; the directory is already 0700
                }
                if shared.clients.load(Ordering::SeqCst) >= opts.max_clients {
                    let mut s = stream;
                    let _ = write_frame(
                        &mut s,
                        &ServerMsg::Error {
                            message: "too many clients".into(),
                        },
                    );
                    continue;
                }
                shared.clients.fetch_add(1, Ordering::SeqCst);
                let sh = shared.clone();
                let spawned = thread::Builder::new()
                    .name("nysm-client".into())
                    .spawn(move || {
                        let _ = serve_client(stream, &sh);
                        sh.clients.fetch_sub(1, Ordering::SeqCst);
                        *sh.last_activity.lock().unwrap() = Instant::now();
                    });
                if spawned.is_err() {
                    shared.clients.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
        if let Some(idle) = opts.idle_exit {
            let idle_for = shared.last_activity.lock().unwrap().elapsed();
            if !keep_alive_for_alerts
                && shared.clients.load(Ordering::SeqCst) == 0
                && idle_for >= idle
            {
                eprintln!(
                    "nysm service: idle for {}s with no clients, exiting",
                    idle.as_secs()
                );
                break;
            }
        }
    }
    shared.stop.store(true, Ordering::SeqCst);
    if let Some(t) = incident_thread {
        let _ = t.join();
    }
    let _ = std::fs::remove_file(&sock);
    eprintln!("nysm service: stopped");
    Ok(())
}

/// Remove row-level detail, keeping totals (lite subscriptions).
fn strip_details(s: &mut nysm_core::snapshot::Snapshot) {
    use nysm_core::{Reading, Status};
    let omitted = || "omitted for lite subscription".to_string();
    s.network.interfaces = Reading::missing(Status::Unsupported, omitted());
    s.storage.devices = Reading::missing(Status::Unsupported, omitted());
    s.storage.filesystems = Reading::missing(Status::Unsupported, omitted());
    s.cpu.per_core.clear();
}

fn serve_client(stream: UnixStream, sh: &Arc<Shared>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let reader = stream.try_clone()?;
    let after_hello = stream.try_clone()?;
    let closer = stream.try_clone()?;
    serve_conn(
        Box::new(reader),
        Box::new(stream),
        Box::new(move || {
            let _ = after_hello.set_read_timeout(None);
        }),
        Box::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        }),
        sh,
    )
}

/// The per-client protocol, independent of the transport. `after_hello`
/// runs once the handshake succeeded; `close` must unblock the reader.
fn serve_conn(
    mut reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    after_hello: Box<dyn FnOnce() + Send>,
    close: Box<dyn Fn() + Send + Sync>,
    sh: &Arc<Shared>,
) -> io::Result<()> {
    let writer = Arc::new(Mutex::new(writer));
    let hello: ClientMsg = read_frame(&mut reader)?;
    let ClientMsg::Hello {
        protocol_version,
        subscribe,
        ..
    } = hello
    else {
        let _ = write_frame(
            &mut *writer.lock().unwrap(),
            &ServerMsg::Error {
                message: "expected hello".into(),
            },
        );
        return Ok(());
    };
    if protocol_version != PROTOCOL_VERSION {
        let message = format!(
            "protocol version {protocol_version} unsupported; this collector speaks {PROTOCOL_VERSION}"
        );
        let _ = write_frame(&mut *writer.lock().unwrap(), &ServerMsg::Error { message });
        return Ok(());
    }
    after_hello();
    let subscribed = Arc::new(AtomicBool::new(subscribe.processes));
    if subscribe.processes {
        set_processes(sh, 1);
    }
    let cg_subscribed = Arc::new(AtomicBool::new(subscribe.cgroups));
    if subscribe.cgroups {
        set_cgroups(sh, 1);
    }
    let (welcome, mut last_seq, mut seen_events, mut history_seq) = sh.live.with_state(|st| {
        let last = st.latest.as_ref().map_or(0, |s| s.seq);
        (
            ServerMsg::Welcome {
                protocol_version: PROTOCOL_VERSION,
                producer: nysm_core::brand::PRODUCER.into(),
                interval_ms: sh.interval_ms,
                history: st.history.iter().copied().collect(),
                alerts: st.alerts.clone(),
                pinned: st.pinned.clone(),
            },
            // Send the current snapshot right away as the first update.
            last.saturating_sub(1),
            st.alert_events_total,
            // The welcome already carried history up to here.
            st.history.last().map_or(0, |p| p.seq),
        )
    });
    write_frame(&mut *writer.lock().unwrap(), &welcome)?;

    let closed = Arc::new(AtomicBool::new(false));
    let reader_thread = {
        let (sh, writer, closed, subscribed) = (
            sh.clone(),
            writer.clone(),
            closed.clone(),
            subscribed.clone(),
        );
        let cg_sub = cg_subscribed.clone();
        thread::Builder::new()
            .name("nysm-client-rx".into())
            .spawn(move || {
                while let Ok(msg) = read_frame::<ClientMsg, _>(&mut reader) {
                    match msg {
                        ClientMsg::SetCgroups { on } => {
                            if cg_sub.swap(on, Ordering::SeqCst) != on {
                                set_cgroups(&sh, if on { 1 } else { -1 });
                            }
                        }
                        ClientMsg::SetProcesses { on } => {
                            if subscribed.swap(on, Ordering::SeqCst) != on {
                                set_processes(&sh, if on { 1 } else { -1 });
                            }
                        }
                        ClientMsg::Pin { id, name } => sh.live.send(Command::Pin { id, name }),
                        ClientMsg::Unpin { id } => sh.live.send(Command::Unpin(id)),
                        ClientMsg::Details {
                            request_id,
                            pid,
                            start_ticks,
                        } => {
                            // Command lines are never sent over the socket.
                            let result = match sh.live.details(
                                pid,
                                start_ticks,
                                false,
                                Duration::from_secs(3),
                            ) {
                                Some(Ok(d)) => Ok(WireDetails {
                                    exe: d.exe.into(),
                                    cwd: d.cwd.into(),
                                    cgroup: d.cgroup.into(),
                                    open_fds: d.open_fds.into(),
                                }),
                                Some(Err(e)) => Err(e.to_string()),
                                None => Err("collector busy".into()),
                            };
                            if write_frame(
                                &mut *writer.lock().unwrap(),
                                &ServerMsg::Details { request_id, result },
                            )
                            .is_err()
                            {
                                break;
                            }
                        }
                        ClientMsg::Ping => {
                            if write_frame(&mut *writer.lock().unwrap(), &ServerMsg::Pong).is_err()
                            {
                                break;
                            }
                        }
                        ClientMsg::Hello { .. } => {}
                    }
                }
                closed.store(true, Ordering::SeqCst);
            })?
    };

    let mut sent_table_ts: Option<i64> = None;
    let mut sent_cg_ts: Option<i64> = None;
    let result = (|| -> io::Result<()> {
        while !closed.load(Ordering::SeqCst) && !sh.stop.load(Ordering::SeqCst) {
            let Some(snap) = sh.live.wait_newer(last_seq, Duration::from_millis(500)) else {
                continue;
            };
            let (history, alerts, events, pinned, total) = sh.live.with_state(|st| {
                let history: Vec<_> = st
                    .history
                    .iter()
                    .filter(|p| p.seq > history_seq)
                    .copied()
                    .collect();
                let (events, total) = st.events_since(seen_events);
                (history, st.alerts.clone(), events, st.pinned.clone(), total)
            });
            if let Some(p) = history.last() {
                history_seq = p.seq;
            }
            let dropped = snap.seq.saturating_sub(last_seq + 1);
            last_seq = snap.seq;
            seen_events = total;
            let mut snapshot = (*snap).clone();
            if subscribe.lite {
                strip_details(&mut snapshot);
            }
            let mut cgroups_unchanged = false;
            if !cg_subscribed.load(Ordering::SeqCst) {
                snapshot.cgroups = None;
                sent_cg_ts = None;
            } else if let Some(t) = &snapshot.cgroups {
                if sent_cg_ts == Some(t.timestamp_ms) {
                    snapshot.cgroups = None;
                    cgroups_unchanged = true;
                } else {
                    sent_cg_ts = Some(t.timestamp_ms);
                }
            }
            let mut processes_unchanged = false;
            if !subscribed.load(Ordering::SeqCst) {
                snapshot.processes = None;
                sent_table_ts = None;
            } else if let Some(t) = &snapshot.processes {
                // Tables refresh less often than samples; send each once.
                if sent_table_ts == Some(t.timestamp_ms) {
                    snapshot.processes = None;
                    processes_unchanged = true;
                } else {
                    sent_table_ts = Some(t.timestamp_ms);
                }
            }
            let msg = ServerMsg::Update {
                seq: snap.seq,
                snapshot: Box::new(snapshot),
                dropped,
                history,
                alerts,
                events,
                pinned,
                processes_unchanged,
                cgroups_unchanged,
            };
            write_frame(&mut *writer.lock().unwrap(), &msg)?;
        }
        Ok(())
    })();
    // Unblock the reader and release subscriptions.
    close();
    let _ = reader_thread.join();
    if subscribed.load(Ordering::SeqCst) {
        set_processes(sh, -1);
    }
    if cg_subscribed.load(Ordering::SeqCst) {
        set_cgroups(sh, -1);
    }
    result
}
