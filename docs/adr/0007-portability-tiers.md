# ADR 0007: Portability tiers

Status: accepted (2026-10-06)

| Tier | Meaning | Members today |
| --- | --- | --- |
| 1 | Implemented, tested on real hardware/OS | Ubuntu 24.04 x86_64 (CLI, TUI) |
| 2 | Implemented, built and unit-tested in CI, runtime not yet verified on real hardware | Linux aarch64 (CI planned) |
| 3 | Compiles; reports everything as `unsupported` | macOS, Windows |
| 4 | Not started | BSD, mobile viewers |

Promotion requires running the actual binary on that OS/architecture and
recording results in docs/platform-support.md. A successful compile never
promotes a platform.
