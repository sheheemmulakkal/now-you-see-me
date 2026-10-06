//! Bounded in-memory history of headline metrics.
//!
//! Each point is fixed-size, so the bound is enforced by both sample count
//! and bytes: capacity = min(max_samples, max_bytes / size_of::<HistoryPoint>()).
//! `None` marks a missing value; it is never drawn as zero.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::snapshot::Snapshot;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HistoryPoint {
    pub seq: u64,
    pub timestamp_ms: i64,
    /// Interpolation must not cross this point's left edge.
    pub gap_before: bool,
    pub cpu_pct: Option<f32>,
    pub mem_used_pct: Option<f32>,
    pub swap_used_bytes: Option<f32>,
    pub net_rx_bytes_per_s: Option<f32>,
    pub net_tx_bytes_per_s: Option<f32>,
    pub disk_read_bytes_per_s: Option<f32>,
    pub disk_write_bytes_per_s: Option<f32>,
    pub mem_pressure_some_pct: Option<f32>,
    pub io_pressure_some_pct: Option<f32>,
    pub cpu_pressure_some_pct: Option<f32>,
}

impl HistoryPoint {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        let f = |v: f64| v as f32;
        let pressure = |p: &crate::Reading<crate::snapshot::Pressure>| {
            p.live().and_then(|p| p.some.interval_pct.map(f))
        };
        HistoryPoint {
            seq: s.seq,
            timestamp_ms: s.timestamp_ms,
            gap_before: s.gap_before,
            cpu_pct: s.cpu.usage.live().map(|c| f(c.total_pct)),
            mem_used_pct: s.memory.usage.live().map(|m| f(m.used_pct)),
            swap_used_bytes: s.memory.swap.live().map(|m| m.used_bytes as f32),
            net_rx_bytes_per_s: s.network.total.live().map(|n| f(n.rx_bytes_per_s)),
            net_tx_bytes_per_s: s.network.total.live().map(|n| f(n.tx_bytes_per_s)),
            disk_read_bytes_per_s: s.storage.total_io.live().map(|d| f(d.read_bytes_per_s)),
            disk_write_bytes_per_s: s.storage.total_io.live().map(|d| f(d.write_bytes_per_s)),
            mem_pressure_some_pct: pressure(&s.memory.pressure),
            io_pressure_some_pct: pressure(&s.storage.io_pressure),
            cpu_pressure_some_pct: pressure(&s.cpu.pressure),
        }
    }
}

#[derive(Debug, Clone)]
pub struct History {
    points: VecDeque<HistoryPoint>,
    capacity: usize,
}

