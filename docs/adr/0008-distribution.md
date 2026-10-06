# ADR 0008: Distribution strategy and licence

Status: accepted (licence chosen 2026-10-06)

## Decision
- Ship separable artifacts: `nysm` (CLI+TUI, no GUI deps), later
  `nysm-service`, `nysm-desktop`, and the GNOME extension.
- First: Linux tarballs (x86_64, aarch64) with SHA-256 checksums, built in
  release mode with the committed `Cargo.lock`; then `.deb`.
- Evaluate Snap/Flatpak only after measuring their effect on `/proc`
  visibility (sandboxing hides host processes) — not by default.
- No autostart by default; install/uninstall instructions are tested.

## Licence
**MIT OR Apache-2.0** (owner decision, 2026-10-06), the Rust ecosystem
convention and the same terms as most dependencies. `LICENSE-MIT` and
`LICENSE-APACHE` are at the repository root and are shipped in the tarball
and in each `.deb` (`/usr/share/doc/<package>/`).

Dependency licences as of 2026-10-06 (`cargo tree -e normal --prefix none
--format "{p} {l}"`): predominantly MIT and/or Apache-2.0, plus two Zlib
crates, three crates that add Unicode-3.0 data terms, and some offering
BSL-1.0 or Unlicense as alternatives. All are permissive, but release
artifacts must ship the corresponding notices (e.g. generated with
`cargo-about` or `cargo-deny`).
