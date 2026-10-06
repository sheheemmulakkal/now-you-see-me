# Recordings and comparison

```sh
nysm record -o before.nysm -- cargo build --release     # record a command
nysm record -o idle.nysm --duration 5m --interval 2s    # record a window
nysm compare before.nysm                                # summarise one
nysm compare before.nysm after.nysm [--json]            # compare two
```

## File format (`nysm-recording`, format_version 1)

JSON Lines, created with mode 0600, never overwritten without `--force`:

1. `{"type":"header", format, format_version, producer, schema_version,
   created_ms, host, interval_ms, target, label?}`. `target` is
   `{"kind":"window"}` or `{"kind":"command","program","root_pid","args"?}`.
   Only the program name is stored; arguments need `--store-args`.
2. `{"type":"sample", snapshot, top, tree?}`: a schema-1 snapshot
   without the process table. Interface/device rows keep those counted in
   totals plus the 8 busiest others (`compact_snapshot`), so totals are
   exact while files stay small. Also stored: the top 5 processes by CPU,
   and for command recordings, a `tree` aggregate.
3. `{"type":"event", timestamp_ms, event}`: `command_started`,
   `command_exited`, `interrupted`, `size_limit_reached`.
4. `{"type":"end", timestamp_ms, samples, dropped_samples, truncated,
   command?}`.

Size: about 9 KB per sample on the reference machine (≈32 MB/hour at 1 s).
`--max-size-mib` (default 64) stops writing samples when reached; later
scheduled samples are counted as dropped and the file is marked truncated.

Readers reject newer `format_version`s and files without the header,
ignore a partial final line (with a warning), fail on a corrupt line in
the middle (with its line number), and treat a missing `end` record as
truncated. Reading is streamed; files above 1 GiB are refused.

## What is exact and what is sampled

| value | how | caveat |
| --- | --- | --- |
| command wall time | monotonic clock around spawn → reap | includes process start-up |
| command CPU time | `wait4()` rusage of the command | includes all descendants the command waited for; orphaned/daemonised descendants are not included |
| largest single-process peak RSS | `ru_maxrss` | the peak of the biggest single process, **not** a sum over the tree |
| tree peak RSS | max over samples of Σ RSS of tree members | lower bound (peaks between samples are missed); shared pages counted once per process |
| tree CPU / disk I/O | Σ over members seen at sample times | processes shorter than one interval are missed |
| system totals | integrated rate × interval | windows with missing rates are reported as uncovered |

Tree membership: descendants of the root process (by parent PID) in each
process table. If the root exits first, remaining children are re-parented
by the kernel and leave the tree.

## Comparison rules

- Every row says whether it is exact or sampled.
- An observation is emitted only when the change is ≥ 5 % **and** the
  absolute difference is meaningful (≥ 1 percentage point, ≥ 4 MiB,
  ≥ 0.05 s). Otherwise "No metric changed…".
- Caveats are added for different hosts or boots, different intervals,
  fewer than 10 samples, truncation, dropped samples, gaps, and mixed
  command/window recordings.
- Observations describe the windows; they do not claim causes.

## Exit status
`nysm record -- CMD` exits with the command's status (128+signal when it
was killed). Window recordings exit 0. Ctrl-C stops a window recording and
marks it truncated; during a command recording Ctrl-C reaches the command
too, and nysm finalises after it exits.
