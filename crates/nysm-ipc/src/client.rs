//! Client side: connects to the collector and mirrors its state into a
//! local `LiveState`, so frontends use the same API as embedded mode.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nysm_collect::{CResult, CollectError, ProcessDetails};
use nysm_core::raw::ProcessId;
use nysm_core::snapshot::Snapshot;
use nysm_engine::live::LiveState;

use crate::paths;
use crate::protocol::*;

type Pending = Arc<Mutex<HashMap<u64, Sender<CResult<ProcessDetails>>>>>;

pub struct RemoteLive {
    state: Arc<(Mutex<LiveState>, Condvar)>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Unblocks the reader thread on drop (socket shutdown / child kill).
    closer: Box<dyn Fn() + Send + Sync>,
    child: Mutex<Option<Child>>,
    pending: Pending,
    next_id: AtomicU64,
    connected: Arc<AtomicBool>,
    /// Snapshots the server coalesced for us (we were slow).
    dropped: Arc<AtomicU64>,
    pub producer: String,
    pub interval_ms: u64,
    reader: Option<JoinHandle<()>>,
}

struct Welcome {
    producer: String,
    interval_ms: u64,
    history: Vec<nysm_core::history::HistoryPoint>,
    alerts: Vec<nysm_core::alerts::ActiveAlert>,
    pinned: Vec<nysm_core::history::PinnedProcess>,
}

fn handshake(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    client: &str,
    subscribe: Subscribe,
) -> io::Result<Welcome> {
    write_frame(
        writer,
        &ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            client: client.into(),
            subscribe,
        },
    )?;
    match read_frame::<ServerMsg, _>(reader)? {
        ServerMsg::Welcome {
            producer,
            interval_ms,
            history,
            alerts,
            pinned,
            ..
        } => Ok(Welcome {
            producer,
            interval_ms,
            history,
            alerts,
            pinned,
        }),
        ServerMsg::Error { message } => Err(io::Error::other(format!(
            "collector refused connection: {message}"
        ))),
        _ => Err(io::Error::other("unexpected first message from collector")),
    }
}

