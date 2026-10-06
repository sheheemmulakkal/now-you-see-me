//! Branding lives here so a rename touches one file (plus crate names).

pub const PRODUCT_NAME: &str = "Now You See Me";
pub const COMMAND_NAME: &str = "nysm";
/// Identifies the producer in machine-readable output.
pub const PRODUCER: &str = concat!("nysm/", env!("CARGO_PKG_VERSION"));
/// Recording file magic/format identifier.
pub const RECORDING_FORMAT: &str = "nysm-recording";
/// Desktop application id (reverse-DNS). Placeholder: change before
/// publishing, together with the .desktop file and icon names.
pub const APP_ID: &str = "dev.nysm.NowYouSeeMe";
