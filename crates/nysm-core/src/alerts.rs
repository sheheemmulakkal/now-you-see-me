//! Sustained-threshold alerts: a pure, deterministic state machine.
//!
//! A rule fires only after its condition holds continuously for `for_s`
//! seconds of observed data, clears only past `clear` (hysteresis), and
//! does not re-notify within `cooldown_s` of resolving. Missing data never
//! counts as a breach or a recovery: a pending breach is reset, and a
//! firing alert stays firing, flagged as lacking data. Sustain timers never
//! span a gap (suspend/stall) because elapsed time is accumulated from
//! sample intervals, not from wall time.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::snapshot::{Pressure, Snapshot};
use crate::{Reading, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertMetric {
    /// Machine CPU utilisation, %.
    CpuPct,
    /// Memory used (total − available), %.
    MemoryUsedPct,
    /// Swap used, % of swap size.
    SwapUsedPct,
    /// PSI `some` over the sample interval, %.
    CpuPressurePct,
    MemoryPressurePct,
    IoPressurePct,
    /// Per filesystem: used / (used + available), %. One state per mount.
    FilesystemUsedPct,
    /// CPU temperature (package sensor, else hottest CPU sensor), °C.
    CpuTemperatureC,
}

impl AlertMetric {
    pub fn label(self) -> &'static str {
        match self {
            AlertMetric::CpuPct => "CPU usage",
            AlertMetric::MemoryUsedPct => "memory used",
            AlertMetric::SwapUsedPct => "swap used",
            AlertMetric::CpuPressurePct => "CPU pressure",
            AlertMetric::MemoryPressurePct => "memory pressure",
            AlertMetric::IoPressurePct => "I/O pressure",
            AlertMetric::FilesystemUsedPct => "filesystem used",
            AlertMetric::CpuTemperatureC => "CPU temperature",
        }
    }

    /// Unit suffix for values and thresholds.
    pub fn unit(self) -> &'static str {
        match self {
            AlertMetric::CpuTemperatureC => " °C",
            _ => "%",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Above,
    Below,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub metric: AlertMetric,
    #[serde(default = "default_direction")]
    pub direction: Direction,
    /// Breach when the value is beyond this.
    pub threshold: f64,
    /// Resolve only when the value is back beyond this (hysteresis).
    pub clear: f64,
    /// Seconds the breach must be sustained before firing.
    pub for_s: f64,
    /// Seconds after resolving during which a new firing is not notified.
    #[serde(default)]
    pub cooldown_s: f64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_direction() -> Direction {
    Direction::Above
}

fn default_true() -> bool {
    true
}

impl Rule {
    pub fn validate(&self) -> Result<(), String> {
        let finite = [self.threshold, self.clear, self.for_s, self.cooldown_s]
            .iter()
            .all(|v| v.is_finite());
        if !finite {
            return Err(format!("rule {}: values must be finite", self.id));
        }
        if self.for_s < 0.0 || self.cooldown_s < 0.0 {
            return Err(format!("rule {}: durations must be ≥ 0", self.id));
        }
        let ok = match self.direction {
            Direction::Above => self.clear <= self.threshold,
            Direction::Below => self.clear >= self.threshold,
        };
        if !ok {
            return Err(format!(
                "rule {}: clear must be on the healthy side of threshold",
                self.id
            ));
        }
        Ok(())
    }

    fn breached(&self, v: f64) -> bool {
        match self.direction {
            Direction::Above => v > self.threshold,
            Direction::Below => v < self.threshold,
        }
    }

    fn cleared(&self, v: f64) -> bool {
        match self.direction {
            Direction::Above => v <= self.clear,
            Direction::Below => v >= self.clear,
        }
    }
}

/// Conservative defaults: free space and sustained stalls, not CPU spikes.
pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: "filesystem-nearly-full".into(),
            metric: AlertMetric::FilesystemUsedPct,
            direction: Direction::Above,
            threshold: 95.0,
            clear: 93.0,
            for_s: 60.0,
            cooldown_s: 600.0,
            enabled: true,
        },
        Rule {
            id: "memory-pressure".into(),
            metric: AlertMetric::MemoryPressurePct,
            direction: Direction::Above,
            threshold: 10.0,
            clear: 5.0,
            for_s: 30.0,
            cooldown_s: 300.0,
            enabled: true,
        },
        Rule {
            id: "io-pressure".into(),
            metric: AlertMetric::IoPressurePct,
            direction: Direction::Above,
            threshold: 30.0,
            clear: 15.0,
            for_s: 60.0,
            cooldown_s: 300.0,
            enabled: true,
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AlertState {
    Ok,
    Pending {
        since_s: f64,
    },
    Firing {
        since_s: f64,
        since_ms: i64,
        notified: bool,
        data_missing: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AlertEventKind {
    Fired {
        value: f64,
        threshold: f64,
        sustained_s: f64,
    },
    Resolved {
        value: f64,
        clear: f64,
        firing_s: f64,
    },
    /// A firing alert lost its data source; it is not considered resolved.
    DataMissing {
        status: Status,
    },
    DataRestored,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertEvent {
    pub rule: String,
    pub metric: AlertMetric,
    /// Sub-target, e.g. the mount point for filesystem rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub kind: AlertEventKind,
}

impl AlertEvent {
    /// Deterministic one-line explanation built from the triggering values.
    pub fn describe(&self) -> String {
        let u = self.metric.unit();
        let what = match &self.target {
            Some(t) => format!("{} on {t}", self.metric.label()),
            None => self.metric.label().to_string(),
        };
        match &self.kind {
            AlertEventKind::Fired {
                value,
                threshold,
                sustained_s,
            } => {
                format!(
                    "{what} at {value:.1}{u} stayed beyond {threshold:.1}{u} for {sustained_s:.0} s"
                )
            }
            AlertEventKind::Resolved {
                value,
                clear,
                firing_s,
            } => {
                format!(
                    "{what} back to {value:.1}{u} (clear at {clear:.1}{u}) after {firing_s:.0} s firing"
                )
            }
            AlertEventKind::DataMissing { status } => {
                format!(
                    "{what}: data {} — alert remains firing until data shows recovery",
                    status.label()
                )
            }
            AlertEventKind::DataRestored => format!("{what}: data available again"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub rule: String,
    pub metric: AlertMetric,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub state: AlertState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone)]
struct Track {
    state: AlertState,
    last_resolved_s: Option<f64>,
    last_value: Option<f64>,
}

/// Evaluates rules against successive snapshots.
#[derive(Debug, Clone)]
pub struct AlertEngine {
    rules: Vec<Rule>,
    tracks: HashMap<(usize, Option<String>), Track>,
    /// Observed time: sum of sample intervals (excludes gaps).
    t_s: f64,
}

/// Durations are sums of float intervals; round to milliseconds for output.
fn round_ms(s: f64) -> f64 {
    (s * 1000.0).round() / 1000.0
}

fn psi(r: &Reading<Pressure>) -> Result<f64, Status> {
    match r.live() {
        Some(p) => p.some.interval_pct.ok_or(Status::WarmingUp),
        None => Err(r.status),
    }
}

fn value_of<T>(r: &Reading<T>, f: impl Fn(&T) -> f64) -> Result<f64, Status> {
    r.live().map(f).ok_or(r.status)
}

impl AlertEngine {
    pub fn new(rules: Vec<Rule>) -> Self {
        AlertEngine {
            rules: rules.into_iter().filter(|r| r.enabled).collect(),
            tracks: HashMap::new(),
            t_s: 0.0,
        }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Values for one rule: one entry per target (mount) or a single
    /// untargeted entry.
    fn values(metric: AlertMetric, s: &Snapshot) -> Vec<(Option<String>, Result<f64, Status>)> {
        let one = |v: Result<f64, Status>| vec![(None, v)];
        match metric {
            AlertMetric::CpuPct => one(value_of(&s.cpu.usage, |c| c.total_pct)),
            AlertMetric::MemoryUsedPct => one(value_of(&s.memory.usage, |m| m.used_pct)),
            AlertMetric::SwapUsedPct => one(match s.memory.swap.live() {
                Some(w) if w.total_bytes > 0 => {
                    Ok(w.used_bytes as f64 / w.total_bytes as f64 * 100.0)
                }
                Some(_) => Err(Status::Unsupported),
                None => Err(s.memory.swap.status),
            }),
            AlertMetric::CpuPressurePct => one(psi(&s.cpu.pressure)),
            AlertMetric::MemoryPressurePct => one(psi(&s.memory.pressure)),
            AlertMetric::IoPressurePct => one(psi(&s.storage.io_pressure)),
            AlertMetric::CpuTemperatureC => one(match s.sensors.live() {
                Some(v) => v
                    .cpu_temperature()
                    .map(|t| t.celsius)
                    .ok_or(Status::Unsupported),
                None => Err(s.sensors.status),
            }),
            AlertMetric::FilesystemUsedPct => match s.storage.filesystems.live() {
                // Read-only filesystems (images, snaps) are full by design.
                Some(fs) => fs
                    .iter()
                    .filter(|f| !f.read_only)
                    .map(|f| (Some(f.mount_point.clone()), Ok(f.used_pct)))
                    .collect(),
                None => one(Err(s.storage.filesystems.status)),
            },
        }
    }

    pub fn evaluate(&mut self, s: &Snapshot) -> Vec<AlertEvent> {
        let dt = s.interval_ms.map_or(0.0, |ms| ms as f64 / 1000.0);
        if s.gap_before {
            // Do not let a sustain timer span a suspend or stall.
            for t in self.tracks.values_mut() {
                if matches!(t.state, AlertState::Pending { .. }) {
                    t.state = AlertState::Ok;
                }
            }
        } else {
            self.t_s += dt;
        }
        let now = self.t_s;
        let mut events = Vec::new();
        for (ri, rule) in self.rules.iter().enumerate() {
            let values = Self::values(rule.metric, s);
            for (target, v) in values {
                let track = self.tracks.entry((ri, target.clone())).or_insert(Track {
                    state: AlertState::Ok,
                    last_resolved_s: None,
                    last_value: None,
                });
                let mut emit = |kind| {
                    events.push(AlertEvent {
                        rule: rule.id.clone(),
                        metric: rule.metric,
                        target: target.clone(),
                        timestamp_ms: s.timestamp_ms,
                        kind,
                    })
                };
                match (track.state, v) {
                    (AlertState::Ok, Ok(v)) => {
                        if rule.breached(v) {
                            if rule.for_s <= 0.0 {
                                Self::fire(rule, track, now, now, v, s.timestamp_ms, &mut emit);
                            } else {
                                track.state = AlertState::Pending { since_s: now };
                            }
                        }
                    }
                    (AlertState::Ok, Err(_)) => {}
                    (AlertState::Pending { since_s }, Ok(v)) => {
                        if !rule.breached(v) {
                            track.state = AlertState::Ok;
                        } else if now - since_s >= rule.for_s {
                            Self::fire(rule, track, since_s, now, v, s.timestamp_ms, &mut emit);
                        }
                    }
                    // Unknown data cannot confirm a sustained breach.
                    (AlertState::Pending { .. }, Err(_)) => track.state = AlertState::Ok,
                    (
                        AlertState::Firing {
                            since_s,
                            since_ms,
                            notified,
                            data_missing,
                        },
                        Ok(v),
                    ) => {
                        if data_missing && notified {
                            emit(AlertEventKind::DataRestored);
                        }
                        if rule.cleared(v) {
                            track.state = AlertState::Ok;
                            track.last_resolved_s = Some(now);
                            if notified {
                                emit(AlertEventKind::Resolved {
                                    value: v,
                                    clear: rule.clear,
                                    firing_s: round_ms(now - since_s),
                                });
                            }
                        } else {
                            track.state = AlertState::Firing {
                                since_s,
                                since_ms,
                                notified,
                                data_missing: false,
                            };
                        }
                    }
                    (
                        AlertState::Firing {
                            since_s,
                            since_ms,
                            notified,
                            data_missing,
                        },
                        Err(status),
                    ) => {
                        if !data_missing && notified {
                            emit(AlertEventKind::DataMissing { status });
                        }
                        track.state = AlertState::Firing {
                            since_s,
                            since_ms,
                            notified,
                            data_missing: true,
                        };
                    }
                }
                track.last_value = v.ok();
            }
        }
        events
    }

    fn fire(
        rule: &Rule,
        track: &mut Track,
        since_s: f64,
        now: f64,
        v: f64,
        ts: i64,
        emit: &mut impl FnMut(AlertEventKind),
    ) {
        let in_cooldown = track
            .last_resolved_s
            .is_some_and(|r| now - r < rule.cooldown_s);
        track.state = AlertState::Firing {
            since_s: now,
            since_ms: ts,
            notified: !in_cooldown,
            data_missing: false,
        };
        if !in_cooldown {
            emit(AlertEventKind::Fired {
                value: v,
                threshold: rule.threshold,
                sustained_s: round_ms(now - since_s),
            });
        }
    }

    /// Alerts currently pending or firing.
    pub fn active(&self) -> Vec<ActiveAlert> {
        let mut out: Vec<ActiveAlert> = self
            .tracks
            .iter()
            .filter(|(_, t)| !matches!(t.state, AlertState::Ok))
            .map(|((ri, target), t)| ActiveAlert {
                rule: self.rules[*ri].id.clone(),
                metric: self.rules[*ri].metric,
                target: target.clone(),
                state: t.state,
                value: t.last_value,
            })
            .collect();
        out.sort_by(|a, b| (&a.rule, &a.target).cmp(&(&b.rule, &b.target)));
        out
    }

    pub fn firing_count(&self) -> usize {
        self.tracks
            .values()
            .filter(|t| matches!(t.state, AlertState::Firing { .. }))
            .count()
    }
}

#[cfg(test)]
mod tests;
