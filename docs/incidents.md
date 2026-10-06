# Incident snapshots (opt-in)

When an alert rule fires, the collector service writes a recording with
the samples **before** the event (kept in a bounded in-memory buffer) and
keeps recording for a window **after** it. Off by default.

```toml
# ~/.config/nysm/config.toml
[incidents]
enabled = true
pre = "5m"            # kept before the event (0s .. 1h)
post = "2m"           # recorded after it
max_files = 20        # oldest deleted first
max_total_mib = 200   # total size cap
# dir = "/path"       # default: $XDG_STATE_HOME/nysm/incidents (~/.local/state/…)
```
```sh
nysm service run            # incidents need continuous collection
nysm incidents              # list: time, rule, message, samples, size
nysm compare <file>         # summary incl. busiest processes in the window
nysm incidents --delete-all
```

## Behaviour
- Only `nysm service run` captures incidents (the TUI and desktop do not);
  without the service, nothing is collected between sessions. Because
  incidents depend on alerts, the service never idle-exits while rules
  exist.
- While incidents are enabled the service keeps the process table on, so
  each sample stores the top 5 processes by CPU (name, PID, CPU, RSS).
  This costs ~0.5 % of one core on the reference machine.
- One capture per rule/target at a time, at most 4 concurrent; further
  alerts during a capture are written as events into it. Captures still
  open at shutdown are closed and marked truncated. Samples the capture
  thread missed are counted as dropped.
- Files use the normal recording format (`target.kind = "incident"`), mode
  0600 in a 0700 directory, each capped at 32 MiB. Retention runs after
  every completed capture: newest kept, older files deleted beyond
  `max_files` or `max_total_mib`.
- Contents: system snapshots (interfaces/devices compacted as in
  recordings), top-5 process names/PIDs, alert messages. No command lines,
  environment or connection data. Process names can still be sensitive;
  delete files you do not need.

## Memory cost
The pre-event buffer holds `pre / interval` compact samples (~9 KB each on
the reference machine): 5 minutes at 1 s ≈ 2.7 MB.

## Verified (2026-10-06)
Unit tests: pre/post window contents, dropped-sample accounting, shutdown
truncation, retention by count, dedupe per rule. End to end with an
isolated config/runtime/state dir: an always-breached rule fired after 3 s,
`incident-…-demo-always.nysm` (0600) was saved with 6 samples (3 s before,
2 s after), listed by `nysm incidents` and summarised by `nysm compare`.
