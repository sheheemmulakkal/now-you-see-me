//! Symbolic icons for the per-metric tray items. Hosts recolour
//! `*-symbolic` icons to the panel's text colour, so they suit light and
//! dark panels. Written at startup into a private directory that is
//! published as the item's `IconThemePath`.

use std::path::PathBuf;

pub const CPU: &str = "nysm-cpu-symbolic";
pub const MEMORY: &str = "nysm-memory-symbolic";
pub const NETWORK: &str = "nysm-network-symbolic";
pub const DISK: &str = "nysm-disk-symbolic";
pub const STORAGE: &str = "nysm-storage-symbolic";

const SVG: [(&str, &str); 5] = [
    (
        STORAGE,
        // Capacity: an outlined bar, partly filled.
        r##"<path fill-rule="evenodd" d="M2.5 4h11A1.5 1.5 0 0 1 15 5.5v5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 1 10.5v-5A1.5 1.5 0 0 1 2.5 4zm0 1.25a.25.25 0 0 0-.25.25v5c0 .14.11.25.25.25h11a.25.25 0 0 0 .25-.25v-5a.25.25 0 0 0-.25-.25z"/><path d="M3.5 6.5h6v3h-6z"/>"##,
    ),
    (
        CPU,
        // Chip: ring with pins on four sides and a core.
        r##"<path d="M5 1h1v2H5zm2.5 0h1v2h-1zM10 1h1v2h-1zM5 13h1v2H5zm2.5 0h1v2h-1zm2.5 0h1v2h-1zM1 5h2v1H1zm0 2.5h2v1H1zM1 10h2v1H1zm12-5h2v1h-2zm0 2.5h2v1h-2zm0 2.5h2v1h-2z"/><path fill-rule="evenodd" d="M4.5 3h7A1.5 1.5 0 0 1 13 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 3 11.5v-7A1.5 1.5 0 0 1 4.5 3zm0 1.5v7h7v-7z"/><path d="M6 6h4v4H6z"/>"##,
    ),
    (
        MEMORY,
        // Memory module: board with three chips and contact pins.
        r##"<path fill-rule="evenodd" d="M2 3.5h12a1 1 0 0 1 1 1V11H1V4.5a1 1 0 0 1 1-1zm1.5 2v3.5h2V5.5zm3.5 0v3.5h2V5.5zm3.5 0v3.5h2V5.5z"/><path d="M2 12h1.5v2H2zm2.5 0H6v2H4.5zM7 12h2v2H7zm3 0h1.5v2H10zm2.5 0H14v2h-1.5z"/>"##,
    ),
    (
        NETWORK,
        // Download and upload arrows.
        r##"<path d="M3.5 1.5h2v8.5H8L4.5 14.5 1 10h2.5zm7 13h2V6H15l-3.5-4.5L8 6h2.5z"/>"##,
    ),
    (
        DISK,
        // Drive: body, activity light and a slot.
        r##"<path fill-rule="evenodd" d="M3 3h10a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zm8.5 5.5a1.25 1.25 0 1 0 0 2.5 1.25 1.25 0 0 0 0-2.5zM3.5 9.25v1h5v-1z"/>"##,
    ),
];

/// Write the icons (if missing or different) and return the directory.
pub fn install() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let dir = base.join("nysm").join("icons");
    std::fs::create_dir_all(&dir).ok()?;
    for (name, body) in SVG {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g fill="#2e3436">{body}</g></svg>"##
        );
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg.as_str()) {
            std::fs::write(&path, svg).ok()?;
        }
    }
    Some(dir)
}
