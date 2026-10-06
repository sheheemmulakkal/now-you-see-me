# Architecture

See ADR 0001 for rationale. Current crates:

```
nysm-core     domain types, Reading/Status, pure math, history, sanitize, query
   ▲
nysm-collect  Platform trait; Linux /proc+/sys adapter; Unsupported adapter; pure parsers
   ▲
nysm-engine   Engine (sampling, deltas, resets, gaps), FsWorker, Live (background thread)
   ▲               ▲
nysm-ipc      protocol, server (`nysm service`), client, Source (local | remote)
   ▲
nysm-tui        nysm-cli (binary `nysm`; `tui` feature pulls nysm-tui)

nysm-record (recording format, compare)   nysm-config (TOML settings)
```

`nysm-core` has no I/O and no UI dependencies. Neither core nor engine may
depend on a UI toolkit.

## Data flow

1. `Engine::sample()` asks the `Platform` for raw counters (CPU ticks,
   meminfo, PSI, net/disk counters, optionally processes).
2. The engine pairs them with the previous raw reading and computes rates
   with `core::math` over monotonic elapsed time; first samples and resets
   yield `warming_up`.
3. It assembles an immutable `Snapshot` (shared via `Arc`) and appends a
   fixed-size `HistoryPoint` to a bounded ring buffer.
4. Frontends render snapshots. Sorting/trees use `core::query`.

## Scheduling

- One sample per call; callers own cadence. `watch` uses a deadline loop
  that realigns rather than bursting after a stall.
- Process scans run at most every `process_interval` (2 s default) and
  only when subscribed; otherwise the previous table is reused.
- Filesystem capacity runs on a dedicated worker thread every 15 s because
  `statvfs` can block indefinitely on network mounts. The engine reads the
  latest cached result and marks it `stale` when older than 3×interval+5 s.
  A blocked OS call cannot be cancelled; the worker is detached and exits
  with the process.
- CPU frequency is per-core sysfs reads each sample (cheap); it can be
  disabled in `EngineConfig`.

## Execution modes

- **Embedded (implemented)**: CLI commands own an `Engine` directly. The TUI
  uses `Live`, which runs the engine on a `nysm-collector` thread and
  publishes the latest snapshot to a shared slot. A slow UI never delays
  collection; it observes a jump in `seq` (coalescing).
  Process details are requested through a command channel and answered by
  the collector thread, so the UI thread never touches `/proc`.
- **Service (implemented on Unix, ADR 0005)**: `nysm service run` runs the
  same Engine/Live behind a per-user Unix socket. Frontends use
  `nysm_ipc::source::Source`, which is either `Local(Live)` or
  `Remote(RemoteLive)`; `RemoteLive` mirrors the server's `LiveState`
  (history, alerts, pinned processes), so UI code is identical in both
  modes. Process collection in the service runs only while at least one
  client subscribes to process tables.

## Rendering (TUI)

Input polling and drawing happen on the main thread; draws happen only when
something changed (new snapshot while not paused, input, resize) and at most
`--max-fps` times per second. Paused mode freezes the displayed snapshot and
history while the collector keeps running.
