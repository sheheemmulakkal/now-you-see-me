//! Linux adapter: reads `/proc` and `/sys` directly so every formula and
//! field is under our control (see docs/adr/0003-metric-sources.md).

pub mod parse;

#[cfg(target_os = "linux")]
mod platform;
#[cfg(target_os = "linux")]
pub use platform::LinuxPlatform;
