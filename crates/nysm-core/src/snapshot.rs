//! Immutable, serializable snapshot shared by every frontend.
//!
//! Units are part of field names (`_pct`, `_bytes`, `_bytes_per_s`, `_ms`)
//! so JSON consumers never have to guess.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::raw::{DiskKind, InterfaceKind, ProcessId};
use crate::status::Reading;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub producer: String,
    /// Monotonically increasing per engine instance.
    pub seq: u64,
    /// Wall-clock sampling time (Unix epoch, ms). For display only.
    pub timestamp_ms: i64,
    /// Monotonic time elapsed since the previous sample used for rates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    /// True when this sample follows a gap (suspend, long stall) longer than
    /// expected; rates still cover the true elapsed time but history should
    /// not interpolate across it.
    #[serde(default)]
    pub gap_before: bool,
    pub host: HostInfo,
    pub cpu: CpuSnapshot,
    pub memory: MemorySnapshot,
    pub network: NetworkSnapshot,
    pub storage: StorageSnapshot,
    /// Own cgroup limits; `unsupported` outside a container.
    #[serde(default = "limits_default")]
    pub limits: Reading<OwnLimits>,
    /// Temperatures, fans, batteries (refreshed on a slower schedule).
    #[serde(default = "sensors_default")]
    pub sensors: Reading<SensorsSnapshot>,
    /// Present only when process collection is subscribed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processes: Option<Arc<ProcessTable>>,
    /// Present only when cgroup collection is subscribed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroups: Option<Arc<CgroupTable>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementScope {
    /// Measurements describe the machine the kernel runs on.
    Host,
    /// A virtual machine guest: values describe the guest only.
    VirtualMachine,
    /// A container: kernel-wide files (e.g. /proc/stat) may describe the
    /// host kernel while the process list is namespaced.
    Container,
    /// Windows Subsystem for Linux: the Linux VM, not the Windows host.
    Wsl,
    Unknown,
}

impl MeasurementScope {
    pub fn describe(self) -> &'static str {
        match self {
            MeasurementScope::Host => "host",
            MeasurementScope::VirtualMachine => {
                "virtual machine guest (values describe the guest, not the hypervisor host)"
            }
            MeasurementScope::Container => {
                "container (system-wide values may describe the host kernel; processes are namespaced)"
            }
            MeasurementScope::Wsl => "WSL (values describe the WSL VM, not the Windows host)",
            MeasurementScope::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub hostname: Option<String>,
    pub os: String,
    pub os_version: Option<String>,
    pub kernel: Option<String>,
    pub arch: String,
    /// Changes every boot; part of process identity in recordings.
    pub boot_id: Option<String>,
    pub uptime_s: Option<f64>,
    pub scope: MeasurementScope,
}

fn limits_default() -> Reading<OwnLimits> {
    Reading::missing(crate::Status::Unsupported, "not collected")
}

fn sensors_default() -> Reading<SensorsSnapshot> {
    Reading::missing(crate::Status::Unsupported, "not collected")
}

// ---------------------------------------------------------------- CPU

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuSnapshot {
    pub logical_cores: Reading<u32>,
    pub usage: Reading<CpuBreakdown>,
    pub per_core: Vec<CoreSnapshot>,
    pub load: Reading<LoadAverage>,
    pub pressure: Reading<Pressure>,
}

/// Share of all logical CPUs' time over the sample interval, 0–100.
/// `total_pct = user + nice + system + irq + softirq`. `iowait` and
/// `steal` are reported separately and are *not* busy time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CpuBreakdown {
    pub total_pct: f64,
    pub user_pct: f64,
    pub nice_pct: f64,
    pub system_pct: f64,
    pub irq_pct: f64,
    pub iowait_pct: f64,
    pub steal_pct: f64,
    pub idle_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreSnapshot {
    pub id: u32,
    pub usage_pct: Reading<f64>,
    pub frequency_mhz: Reading<f64>,
}

/// Run-queue averages. Not a percentage; compare against `logical_cores`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
    pub runnable: u32,
    pub tasks: u32,
}

/// Linux PSI: share of wall time in which some (or all non-idle) tasks were
/// stalled on the resource. A stall measure, not a utilisation measure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pressure {
    pub some: PressureWindow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<PressureWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PressureWindow {
    /// Kernel exponential moving averages, percent.
    pub avg10_pct: f64,
    pub avg60_pct: f64,
    pub avg300_pct: f64,
    /// Stall percentage over exactly this sample interval, from the
    /// cumulative `total` counter. Absent on the first sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_pct: Option<f64>,
}

// ---------------------------------------------------------------- Memory

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub usage: Reading<MemoryUsage>,
    pub swap: Reading<SwapUsage>,
    pub swap_activity: Reading<SwapActivity>,
    pub pressure: Reading<Pressure>,
}

