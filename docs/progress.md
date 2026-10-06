# Progress log

Read this first when resuming. Newest entry on top.

## 2026-10-06 — on-demand diagnostics and storage growth

- `nysm net check HOST[:PORT]` (DNS + TCP connect latency, no ICMP) and
  `nysm disk usage PATH` (bounded, cancellable scan), docs/diagnostics.md.
- Filesystem growth trend + time-to-full projection (Theil–Sen over the
  15 s capacity refreshes; shown after 2 min). Live check: a 1 MiB/s writer
  read as ~3.4 GiB/h even though another process freed 2 GB mid-run, which
  had pushed a least-squares fit to −73 GB/h. That is why the estimator is
  robust.
- SSH remote testing on real hardware paused (no second machine).

## 2026-10-06 — paused by the user (resume here)

Done since last entry: optional systemd provider (`services` STATE column,
`services --failed`); release tarball + per-user install/uninstall (tested,
uninstaller only stops a collector running its own binary); container
verification on Debian 12 and Alpine with a static musl build; container
network fallback (count eth0/veth when no physical NIC is visible); own
cgroup limits shown inside containers (`snapshot.limits`).

Finding: glibc builds from Ubuntu 24.04 need glibc >= 2.39; static musl
builds (`--target x86_64-unknown-linux-musl`) of `nysm` and `nysm-tray`
work everywhere tested.

Resumed: package now ships static musl `nysm`/`nysm-tray` (install/
uninstall re-tested, runs on Debian 12); container docs written.
Added split .deb packages (scripts/package-deb.sh), tested in a Debian 12 container.
Added incident snapshots (docs/incidents.md), "busiest processes" in recording summaries, GPU via DRM sysfs,
ADR 0009 (GUI strategy) and 0010 (remote threat model), and `nysm tui --remote` over SSH (docs/remote.md), opt-in container names (`--names`),
soak data (docs/performance.md), tray re-attach to a returning service.
Committed as 23ed65c on feat/initial-implementation (user approved commits).
Then: inspector sockets + belongs-to, timeline alert-event correlation.
Next: aarch64 (needs emulation or hardware), macOS/Windows adapters, GPU (optional), container name resolver (opt-in).

Open owner decisions: licence; whether to commit (all work is staged,
nothing committed); optional `libadwaita-1-dev`.

## 2026-10-06 — Milestone 4: containers/services and sensors

- cgroup v2 groups (containers, services, apps): `nysm groups|containers|
  services`, TUI view 7, desktop page, service subscription (ref-counted,
  only while viewed). Scan 13 ms for ~120 groups; tree re-walked every 5th
  scan. docs/groups.md.
- Sensors: hwmon temperatures/fans and batteries via a generic slow-provider
  worker (`SlowWorker`, also used for filesystems). Shown in summary,
  capabilities, TUI CPU view, desktop CPU card, tray menu;
  `cpu_temperature_c` alert metric.
- Tests: 125 workspace + desktop + gjs; clippy clean; pty 8/8.

Next: packaging (.deb / tarball, install/uninstall docs, opt-in autostart
for service + tray), systemd service state provider (optional), GPU
(optional), then broader platform work.

## 2026-10-06 — Milestone 3: tray, optional extension, desktop redesign

- `nysm-tray` (StatusNotifierItem via `ksni`) is the default top-bar
  component; confirmed visible and updating on the user's Ubuntu GNOME 46.
  0.03 % CPU / 5.7 MiB (release, attached). `ksni` cannot send Ubuntu's
  text label, so the icon is a live CPU/memory meter.
- GNOME extension is optional: it could not load on the reference machine
  (`disable-user-extensions = true`), was uninstalled and the user's
  extension list restored.
- Desktop redesigned to the user's reference collage (B layout, D light
  theme): cards, area charts with round time ticks, binary byte scales,
  icon sidebar, time-range and theme selectors, `display.theme` config.
- Tests: 114 workspace + 3 desktop + gjs format tests; clippy clean.

Left running on the user's machine at their request during testing:
`nysm service run` and `nysm-tray` (stop with `nysm service stop`, tray
menu → Quit).

Next: Milestone 4 (cgroup/container view, systemd services, hwmon
sensors), packaging (.deb, autostart opt-in), licence decision.

## 2026-10-06 — Milestone 3: collector service

`nysm-ipc` crate and `nysm service run|status|stop|unit`. TUI attaches
automatically (`--attach auto|never|require`) and falls back locally if the
service disappears. Measured: 2 attached TUIs + service 0.83 % CPU vs
1.16 % for 2 standalone TUIs. Tests: 107 passing.

Next: GTK4 desktop app (`nysm-desktop`, libadwaita optional because
`libadwaita-1-dev` is not installed here), then the GNOME Shell 46
extension (needs permission before installing into the user's session).

## 2026-10-06 — Milestone 2: alerts and configuration

Added `nysm_core::alerts` (pure state machine), alert evaluation in the
live collector (TUI badge + overview line), `nysm alerts`, and the
`nysm-config` crate with `nysm config`. CLI flags > config > defaults.
Tests: 94 passing, clippy clean, pty smoke 8/8.

Remaining M2: per-process history for pinned processes, macOS/Windows
adapters (no machines here), ARM64 runtime verification, CI first run.
Next planned: TUI idle wake-ups, then Milestone 3 (service + IPC).

## 2026-10-06 — Milestone 2: ports and recordings

Added `nysm ports` (socket → owner, permission-aware) and the
`nysm-record` crate with `nysm record` / `nysm compare` (docs/recordings.md).
Tests: 79 passing, clippy clean.

Verified by hand: recording two Python workloads (400 vs 200 hashes over
1 MiB, the first holding a 30 MiB buffer): exact CPU time 0.97 s → 0.49 s,
largest-process peak RSS 44 MiB → 14 MiB, files mode 0600, Ctrl-C on a
window recording finalises with `truncated: true`.

Next: sustained alerts (pure state machine in core), configuration file,
TUI idle wake-ups, then Milestone 3 service/IPC.

## 2026-10-06 — Milestone 1 complete (Ubuntu 24.04 x86_64)

State: all crates build; `cargo test --workspace` passes (core 26,
collect 13, engine 9, tui 4+5, cli 9 = 66 tests); clippy clean;
`scripts/pty_smoke.py` passes; overhead measured (docs/performance.md).

Product name "Now You See Me", command `nysm` (user request). Branding
constants live in `crates/nysm-core/src/brand.rs`; crate names use the
`nysm-` prefix.

Decisions: ADRs 0001–0008. Notably Linux reads /proc directly (ADR 0003);
service/IPC is designed but not built (ADR 0005); licence is an open owner
decision (ADR 0008).

Known gaps / next steps (Milestone 2, in order):
1. `nysm ports` (listening/connected sockets → owning process where
   permitted): parse `/proc/net/{tcp,tcp6,udp,udp6}`, map socket inodes via
   `/proc/<pid>/fd` (permission-limited), add `Platform::sockets()`.
2. Recording format + `record`/`compare` (versioned JSONL with header,
   0600 permissions, size limits, truncated-file handling).
3. Sustained alert engine in core (pure state machine, tested), surfaced in
   TUI/CLI.
4. Config file (XDG path, versioned, atomic write, corrupt-file recovery).
5. CI first run on GitHub (needs a remote; none configured).
6. TUI: block on snapshot notification instead of 100 ms idle poll.

Verification gaps: no ARM64/macOS/Windows/container/VM runs yet.
