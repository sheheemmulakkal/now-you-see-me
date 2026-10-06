# ADR 0004: Terminal and GUI toolkits

Status: accepted for TUI; GUI decision recorded, not yet implemented (2026-10-06)

## Decision
- **TUI**: Ratatui 0.30 with its re-exported Crossterm backend (default
  features off; only `crossterm` and `layout-cache`). Crossterm works on
  Linux, macOS and Windows terminals.
- **Ubuntu desktop (planned)**: GTK4 + libadwaita via gtk-rs, consuming the
  collector service. GNOME panel: a small GJS extension that only renders and
  subscribes.
- **macOS/Windows GUI**: not planned until CLI/TUI adapters exist. If one
  GUI across all three OSes becomes a hard requirement, evaluate a
  cross-platform toolkit (e.g. Slint, egui, Tauri) in a new ADR before
  replacing the GTK frontend. We will not build several GUI stacks early.

## Consequences
GTK is native on GNOME but not on macOS/Windows; that is accepted and
stated in docs/platform-support.md.
