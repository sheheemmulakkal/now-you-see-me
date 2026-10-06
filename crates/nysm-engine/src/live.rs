//! A background collection thread for interactive frontends.
//!
//! Collection never waits for consumers: each sample replaces the latest
//! snapshot in a shared slot. A slow consumer simply observes a jump in
//! `seq` (coalesced snapshots) instead of delaying the collector.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use std::collections::VecDeque;

use nysm_collect::{CResult, ProcessDetails};
use nysm_core::alerts::{ActiveAlert, AlertEngine, AlertEvent, Rule};
use nysm_core::history::{History, HistoryPoint};
use nysm_core::snapshot::Snapshot;

use crate::Engine;

pub enum Command {
    SetProcesses(bool),
    SetCgroups(bool),
    Pin {
        id: nysm_core::raw::ProcessId,
        name: String,
    },
    Unpin(nysm_core::raw::ProcessId),
    SetInterval(Duration),
    Details {
        pid: u32,
        start_ticks: Option<u64>,
        include_cmdline: bool,
        reply: Sender<CResult<ProcessDetails>>,
    },
}

pub struct LiveState {
    pub latest: Option<Arc<Snapshot>>,
    pub history: History,
    /// Alerts currently pending or firing (evaluated on every sample, so
    /// a slow or paused UI cannot miss a transition).
    pub alerts: Vec<ActiveAlert>,
    /// Most recent alert events, newest last, bounded.
    pub alert_events: VecDeque<AlertEvent>,
    /// Detailed history of pinned processes.
    pub pinned: Vec<nysm_core::history::PinnedProcess>,
    /// Total alert events ever produced (lets readers find new ones).
    pub alert_events_total: u64,
}

impl LiveState {
    pub fn new(history_samples: usize, history_bytes: usize) -> Self {
        LiveState {
            latest: None,
            history: History::new(history_samples, history_bytes),
            alerts: Vec::new(),
            alert_events: VecDeque::new(),
            pinned: Vec::new(),
            alert_events_total: 0,
        }
    }

    /// Events newer than `seen_total`, and the new total.
    pub fn events_since(&self, seen_total: u64) -> (Vec<AlertEvent>, u64) {
        let new = (self.alert_events_total.saturating_sub(seen_total) as usize)
            .min(self.alert_events.len());
        (
            self.alert_events
                .iter()
                .skip(self.alert_events.len() - new)
                .cloned()
                .collect(),
            self.alert_events_total,
        )
    }

    pub fn push_event(&mut self, e: AlertEvent) {
        if self.alert_events.len() == MAX_ALERT_EVENTS {
            self.alert_events.pop_front();
        }
        self.alert_events.push_back(e);
        self.alert_events_total += 1;
    }
}

/// Retained alert events in `LiveState`.
pub const MAX_ALERT_EVENTS: usize = 64;

struct Shared {
    state: Mutex<LiveState>,
    cv: Condvar,
}

pub struct Live {
    shared: Arc<Shared>,
    tx: Option<Sender<Command>>,
    handle: Option<JoinHandle<()>>,
}

impl Live {
    pub fn spawn(engine: Engine) -> Self {
        Self::spawn_with_alerts(engine, Vec::new())
    }

    pub fn spawn_with_alerts(mut engine: Engine, rules: Vec<Rule>) -> Self {
        let cfg = engine.config().clone();
        let mut alerts = AlertEngine::new(rules);
        let shared = Arc::new(Shared {
            state: Mutex::new(LiveState::new(cfg.history_samples, cfg.history_bytes)),
            cv: Condvar::new(),
        });
        let (tx, rx) = mpsc::channel::<Command>();
        let sh = shared.clone();
        let handle = thread::Builder::new()
            .name("nysm-collector".into())
            .spawn(move || {
                let mut interval = cfg.interval;
                let mut next = Instant::now();
                loop {
                    let snap = Arc::new(engine.sample());
                    let events = alerts.evaluate(&snap);
                    let active = alerts.active();
                    {
                        let mut st = sh.state.lock().unwrap();
                        st.history.push(HistoryPoint::from_snapshot(&snap));
                        st.latest = Some(snap);
                        st.alerts = active;
                        st.pinned = engine.pinned().to_vec();
                        for e in events {
                            st.push_event(e);
                        }
                    }
                    sh.cv.notify_all();
                    next += interval;
                    let now = Instant::now();
                    if next < now {
                        // We fell behind (suspend, slow provider): realign
                        // instead of bursting to catch up.
                        next = now + interval;
                    }
                    loop {
                        let wait = next.saturating_duration_since(Instant::now());
                        if wait.is_zero() {
                            break;
                        }
                        match rx.recv_timeout(wait) {
                            Ok(Command::SetProcesses(on)) => engine.set_processes(on),
                            Ok(Command::SetCgroups(on)) => engine.set_cgroups(on),
                            Ok(Command::Pin { id, name }) => {
                                // Over the limit is ignored; the UI shows the pin count.
                                let _ = engine.pin(id, name);
                                sh.state.lock().unwrap().pinned = engine.pinned().to_vec();
                            }
                            Ok(Command::Unpin(id)) => {
                                engine.unpin(&id);
                                sh.state.lock().unwrap().pinned = engine.pinned().to_vec();
                            }
                            Ok(Command::SetInterval(d)) => {
                                interval = d;
                                engine.set_interval(d);
                                next = Instant::now() + d;
                            }
                            Ok(Command::Details {
                                pid,
                                start_ticks,
                                include_cmdline,
                                reply,
                            }) => {
                                let _ = reply.send(engine.process_details(
                                    pid,
                                    start_ticks,
                                    include_cmdline,
                                ));
                            }
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
            })
            .expect("spawn collector thread");
        Live {
            shared,
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.shared.state.lock().unwrap().latest.clone()
    }

    pub fn with_history<R>(&self, f: impl FnOnce(&History) -> R) -> R {
        f(&self.shared.state.lock().unwrap().history)
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&LiveState) -> R) -> R {
        f(&self.shared.state.lock().unwrap())
    }

    /// Wait until a snapshot with `seq > after` exists, up to `timeout`.
    pub fn wait_newer(&self, after: u64, timeout: Duration) -> Option<Arc<Snapshot>> {
        let g = self.shared.state.lock().unwrap();
        let (g, _) = self
            .shared
            .cv
            .wait_timeout_while(g, timeout, |s| {
                s.latest.as_ref().is_none_or(|l| l.seq <= after)
            })
            .unwrap();
        g.latest.clone().filter(|l| l.seq > after)
    }

    pub fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// Ask the collector thread for process details without blocking longer
    /// than `timeout`.
    pub fn details(
        &self,
        pid: u32,
        start_ticks: Option<u64>,
        include_cmdline: bool,
        timeout: Duration,
    ) -> Option<CResult<ProcessDetails>> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Details {
            pid,
            start_ticks,
            include_cmdline,
            reply,
        });
        rx.recv_timeout(timeout).ok()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // Disconnecting the channel stops the thread at its next wait.
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
