//! Platform adapters. Each adapter reports raw counters and gauges; it does
//! not compute rates (that is the engine's job, using `nysm_core::math`).
//!
//! Parsers are pure functions compiled on every OS so their fixture tests
//! run everywhere; the code that touches the real filesystem is gated by
//! `cfg(target_os = ...)`.

pub mod error;
pub mod linux;
pub mod runtime;
pub mod systemd;
mod unsupported;

pub use error::{CResult, CollectError};

use nysm_core::raw::{
    RawCpu, RawDisk, RawInterface, RawLoad, RawMemory, RawPressure, RawProcess, RawSwapActivity,
};
use nysm_core::snapshot::{FilesystemSnapshot, HostInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureResource {
    Cpu,
    Memory,
    Io,
}

/// cgroup v2 groups of interest (containers, services, apps).
pub struct CgroupScan {
    pub groups: Vec<nysm_core::raw::RawCgroup>,
    /// Groups found beyond the reporting bound.
    pub truncated: u32,
}

/// Raw process list plus the number of entries that could not be read.
pub struct ProcessScan {
    pub processes: Vec<RawProcess>,
    pub unreadable: u32,
}

/// On-demand details for one process (developer inspector).
#[derive(Debug, Clone)]
pub struct ProcessDetails {
    pub exe: CResult<String>,
    pub cwd: CResult<String>,
    /// Full argument vector. Sensitive: only read when explicitly requested.
    pub cmdline: Option<CResult<Vec<String>>>,
    pub cgroup: CResult<String>,
    pub open_fds: CResult<u32>,
}

/// A platform's measurement surface. One instance is owned by one engine;
/// methods take `&mut self` so adapters can reuse buffers and caches.
pub trait Platform: Send {
    fn host_info(&mut self) -> HostInfo;
    fn clock_ticks_per_s(&self) -> u64;
    fn cpu_times(&mut self) -> CResult<RawCpu>;
    /// Current frequency for each requested CPU id, in MHz.
    fn cpu_frequencies(&mut self, ids: &[u32]) -> Vec<CResult<f64>>;
    fn load(&mut self) -> CResult<RawLoad>;
    fn memory(&mut self) -> CResult<RawMemory>;
    fn swap_activity(&mut self) -> CResult<RawSwapActivity>;
    fn page_size(&self) -> u64;
    fn pressure(&mut self, resource: PressureResource) -> CResult<RawPressure>;
    fn interfaces(&mut self) -> CResult<Vec<RawInterface>>;
    fn disks(&mut self) -> CResult<Vec<RawDisk>>;
    fn processes(&mut self) -> CResult<ProcessScan>;
    /// Limits of our own cgroup (containers); unsupported in the root cgroup.
    fn own_limits(&mut self) -> CResult<nysm_core::snapshot::OwnLimits>;
    /// Per-container / service / app accounting (cgroup v2). On demand.
    fn cgroups(&mut self) -> CResult<CgroupScan>;
    fn process_details(
        &mut self,
        pid: u32,
        start_ticks: Option<u64>,
        include_cmdline: bool,
    ) -> CResult<ProcessDetails>;
    fn user_name(&mut self, uid: u32) -> Option<String>;
    /// TCP/UDP sockets visible in this network namespace, optionally with
    /// owning processes (only those this user may inspect). On demand only.
    fn sockets(&mut self, resolve_owners: bool) -> CResult<Vec<nysm_core::sockets::SocketEntry>>;
    /// A provider that may be moved to a worker thread, because capacity
    /// queries can block for a long time on network filesystems.
    fn filesystem_provider(&self) -> Box<dyn FilesystemProvider>;
    /// Temperatures, fans and batteries. Runs on a worker thread because
    /// some sensors (e.g. NVMe) query the device.
    fn sensors_provider(
        &self,
    ) -> Box<dyn FnMut() -> CResult<nysm_core::snapshot::SensorsSnapshot> + Send>;
    /// Human description of where a metric id comes from.
    fn source(&self, metric_id: &str) -> &'static str;
}

pub trait FilesystemProvider: Send {
    fn filesystems(&mut self) -> CResult<Vec<FilesystemSnapshot>>;
}

/// An adapter that reports every metric as unsupported. Used on operating
/// systems without an implementation, and to test degraded states.
pub fn unsupported() -> Box<dyn Platform> {
    Box::new(unsupported::UnsupportedPlatform)
}

/// The adapter for the OS this binary was built for.
pub fn native() -> Box<dyn Platform> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxPlatform::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsupported()
    }
}
