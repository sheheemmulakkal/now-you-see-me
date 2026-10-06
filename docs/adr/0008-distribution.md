# ADR 0008: Distribution strategy and licence

Status: proposed

## Decision
- Ship separable artifacts: `nysm` (CLI+TUI, no GUI deps), later
  `nysm-service`, `nysm-desktop`, and the GNOME extension.
- First: Linux tarballs (x86_64, aarch64) with SHA-256 checksums, built in
  release mode with the committed `Cargo.lock`; then `.deb`.
- Evaluate Snap/Flatpak only after measuring their effect on `/proc`
  visibility (sandboxing hides host processes) — not by default.
- No autostart by default; install/uninstall instructions are tested.

## Owner decision required
**Project licence is not chosen.** `license` is deliberately unset in
`Cargo.toml`. Choose one before any public distribution.

Dependency licences as of 2026-10-06 (`cargo tree -e normal --prefix none
--format "{p} {l}"`): predominantly MIT and/or Apache-2.0, plus two Zlib
crates, three crates that add Unicode-3.0 data terms, and some offering
BSL-1.0 or Unlicense as alternatives. All are permissive, but release
artifacts must ship the corresponding notices (e.g. generated with
`cargo-about` or `cargo-deny`).
