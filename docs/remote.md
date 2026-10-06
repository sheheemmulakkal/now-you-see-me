# Remote monitoring over SSH

```sh
nysm tui --remote user@server                       # nysm must be on the server's PATH
nysm tui --remote user@server --remote-nysm ~/.local/bin/nysm
```

The TUI runs `ssh -T -- user@server nysm service stdio` and speaks the
normal collector protocol over that SSH channel. There is **no network
listener**: SSH provides host-key verification, authentication (your
keys, agent, `~/.ssh/config`) and encryption. This is step 1 of ADR 0010.

On the server, `nysm service stdio` starts an embedded collector for that
one client and exits when the connection closes. It never sends command
lines (process details are exe/cwd/cgroup only, subject to the remote
user's permissions). Running it directly on a terminal is refused,
because it writes a binary protocol to stdout.

If the connection drops, the TUI shows "remote connection lost — data
shown is stale" and keeps the last data visibly stale. It never falls back
to showing the local machine's data.

Install on the server: copy the static `nysm` binary from the release
tarball (`bin/nysm`) or install the `nysm` .deb.

## Verified
`scripts/remote_smoke.py` (8 checks): a stand-in `ssh` earlier in PATH
runs the remote command locally; the TUI renders remote data, invokes
`ssh -T -- DEST`, shows connection loss without local fallback, exits
cleanly, and the remote collector ends both on connection loss and on a
normal quit. Not yet run against a real remote host over real SSH.
