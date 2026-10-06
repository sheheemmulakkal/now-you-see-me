# Testing

```sh
cargo fmt --all --check
cargo clippy --all-targets --workspace -- -D warnings
cargo test --workspace
python3 scripts/pty_smoke.py target/debug/nysm     # real pseudo-terminal TUI checks
python3 scripts/remote_smoke.py target/debug/nysm  # tui --remote via a stand-in ssh
python3 scripts/measure.py target/release/nysm 60  # overhead measurement
cargo run -p nysm-tui --example screens -- 80 24   # print every TUI view as text
```

## Coverage by area

| area | tests |
| --- | --- |
| procfs parsers | `nysm-collect/src/linux/parse.rs`: missing/unknown/malformed fields, old kernels, extra diskstats fields, hostile `comm` with `)` and spaces, mountinfo octal escapes and optional fields, PSI with meaningless `full` |
| counter math | `nysm-core/src/math.rs`: warm-up, reset, wrap-as-reset, guest double counting, iowait regression, elapsed-time variation, minimum interval, normalisation and clamping, MemAvailable semantics, swap, PSI interval, disk latency/busy |
| engine | `nysm-engine/src/tests.rs` with a scripted test-only platform: first-sample warm-up, CPU reset, CPU hot-plug, permission denied, interface hot-plug and exclusion from totals, PID reuse, process exit, bounded history, gap detection, live-thread coalescing and prompt shutdown |
| queries | sorting with missing values last, filtering, process tree with orphans and cycles |
| sanitisation | escape sequences, C1, bidi overrides, Unicode names preserved |
| TUI | key handling; TestBackend rendering of every view at 80×24 and 200×60 with real data; unsupported platform renders reasons not zeros; tiny and zero-size terminals; ASCII mode has no box-drawing characters |
| TUI in a real pty | `scripts/pty_smoke.py`: alternate screen enter/leave, cursor restore, resize, navigation, pause banner, exit on `q`, exit after hangup, tiny terminal |
| CLI | `nysm-cli/tests/cli.rs`: exit codes, help, no ANSI when redirected, NO_COLOR, `--color always`, versioned JSON, JSONL sequence, sorting, inspect (cmdline opt-in, not found → 4), capabilities, TUI refuses non-terminals |
| sockets | `/proc/net` parsing for IPv4/IPv6/IPv4-mapped, malformed rows, `socket:[N]` links; CLI test binds a listener and checks the owner PID is the test process |
| recordings | `nysm-record`: round trip, 0600 mode, no-overwrite, size limit with dropped-sample accounting, partial final line, corrupt middle line, newer version, foreign/empty files, read limit, stats with missing values, compaction, comparison thresholds and caveats; CLI test records a command (exit status 7 propagated) and a window, then compares |
| alerts | `nysm-core/src/alerts/tests.rs`: sustain, short spikes, hysteresis, cooldown, missing data while pending/firing, gaps, per-mount filesystem rules, validation; TUI badge rendering; CLI fires an always-breached rule |
| configuration | `nysm-config`: defaults round trip, minimal file, rule override by id, unknown keys, bad durations, newer version, missing version, parse errors name the file, duplicate ids, atomic 0600 save, appending rules to a generated file, corrupt-file fallback; CLI init/check/no-overwrite/fallback warning |
| service / IPC | `nysm-ipc`: frame round trip, oversized/garbage/unknown frames, private runtime dir checks; integration over a real socket: two clients share one collector, gap-free history, process tables only for subscribers, details round trip, second instance refused, stale socket replaced, version mismatch, garbage client does not affect others, **stalled client does not slow collection**, fallback/require modes, disconnect detection. Manual: TUI attaches ("via service") and falls back on `service stop` (pty) |
| cgroups | parsers (`cpu.stat`, limits, `cpu.max`, `io.stat` device filtering), classification incl. systemd escaping and snaps; engine deltas, CPU quota, warm-up, reset, missing controller, unsubscribe; CLI JSON; TUI Groups render with real data; `bench_scan` example for scan cost |
| sensors | hwmon parsing (thresholds of 0, unreadable thresholds, impossible values), sensor classes, generic slow worker never blocks callers, temperature alert units |
| incidents | pre/post window, dropped samples, shutdown truncation, retention, per-rule dedupe; end-to-end via isolated service run |
| portability | `cargo check` for x86_64-pc-windows-gnu, x86_64-apple-darwin, aarch64-unknown-linux-gnu (compile only) |
| headless | dependency tree of `nysm-cli` has no GTK/X11/Wayland/D-Bus/systemd crates |

## Cross-checks against OS tools (2026-10-06, reference machine)

Performed manually; not automated because two tools never sample the same
window.

| check | nysm | reference | result |
| --- | --- | --- | --- |
| memory total | 16541954048 B | `free -b` 16541954048 B | identical |
| memory available | 4214620160 B | `free -b` 4249866240 B | 0.8 % apart (sampled at different instants) |
| busy-loop process CPU, 2 s window, overlapping | 23.8 % of machine = 95.2 % of one core | `top -d 2` 96.5 % | within 1.3 points; machine load average was 14 |
| disk write throughput (`dd` 200 MiB, fsync) | 326 MiB over 5 s | — | inconclusive: an unrelated `npm ci` was writing at the same time |

## Not covered yet
- Real ARM64, macOS, Windows runs; containers, VMs, WSL.
- Long-duration soak tests; slow/hung NFS mounts (FsWorker logic is
  exercised only by unit design, not with a real hung mount).
- A clean disk and network throughput cross-check on an idle machine.
