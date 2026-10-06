# ADR 0001: Architecture boundaries and language

Status: accepted (2026-10-06)

## Context
The product needs one set of metric semantics shared by a CLI, a TUI, a
future GTK desktop app and a GNOME panel extension, and must build on
headless servers without GUI libraries.

## Options
- **Rust**: no GC pauses, small static binaries, strong typing for units and
  statuses, good TUI (Ratatui) and GTK4 (gtk-rs) bindings.
- **Go**: simple, but GC and larger binaries; weaker GTK4 story.
- **C/C++**: maximal control, but memory-safety risk when parsing untrusted
  `/proc` text (process names are attacker-controlled).
- **Zig**: immature ecosystem for TUI/GTK.

## Decision
Rust workspace with crates split along the brief's responsibilities:

| crate | role | may depend on |
| --- | --- | --- |
| `nysm-core` | domain model, statuses, units, pure math, history, sanitisation, shared queries | serde only |
| `nysm-collect` | platform adapters returning raw counters; pure parsers | core, libc |
| `nysm-engine` | scheduling, deltas, resets, gaps, slow-provider isolation, live thread | core, collect |
| `nysm-tui` | terminal UI | core, collect, engine, ratatui |
| `nysm-cli` | `nysm` binary (TUI optional via `tui` feature) | all above |

Future: `nysm-service` (collector + IPC), `nysm-desktop` (GTK), and a GJS
extension consuming the service. None may be depended on by core/engine.

Rules: frontends never compute rates (all formulas live in `core::math`),
sorting/trees live in `core::query`, and snapshots are immutable `Arc`s.

## Consequences
- Parsers are pure and fixture-tested on every OS.
- Adding a platform means implementing the `Platform` trait only.
- The CLI binary links Ratatui by default; `--no-default-features` builds a
  CLI without it.

## Revisit when
A single cross-platform GUI becomes a hard requirement (see ADR 0004).
