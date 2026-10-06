# Performance

## Budgets (targets, not guarantees)
- Collector + idle panel: < 1 % of one logical core average at default
  cadence; < 30 MiB RSS with default history.
- No steady disk writes during passive monitoring.
- Responsive input with many processes and a slow provider.

## Method
`scripts/measure.py [binary] [seconds]` starts scenarios concurrently,
skips 3 s of startup, then reads `/proc/<pid>/stat` (utime+stime) and
`/proc/<pid>/status` (VmHWM, VmRSS, voluntary context switches, threads)
from outside the process. CPU % is of one logical core. Resolution is one
clock tick (10 ms) over the window.

## Results

### 2026-10-06 — Milestone 1
Machine: Intel Core i5-7500 (4 cores, no SMT), 15.4 GiB RAM, NVMe + SATA,
Ubuntu 24.04.3, kernel 7.0.0-34-generic, ~510 processes, Docker running
(~20 virtual interfaces). Release build (`lto = "thin"`), default settings
(1 s interval, processes every 2 s, filesystems every 15 s, frequency on).
Duration 60 s, all three scenarios running at the same time.

| scenario | CPU % of one core | peak RSS | RSS | vol. ctx switches/s | threads |
| --- | --- | --- | --- | --- | --- |
| `watch --format jsonl` (no processes) | 0.12 | 3.3 MiB | 3.2 MiB | 1.0 | 2 |
| `watch --format jsonl --processes` | 0.57 | 4.2 MiB | 4.0 MiB | 1.0 | 2 |
| `tui` 80×24, idle (processes on) | 0.53 | 4.5 MiB | 4.5 MiB | 10.0 | 3 |

Binary size (release, x86_64): 2.3 MB.

Re-measured after the TUI idle-wait change (30 s, same machine, release):
`watch` 0.13 % / 4.4 MiB; with processes 0.57 % / 4.8 MiB; TUI 0.50 % /
5.6 MiB and **1.1** voluntary context switches/s (was 10/s with the fixed
100 ms poll). RSS grew ~1 MiB since M1 (configuration and alert code).

### Service vs standalone (30 s, release, same machine)
`scripts/measure_service.py`: two TUIs collecting locally vs one
`nysm service run` with two attached TUIs.

| setup | process | CPU % of one core | RSS |
| --- | --- | --- | --- |
| A | TUI #1 local | 0.60 | 5.5 MiB |
| A | TUI #2 local | 0.57 | 5.5 MiB |
| A | **total** | **1.16** | |
| B | service | 0.63 | 6.1 MiB |
| B | TUI #1 attached | 0.10 | 6.9 MiB |
| B | TUI #2 attached | 0.10 | 7.0 MiB |
| B | **total** | **0.83** | |

Only the service samples; each attached client costs ~0.1 % (JSON decode).
The first version re-sent the 2-second process table on every 1-second
update, which made both setups cost the same (1.00 %); sending tables only
when they change fixed that. Attached clients use ~1.5 MiB more RSS for
frame buffers.

### High process count, 2026-10-06
3,000 idle `sleep` processes added (3,483 total), release build:

| measure | ~485 processes | ~3,483 processes |
| --- | --- | --- |
| process scan (`bench_scan` example) | 9.2 ms | 70.8 ms (~20 µs/process, linear) |
| TUI on the Processes view, CPU % of one core | ~0.5 | 2.9 |
| TUI RSS | 5.6 MiB | 8 MiB |
| key press → redraw latency (20 presses) | — | median < 1 ms, max 3 ms |

Collection runs on its own thread, so input stays responsive regardless of
scan time; the scan runs every 2 s and only while processes are
subscribed. A blocked slow provider (e.g. a hung network mount) is covered
by the `SlowWorker` unit test (callers never wait on it); a real hung
mount was not reproduced because that needs root.

### Long-run (soak) observation, 2026-10-06
Processes left running on the reference machine during normal use (heavy
background load; clients attaching and detaching during testing):

| process | uptime | average CPU % of one core | RSS now | peak RSS | threads | open fds |
| --- | --- | --- | --- | --- | --- | --- |
| `nysm-tray` (release) | 155 min | 0.14 | 4.4 MiB | 6.4 MiB | 6 | 7 |
| `nysm service run` (release) | 79 min | 0.23 | 4.8 MiB | 5.3 MiB | 5 | 5 |

No memory, thread or descriptor growth. The tray's average includes a
period of local collection after its service was stopped (this run predates
automatic re-attach, added afterwards: it now re-attaches within 30 s). Multi-day runs
have not been done.

Observations:
- The process scan (~510 × `/proc/<pid>/stat` + `/io`) dominates cost.
- The TUI now sleeps until the next snapshot is due when idle; input
  still wakes it immediately.
- The service/panel budget cannot be measured yet (not built).
- Long-run growth (hours) has not been measured yet.
