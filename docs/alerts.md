# Alerts

```sh
nysm alerts --list                   # active rules
nysm alerts                          # print events as they happen (text)
nysm alerts --format jsonl | your-notifier
```

The TUI evaluates the same rules on its collector thread and shows a red
"N alerts" badge plus an `ALERT` line on the overview while any rule is
firing. Evaluation happens on every sample even while the display is
paused.

## Semantics (`nysm_core::alerts`)

| concept | rule field | behaviour |
| --- | --- | --- |
| threshold | `threshold`, `direction` (`above`/`below`) | breach when the value is beyond it |
| sustain | `for_s` | fire only after the breach holds for this many seconds of observed samples |
| hysteresis | `clear` | resolve only when the value returns beyond `clear` |
| cooldown | `cooldown_s` | a re-fire within this time after resolving is tracked but not notified |
| missing data | — | never a breach and never a recovery: a pending breach resets; a firing alert stays firing and emits `data_missing` once, then `data_restored` |
| gaps | — | time is summed from sample intervals; a gap (suspend/stall) resets pending timers |
| per target | `filesystem_used_pct` | evaluated per mount; read-only filesystems are skipped |

Metrics: `cpu_pct`, `memory_used_pct`, `swap_used_pct`,
`cpu_pressure_pct`, `memory_pressure_pct`, `io_pressure_pct` (PSI `some`
over each sample interval), `filesystem_used_pct`, `cpu_temperature_c`
(°C; no default rule — add one with a sustained duration, e.g. > 90 °C
for 120 s, clear at 80 °C).

Built-in rules are deliberately few and conservative:
- `filesystem-nearly-full`: > 95 % for 60 s, clears at 93 %, 10 min cooldown
- `memory-pressure`: PSI some > 10 % for 30 s, clears at 5 %
- `io-pressure`: PSI some > 30 % for 60 s, clears at 15 %

There is no default CPU-utilisation rule: busy CPUs are normal. CPU
*pressure* (tasks waiting for a CPU) is the better signal and can be added
in the config. Thresholds are starting points, not universal truths.

Event messages are deterministic, built from the triggering value, the
threshold and the sustained duration (e.g. "memory pressure at 23.4%
stayed beyond 10.0% for 30 s"). No inference about causes is made.

## Lifecycle
Alerts are evaluated only while a `nysm` process runs (`nysm alerts` or
the TUI). Background evaluation without a terminal requires the optional
per-user service (planned, ADR 0005), which is why alert rules are a
reason for that service to stay alive.
