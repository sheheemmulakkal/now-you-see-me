use serde::{Deserialize, Serialize};

use nysm_core::raw::ProcessId;
use nysm_core::snapshot::{HostInfo, Snapshot};

pub const FORMAT: &str = nysm_core::brand::RECORDING_FORMAT;
/// Bump on incompatible changes. Readers reject versions above this.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Record {
    Header(Box<Header>),
    Sample(Box<Sample>),
    Event(Event),
    End(End),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub format_version: u32,
    pub producer: String,
    /// Snapshot schema used inside samples.
    pub schema_version: u32,
    pub created_ms: i64,
    pub host: HostInfo,
    pub interval_ms: u64,
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// Whole-system recording over a time window.
    Window,
    /// Automatic capture around an alert (pre- and post-event window).
    Incident {
        rule: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        message: String,
        pre_s: f64,
        post_s: f64,
    },
    /// An explicitly launched command and its process tree.
    Command {
        /// Program name only; arguments are stored only with consent.
        program: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Vec<String>>,
        root_pid: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// System snapshot without the process table.
    pub snapshot: Snapshot,
    /// Top processes by CPU at this sample (attribution context).
    #[serde(default)]
    pub top: Vec<ProcessBrief>,
    /// Aggregate of the recorded command's process tree, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<TreeSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessBrief {
    #[serde(flatten)]
    pub id: ProcessId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    pub rss_bytes: u64,
}

/// Best-effort sampled view of a command's process tree. Children that
/// start and exit between samples are missed; processes that re-parent
/// away (daemonise) leave the tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSample {
    pub processes: u32,
    /// Sum of members' machine-share CPU % (members still warming up excluded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    /// Sum of members' RSS. Shared pages are counted once per process, so
    /// this is not deduplicated memory use.
    pub rss_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_bytes_per_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_bytes_per_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    CommandStarted,
    CommandExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
    },
    Interrupted,
    SizeLimitReached,
    /// An alert fired (incident trigger or a later alert during capture).
    AlertFired {
        rule: String,
        message: String,
    },
    /// Another alert transition during an incident capture.
    AlertUpdate {
        rule: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct End {
    pub timestamp_ms: i64,
    pub samples: u64,
    /// Scheduled samples that were not taken (collector fell behind).
    pub dropped_samples: u64,
    /// Stopped early because of the size limit or an interruption.
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandResult>,
}

/// Exact accounting from the OS for an explicitly launched command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub wall_s: f64,
    /// CPU time of the command and all descendants it waited for
    /// (`wait4` rusage). Exact, unlike sampled values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_cpu_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_cpu_s: Option<f64>,
    /// Peak RSS of the *largest single process* in the waited tree
    /// (`ru_maxrss`), not the sum across processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_single_process_rss_bytes: Option<u64>,
}

/// Non-counted interfaces/devices kept per sample (the busiest ones).
pub const MAX_EXTRA_ROWS: usize = 8;

/// Reduce a snapshot for storage: keep per-interface and per-device rows
/// that count towards totals, plus the `MAX_EXTRA_ROWS` busiest others.
/// Totals, CPU, memory and filesystems are kept unchanged.
pub fn compact_snapshot(s: &mut Snapshot) {
    fn keep<T>(rows: &mut Vec<T>, counted: impl Fn(&T) -> bool, activity: impl Fn(&T) -> f64) {
        let mut extra: Vec<(usize, f64)> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !counted(r))
            .map(|(i, r)| (i, activity(r)))
            .filter(|(_, a)| *a > 0.0)
            .collect();
        extra.sort_by(|a, b| b.1.total_cmp(&a.1));
        extra.truncate(MAX_EXTRA_ROWS);
        let mut i = 0;
        rows.retain(|r| {
            let k = counted(r) || extra.iter().any(|(j, _)| *j == i);
            i += 1;
            k
        });
    }
    if let Some(ifs) = s.network.interfaces.value.as_mut() {
        keep(
            ifs,
            |i| i.counted_in_total,
            |i| {
                i.rates
                    .live()
                    .map_or(0.0, |r| r.rx_bytes_per_s + r.tx_bytes_per_s)
            },
        );
    }
    if let Some(devs) = s.storage.devices.value.as_mut() {
        keep(
            devs,
            |d| d.counted_in_total,
            |d| {
                d.io.live().map_or(0.0, |io| {
                    io.read_bytes_per_s
                        + io.write_bytes_per_s
                        + io.read_ops_per_s
                        + io.write_ops_per_s
                })
            },
        );
    }
}