impl History {
    pub fn new(max_samples: usize, max_bytes: usize) -> Self {
        let by_bytes = max_bytes / std::mem::size_of::<HistoryPoint>();
        let capacity = max_samples.min(by_bytes).max(1);
        History {
            points: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn push(&mut self, p: HistoryPoint) {
        if self.points.len() == self.capacity {
            self.points.pop_front();
        }
        self.points.push_back(p);
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &HistoryPoint> + ExactSizeIterator {
        self.points.iter()
    }

    pub fn last(&self) -> Option<&HistoryPoint> {
        self.points.back()
    }

    /// Approximate heap bytes held by the buffer.
    pub fn heap_bytes(&self) -> usize {
        self.points.capacity() * std::mem::size_of::<HistoryPoint>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_samples_are_all_kept() {
        // Regression: a 64-byte budget per point (points are larger) kept
        // only ~60 % of the configured history.
        let h = History::new(600, history_bytes(600));
        assert_eq!(h.capacity(), 600);
        assert!(max_history_samples() >= 24 * 3600 + 1, "a day at 1 s fits");
    }

    fn point(seq: u64) -> HistoryPoint {
        HistoryPoint {
            seq,
            timestamp_ms: seq as i64 * 1000,
            gap_before: false,
            cpu_pct: Some(1.0),
            mem_used_pct: None,
            swap_used_bytes: None,
            net_rx_bytes_per_s: None,
            net_tx_bytes_per_s: None,
            disk_read_bytes_per_s: None,
            disk_write_bytes_per_s: None,
            mem_pressure_some_pct: None,
            io_pressure_some_pct: None,
            cpu_pressure_some_pct: None,
        }
    }

    #[test]
    fn bounded_by_samples() {
        let mut h = History::new(3, usize::MAX);
        for i in 0..10 {
            h.push(point(i));
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.iter().map(|p| p.seq).collect::<Vec<_>>(), vec![7, 8, 9]);
    }

    #[test]
    fn bounded_by_bytes() {
        let size = std::mem::size_of::<HistoryPoint>();
        let mut h = History::new(1000, size * 5);
        for i in 0..100 {
            h.push(point(i));
        }
        assert_eq!(h.len(), 5);
        assert!(h.heap_bytes() <= size * 8);
    }
}

/// Upper bound on retained history: 16 MiB, about 44 h at a 1 s interval.
pub const MAX_HISTORY_BYTES: usize = 16 << 20;

/// Bytes needed to keep `samples` points (the real point size), capped at
/// [`MAX_HISTORY_BYTES`]. Use with `History::new(samples, history_bytes(samples))`.
pub fn history_bytes(samples: usize) -> usize {
    samples
        .saturating_mul(std::mem::size_of::<HistoryPoint>())
        .min(MAX_HISTORY_BYTES)
}

/// Most points that fit in [`MAX_HISTORY_BYTES`].
pub fn max_history_samples() -> usize {
    MAX_HISTORY_BYTES / std::mem::size_of::<HistoryPoint>()
}

/// One sample of a pinned process.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProcessPoint {
    pub timestamp_ms: i64,
    pub cpu_pct: Option<f32>,
    pub rss_bytes: u64,
    pub read_bytes_per_s: Option<f32>,
    pub write_bytes_per_s: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedProcess {
    pub id: crate::raw::ProcessId,
    pub name: String,
    /// The process is gone; history is kept until unpinned.
    pub exited: bool,
    pub points: VecDeque<ProcessPoint>,
}

/// Detailed history for a bounded set of pinned processes, so top-N
/// tables never need unbounded per-process history.
#[derive(Debug, Clone)]
pub struct ProcessHistory {
    max_pins: usize,
    max_points: usize,
    pinned: Vec<PinnedProcess>,
}

impl ProcessHistory {
    pub const DEFAULT_MAX_PINS: usize = 8;

    pub fn new(max_pins: usize, max_points: usize) -> Self {
        ProcessHistory {
            max_pins,
            max_points: max_points.max(1),
            pinned: Vec::new(),
        }
    }

    pub fn pin(&mut self, id: crate::raw::ProcessId, name: String) -> Result<(), String> {
        if self.pinned.iter().any(|p| p.id == id) {
            return Ok(());
        }
        if self.pinned.len() >= self.max_pins {
            return Err(format!("at most {} processes can be pinned", self.max_pins));
        }
        self.pinned.push(PinnedProcess {
            id,
            name,
            exited: false,
            points: VecDeque::new(),
        });
        Ok(())
    }

    pub fn unpin(&mut self, id: &crate::raw::ProcessId) {
        self.pinned.retain(|p| &p.id != id);
    }

    pub fn is_pinned(&self, id: &crate::raw::ProcessId) -> bool {
        self.pinned.iter().any(|p| &p.id == id)
    }

    pub fn pinned(&self) -> &[PinnedProcess] {
        &self.pinned
    }

    /// Record a new process table.
    pub fn observe(&mut self, table: &crate::snapshot::ProcessTable) {
        for pin in &mut self.pinned {
            match table.entries.iter().find(|e| e.id == pin.id) {
                Some(e) => {
                    if pin.points.len() == self.max_points {
                        pin.points.pop_front();
                    }
                    let io = e.disk_io.live();
                    pin.points.push_back(ProcessPoint {
                        timestamp_ms: table.timestamp_ms,
                        cpu_pct: e.cpu_pct.live().map(|v| *v as f32),
                        rss_bytes: e.rss_bytes,
                        read_bytes_per_s: io.map(|d| d.read_bytes_per_s as f32),
                        write_bytes_per_s: io.map(|d| d.write_bytes_per_s as f32),
                    });
                }
                None => pin.exited = true,
            }
        }
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;
    use crate::raw::ProcessId;
    use crate::snapshot::{ProcessSnapshot, ProcessTable};
    use crate::{Reading, Status};

    fn table(ts: i64, pids: &[(u32, u64)]) -> ProcessTable {
        ProcessTable {
            timestamp_ms: ts,
            interval_ms: Some(1000),
            logical_cores: 1,
            unreadable: 0,
            entries: pids
                .iter()
                .map(|(pid, start)| ProcessSnapshot {
                    id: ProcessId {
                        pid: *pid,
                        start_ticks: *start,
                    },
                    ppid: 1,
                    name: "x".into(),
                    state: 'S',
                    user: None,
                    uid: None,
                    threads: 1,
                    cpu_pct: Reading::ok(5.0),
                    rss_bytes: ts as u64,
                    virtual_bytes: 0,
                    cpu_time_s: 0.0,
                    disk_io: Reading::missing(Status::PermissionDenied, "x"),
                })
                .collect(),
        }
    }

    #[test]
    fn pins_are_bounded_and_track_identity() {
        let mut h = ProcessHistory::new(2, 3);
        let a = ProcessId {
            pid: 10,
            start_ticks: 1,
        };
        h.pin(a.clone(), "a".into()).unwrap();
        h.pin(a.clone(), "a".into()).unwrap();
        h.pin(
            ProcessId {
                pid: 11,
                start_ticks: 1,
            },
            "b".into(),
        )
        .unwrap();
        assert!(
            h.pin(
                ProcessId {
                    pid: 12,
                    start_ticks: 1
                },
                "c".into()
            )
            .is_err()
        );
        for ts in 0..5 {
            h.observe(&table(ts, &[(10, 1)]));
        }
        let p = &h.pinned()[0];
        assert_eq!(p.points.len(), 3);
        assert_eq!(p.points.back().unwrap().rss_bytes, 4);
        assert!(p.points[0].read_bytes_per_s.is_none());
        // PID reused by another process: the pinned one has exited.
        h.observe(&table(9, &[(10, 99)]));
        assert!(h.pinned()[0].exited);
        assert_eq!(h.pinned()[0].points.len(), 3);
        h.unpin(&a);
        assert!(!h.is_pinned(&a));
    }
}
