//! Incident snapshots: when an alert fires, write a recording containing
//! the samples from before the event (kept in a bounded in-memory buffer)
//! and keep recording for a window after it. Opt-in; bounded by sample
//! count, file count and total bytes; oldest incidents are deleted first.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nysm_core::alerts::{AlertEvent, AlertEventKind};
use nysm_core::snapshot::Snapshot;

use crate::format::*;
use crate::writer::RecordingWriter;

#[derive(Debug, Clone)]
pub struct IncidentConfig {
    pub dir: PathBuf,
    pub pre: Duration,
    pub post: Duration,
    pub interval: Duration,
    pub max_files: usize,
    pub max_total_bytes: u64,
}

/// Concurrent captures are bounded; further alerts while this many are
/// open are added as events to the newest one.
const MAX_ACTIVE: usize = 4;
/// Per-file cap, so one incident cannot consume the whole budget.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

struct Active {
    key: (String, Option<String>),
    writer: RecordingWriter,
    remaining: u64,
    path: PathBuf,
}

pub struct IncidentRecorder {
    cfg: IncidentConfig,
    buffer: VecDeque<Sample>,
    buffer_cap: usize,
    active: Vec<Active>,
    last_seq: Option<u64>,
}

fn samples_for(d: Duration, interval: Duration) -> u64 {
    (d.as_secs_f64() / interval.as_secs_f64().max(0.001)).ceil() as u64
}

fn file_name(e: &AlertEvent) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let target = e
        .target
        .as_deref()
        .map(|t| format!("-{}", clean(t)))
        .unwrap_or_default();
    format!(
        "incident-{}-{}{}.nysm",
        e.timestamp_ms,
        clean(&e.rule),
        target
    )
}

