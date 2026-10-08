//! Where a frontend gets data: an attached collector service, or an
//! embedded engine. Both expose the same `LiveState`.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use nysm_collect::{CResult, ProcessDetails};
use nysm_core::alerts::Rule;
use nysm_core::raw::ProcessId;
use nysm_core::snapshot::Snapshot;
use nysm_engine::live::{Command, Live, LiveState};
use nysm_engine::{Engine, EngineConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    /// Use the service if one is running, else collect locally.
    Auto,
    /// Always collect locally.
    Never,
    /// Fail if no service is running.
    Require,
}

pub enum Source {
    Local(Live),
    #[cfg(unix)]
    Remote(crate::client::RemoteLive),
}

impl Source {
    pub fn local(engine: EngineConfig, rules: Vec<Rule>) -> Self {
        Source::Local(Live::spawn_with_alerts(Engine::new(engine), rules))
    }

    /// Open a source according to `attach`. `processes` is the initial
    /// process-table subscription.
    pub fn open(
        attach: Attach,
        client: &str,
        engine: EngineConfig,
        rules: Vec<Rule>,
    ) -> std::io::Result<Self> {
        Self::open_with(attach, client, engine, rules, false)
    }

    /// Like `open`; `lite` asks an attached service for totals only.
    pub fn open_with(
        attach: Attach,
        client: &str,
        engine: EngineConfig,
        rules: Vec<Rule>,
        lite: bool,
    ) -> std::io::Result<Self> {
        #[cfg(unix)]
        if attach != Attach::Never {
            let sub = crate::protocol::Subscribe {
                lite,
                cgroups: engine.cgroups,
                processes: engine.processes,
            };
            match crate::client::RemoteLive::connect(&crate::paths::runtime_dir(), client, sub) {
                Ok(r) => return Ok(Source::Remote(r)),
                Err(e) if attach == Attach::Require => return Err(e),
                Err(_) => {}
            }
        }
        #[cfg(not(unix))]
        let _ = (client, lite); // the service needs Unix sockets
        #[cfg(not(unix))]
        if attach == Attach::Require {
            return Err(std::io::Error::other(
                "the collector service is not supported on this platform yet",
            ));
        }
        Ok(Self::local(engine, rules))
    }

    /// Attach to a collector reached through a command's stdin/stdout,
    /// e.g. `ssh host nysm service stdio`.
    #[cfg(unix)]
    pub fn command(
        cmd: std::process::Command,
        client: &str,
        processes: bool,
        cgroups: bool,
    ) -> std::io::Result<Self> {
        let sub = crate::protocol::Subscribe {
            processes,
            lite: false,
            cgroups,
        };
        Ok(Source::Remote(crate::client::RemoteLive::connect_command(
            cmd, client, sub,
        )?))
    }

    pub fn is_remote(&self) -> bool {
        !matches!(self, Source::Local(_))
    }

    /// False once an attached service has gone away.
    pub fn is_connected(&self) -> bool {
        match self {
            Source::Local(_) => true,
            #[cfg(unix)]
            Source::Remote(r) => r.is_connected(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Local(_) => "embedded collector".into(),
            #[cfg(unix)]
            Source::Remote(r) => format!("attached to {} service", r.producer),
        }
    }

    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        match self {
            Source::Local(l) => l.latest(),
            #[cfg(unix)]
            Source::Remote(r) => r.latest(),
        }
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&LiveState) -> R) -> R {
        match self {
            Source::Local(l) => l.with_state(f),
            #[cfg(unix)]
            Source::Remote(r) => r.with_state(f),
        }
    }

    pub fn wait_newer(&self, after: u64, timeout: Duration) -> Option<Arc<Snapshot>> {
        match self {
            Source::Local(l) => l.wait_newer(after, timeout),
            #[cfg(unix)]
            Source::Remote(r) => r.wait_newer(after, timeout),
        }
    }

    pub fn request_details(
        &self,
        pid: u32,
        start_ticks: Option<u64>,
    ) -> mpsc::Receiver<CResult<ProcessDetails>> {
        match self {
            Source::Local(l) => {
                let (tx, rx) = mpsc::channel();
                l.send(Command::Details {
                    pid,
                    start_ticks,
                    include_cmdline: false,
                    reply: tx,
                });
                rx
            }
            #[cfg(unix)]
            Source::Remote(r) => r.request_details(pid, start_ticks),
        }
    }

    pub fn pin(&self, id: ProcessId, name: String) {
        match self {
            Source::Local(l) => l.send(Command::Pin { id, name }),
            #[cfg(unix)]
            Source::Remote(r) => r.pin(id, name),
        }
    }

    /// Subscribe to (or stop) per-container/service/app tables.
    pub fn set_cgroups(&self, on: bool) {
        match self {
            Source::Local(l) => l.send(Command::SetCgroups(on)),
            #[cfg(unix)]
            Source::Remote(r) => r.set_cgroups(on),
        }
    }

    pub fn unpin(&self, id: ProcessId) {
        match self {
            Source::Local(l) => l.send(Command::Unpin(id)),
            #[cfg(unix)]
            Source::Remote(r) => r.unpin(id),
        }
    }
}
