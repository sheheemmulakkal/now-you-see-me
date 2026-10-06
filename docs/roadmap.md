# Roadmap and milestone status

Legend: ✅ done and verified · 🟡 partial · ⬜ not started

## Milestone 1: correct headless vertical slice ✅ (Ubuntu x86_64)
- ✅ Core models: Reading/Status, units, snapshot schema v1, history
- ✅ Linux adapters: CPU (total, per-core, frequency, load, PSI), memory
  (MemAvailable, swap, swap activity, PSI), network (per-interface,
  classified, physical total), storage (diskstats per device, filesystems
  on a worker, I/O PSI), processes (identity, CPU, RSS, disk I/O)
- ✅ CLI: summary, watch (text/jsonl, count), processes, inspect,
  capabilities, doctor; exit codes; NO_COLOR; sanitisation
- ✅ TUI: overview with trends, processes (search, sort, tree, details),
  CPU/Memory/Network/Disk views, pause, shared timeline cursor, help,
  80×24, tiny fallback, ASCII/monochrome, render cap
- ✅ Headless build with no GUI dependencies; overhead measured
- ⬜ CI workflow run on GitHub (file added; not yet run on a remote)

## Milestone 2: engineer workflows and portable core
- ✅ Ports lookup (`nysm ports`: /proc/net/{tcp,udp}{,6} + fd inode map)
- ✅ Recording/compare (`record --duration`, `record -- cmd`, `compare`),
  versioned file format, dropped-sample accounting, exact rusage for commands
- ✅ Sustained alerts (duration, hysteresis, cooldown, missing-data and gap handling); CLI `alerts`, TUI badge
- ✅ Per-process history for pinned processes (≤ 8 pins, bounded to the history window)
- ✅ Configuration file (versioned, validated, atomic writes, invalid-file fallback)
- ⬜ macOS and Windows CLI/TUI adapters (needs those machines/runners)
- ⬜ ARM64 run on real hardware or CI runner

## Inspector and timeline (brief §8.1, §8.3)
- ✅ `inspect`: listening ports and connections of the process, service/container/app it belongs to (also in TUI and desktop details)
- ✅ TUI timeline cursor shows alert events at that moment and gaps; live header shows the last alert event

## Milestone 3: Ubuntu desktop and panel
- ✅ Per-user collector service + Unix socket protocol (ADR 0005); TUI attaches with fallback
- 🟡 GTK4 desktop (`nysm-desktop`): all pages, attach/fallback, dark mode, hidden-window idle (experimental; libadwaita and ColumnView process list pending; see docs/desktop.md)
- ✅ Tray indicator `nysm-tray` (StatusNotifierItem): default top-bar component, confirmed on Ubuntu GNOME 46 (docs/tray.md)
- 🟡 Optional GNOME Shell 46 extension: built, client/reconnect tested with gjs; not loaded in a live shell (blocked by the user's global extension setting)
- ✅ Desktop redesign to the reference collage: cards, area charts with time axis, icon sidebar, range selector, warm light + dark themes (B, D). C (engineer workspace) is covered by the TUI and Processes page; a combined dense view is not built.
- ✅ Simultaneous-client benchmark (docs/performance.md)

## Milestone 4: optional modules and distribution
- ✅ Containers, services and apps from cgroup v2 (CLI `groups`/`containers`/`services`, TUI view 7, service subscription; docs/groups.md)
- ✅ Optional systemd provider: unit state in `services`, `services --failed` (bounded subprocess)
- ✅ Container name resolver, opt-in `--names` (Docker/Podman API, read-only)
- ✅ Sensors: hwmon temperatures (CPU/storage/memory/chipset/GPU classes), fans, batteries on a worker thread; summary, capabilities, TUI, desktop, tray; `cpu_temperature_c` alert metric
- ✅ GPU via DRM sysfs: Intel frequency (verified), AMD busy %/VRAM/clock (fixture-tested only)
- ⬜ NVIDIA (NVML), Intel utilisation via perf PMU (needs privileges; opt-in)
- ✅ Incident snapshots (opt-in, bounded pre/post windows, retention; docs/incidents.md)
- ✅ Release tarball (Linux x86_64) with checksums, third-party licence list, per-user install/uninstall scripts (tested)
- ✅ Release binaries are static musl (portable across distributions)
- ✅ Container verification (Debian 12, Alpine), own cgroup limits, container network fallback
- ✅ Split .deb packages (nysm / nysm-tray / nysm-desktop), install/purge tested in Debian 12 container
- ⬜ aarch64 builds, signed releases, apt repository; licence decision (owner)

- ✅ Network diagnostics (`net check`) and storage scan (`disk usage`), on demand (docs/diagnostics.md)

## Milestone 5: broader graphical access
- 🟡 GUI strategy for macOS/Windows proposed (ADR 0009: tray/menu-bar first, then evaluate Slint)
- 🟡 Remote viewer threat model proposed (ADR 0010); ✅ step 1 implemented: `nysm tui --remote user@host` over SSH, no listener (docs/remote.md)