/// `used_bytes = total_bytes - available_bytes`: memory that cannot be
/// handed to a new workload without swapping. Cache that the kernel can
/// reclaim counts as available, not used.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_pct: f64,
    pub free_bytes: Option<u64>,
    /// Page cache + buffers (part of it is reclaimable, not all).
    pub cache_bytes: Option<u64>,
    pub reclaimable_slab_bytes: Option<u64>,
    /// tmpfs/shared memory; counted in cache but not easily reclaimable.
    pub shared_bytes: Option<u64>,
    pub dirty_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SwapUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SwapActivity {
    pub in_bytes_per_s: f64,
    pub out_bytes_per_s: f64,
}

// ---------------------------------------------------------------- Network

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSnapshot {
    pub interfaces: Reading<Vec<InterfaceSnapshot>>,
    /// Sum over interfaces with `counted_in_total`.
    pub total: Reading<NetworkRates>,
    pub total_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceSnapshot {
    pub name: String,
    pub kind: InterfaceKind,
    pub up: Option<bool>,
    pub counted_in_total: bool,
    pub rx_total_bytes: u64,
    pub tx_total_bytes: u64,
    pub rates: Reading<NetworkRates>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct NetworkRates {
    pub rx_bytes_per_s: f64,
    pub tx_bytes_per_s: f64,
    pub rx_packets_per_s: f64,
    pub tx_packets_per_s: f64,
    /// Errors + drops during the interval (counts, not rates).
    pub rx_errors_dropped: u64,
    pub tx_errors_dropped: u64,
}

// ---------------------------------------------------------------- Storage

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSnapshot {
    pub devices: Reading<Vec<DiskSnapshot>>,
    /// Sum over devices with `counted_in_total` (whole physical disks).
    pub total_io: Reading<DiskIo>,
    pub filesystems: Reading<Vec<FilesystemSnapshot>>,
    /// Age of the filesystem capacity data (refreshed on a slow schedule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystems_age_ms: Option<u64>,
    pub io_pressure: Reading<Pressure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSnapshot {
    pub name: String,
    pub kind: DiskKind,
    pub counted_in_total: bool,
    pub io: Reading<DiskIo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DiskIo {
    pub read_bytes_per_s: f64,
    pub write_bytes_per_s: f64,
    pub read_ops_per_s: f64,
    pub write_ops_per_s: f64,
    /// Mean time per completed read in the interval (queue + service).
    pub read_latency_ms: Option<f64>,
    pub write_latency_ms: Option<f64>,
    /// Share of the interval with at least one I/O in flight. Not a
    /// saturation score for devices that serve requests in parallel. In the
    /// disk total this is the busiest disk's value.
    pub busy_pct: Option<f64>,
    pub in_flight: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemSnapshot {
    pub mount_point: String,
    pub source: String,
    pub fs_type: String,
    pub read_only: bool,
    pub total_bytes: u64,
    /// `total - free` (includes space reserved for root).
    pub used_bytes: u64,
    /// Space an unprivileged user can still allocate.
    pub available_bytes: u64,
    /// `used / (used + available)`, matching `df`.
    pub used_pct: f64,
    /// Additional mount points of the same filesystem (bind mounts etc).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_mounted_at: Vec<String>,
    /// Trend of used space over the observed window (≥ 2 min), bytes/hour;
    /// negative when space is being freed. Absent until enough data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub growth_bytes_per_hour: Option<f64>,
    /// Hours until full if the observed trend continued (projection only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_in_hours: Option<f64>,
}

// ---------------------------------------------------------------- Processes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessTable {
    pub timestamp_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    pub logical_cores: u32,
    /// Processes visible to this user (hidepid or namespaces may hide some).
    pub entries: Vec<ProcessSnapshot>,
    /// Processes whose details could not be read (exited mid-read, denied).
    pub unreadable: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSnapshot {
    #[serde(flatten)]
    pub id: ProcessId,
    pub ppid: u32,
    /// Kernel `comm` (≤15 bytes on Linux). Untrusted text: sanitize before
    /// printing to a terminal.
    pub name: String,
    pub state: char,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub threads: u32,
    /// Share of the whole machine (all logical cores), 0–100.
    pub cpu_pct: Reading<f64>,
    pub rss_bytes: u64,
    pub virtual_bytes: u64,
    /// Cumulative CPU time (user+system), seconds.
    pub cpu_time_s: f64,
    pub disk_io: Reading<ProcessDiskIo>,
}

impl ProcessSnapshot {
    /// CPU on the "one core = 100%" scale (may exceed 100).
    pub fn cpu_one_core_pct(&self, logical_cores: u32) -> Option<f64> {
        self.cpu_pct.live().map(|p| p * logical_cores as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProcessDiskIo {
    pub read_bytes_per_s: f64,
    pub write_bytes_per_s: f64,
}

// ---------------------------------------------------------------- Cgroups

/// Resource use per container / service / app from cgroup v2 accounting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgroupTable {
    pub timestamp_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    pub logical_cores: u32,
    pub groups: Vec<CgroupSnapshot>,
    /// Groups found but not reported because of the size bound.
    pub truncated: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgroupSnapshot {
    pub path: String,
    pub kind: crate::raw::CgroupKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_process: Option<String>,
    /// Share of the whole machine (all logical cores), 0–100.
    pub cpu_pct: Reading<f64>,
    /// CPU limit in cores from `cpu.max` (None = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_limit_cores: Option<f64>,
    /// Charged memory (includes page cache charged to the group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Hard limit (`memory.max`), None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_bytes: Option<u64>,
    /// Throttling threshold (`memory.high`), None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_high_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u64>,
    pub disk_io: Reading<ProcessDiskIo>,
    /// PSI `some` for this group over the sample interval, %.
    pub memory_pressure_pct: Reading<f64>,
    pub cpu_pressure_pct: Reading<f64>,
}

// ---------------------------------------------------------------- Sensors

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SensorsSnapshot {
    pub temperatures: Vec<Temperature>,
    pub fans: Vec<Fan>,
    pub batteries: Vec<Battery>,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
}

/// GPU data exposed by the kernel driver without privileges. Absent fields
/// mean the driver does not expose them (e.g. i915 utilisation needs perf
/// PMU access; NVIDIA needs NVML, not implemented).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gpu {
    pub card: String,
    pub driver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_mhz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frequency_mhz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_used_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorClass {
    Cpu,
    Gpu,
    Storage,
    Memory,
    Chipset,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Temperature {
    /// Driver/chip name, e.g. `coretemp`, `nvme`, `amdgpu`.
    pub chip: String,
    pub label: String,
    pub class: SensorClass,
    pub celsius: f64,
    /// Driver-reported thresholds; absent when the driver gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_celsius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critical_celsius: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fan {
    pub chip: String,
    pub label: String,
    pub rpm: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Battery {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_pct: Option<f64>,
    /// Kernel status string: Charging, Discharging, Full, Not charging, Unknown.
    pub status: String,
    /// Positive while discharging or charging, watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_watts: Option<f64>,
}

impl SensorsSnapshot {
    /// The most representative CPU temperature: a package sensor if any,
    /// otherwise the hottest CPU-class sensor.
    pub fn cpu_temperature(&self) -> Option<&Temperature> {
        let cpu = || {
            self.temperatures
                .iter()
                .filter(|t| t.class == SensorClass::Cpu)
        };
        cpu()
            .find(|t| {
                let l = t.label.to_ascii_lowercase();
                l.contains("package") || l == "tctl" || l == "tdie"
            })
            .or_else(|| cpu().max_by(|a, b| a.celsius.total_cmp(&b.celsius)))
    }

    /// One line per class: "CPU 76 °C · storage 55 °C".
    pub fn summary(&self) -> Vec<(SensorClass, f64)> {
        let mut out: Vec<(SensorClass, f64)> = Vec::new();
        if let Some(t) = self.cpu_temperature() {
            out.push((SensorClass::Cpu, t.celsius));
        }
        for class in [
            SensorClass::Gpu,
            SensorClass::Storage,
            SensorClass::Memory,
            SensorClass::Chipset,
        ] {
            if let Some(m) = self
                .temperatures
                .iter()
                .filter(|t| t.class == class)
                .map(|t| t.celsius)
                .reduce(f64::max)
            {
                out.push((class, m));
            }
        }
        out
    }
}

impl SensorClass {
    pub fn label(self) -> &'static str {
        match self {
            SensorClass::Cpu => "CPU",
            SensorClass::Gpu => "GPU",
            SensorClass::Storage => "storage",
            SensorClass::Memory => "memory",
            SensorClass::Chipset => "chipset",
            SensorClass::Other => "other",
        }
    }
}

/// Limits of the cgroup this process runs in, when that is not the root
/// cgroup (i.e. inside a container or a limited systemd unit).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OwnLimits {
    pub memory_bytes: Option<u64>,
    /// None = unlimited.
    pub memory_max_bytes: Option<u64>,
    pub memory_high_bytes: Option<u64>,
    /// CPU quota in cores; None = unlimited.
    pub cpu_limit_cores: Option<f64>,
    pub pids: Option<u64>,
    pub pids_max: Option<u64>,
}
