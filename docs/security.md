# Security and privacy

To report a vulnerability, see [SECURITY.md](../SECURITY.md).

See ADR 0006.

## Data collected by default
Process name (`comm`), PID, start time, parent PID, owning UID and user name
(from `/etc/passwd`; NSS sources like LDAP are not consulted), state,
threads, CPU/memory counters, disk I/O counters where readable, interface
and device names, mount points and filesystem types.

## Not collected by default
- Command-line arguments: only `nysm inspect --show-args`.
- Environment variables: never.
- Connection lists/history: not collected. `nysm ports` looks up sockets
  for a port on demand and keeps nothing.

## Privileges
Runs as a normal user. Nothing requests elevation, adds groups or changes
system configuration. Container runtime sockets (Docker/Podman) are opened
only when you turn on container names (`--names`, the desktop switch or
`display.container_names`): one read-only `GET /containers/json`, because
access to that socket is root-equivalent on most systems. Data that needs
privileges shows `permission_denied` (for example `/proc/<pid>/io`,
`exe`, `cwd` of other users' processes).

## Terminal safety
Process names, paths, mount points and interface names are untrusted.
`core::sanitize::for_terminal` replaces C0/C1 control characters, DEL and
Unicode bidi controls with visible escapes before any text output. JSON
output relies on serde's string escaping.

## Local endpoints and files
The optional collector service (`nysm service run`) listens on a Unix
socket only: `$XDG_RUNTIME_DIR/nysm/collector.sock` (directory 0700,
socket 0600, refused if the directory is not ours or is group/other
accessible). Peers are additionally checked by uid. There is no TCP
listener. Command lines are never sent over the socket; process details
contain exe/cwd/cgroup only where the service's user may read them.
Otherwise there are no listeners. The only files written are recordings, and only
when requested: created with mode 0600, never overwritten without
`--force`, size-limited (`--max-size-mib`). They contain hostname, boot
id, process names/PIDs of the top processes and the recorded command's
program name; arguments only with `--store-args`. Delete them like any
file; there is no hidden copy.

## Container runtime socket
Only with `--names`: a single read-only `GET /containers/json` over the
Docker or Podman Unix socket. Never automatic, never a write.

## Ports lookup
`nysm ports` reads `/proc/net/{tcp,tcp6,udp,udp6}` (this network namespace
only) and maps socket inodes to processes through `/proc/<pid>/fd`, which
only works for processes the user may inspect. Other users' sockets are
listed with `permission_denied` owners. Nothing is terminated or modified.

## Remote access
Supported: run `nysm` on the remote host over SSH. No HTTP/WebSocket
endpoint exists or will be added without a separate authenticated,
encrypted design and threat model.

## Logs
`doctor` reports presence of environment variables, never values, and no
command lines. There is no persistent log yet.
