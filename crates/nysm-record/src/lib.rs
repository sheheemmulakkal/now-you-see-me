//! Recording files: JSON Lines with a header record, sample records,
//! events and an end record.
//!
//! ```text
//! {"type":"header","format":"nysm-recording","format_version":1,...}
//! {"type":"sample",...}
//! {"type":"event",...}
//! {"type":"end",...}
//! ```
//!
//! A file without an `end` record was interrupted and is reported as
//! truncated; a final partial line is ignored with a warning. Readers
//! refuse newer major format versions instead of guessing.

pub mod compare;
pub mod format;
pub mod incident;
pub mod reader;
pub mod writer;

pub use format::*;
