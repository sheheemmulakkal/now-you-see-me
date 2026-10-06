pub mod alerts;
pub mod capabilities;
pub mod compare;
pub mod config;
#[cfg(unix)]
pub mod diskusage;
pub mod doctor;
pub mod groups;
pub mod incidents;
pub mod inspect;
pub mod netcheck;
pub mod ports;
pub mod processes;
pub mod record;
pub mod service;

pub mod summary;
pub mod watch;

use nysm_core::Status;
use nysm_core::snapshot::Snapshot;

use crate::exit;

/// PARTIAL if a core metric failed for a reason other than warm-up.
pub fn exit_code_for(s: &Snapshot) -> u8 {
    let failed = |st: Status| matches!(st, Status::CollectionError | Status::PermissionDenied);
    if failed(s.cpu.usage.status) || failed(s.memory.usage.status) {
        exit::PARTIAL
    } else {
        exit::OK
    }
}
