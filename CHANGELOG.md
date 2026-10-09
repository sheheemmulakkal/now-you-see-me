# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/) (0.x: anything may change).

## [Unreleased]

## [0.1.0] - 2026-10-09

First public release.

### Added
- **Live view** of CPU (per core, frequency, temperature), memory, swap,
  network per interface, disks (throughput, latency, busy %) and
  filesystems, with pressure (PSI) to show when work is waiting.
- **Who is responsible**: processes (filter, sort, tree, pin and watch,
  live details), memory per app with PSS/USS, containers, systemd services
  and desktop apps against their limits, optional container names, and
  which process owns a port.
- **Over time**: recent history in memory, a timeline cursor in the TUI,
  recordings of the system or of one command, and comparison of runs.
- **Alerts** with hysteresis and cooldown, and opt-in incident snapshots.
- **Storage insight**: growth trend and time-to-full per filesystem, and a
  bounded `disk usage` scan.
- **Diagnostics**: `net check` (DNS and TCP connect latency),
  `capabilities`, `doctor`.
- **Interfaces**: CLI with `--json` everywhere, terminal UI (also over SSH
  with `--remote`), GTK 4 desktop app, top-bar indicator with live values,
  optional shared collector service, optional GNOME extension.
- **Packaging**: static tarball with per-user install and uninstall, and
  `.deb` packages for amd64 and arm64 (Ubuntu 22.04+; the desktop app needs
  24.04+), with man pages, bash/zsh/fish completions
  (`nysm completions <shell>`) and AppStream metadata.
