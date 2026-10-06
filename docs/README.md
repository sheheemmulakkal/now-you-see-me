# Documentation

Start with the **[user guide](guide.md)** — what Now You See Me can do and how
to use it, task by task. The pages below go deeper.

## Using it

| Page | What it covers |
| --- | --- |
| [User guide](guide.md) | Features, how-to by task, keys, troubleshooting |
| [Install and uninstall](install.md) | Tarball, `.deb` packages, per-user install, autostart, service unit |
| [Configuration](configuration.md) | Config file, `nysm config`, every setting |
| [What the numbers mean](metrics.md) | Exact definition and source of every metric and status |
| [Tray indicator](tray.md) | `nysm-tray`: icon, menu, desktop support |
| [Desktop app](desktop.md) | `nysm-desktop`: pages, keys, settings, measured cost |
| [GNOME extension](gnome-extension.md) | Optional panel indicators |
| [Containers, services and apps](groups.md) | Per-cgroup usage, limits, container names, systemd state |
| [Alerts](alerts.md) | Built-in and custom rules, hysteresis, cooldown |
| [Incident snapshots](incidents.md) | Opt-in recordings around alerts |
| [Recordings and comparison](recordings.md) | `record`, `compare`, file format |
| [On-demand diagnostics](diagnostics.md) | `net check`, `disk usage` |
| [Remote monitoring](remote.md) | `nysm tui --remote` over SSH |
| [Security and privacy](security.md) | What is read, stored and never sent |
| [Platform support](platform-support.md) | What is built, tested and planned per OS |

## Building and contributing

| Page | What it covers |
| --- | --- |
| [Product](product.md) | Users, goals, non-goals |
| [Architecture](architecture.md) | Crates, data flow, threading, IPC |
| [Testing](testing.md) | Test suites, smoke tests, how to run them |
| [Performance](performance.md) | Budgets, measurements, soak results, method |
| [Roadmap](roadmap.md) | Milestone status: done, partial, planned |
| [Progress log](progress.md) | Development log, newest first |

## Design decisions (ADRs)

1. [Architecture boundaries and language](adr/0001-architecture-boundaries.md)
2. [Metric semantics](adr/0002-metric-semantics.md)
3. [Direct OS collection vs a metrics library](adr/0003-metric-sources.md)
4. [Terminal and GUI toolkits](adr/0004-ui-toolkits.md)
5. [Embedded collector, optional per-user service, local IPC](adr/0005-service-and-ipc.md)
6. [Privacy and least-privilege defaults](adr/0006-privacy-defaults.md)
7. [Portability tiers](adr/0007-portability-tiers.md)
8. [Distribution strategy and licence](adr/0008-distribution.md)
9. [macOS/Windows GUI strategy](adr/0009-cross-platform-gui.md)
10. [Remote viewer: constraints and threat model](adr/0010-remote-viewer-threat-model.md)
