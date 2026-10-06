//! Raw counter readings produced by platform collectors.
//!
//! These are absolute counters/gauges as the OS reports them. The engine
//! turns pairs of them into rates using `crate::math`.

use serde::{Deserialize, Serialize};

/// Cumulative CPU time, in OS clock ticks, as reported by Linux `/proc/stat`.
///
/// Per the kernel, `user` already includes `guest` and `nice` already
/// includes `guest_nice`. They must not be added again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
    pub guest: u64,
    pub guest_nice: u64,
}

impl CpuTimes {
    /// All accounted time, excluding guest fields (already inside user/nice).
    pub fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawCpu {
    pub total: CpuTimes,
    /// Online logical CPUs keyed by kernel CPU id (ids may be sparse).
    pub per_cpu: Vec<(u32, CpuTimes)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RawLoad {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
    pub runnable: u32,
    pub tasks: u32,
}

/// Linux `/proc/meminfo` values in bytes. Absent fields stay `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawMemory {
    pub total: Option<u64>,
    pub free: Option<u64>,
    pub available: Option<u64>,
    pub buffers: Option<u64>,
    pub cached: Option<u64>,
    pub s_reclaimable: Option<u64>,
    pub shmem: Option<u64>,
    pub dirty: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_free: Option<u64>,
    pub swap_cached: Option<u64>,
}

/// Cumulative swap traffic counters (pages).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RawSwapActivity {
    pub pages_in: u64,
    pub pages_out: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RawPsiLine {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    /// Cumulative stall time in microseconds.
    pub total_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPressure {
    pub some: RawPsiLine,
    /// Absent for CPU on older kernels / at system level.
    pub full: Option<RawPsiLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceKind {
    Loopback,
    Physical,
    Wireless,
    Bridge,
    Tunnel,
    VirtualEthernet,
    Virtual,
    Unknown,
}

impl InterfaceKind {
    /// Whether this kind contributes to the default network total. Only
    /// hardware-backed links count; bridges, veths and tunnels would
    /// double-count traffic that also crosses a physical link.
    pub fn counted_by_default(self) -> bool {
        matches!(self, InterfaceKind::Physical | InterfaceKind::Wireless)
    }

    pub fn label(self) -> &'static str {
        match self {
            InterfaceKind::Loopback => "loopback",
            InterfaceKind::Physical => "ethernet",
            InterfaceKind::Wireless => "wireless",
            InterfaceKind::Bridge => "bridge",
            InterfaceKind::Tunnel => "tunnel/vpn",
            InterfaceKind::VirtualEthernet => "veth",
            InterfaceKind::Virtual => "virtual",
            InterfaceKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawInterface {
    pub name: String,
    pub kind: InterfaceKind,
    /// Operational state if known (`operstate == up`).
    pub up: Option<bool>,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub rx_dropped: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
    pub tx_dropped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskKind {
    /// A whole hardware-backed block device (disk, NVMe namespace, mmc).
    Disk,
    Partition,
    /// device-mapper, md RAID, etc: stacked on other devices.
    Stacked,
    Loop,
    /// zram / ram disks (memory-backed).
    Memory,
    Other,
}

impl DiskKind {
    pub fn counted_by_default(self) -> bool {
        matches!(self, DiskKind::Disk)
    }
}

/// Linux `/proc/diskstats` fields for one device. Times are milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDisk {
    pub name: String,
    pub major: u32,
    pub minor: u32,
    pub kind: DiskKind,
    pub reads: u64,
    pub sectors_read: u64,
    pub read_ms: u64,
    pub writes: u64,
    pub sectors_written: u64,
    pub write_ms: u64,
    pub in_flight: u64,
    pub io_ms: u64,
    pub weighted_io_ms: u64,
}

/// Linux reports diskstats sectors in fixed 512-byte units regardless of the
/// device's logical sector size.
pub const DISKSTATS_SECTOR_BYTES: u64 = 512;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessId {
    pub pid: u32,
    /// Process start time in clock ticks since boot (Linux `starttime`).
    /// Together with the PID and boot id this survives PID reuse.
    pub start_ticks: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawProcess {
    pub id: ProcessId,
    pub ppid: u32,
    pub name: String,
    pub state: char,
    pub uid: Option<u32>,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub threads: u32,
    pub rss_bytes: u64,
    pub virtual_bytes: u64,
    /// `Err` holds why I/O counters were not readable (usually permissions).
    pub io: Result<RawProcessIo, crate::status::Status>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawProcessIo {
    /// Bytes the process caused to be fetched from storage.
    pub read_bytes: u64,
    /// Bytes the process caused to be sent to storage (incl. later writeback).
    pub write_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CgroupKind {
    /// A container (docker, podman, containerd, kubernetes pod).
    Container,
    /// A systemd system service.
    Service,
    /// A desktop/user application or user service.
    UserApp,
    /// A VM or nspawn machine (machine.slice).
    Machine,
    Other,
}

impl CgroupKind {
    pub fn label(self) -> &'static str {
        match self {
            CgroupKind::Container => "container",
            CgroupKind::Service => "service",
            CgroupKind::UserApp => "app",
            CgroupKind::Machine => "machine",
            CgroupKind::Other => "other",
        }
    }
}

/// Counters and limits of one cgroup v2 group. Absent files stay `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCgroup {
    /// Path relative to the cgroup root, e.g. `system.slice/cron.service`.
    pub path: String,
    pub kind: CgroupKind,
    /// Human name: unit name, or runtime + short container id.
    pub name: String,
    /// Name of the first process in the group (best effort).
    pub main_process: Option<String>,
    pub cpu_usage_usec: Option<u64>,
    /// `cpu.max`: (quota µs, period µs); `None` quota = unlimited.
    pub cpu_max: Option<(Option<u64>, u64)>,
    pub memory_current: Option<u64>,
    pub memory_max: Option<u64>,
    pub memory_high: Option<u64>,
    pub pids_current: Option<u64>,
    pub pids_max: Option<u64>,
    /// Bytes read/written on hardware disks only (not partitions or
    /// stacked devices, which would double count).
    pub io_read_bytes: Option<u64>,
    pub io_write_bytes: Option<u64>,
    pub memory_pressure_some_us: Option<u64>,
    pub cpu_pressure_some_us: Option<u64>,
}
