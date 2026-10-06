//! Optional per-user collector service and its local protocol.
//!
//! One `nysm service run` process samples; any number of local clients
//! (TUI, desktop app, GNOME panel) attach over a user-only Unix socket and
//! share its measurements instead of sampling separately. Clients fall back
//! to embedded collection when no service is running (`source::Source`).
//!
//! Unix only for now; Windows named pipes are planned (ADR 0005).

pub mod protocol;
pub mod source;

#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod paths;
#[cfg(unix)]
pub mod server;
