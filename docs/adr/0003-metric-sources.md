# ADR 0003: Direct OS collection vs a metrics library

Status: accepted (2026-10-06)

## Context
`sysinfo` offers a portable API, but its definitions (e.g. memory "used",
which interfaces are summed, process CPU scaling, refresh cost of reading
everything) do not match ADR 0002, and it refreshes broad sets of data.

## Decision
- **Linux**: read `/proc` and `/sys` directly through small pure parsers.
  We control every field, can attach precise statuses (permission denied vs
  unsupported vs exited), reuse buffers, and avoid refreshing data nobody
  subscribed to.
- **macOS/Windows**: not implemented yet. The next step is native adapters
  (`host_processor_info`, `vm_statistics64`, `sysctl`; PDH/`GetSystemTimes`,
  `GetIfTable2`), with `sysinfo` acceptable *behind the adapter* only where
  its semantics match and are documented.
- Until then those platforms use `UnsupportedPlatform`, which reports
  `unsupported` for everything rather than substituting Linux semantics.

## Consequences
More code to maintain per platform; in exchange, definitions are exact and
testable from fixtures.

## Revisit when
A platform adapter would mostly duplicate `sysinfo` with identical
semantics.
