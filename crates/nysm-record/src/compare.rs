//! Summaries of recordings and deterministic before/after comparison.
//!
//! Statements are observations about the recorded windows, never claims
//! about cause.

use serde::Serialize;

use crate::format::{CommandResult, Header, Sample, Target};
use crate::reader::Contents;

/// Distribution of a sampled value. Missing samples are excluded and
/// counted, never treated as zero.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub samples: usize,
    pub missing: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip)]
    values: Vec<f64>,
}

impl Stats {
    fn push(&mut self, v: Option<f64>) {
        match v.filter(|v| v.is_finite()) {
            Some(v) => self.values.push(v),
            None => self.missing += 1,
        }
    }

    fn finish(&mut self) {
        self.samples = self.values.len();
        if self.values.is_empty() {
            return;
        }
        let mut v = std::mem::take(&mut self.values);
        v.sort_by(|a, b| a.total_cmp(b));
        self.mean = Some(v.iter().sum::<f64>() / v.len() as f64);
        // Nearest-rank p95.
        let rank = ((0.95 * v.len() as f64).ceil() as usize).clamp(1, v.len());
        self.p95 = Some(v[rank - 1]);
        self.max = v.last().copied();
    }
}

/// Integral of a rate over sample intervals (bytes).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Total {
    pub bytes: f64,
    /// Seconds of the window not covered because the rate was missing.
    pub uncovered_s: f64,
}

