//! Platform-neutral domain model for Now You See Me.
//!
//! This crate defines what a metric *means*: identities, units, statuses,
//! snapshot shapes and the pure arithmetic that turns raw counters into
//! rates. It performs no I/O and must never depend on a UI toolkit.

pub mod alerts;
pub mod brand;
pub mod capabilities;
pub mod history;
pub mod math;
pub mod query;
pub mod raw;
pub mod sanitize;
pub mod snapshot;
pub mod sockets;
pub mod status;
pub mod units;

pub use status::{Reading, Status};

/// Version of the JSON snapshot schema emitted by CLI/IPC. Bump on any
/// incompatible change to field names, units or meaning.
pub const SCHEMA_VERSION: u32 = 1;