impl RemoteLive {
    /// Connect to the collector in `dir` (see `paths::runtime_dir`).
    pub fn connect(dir: &Path, client: &str, subscribe: Subscribe) -> io::Result<Self> {
        paths::check_socket_path(&paths::socket_path(dir))?;
        let stream = UnixStream::connect(paths::socket_path(dir))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = stream.try_clone()?;
        let mut writer = stream.try_clone()?;
        let welcome = handshake(&mut reader, &mut writer, client, subscribe)?;
        stream.set_read_timeout(None)?;
        let closer = stream.try_clone()?;
        Self::start(
            welcome,
            Box::new(reader),
            Box::new(writer),
            Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            }),
            None,
        )
    }

    /// Run `cmd` (e.g. `ssh host nysm service stdio`) and speak the protocol
    /// over its stdin/stdout. Its stderr is inherited so SSH can prompt and
    /// report errors. Authentication and encryption are the command's.
    pub fn connect_command(
        mut cmd: Command,
        client: &str,
        subscribe: Subscribe,
    ) -> io::Result<Self> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut reader = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        let mut writer = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        let welcome = match handshake(&mut reader, &mut writer, client, subscribe) {
            Ok(w) => w,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    e.kind(),
                    format!("remote collector did not answer: {e}"),
                ));
            }
        };
        let pid = child.id();
        Self::start(
            welcome,
            Box::new(reader),
            Box::new(writer),
            Box::new(move || {
                // SAFETY: plain kill(2) of our own child process.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }),
            Some(child),
        )
    }

    fn start(
        welcome: Welcome,
        mut rx: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        closer: Box<dyn Fn() + Send + Sync>,
        child: Option<Child>,
    ) -> io::Result<Self> {
        let Welcome {
            producer,
            interval_ms,
            history,
            alerts,
            pinned,
        } = welcome;
        let mut st = LiveState::new(history.len().max(600), 4 << 20);
        for p in history {
            st.history.push(p);
        }
        st.alerts = alerts;
        st.pinned = pinned;
        let state = Arc::new((Mutex::new(st), Condvar::new()));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let connected = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicU64::new(0));
        let reader = {
            let (state, pending, connected, dropped) = (
                state.clone(),
                pending.clone(),
                connected.clone(),
                dropped.clone(),
            );
            thread::Builder::new()
                .name("nysm-remote-rx".into())
                .spawn(move || {
                    while let Ok(msg) = read_frame::<ServerMsg, _>(&mut rx) {
                        match msg {
                            ServerMsg::Update {
                                mut snapshot,
                                dropped: d,
                                history,
                                alerts,
                                events,
                                pinned,
                                processes_unchanged,
                                cgroups_unchanged,
                                ..
                            } => {
                                {
                                    let mut st = state.0.lock().unwrap();
                                    let newest = st.history.last().map_or(0, |p| p.seq);
                                    for p in history.into_iter().filter(|p| p.seq > newest) {
                                        st.history.push(p);
                                    }
                                    if cgroups_unchanged {
                                        snapshot.cgroups =
                                            st.latest.as_ref().and_then(|l| l.cgroups.clone());
                                    }
                                    if processes_unchanged {
                                        snapshot.processes =
                                            st.latest.as_ref().and_then(|l| l.processes.clone());
                                    }
                                    st.latest = Some(Arc::new(*snapshot));
                                    st.alerts = alerts;
                                    st.pinned = pinned;
                                    for e in events {
                                        st.push_event(e);
                                    }
                                }
                                dropped.fetch_add(d, Ordering::Relaxed);
                                state.1.notify_all();
                            }
                            ServerMsg::Details { request_id, result } => {
                                if let Some(tx) = pending.lock().unwrap().remove(&request_id) {
                                    let _ = tx.send(match result {
                                        Ok(w) => Ok(ProcessDetails {
                                            exe: w.exe.into(),
                                            cwd: w.cwd.into(),
                                            cmdline: None,
                                            cgroup: w.cgroup.into(),
                                            open_fds: w.open_fds.into(),
                                            fd_limit: w.fd_limit.into(),
                                            swap_bytes: w.swap_bytes.into(),
                                            memory: w.memory.into(),
                                        }),
                                        Err(m) => Err(if m.contains("no longer exists") {
                                            CollectError::Gone
                                        } else {
                                            CollectError::Failed(m)
                                        }),
                                    });
                                }
                            }
                            ServerMsg::Error { .. }
                            | ServerMsg::Welcome { .. }
                            | ServerMsg::Pong => {}
                        }
                    }
                    connected.store(false, Ordering::SeqCst);
                    state.1.notify_all();
                    // Fail outstanding requests instead of leaving them hanging.
                    for (_, tx) in pending.lock().unwrap().drain() {
                        let _ = tx.send(Err(CollectError::Failed("collector disconnected".into())));
                    }
                })?
        };
        Ok(RemoteLive {
            state,
            writer: Mutex::new(writer),
            closer,
            child: Mutex::new(child),
            pending,
            next_id: AtomicU64::new(1),
            connected,
            dropped,
            producer,
            interval_ms,
            reader: Some(reader),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.state.0.lock().unwrap().latest.clone()
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&LiveState) -> R) -> R {
        f(&self.state.0.lock().unwrap())
    }

    pub fn wait_newer(&self, after: u64, timeout: Duration) -> Option<Arc<Snapshot>> {
        let g = self.state.0.lock().unwrap();
        let connected = &self.connected;
        let (g, _) = self
            .state
            .1
            .wait_timeout_while(g, timeout, |s| {
                connected.load(Ordering::SeqCst) && s.latest.as_ref().is_none_or(|l| l.seq <= after)
            })
            .unwrap();
        g.latest.clone().filter(|l| l.seq > after)
    }

    pub fn send(&self, msg: &ClientMsg) -> io::Result<()> {
        write_frame(&mut *self.writer.lock().unwrap(), msg)
    }

    pub fn request_details(
        &self,
        pid: u32,
        start_ticks: Option<u64>,
    ) -> mpsc::Receiver<CResult<ProcessDetails>> {
        let (tx, rx) = mpsc::channel();
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().unwrap().insert(request_id, tx);
        if self
            .send(&ClientMsg::Details {
                request_id,
                pid,
                start_ticks,
            })
            .is_err()
            && let Some(tx) = self.pending.lock().unwrap().remove(&request_id)
        {
            let _ = tx.send(Err(CollectError::Failed("collector disconnected".into())));
        }
        rx
    }

    pub fn pin(&self, id: ProcessId, name: String) {
        let _ = self.send(&ClientMsg::Pin { id, name });
    }

    pub fn set_cgroups(&self, on: bool) {
        let _ = self.send(&ClientMsg::SetCgroups { on });
    }

    pub fn unpin(&self, id: ProcessId) {
        let _ = self.send(&ClientMsg::Unpin { id });
    }
}

impl Drop for RemoteLive {
    fn drop(&mut self) {
        (self.closer)();
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.wait();
        }
    }
}