impl Total {
    fn add(&mut self, rate: Option<f64>, interval_s: f64) {
        match rate {
            Some(r) => self.bytes += r * interval_s,
            None => self.uncovered_s += interval_s,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TreeSummary {
    pub cpu_pct: Stats,
    /// Highest sampled sum of tree RSS: a lower bound of the true peak, and
    /// not deduplicated for shared pages.
    pub sampled_peak_rss_bytes: u64,
    pub max_processes: u32,
    pub read: Total,
    pub write: Total,
}

/// A process that appeared in the per-sample top-5 lists.
#[derive(Debug, Clone, Serialize)]
pub struct TopProcess {
    pub name: String,
    pub pid: u32,
    /// Samples in which it was among the top 5 by CPU.
    pub samples: u64,
    pub max_cpu_pct: Option<f64>,
    pub mean_cpu_pct: Option<f64>,
    pub max_rss_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub hostname: Option<String>,
    pub boot_id: Option<String>,
    pub target: String,
    pub interval_ms: u64,
    pub samples: u64,
    pub dropped_samples: u64,
    pub gaps: u64,
    pub truncated: bool,
    pub duration_s: f64,
    pub cpu_pct: Stats,
    pub mem_used_pct: Stats,
    pub mem_used_bytes: Stats,
    pub swap_used_bytes: Stats,
    pub cpu_pressure_pct: Stats,
    pub mem_pressure_pct: Stats,
    pub io_pressure_pct: Stats,
    pub net_rx: Total,
    pub net_tx: Total,
    pub disk_read: Total,
    pub disk_write: Total,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<TreeSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandResult>,
    /// Busiest processes across the window (from sampled top-5 lists, so
    /// short-lived or briefly busy processes can be missed).
    pub top_processes: Vec<TopProcess>,
    pub warnings: Vec<String>,
}

#[derive(Default)]
struct TopAcc {
    name: String,
    samples: u64,
    cpu_sum: f64,
    cpu_n: u64,
    cpu_max: Option<f64>,
    rss_max: u64,
}

/// Streaming accumulator: feed samples from `reader::read`.
#[derive(Default)]
pub struct Summarizer {
    samples: u64,
    gaps: u64,
    first_ms: Option<i64>,
    last_ms: Option<i64>,
    covered_s: f64,
    cpu: Stats,
    mem_pct: Stats,
    mem_bytes: Stats,
    swap: Stats,
    cpu_psi: Stats,
    mem_psi: Stats,
    io_psi: Stats,
    net_rx: Total,
    net_tx: Total,
    disk_r: Total,
    disk_w: Total,
    tree: Option<TreeSummary>,
    /// Keyed by (pid, start ticks) so PID reuse does not merge processes.
    top: std::collections::HashMap<(u32, u64), TopAcc>,
}

impl Summarizer {
    pub fn add(&mut self, s: &Sample) {
        let snap = &s.snapshot;
        self.samples += 1;
        if snap.gap_before {
            self.gaps += 1;
        }
        self.first_ms.get_or_insert(snap.timestamp_ms);
        self.last_ms = Some(snap.timestamp_ms);
        let interval_s = snap.interval_ms.map_or(0.0, |ms| ms as f64 / 1000.0);
        self.covered_s += interval_s;
        let psi = |p: &nysm_core::Reading<nysm_core::snapshot::Pressure>| {
            p.live().and_then(|p| p.some.interval_pct)
        };
        self.cpu.push(snap.cpu.usage.live().map(|c| c.total_pct));
        self.mem_pct
            .push(snap.memory.usage.live().map(|m| m.used_pct));
        self.mem_bytes
            .push(snap.memory.usage.live().map(|m| m.used_bytes as f64));
        self.swap
            .push(snap.memory.swap.live().map(|m| m.used_bytes as f64));
        self.cpu_psi.push(psi(&snap.cpu.pressure));
        self.mem_psi.push(psi(&snap.memory.pressure));
        self.io_psi.push(psi(&snap.storage.io_pressure));
        let net = snap.network.total.live();
        self.net_rx.add(net.map(|n| n.rx_bytes_per_s), interval_s);
        self.net_tx.add(net.map(|n| n.tx_bytes_per_s), interval_s);
        let disk = snap.storage.total_io.live();
        self.disk_r
            .add(disk.map(|d| d.read_bytes_per_s), interval_s);
        self.disk_w
            .add(disk.map(|d| d.write_bytes_per_s), interval_s);
        for p in &s.top {
            let e = self
                .top
                .entry((p.id.pid, p.id.start_ticks))
                .or_insert_with(|| TopAcc {
                    name: p.name.clone(),
                    ..Default::default()
                });
            e.samples += 1;
            if let Some(c) = p.cpu_pct {
                e.cpu_sum += c;
                e.cpu_n += 1;
                e.cpu_max = Some(e.cpu_max.map_or(c, |m| m.max(c)));
            }
            e.rss_max = e.rss_max.max(p.rss_bytes);
        }
        if let Some(t) = &s.tree {
            let ts = self.tree.get_or_insert_with(TreeSummary::default);
            ts.cpu_pct.push(t.cpu_pct);
            ts.sampled_peak_rss_bytes = ts.sampled_peak_rss_bytes.max(t.rss_bytes);
            ts.max_processes = ts.max_processes.max(t.processes);
            ts.read.add(t.read_bytes_per_s, interval_s);
            ts.write.add(t.write_bytes_per_s, interval_s);
        }
    }

    pub fn finish(mut self, header: &Header, contents: &Contents) -> Summary {
        for s in [
            &mut self.cpu,
            &mut self.mem_pct,
            &mut self.mem_bytes,
            &mut self.swap,
            &mut self.cpu_psi,
            &mut self.mem_psi,
            &mut self.io_psi,
        ] {
            s.finish();
        }
        if let Some(t) = self.tree.as_mut() {
            t.cpu_pct.finish();
        }
        let end = contents.end.as_ref();
        let target = match &header.target {
            Target::Window => "time window".to_string(),
            Target::Incident { rule, .. } => format!("incident `{rule}`"),
            Target::Command { program, .. } => format!("command `{program}`"),
        };
        let mut warnings = contents.warnings.clone();
        if self.gaps > 0 {
            warnings.push(format!(
                "{} gaps (suspend or stalls) inside the recording",
                self.gaps
            ));
        }
        Summary {
            hostname: header.host.hostname.clone(),
            boot_id: header.host.boot_id.clone(),
            target,
            interval_ms: header.interval_ms,
            samples: self.samples,
            dropped_samples: end.map_or(0, |e| e.dropped_samples),
            gaps: self.gaps,
            truncated: end.is_none_or(|e| e.truncated),
            duration_s: self.covered_s,
            cpu_pct: self.cpu,
            mem_used_pct: self.mem_pct,
            mem_used_bytes: self.mem_bytes,
            swap_used_bytes: self.swap,
            cpu_pressure_pct: self.cpu_psi,
            mem_pressure_pct: self.mem_psi,
            io_pressure_pct: self.io_psi,
            net_rx: self.net_rx,
            net_tx: self.net_tx,
            disk_read: self.disk_r,
            disk_write: self.disk_w,
            tree: self.tree,
            command: end.and_then(|e| e.command.clone()),
            top_processes: {
                let mut v: Vec<TopProcess> = self
                    .top
                    .into_iter()
                    .map(|((pid, _), t)| TopProcess {
                        name: t.name,
                        pid,
                        samples: t.samples,
                        max_cpu_pct: t.cpu_max,
                        mean_cpu_pct: (t.cpu_n > 0).then(|| t.cpu_sum / t.cpu_n as f64),
                        max_rss_bytes: t.rss_max,
                    })
                    .collect();
                v.sort_by(|a, b| {
                    b.mean_cpu_pct
                        .unwrap_or(0.0)
                        .total_cmp(&a.mean_cpu_pct.unwrap_or(0.0))
                        .then(a.pid.cmp(&b.pid))
                });
                v.truncate(8);
                v
            },
            warnings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Percent,
    Bytes,
    Seconds,
    Count,
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub metric: &'static str,
    pub kind: Kind,
    /// Lower is better for every metric listed here (resource use).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<f64>,
    /// Relative change in percent, when both sides exist and before > 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_pct: Option<f64>,
    /// Exact OS accounting rather than a sampled value.
    pub exact: bool,
    pub note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub rows: Vec<Row>,
    pub observations: Vec<String>,
    pub caveats: Vec<String>,
}

fn row(
    metric: &'static str,
    kind: Kind,
    before: Option<f64>,
    after: Option<f64>,
    exact: bool,
    note: &'static str,
) -> Row {
    let change_pct = match (before, after) {
        (Some(b), Some(a)) if b.abs() > f64::EPSILON => Some((a - b) / b * 100.0),
        _ => None,
    };
    Row {
        metric,
        kind,
        before,
        after,
        change_pct,
        exact,
        note,
    }
}

/// Changes smaller than this are reported as "no clear change".
pub const NOISE_PCT: f64 = 5.0;

/// Minimum absolute difference for an observation, so that tiny values
/// (0.2% → 1.0% pressure, 84 KiB → 2 MiB) do not produce large percentages.
fn significant_abs(kind: Kind, before: f64, after: f64) -> bool {
    let d = (after - before).abs();
    match kind {
        Kind::Percent => d >= 1.0,
        Kind::Bytes => d >= 4.0 * 1024.0 * 1024.0,
        Kind::Seconds => d >= 0.05,
        Kind::Count => d >= 1.0,
    }
}

pub fn compare(a: &Summary, b: &Summary) -> Comparison {
    let mut rows = Vec::new();
    if let (Some(ca), Some(cb)) = (&a.command, &b.command) {
        let cpu = |c: &CommandResult| match (c.user_cpu_s, c.system_cpu_s) {
            (Some(u), Some(s)) => Some(u + s),
            _ => None,
        };
        rows.push(row(
            "command wall time",
            Kind::Seconds,
            Some(ca.wall_s),
            Some(cb.wall_s),
            true,
            "measured around wait",
        ));
        rows.push(row(
            "command CPU time (user+sys)",
            Kind::Seconds,
            cpu(ca),
            cpu(cb),
            true,
            "wait4 rusage of waited tree",
        ));
        rows.push(row(
            "largest single-process peak RSS",
            Kind::Bytes,
            ca.max_single_process_rss_bytes.map(|v| v as f64),
            cb.max_single_process_rss_bytes.map(|v| v as f64),
            true,
            "ru_maxrss; not a sum",
        ));
    }
    if let (Some(ta), Some(tb)) = (&a.tree, &b.tree) {
        rows.push(row(
            "tree sampled peak RSS (sum)",
            Kind::Bytes,
            Some(ta.sampled_peak_rss_bytes as f64),
            Some(tb.sampled_peak_rss_bytes as f64),
            false,
            "lower bound, shared pages counted per process",
        ));
        rows.push(row(
            "tree mean CPU",
            Kind::Percent,
            ta.cpu_pct.mean,
            tb.cpu_pct.mean,
            false,
            "% of machine",
        ));
        rows.push(row(
            "tree disk read",
            Kind::Bytes,
            Some(ta.read.bytes),
            Some(tb.read.bytes),
            false,
            "members seen at sample times",
        ));
        rows.push(row(
            "tree disk write",
            Kind::Bytes,
            Some(ta.write.bytes),
            Some(tb.write.bytes),
            false,
            "members seen at sample times",
        ));
    }
    rows.push(row(
        "duration",
        Kind::Seconds,
        Some(a.duration_s),
        Some(b.duration_s),
        false,
        "covered by samples",
    ));
    rows.push(row(
        "system CPU mean",
        Kind::Percent,
        a.cpu_pct.mean,
        b.cpu_pct.mean,
        false,
        "% of all cores",
    ));
    rows.push(row(
        "system CPU p95",
        Kind::Percent,
        a.cpu_pct.p95,
        b.cpu_pct.p95,
        false,
        "",
    ));
    rows.push(row(
        "memory used max",
        Kind::Bytes,
        a.mem_used_bytes.max,
        b.mem_used_bytes.max,
        false,
        "total − available",
    ));
    rows.push(row(
        "swap used max",
        Kind::Bytes,
        a.swap_used_bytes.max,
        b.swap_used_bytes.max,
        false,
        "",
    ));
    rows.push(row(
        "CPU pressure mean",
        Kind::Percent,
        a.cpu_pressure_pct.mean,
        b.cpu_pressure_pct.mean,
        false,
        "PSI some",
    ));
    rows.push(row(
        "memory pressure mean",
        Kind::Percent,
        a.mem_pressure_pct.mean,
        b.mem_pressure_pct.mean,
        false,
        "PSI some",
    ));
    rows.push(row(
        "I/O pressure mean",
        Kind::Percent,
        a.io_pressure_pct.mean,
        b.io_pressure_pct.mean,
        false,
        "PSI some",
    ));
    rows.push(row(
        "network received",
        Kind::Bytes,
        Some(a.net_rx.bytes),
        Some(b.net_rx.bytes),
        false,
        "physical interfaces",
    ));
    rows.push(row(
        "network sent",
        Kind::Bytes,
        Some(a.net_tx.bytes),
        Some(b.net_tx.bytes),
        false,
        "physical interfaces",
    ));
    rows.push(row(
        "disk read",
        Kind::Bytes,
        Some(a.disk_read.bytes),
        Some(b.disk_read.bytes),
        false,
        "whole disks",
    ));
    rows.push(row(
        "disk written",
        Kind::Bytes,
        Some(a.disk_write.bytes),
        Some(b.disk_write.bytes),
        false,
        "whole disks",
    ));

    let mut observations = Vec::new();
    for r in &rows {
        if r.metric == "duration" {
            continue;
        }
        if let (Some(c), Some(b), Some(a)) = (r.change_pct, r.before, r.after)
            && c.abs() >= NOISE_PCT
            && significant_abs(r.kind, b, a)
        {
            let dir = if c < 0.0 { "lower" } else { "higher" };
            let src = if r.exact { "exact" } else { "sampled" };
            observations.push(format!(
                "{} was {:.0}% {dir} in the second recording ({src}).",
                r.metric,
                c.abs()
            ));
        }
    }
    if observations.is_empty() {
        observations.push(format!(
            "No metric changed by {NOISE_PCT:.0}% or more (with a meaningful absolute difference)."
        ));
    }

    let mut caveats = vec![
        "Observations describe the recorded windows; they do not establish cause.".to_string(),
    ];
    if a.hostname != b.hostname {
        caveats.push("Recordings come from different hosts.".into());
    } else if a.boot_id != b.boot_id {
        caveats
            .push("Recordings span different boots (caches, background work may differ).".into());
    }
    if a.interval_ms != b.interval_ms {
        caveats.push(format!(
            "Sample intervals differ ({} ms vs {} ms).",
            a.interval_ms, b.interval_ms
        ));
    }
    if a.samples < 10 || b.samples < 10 {
        caveats.push("Fewer than 10 samples on one side: sampled statistics are noisy.".into());
    }
    for (name, s) in [("first", a), ("second", b)] {
        if s.truncated {
            caveats.push(format!("The {name} recording is truncated."));
        }
        if s.dropped_samples > 0 {
            caveats.push(format!(
                "The {name} recording dropped {} samples.",
                s.dropped_samples
            ));
        }
        if s.gaps > 0 {
            caveats.push(format!("The {name} recording contains {} gaps.", s.gaps));
        }
    }
    if a.command.is_some() != b.command.is_some() {
        caveats.push("Only one recording is a command recording; command rows are omitted.".into());
    }
    Comparison {
        rows,
        observations,
        caveats,
    }
}

#[cfg(test)]
pub(crate) mod tests;
