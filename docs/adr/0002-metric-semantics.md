# ADR 0002: Metric semantics

Status: accepted (2026-10-06)

## Context
Monitors disagree because they define "CPU %", "used memory" and "network
total" differently and silently show zero for unknown values.

## Decision
- Every value is a `Reading<T>` with a `Status` (`available`, `warming_up`,
  `unsupported`, `permission_denied`, `stale`, `collection_error`) and an
  optional reason. Missing values are absent, never zero.
- Rates come from counter deltas over **monotonic** elapsed time; wall time
  is display-only. A counter that goes backwards means a reset: the sample
  re-baselines (`warming_up`) instead of producing a huge or negative rate.
- CPU total = (user+nice+system+irq+softirq) / all accounted time, 0–100 %
  of the whole machine. `guest`/`guest_nice` are already inside user/nice
  and are not added. `iowait` and `steal` are reported separately and are
  not busy time.
- Process CPU defaults to share of the whole machine; the one-core scale
  (may exceed 100 %) is opt-in and labelled.
- Memory used = MemTotal − MemAvailable. Without MemAvailable we report
  `unsupported` rather than invent a formula.
- PSI is reported as stall share, with kernel averages and an exact
  per-interval value computed from the `total` counter.
- Network total = up physical (ethernet/wireless) interfaces only; bridges,
  veth, tunnels and loopback are listed but excluded to avoid double
  counting. The scope string travels with the value.
- Disk total = whole hardware disks only (not partitions, dm/md, loop,
  zram). Latency and in-flight time are per device, never summed.
- Process identity = PID + start time (+ boot id for recordings).
- A sample following a gap (suspend, stall) is flagged `gap_before`;
  history never interpolates across it.

See docs/metrics.md for formulas.

## Consequences
Values may differ from `top`/`free`/`ifstat` where those tools use other
definitions; differences are documented rather than hidden.