impl IncidentRecorder {
    /// Creates the directory with user-only permissions.
    pub fn new(cfg: IncidentConfig) -> io::Result<Self> {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&cfg.dir)?;
        let buffer_cap = samples_for(cfg.pre, cfg.interval) as usize + 1;
        Ok(IncidentRecorder {
            cfg,
            buffer: VecDeque::with_capacity(buffer_cap),
            buffer_cap,
            active: Vec::new(),
            last_seq: None,
        })
    }

    /// Bytes held by the pre-event buffer (approximate, for diagnostics).
    pub fn buffered_samples(&self) -> usize {
        self.buffer.len()
    }

    /// Feed one snapshot and the alert events produced for it. Returns the
    /// paths of incident files completed by this call.
    pub fn observe(&mut self, snap: &Snapshot, events: &[AlertEvent]) -> io::Result<Vec<PathBuf>> {
        // Samples the caller skipped (it fell behind) are counted as dropped.
        let missed = self.last_seq.map_or(0, |l| snap.seq.saturating_sub(l + 1));
        self.last_seq = Some(snap.seq);
        let sample = to_sample(snap);
        for a in &mut self.active {
            a.writer.dropped += missed;
            let _ = a.writer.sample(sample.clone())?;
            a.remaining = a.remaining.saturating_sub(1 + missed);
        }
        if self.buffer.len() == self.buffer_cap {
            self.buffer.pop_front();
        }
        self.buffer.push_back(sample);

        for e in events {
            let AlertEventKind::Fired { .. } = e.kind else {
                // Resolutions and data events are recorded in open captures.
                for a in &mut self.active {
                    a.writer.event(Event {
                        timestamp_ms: e.timestamp_ms,
                        kind: EventKind::AlertUpdate {
                            rule: e.rule.clone(),
                            message: e.describe(),
                        },
                    })?;
                }
                continue;
            };
            let key = (e.rule.clone(), e.target.clone());
            let fired = || Event {
                timestamp_ms: e.timestamp_ms,
                kind: EventKind::AlertFired {
                    rule: e.rule.clone(),
                    message: e.describe(),
                },
            };
            if self.active.iter().any(|a| a.key == key) {
                continue;
            }
            if self.active.len() >= MAX_ACTIVE {
                // Record it in the newest open capture instead.
                if let Some(a) = self.active.last_mut() {
                    a.writer.event(fired())?;
                }
                continue;
            }
            self.active.push(self.start(e, snap)?);
        }

        let mut done = Vec::new();
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i].remaining == 0 {
                let a = self.active.remove(i);
                a.writer.finish(End {
                    timestamp_ms: snap.timestamp_ms,
                    samples: 0,
                    dropped_samples: 0,
                    truncated: false,
                    command: None,
                })?;
                done.push(a.path);
            } else {
                i += 1;
            }
        }
        if !done.is_empty() {
            self.enforce_retention()?;
        }
        Ok(done)
    }

    fn start(&self, e: &AlertEvent, snap: &Snapshot) -> io::Result<Active> {
        let path = self.cfg.dir.join(file_name(e));
        let header = Header {
            format: FORMAT.into(),
            format_version: FORMAT_VERSION,
            producer: nysm_core::brand::PRODUCER.into(),
            schema_version: nysm_core::SCHEMA_VERSION,
            created_ms: e.timestamp_ms,
            host: snap.host.clone(),
            interval_ms: self.cfg.interval.as_millis() as u64,
            target: Target::Incident {
                rule: e.rule.clone(),
                target: e.target.clone(),
                message: e.describe(),
                pre_s: self.cfg.pre.as_secs_f64(),
                post_s: self.cfg.post.as_secs_f64(),
            },
            label: None,
        };
        let mut writer = RecordingWriter::create(
            &path,
            &header,
            MAX_FILE_BYTES.min(self.cfg.max_total_bytes),
            true,
        )?;
        // The pre-event window (it already ends with the current sample).
        for s in &self.buffer {
            let _ = writer.sample(s.clone())?;
        }
        writer.event(Event {
            timestamp_ms: e.timestamp_ms,
            kind: EventKind::AlertFired {
                rule: e.rule.clone(),
                message: e.describe(),
            },
        })?;
        Ok(Active {
            key: (e.rule.clone(), e.target.clone()),
            writer,
            remaining: samples_for(self.cfg.post, self.cfg.interval),
            path,
        })
    }

    /// Close open captures (e.g. on shutdown); they are marked truncated.
    pub fn finish_all(&mut self, timestamp_ms: i64) -> io::Result<Vec<PathBuf>> {
        let mut done = Vec::new();
        for a in self.active.drain(..) {
            a.writer.finish(End {
                timestamp_ms,
                samples: 0,
                dropped_samples: 0,
                truncated: true,
                command: None,
            })?;
            done.push(a.path);
        }
        self.enforce_retention()?;
        Ok(done)
    }

    /// Delete the oldest incident files beyond the count or size limits.
    pub fn enforce_retention(&self) -> io::Result<()> {
        let mut files = list(&self.cfg.dir)?;
        let open: Vec<&PathBuf> = self.active.iter().map(|a| &a.path).collect();
        files.retain(|f| !open.contains(&&f.path));
        // Newest first.
        files.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
        let mut total = 0u64;
        for (i, f) in files.iter().enumerate() {
            total += f.bytes;
            if i >= self.cfg.max_files || total > self.cfg.max_total_bytes {
                std::fs::remove_file(&f.path)?;
            }
        }
        Ok(())
    }
}

fn to_sample(s: &Snapshot) -> Sample {
    let mut snap = s.clone();
    let top = snap
        .processes
        .take()
        .map(|t| {
            nysm_core::query::view(&t.entries, nysm_core::query::ProcessSort::Cpu, "")
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
                .collect()
        })
        .unwrap_or_default();
    snap.cgroups = None;
    compact_snapshot(&mut snap);
    Sample {
        snapshot: snap,
        top,
        tree: None,
    }
}

#[derive(Debug, Clone)]
pub struct IncidentFile {
    pub path: PathBuf,
    pub bytes: u64,
    /// From the file name (event time, Unix ms).
    pub created_ms: i64,
}

/// Incident files in `dir`, newest first.
pub fn list(dir: &Path) -> io::Result<Vec<IncidentFile>> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name
            .strip_prefix("incident-")
            .and_then(|r| r.strip_suffix(".nysm"))
        else {
            continue;
        };
        let created_ms = rest
            .split('-')
            .next()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(IncidentFile {
            path: e.path(),
            bytes,
            created_ms,
        });
    }
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    Ok(out)
}

#[cfg(test)]
mod tests;
