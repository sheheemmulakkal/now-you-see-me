# ADR 0005: Embedded collector, optional per-user service, local IPC

Status: accepted; implemented for Unix (2026-10-06). Windows named pipe and D-Bus adapter not implemented.

## Context
CLI/TUI must work with no daemon. Desktop, panel and multiple clients
should share one collector instead of each sampling separately.

## Decision
- **Embedded mode (implemented)**: CLI and TUI embed `nysm-engine`. The TUI
  runs collection on a background thread and renders on the main thread.
- **Service mode (planned)**: an optional per-user `nysm-service` exposing
  versioned snapshots over a Unix-domain socket in `$XDG_RUNTIME_DIR/nysm/`
  (directory 0700, socket 0600), and a user-restricted named pipe on
  Windows. No TCP listener. A Linux D-Bus adapter may be added only for the
  GNOME extension; the core never requires D-Bus.
- Protocol: length-prefixed JSON frames with `schema_version`,
  `producer`, `seq`, timestamps, capabilities and the client's
  subscriptions; bounded frame size and per-client queue of 1 (latest wins)
  with gap counts for live clients. Recording clients get explicit
  dropped-sample accounting.
- Clients try the socket first and fall back to embedded collection.
  Startup races: the service takes an `flock` on a lock file before binding;
  a losing instance exits.
- Lifecycle: the service exits after an idle timeout with no clients unless
  alert rules are configured (alerts need continuous collection). Autostart
  is opt-in.

## Implementation notes (2026-10-06)
- Crate `nysm-ipc`: `protocol` (v1 frames, 8 MiB cap), `paths` (runtime dir
  `$NYSM_RUNTIME_DIR` > `$XDG_RUNTIME_DIR/nysm` > `/tmp/nysm-<uid>`; the
  directory must be ours with no group/other bits), `server`, `client`,
  `source::Source` (Local | Remote, same `LiveState` API).
- Peer uid is checked with `SO_PEERCRED`/`getpeereid` in addition to file
  permissions. Socket paths over 103 bytes are rejected with advice.
- Per-client writer thread waits on the newest snapshot (coalescing) and
  sends every history point and alert event the client has not seen, so
  history stays gap-free. Process tables are sent only to subscribers and
  only when they change. 5 s write timeout disconnects a stalled client.
- Single instance: `flock` on `collector.lock` (PID recorded inside);
  any socket file found while holding the lock is stale and replaced.
- Command lines are never sent over the socket.
- `nysm service run|status|stop|unit`; `unit` prints a systemd user unit
  but does not install or enable it.
- The TUI attaches by default when a service is running (`--attach`), and
  falls back to local collection with a notice if the service goes away.

## Consequences
Embedded mode remains the reference implementation; the service reuses the
same `Engine` and `Live` types.
