//! Shared process queries (sorting, filtering, trees) so every frontend
//! orders and groups processes identically.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::snapshot::ProcessSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSort {
    Cpu,
    Memory,
    DiskIo,
    Pid,
    Name,
}

impl ProcessSort {
    pub fn label(self) -> &'static str {
        match self {
            ProcessSort::Cpu => "CPU",
            ProcessSort::Memory => "memory",
            ProcessSort::DiskIo => "disk I/O",
            ProcessSort::Pid => "PID",
            ProcessSort::Name => "name",
        }
    }
}

fn io_total(p: &ProcessSnapshot) -> Option<f64> {
    p.disk_io
        .live()
        .map(|d| d.read_bytes_per_s + d.write_bytes_per_s)
}

/// Descending for resources (missing values last), ascending for PID/name.
pub fn compare(a: &ProcessSnapshot, b: &ProcessSnapshot, key: ProcessSort) -> Ordering {
    let desc_opt = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    let primary = match key {
        ProcessSort::Cpu => desc_opt(a.cpu_pct.live().copied(), b.cpu_pct.live().copied())
            .then_with(|| {
                b.cpu_time_s
                    .partial_cmp(&a.cpu_time_s)
                    .unwrap_or(Ordering::Equal)
            }),
        ProcessSort::Memory => b.rss_bytes.cmp(&a.rss_bytes),
        ProcessSort::DiskIo => desc_opt(io_total(a), io_total(b)),
        ProcessSort::Pid => a.id.pid.cmp(&b.id.pid),
        ProcessSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    };
    primary.then_with(|| a.id.pid.cmp(&b.id.pid))
}

pub fn matches(p: &ProcessSnapshot, needle_lower: &str) -> bool {
    needle_lower.is_empty()
        || p.name.to_lowercase().contains(needle_lower)
        || p.id.pid.to_string() == needle_lower
        || p.user
            .as_deref()
            .is_some_and(|u| u.to_lowercase() == needle_lower)
}

/// Sorted, filtered view as indices into `entries`.
pub fn view(entries: &[ProcessSnapshot], key: ProcessSort, filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    let mut idx: Vec<usize> = (0..entries.len())
        .filter(|&i| matches(&entries[i], &needle))
        .collect();
    idx.sort_by(|&a, &b| compare(&entries[a], &entries[b], key));
    idx
}

/// A row of a flattened process tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub index: usize,
    pub depth: usize,
    pub has_children: bool,
}

/// Depth-first process tree, siblings ordered by `key`. Parents are matched
/// by PID only, since Linux does not expose the parent's start time; a
/// parent that exited leaves its children re-parented by the kernel, so
/// this is exact for live processes. Processes whose parent is not visible
/// become roots. Cycles (impossible in a consistent snapshot, possible in a
/// racy one) are broken by visiting each process once.
pub fn tree(entries: &[ProcessSnapshot], key: ProcessSort) -> Vec<TreeRow> {
    let by_pid: HashMap<u32, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.pid, i))
        .collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (i, p) in entries.iter().enumerate() {
        match by_pid.get(&p.ppid) {
            Some(&parent) if parent != i && p.ppid != 0 => {
                children.entry(parent).or_default().push(i)
            }
            _ => roots.push(i),
        }
    }
    let sort = |v: &mut Vec<usize>| v.sort_by(|&a, &b| compare(&entries[a], &entries[b], key));
    sort(&mut roots);
    for v in children.values_mut() {
        sort(v);
    }
    let mut out = Vec::with_capacity(entries.len());
    let mut seen = vec![false; entries.len()];
    let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|r| (r, 0)).collect();
    while let Some((i, depth)) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let kids = children.get(&i);
        out.push(TreeRow {
            index: i,
            depth,
            has_children: kids.is_some_and(|k| !k.is_empty()),
        });
        if let Some(k) = kids {
            stack.extend(k.iter().rev().map(|&c| (c, depth + 1)));
        }
    }
    // Anything unreached is part of a cycle; append as roots.
    for (i, s) in seen.iter().enumerate() {
        if !s {
            out.push(TreeRow {
                index: i,
                depth: 0,
                has_children: false,
            });
        }
    }
    out
}

/// Order cgroup rows for display (descending resources, missing last).
pub fn sort_groups(groups: &mut [&crate::snapshot::CgroupSnapshot], key: ProcessSort) {
    let io = |g: &crate::snapshot::CgroupSnapshot| {
        g.disk_io
            .live()
            .map(|d| d.read_bytes_per_s + d.write_bytes_per_s)
    };
    groups.sort_by(|a, b| {
        let desc = |x: Option<f64>, y: Option<f64>| match (x, y) {
            (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(Ordering::Equal),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        match key {
            ProcessSort::Cpu => desc(a.cpu_pct.live().copied(), b.cpu_pct.live().copied()),
            ProcessSort::Memory => desc(
                a.memory_bytes.map(|v| v as f64),
                b.memory_bytes.map(|v| v as f64),
            ),
            ProcessSort::DiskIo => desc(io(a), io(b)),
            ProcessSort::Name | ProcessSort::Pid => {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
        }
        .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::ProcessId;
    use crate::{Reading, Status};

    fn p(pid: u32, ppid: u32, name: &str, cpu: Option<f64>, rss: u64) -> ProcessSnapshot {
        ProcessSnapshot {
            id: ProcessId {
                pid,
                start_ticks: 1,
            },
            ppid,
            name: name.into(),
            state: 'S',
            user: None,
            uid: None,
            threads: 1,
            cpu_pct: cpu.map_or_else(|| Reading::missing(Status::WarmingUp, "x"), Reading::ok),
            rss_bytes: rss,
            virtual_bytes: 0,
            cpu_time_s: 0.0,
            disk_io: Reading::missing(Status::PermissionDenied, "x"),
        }
    }

    #[test]
    fn sort_puts_missing_last() {
        let e = vec![
            p(1, 0, "a", None, 5),
            p(2, 0, "b", Some(1.0), 1),
            p(3, 0, "c", Some(9.0), 3),
        ];
        assert_eq!(view(&e, ProcessSort::Cpu, ""), vec![2, 1, 0]);
        assert_eq!(view(&e, ProcessSort::Memory, ""), vec![0, 2, 1]);
        assert_eq!(view(&e, ProcessSort::Name, "B"), vec![1]);
        assert_eq!(view(&e, ProcessSort::Pid, "3"), vec![2]);
    }

    #[test]
    fn tree_orders_depth_first_and_handles_orphans_and_cycles() {
        let e = vec![
            p(1, 0, "init", Some(0.0), 0),
            p(10, 1, "shell", Some(1.0), 0),
            p(11, 10, "cargo", Some(5.0), 0),
            p(12, 1, "busy", Some(50.0), 0),
            p(99, 77, "orphan", Some(0.0), 0),
            p(200, 201, "x", None, 0),
            p(201, 200, "y", None, 0),
        ];
        let t = tree(&e, ProcessSort::Cpu);
        let order: Vec<(u32, usize)> = t.iter().map(|r| (e[r.index].id.pid, r.depth)).collect();
        assert_eq!(&order[..5], &[(1, 0), (12, 1), (10, 1), (11, 2), (99, 0)]);
        assert_eq!(t.len(), e.len());
        assert!(t[0].has_children);
    }
}
