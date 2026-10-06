# ADR 0009: macOS/Windows GUI strategy

Status: proposed (Milestone 5). No code until macOS/Windows collectors exist
and run on real machines.

## Context
The Ubuntu desktop app is GTK4 (ADR 0004). GTK runs on macOS/Windows but
looks foreign there, packaging is heavy (bundling GTK and its runtime), and
tray/menu-bar integration differs per OS. The tray (`nysm-tray`, SNI) is
Linux/BSD only. All frontends already consume the same `Source`/snapshot
model, so the GUI layer is replaceable.

## Options
| option | pros | cons |
| --- | --- | --- |
| A. GTK4 everywhere | one codebase | non-native look; large bundles; weak menu-bar/tray story |
| B. Native per OS (SwiftUI/AppKit, WinUI) | best integration | three UI codebases, needs Swift/C# skills, FFI to Rust core |
| C. Rust cross-platform toolkit (Slint, egui/eframe, iced) | single Rust codebase, small binaries, GPU-accelerated charts | not fully native widgets; accessibility maturity varies (egui/iced weaker; Slint has accessibility work) |
| D. Webview shell (Tauri) | web charting libraries, native tray APIs | JS/TS stack, webview differences, larger attack surface |
| E. No GUI on macOS/Windows; TUI + menu-bar/tray only | smallest scope | less discoverable for non-terminal users |

## Decision (proposed)
1. First ship **E**: CLI/TUI plus a small native tray/menu-bar item per OS
   (via a cross-platform tray crate) once collectors exist.
2. If a full GUI is demanded on all three OSes, evaluate **C with Slint**
   first (Rust, accessibility support, small runtime), keeping GTK on Linux
   until Slint reaches parity, and only then decide whether to retire GTK.
3. Do not build B unless a platform-specific requirement justifies it.

## Revisit when
Users on macOS/Windows ask for a GUI; Slint/egui accessibility is
re-evaluated with a screen reader on each OS; macOS/Windows collectors pass
on real hardware.
