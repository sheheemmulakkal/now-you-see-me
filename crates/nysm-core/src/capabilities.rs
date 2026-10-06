//! Per-metric capability report derived from a real snapshot, so it always
//! reflects what this machine actually delivered rather than a static table.

use serde::Serialize;

use crate::snapshot::Snapshot;
use crate::status::{Reading, Status};

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    /// Stable metric identifier, e.g. `cpu.usage`.
    pub id: &'static str,
    pub unit: &'static str,
    pub status: Status,
    /// Where the value comes from on this platform.
    pub source: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityReport {
    pub schema_version: u32,
    pub producer: String,
    pub os: String,
    pub arch: String,
    pub scope: crate::snapshot::MeasurementScope,
    pub capabilities: Vec<Capability>,
}

fn cap<T>(
    id: &'static str,
    unit: &'static str,
    source: &'static str,
    r: &Reading<T>,
) -> Capability {
    Capability {
        id,
        unit,
        status: r.status,
        source,
        reason: r.reason.clone(),
    }
}

/// `sources` maps metric id to a platform source description.
pub fn report(s: &Snapshot, sources: &dyn Fn(&str) -> &'static str) -> CapabilityReport {
    let mut caps = vec![
        cap(
            "cpu.usage",
            "percent of all logical CPUs",
            sources("cpu.usage"),
            &s.cpu.usage,
        ),
        cap(
            "cpu.logical_cores",
            "count",
            sources("cpu.logical_cores"),
            &s.cpu.logical_cores,
        ),
        cap(
            "cpu.load",
            "run-queue average",
            sources("cpu.load"),
            &s.cpu.load,
        ),
        cap(
            "cpu.pressure",
            "percent of time stalled",
            sources("cpu.pressure"),
            &s.cpu.pressure,
        ),
        cap(
            "memory.usage",
            "bytes",
            sources("memory.usage"),
            &s.memory.usage,
        ),
        cap(
            "memory.swap",
            "bytes",
            sources("memory.swap"),
            &s.memory.swap,
        ),
        cap(
            "memory.swap_activity",
            "bytes/s",
            sources("memory.swap_activity"),
            &s.memory.swap_activity,
        ),
        cap(
            "memory.pressure",
            "percent of time stalled",
            sources("memory.pressure"),
            &s.memory.pressure,
        ),
        cap(
            "network.interfaces",
            "bytes/s",
            sources("network.interfaces"),
            &s.network.interfaces,
        ),
        cap(
            "storage.devices",
            "bytes/s, ops/s, ms",
            sources("storage.devices"),
            &s.storage.devices,
        ),
        cap(
            "storage.filesystems",
            "bytes",
            sources("storage.filesystems"),
            &s.storage.filesystems,
        ),
        cap(
            "io.pressure",
            "percent of time stalled",
            sources("io.pressure"),
            &s.storage.io_pressure,
        ),
    ];

    // Per-core frequency: summarise across cores.
    let freq_status = s
        .cpu
        .per_core
        .iter()
        .map(|c| c.frequency_mhz.status)
        .find(|st| *st == Status::Available)
        .or_else(|| s.cpu.per_core.first().map(|c| c.frequency_mhz.status))
        .unwrap_or(Status::Unsupported);
    caps.push(Capability {
        id: "cpu.frequency",
        unit: "MHz",
        status: freq_status,
        source: sources("cpu.frequency"),
        reason: s
            .cpu
            .per_core
            .first()
            .and_then(|c| c.frequency_mhz.reason.clone()),
    });

    match &s.processes {
        Some(p) => {
            caps.push(Capability {
                id: "process.list",
                unit: "count",
                status: Status::Available,
                source: sources("process.list"),
                reason: (p.unreadable > 0)
                    .then(|| format!("{} processes could not be read", p.unreadable)),
            });
            let denied = p
                .entries
                .iter()
                .filter(|e| e.disk_io.status == Status::PermissionDenied)
                .count();
            let readable = p.entries.len() - denied;
            caps.push(Capability {
                id: "process.disk_io",
                unit: "bytes/s",
                status: if readable > 0 { Status::Available } else { Status::PermissionDenied },
                source: sources("process.disk_io"),
                reason: (denied > 0).then(|| {
                    format!("readable for {readable} of {} processes (others need the same user or CAP_SYS_PTRACE)", p.entries.len())
                }),
            });
        }
        None => caps.push(Capability {
            id: "process.list",
            unit: "count",
            status: Status::Unsupported,
            source: sources("process.list"),
            reason: Some("not collected in this sample".into()),
        }),
    }

    match &s.cgroups {
        Some(t) => caps.push(Capability {
            id: "groups.cgroups",
            unit: "percent, bytes, bytes/s",
            status: Status::Available,
            source: sources("groups.cgroups"),
            reason: Some(format!(
                "{} containers / services / apps (container names need the runtime; shown as runtime + id)",
                t.groups.len()
            )),
        }),
        None => caps.push(Capability {
            id: "groups.cgroups",
            unit: "percent, bytes, bytes/s",
            status: Status::Unsupported,
            source: sources("groups.cgroups"),
            reason: Some("cgroup v2 accounting not available here".into()),
        }),
    }
    caps.push(Capability {
        id: "process.network_bandwidth",
        unit: "bytes/s",
        status: Status::Unsupported,
        source: "none",
        reason: Some(
            "per-process bandwidth needs an OS-specific accounting adapter; not implemented".into(),
        ),
    });
    let sens = |id: &'static str, unit: &'static str, n: Option<usize>| match (
        &s.sensors.value,
        s.sensors.status,
    ) {
        (Some(_), Status::Available | Status::Stale) if n.unwrap_or(0) > 0 => Capability {
            id,
            unit,
            status: s.sensors.status,
            source: sources(id),
            reason: Some(format!("{} sensors", n.unwrap_or(0))),
        },
        (Some(_), _) => Capability {
            id,
            unit,
            status: Status::Unsupported,
            source: sources(id),
            reason: Some("none exposed by this hardware/driver".into()),
        },
        (None, st) => Capability {
            id,
            unit,
            status: st,
            source: sources(id),
            reason: s.sensors.reason.clone(),
        },
    };
    let v = s.sensors.value.as_ref();
    caps.push(sens(
        "sensors.temperature",
        "°C",
        v.map(|v| v.temperatures.len()),
    ));
    caps.push(sens("sensors.fans", "RPM", v.map(|v| v.fans.len())));
    caps.push(sens(
        "sensors.battery",
        "percent, W",
        v.map(|v| v.batteries.len()),
    ));
    let gpus = v.map(|v| v.gpus.as_slice()).unwrap_or(&[]);
    let gpu_cap = |id: &'static str,
                   unit: &'static str,
                   has: &dyn Fn(&crate::snapshot::Gpu) -> bool,
                   why: &str| {
        let n = gpus.iter().filter(|g| has(g)).count();
        if n > 0 {
            Capability {
                id,
                unit,
                status: Status::Available,
                source: sources(id),
                reason: Some(format!("{n} GPU(s)")),
            }
        } else if gpus.is_empty() {
            Capability {
                id,
                unit,
                status: Status::Unsupported,
                source: sources(id),
                reason: Some("no DRM GPU visible".into()),
            }
        } else {
            let drivers: Vec<&str> = gpus.iter().map(|g| g.driver.as_str()).collect();
            Capability {
                id,
                unit,
                status: Status::Unsupported,
                source: sources(id),
                reason: Some(format!("{why} (driver: {})", drivers.join(", "))),
            }
        }
    };
    caps.push(gpu_cap("gpu.usage", "percent", &|g| g.busy_pct.is_some(), "driver does not expose utilisation without privileges (i915 needs perf PMU; NVIDIA needs NVML, not implemented)"));
    caps.push(gpu_cap(
        "gpu.frequency",
        "MHz",
        &|g| g.frequency_mhz.is_some(),
        "driver does not expose frequency",
    ));
    caps.push(gpu_cap(
        "gpu.vram",
        "bytes",
        &|g| g.vram_total_bytes.is_some(),
        "driver does not expose VRAM (integrated GPUs share system RAM)",
    ));

    CapabilityReport {
        schema_version: crate::SCHEMA_VERSION,
        producer: crate::brand::PRODUCER.into(),
        os: s.host.os.clone(),
        arch: s.host.arch.clone(),
        scope: s.host.scope,
        capabilities: caps,
    }
}
