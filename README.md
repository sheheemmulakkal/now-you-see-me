# Now You See Me (`nysm`)

A lightweight, precise resource monitor for engineers. It answers: what is
being used now, what changed in the last few minutes, which process is
responsible, and whether work is actually *waiting* on a resource.

Works offline, as a normal user, over SSH, with no daemon, no D-Bus, no
systemd, no account and no telemetry.

> Status: **Milestone 1 implemented and verified on Ubuntu 24.04 x86_64;
> Milestone 2 done except other-OS adapters; Milestone 3 in progress**
> (collector service done). Other
> platforms compile but report every metric as `unsupported` (see
> [platform support](docs/platform-support.md)). Desktop app, GNOME panel
> are planned, not built.

## Install

See [docs/install.md](docs/install.md): `scripts/package.sh` builds a tarball with
per-user `install.sh` / `uninstall.sh` (no root, autostart opt-in).

## Build and run

Requires Rust ≥ 1.88 (developed with 1.92). No system libraries are needed.

```sh
cargo build --release
./target/release/nysm                 # same as `nysm summary`
./target/release/nysm tui
```

Headless/CLI-only build without the TUI dependency:

```sh
cargo build --release -p nysm-cli --no-default-features
```

Tray (top bar, any StatusNotifier desktop): `cargo build --release -p nysm-tray`
→ `./target/release/nysm-tray`. See [docs/tray.md](docs/tray.md).

Desktop (experimental, GTK ≥ 4.12): `cargo build --release -p nysm-desktop`
→ `./target/release/nysm-desktop`. See [docs/desktop.md](docs/desktop.md).

## Commands (implemented)

```sh
nysm summary                     # one-shot overview; samples 1 s so rates are real
nysm summary --json              # versioned snapshot (schema_version 1)
nysm watch --interval 1s         # one line per sample
nysm watch --format jsonl --count 10 [--processes]
nysm processes --sort cpu|mem|io|pid|name --limit 20 [--filter ssh] [--per-core] [--json]
nysm inspect --pid 1234 [--show-args] [--json]
nysm containers | nysm services | nysm groups [--kind app] [--json]   # per-cgroup CPU/memory/IO vs limits
nysm ports [--port 3000] [--proto tcp|udp] [--all] [--json]
nysm record -o build.nysm -- cargo build    # exact CPU time + sampled tree
nysm record -o idle.nysm --duration 60s     # whole-system window
nysm compare before.nysm [after.nysm] [--json]
nysm alerts [--list] [--format jsonl]       # sustained-threshold alert events
nysm incidents [--delete-all]   # opt-in captures around alerts (see docs/incidents.md)
nysm config path|show|init|check
nysm service run|status|stop|unit   # optional shared collector; the TUI attaches automatically
nysm capabilities [--json]       # what this machine can report, and why not
nysm doctor                      # environment, permissions and adapter diagnostics
nysm tui [--ascii] [--interval 1s] [--max-fps 10]
nysm tui --remote user@host      # monitor another machine over SSH (no listener)
```

Global options: `--color auto|always|never` (honours `NO_COLOR`),
`--rate-unit bytes|bits` for network rates.

Exit codes: `0` ok · `1` runtime failure · `2` invalid usage ·
`3` output produced but a core metric (CPU/memory) failed · `4` target not
found (PID, or `ports --port` with no match). `record -- CMD` exits with
the command's own status.

Logs and errors go to stderr; stdout carries only results. When stdout is
not a terminal no ANSI escapes are written. Process names, paths and other
untrusted text are escaped before printing.

### TUI keys

`1`–`6`/Tab views · `Space` pause display (collection continues) ·
`←/→` timeline cursor shared by all charts, `End`/`Esc` back to live ·
`↑/↓` select · `Enter` process details · `*` pin (keeps CPU/RSS history) ·
`/` filter · `c m d n P` sort ·
`t` tree · `?` help · `q` quit. Works at 80×24, degrades to a summary below
50×12, `--ascii` avoids Unicode, `NO_COLOR` gives monochrome.

## What the numbers mean (short version)

- **CPU %** is a share of *all* logical cores (0–100). iowait and steal are
  shown separately and are not counted as busy.
- **Memory used** = total − `MemAvailable`. Reclaimable cache is available.
- **Pressure (PSI)** is the share of time tasks *stalled* waiting for a
  resource. It is not utilisation.
- **Network total** sums only up physical interfaces (no bridges, veth,
  tunnels, loopback) to avoid double counting. It includes LAN traffic.
- **Disk total** sums whole disks only. Latency is per device.
- Values that cannot be measured show as `— (reason)`, never `0`.

Full definitions: [docs/metrics.md](docs/metrics.md).

## Measured overhead

On an Intel Core i5-7500 (4 cores) Ubuntu 24.04 desktop with ~510 processes, release build,
60 s, measured externally from `/proc`: `watch` 0.12 % of one core and
3.3 MiB peak RSS; with processes 0.57 %, 4.2 MiB; idle TUI 0.53 %,
4.5 MiB. Method and caveats: [docs/performance.md](docs/performance.md).

## Documentation

[product](docs/product.md) · [architecture](docs/architecture.md) ·
[metrics](docs/metrics.md) · [platform support](docs/platform-support.md) ·
[security](docs/security.md) · [performance](docs/performance.md) ·
[testing](docs/testing.md) · [recordings](docs/recordings.md) ·
[alerts](docs/alerts.md) · [incidents](docs/incidents.md) · [remote over SSH](docs/remote.md) · [configuration](docs/configuration.md) ·
[containers & services](docs/groups.md) · [tray](docs/tray.md) · [desktop](docs/desktop.md) · [GNOME extension (optional)](docs/gnome-extension.md) · [roadmap](docs/roadmap.md) ·
[progress log](docs/progress.md) · [ADRs](docs/adr/)

## Licence

Not yet chosen; this is an owner decision recorded in
[ADR 0008](docs/adr/0008-distribution.md). Do not redistribute until it is set.
