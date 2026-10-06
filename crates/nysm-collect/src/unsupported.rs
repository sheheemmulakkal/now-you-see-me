//! Placeholder adapter for operating systems without an implementation.
//! Every metric reports `unsupported`; nothing is substituted or faked.

use nysm_core::raw::{
    RawCpu, RawDisk, RawInterface, RawLoad, RawMemory, RawPressure, RawSwapActivity,
};
use nysm_core::snapshot::{FilesystemSnapshot, HostInfo, MeasurementScope};

use crate::error::{CResult, CollectError};
use crate::{FilesystemProvider, Platform, PressureResource, ProcessDetails, ProcessScan};

pub struct UnsupportedPlatform;

fn no<T>() -> CResult<T> {
    Err(CollectError::Unsupported(format!(
        "no adapter for {} yet",
        std::env::consts::OS
    )))
}

impl Platform for UnsupportedPlatform {
    fn host_info(&mut self) -> HostInfo {
        HostInfo {
            hostname: None,
            os: std::env::consts::OS.into(),
            os_version: None,
            kernel: None,
            arch: std::env::consts::ARCH.into(),
            boot_id: None,
            uptime_s: None,
            scope: MeasurementScope::Unknown,
        }
    }
    fn clock_ticks_per_s(&self) -> u64 {
        100
    }
    fn page_size(&self) -> u64 {
        4096
    }
    fn cpu_times(&mut self) -> CResult<RawCpu> {
        no()
    }
    fn cpu_frequencies(&mut self, ids: &[u32]) -> Vec<CResult<f64>> {
        ids.iter().map(|_| no()).collect()
    }
    fn load(&mut self) -> CResult<RawLoad> {
        no()
    }
    fn memory(&mut self) -> CResult<RawMemory> {
        no()
    }
    fn swap_activity(&mut self) -> CResult<RawSwapActivity> {
        no()
    }
    fn pressure(&mut self, _: PressureResource) -> CResult<RawPressure> {
        no()
    }
    fn interfaces(&mut self) -> CResult<Vec<RawInterface>> {
        no()
    }
    fn disks(&mut self) -> CResult<Vec<RawDisk>> {
        no()
    }
    fn processes(&mut self) -> CResult<ProcessScan> {
        no()
    }
    fn own_limits(&mut self) -> CResult<nysm_core::snapshot::OwnLimits> {
        no()
    }
    fn cgroups(&mut self) -> CResult<crate::CgroupScan> {
        no()
    }
    fn process_details(&mut self, _: u32, _: Option<u64>, _: bool) -> CResult<ProcessDetails> {
        no()
    }
    fn user_name(&mut self, _: u32) -> Option<String> {
        None
    }
    fn sockets(&mut self, _: bool) -> CResult<Vec<nysm_core::sockets::SocketEntry>> {
        no()
    }
    fn sensors_provider(
        &self,
    ) -> Box<dyn FnMut() -> CResult<nysm_core::snapshot::SensorsSnapshot> + Send> {
        Box::new(no)
    }
    fn filesystem_provider(&self) -> Box<dyn FilesystemProvider> {
        Box::new(NoFilesystems)
    }
    fn source(&self, _: &str) -> &'static str {
        "none"
    }
}

struct NoFilesystems;

impl FilesystemProvider for NoFilesystems {
    fn filesystems(&mut self) -> CResult<Vec<FilesystemSnapshot>> {
        no()
    }
}
