# ADR 0006: Privacy and least-privilege defaults

Status: accepted (2026-10-06)

## Decision
- Core monitoring runs as a normal user; nothing escalates privileges.
- Collected by default: process name (`comm`), PID, parent, owner, CPU/memory
  counters, and I/O counters where readable.
- Not collected by default: command-line arguments (only `inspect
  --show-args`), environment variables (never), per-connection network
  history.
- Permission-limited data is shown as `permission_denied`, never guessed.
- All untrusted text is escaped before terminal output (control and bidi
  characters), see `core::sanitize`.
- `doctor` reports presence/absence of env vars, never their values, and
  prints no command lines.
- No telemetry, no network access by any core component.
- Future recordings/sockets: user-only permissions (0600/0700).

## Consequences
Some debugging needs `--show-args` explicitly. Redaction heuristics are not
promised to remove all secrets.
