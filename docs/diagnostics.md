# On-demand diagnostics

Both commands run only when invoked; nothing runs in the background.

## `nysm net check HOST[:PORT]`
Active reachability check against an explicit endpoint, separate from the
passive interface counters.

```sh
nysm net check example.com            # port 443 by default
nysm net check 10.0.0.5:22 --count 10
nysm net check '[::1]:8080' --json
```
Reports DNS resolution time and the addresses returned, then TCP connect
latency (handshake round trip including the server's accept) for
`--count` attempts with `--timeout` each, plus min/avg/max and failures.
Exit 1 if the name cannot be resolved or no attempt connects. No ICMP
(raw sockets need privileges) and nothing is sent after connecting. This
measures reachability and latency, not bandwidth or internet speed.

Verified locally: reachable listener (0.1 ms), refused port (exit 1),
unresolvable name (exit 1); target parsing for host, host:port, IPv6.

## `nysm disk usage PATH`
Largest entries directly under PATH, from a bounded and cancellable scan.

```sh
nysm disk usage ~/projects --top 10
nysm disk usage /var --max-entries 200000 --timeout 20s
```
- Bounds: `--max-entries` (default 2,000,000), `--timeout` (default 60 s),
  Ctrl-C. When stopped early, sizes are labelled lower bounds.
- Stays on PATH's filesystem unless `--cross-filesystems`; never follows
  symlinks; counts allocated space (`st_blocks × 512`); hard links once.
- Unreadable directories are counted and reported, not silently treated
  as empty.

Verified on the reference machine: 495,046 entries in 10.6 s; entry limit
stops with a lower-bound warning. It also found this project's own 15 GiB
debug build cache, which led to `profile.dev.debug = "line-tables-only"`
(debug build now ~1.9 GiB).
