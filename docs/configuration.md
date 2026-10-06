# Configuration

Optional. Without a file, built-in defaults apply.

```sh
nysm config path            # where the file is looked for
nysm config init [--force]  # write defaults (mode 0600, atomic rename)
nysm config show            # effective configuration
nysm config check           # validate; exit 2 if invalid
nysm --config other.toml …  # use another file for one invocation
```

Location: `$NYSM_CONFIG`; otherwise `$XDG_CONFIG_HOME/nysm/config.toml`
(`~/.config/nysm/config.toml`) on Linux/BSD,
`~/Library/Application Support/nysm/config.toml` on macOS,
`%APPDATA%\nysm\config.toml` on Windows.

```toml
version = 1

[sampling]
interval = "1s"             # 100ms .. 1h
process_interval = "2s"     # process table refresh when subscribed
filesystem_interval = "15s" # capacity refresh (worker thread)
history = "10m"             # in-memory history window (10s .. 24h)
cpu_frequency = true

[display]
rate_unit = "bytes"         # or "bits"
ascii = false               # TUI: ASCII-only charts/borders
theme = "system"            # desktop app: system | light | dark

[incidents]                 # opt-in, see docs/incidents.md
enabled = false
pre = "5m"
post = "2m"
max_files = 20
max_total_mib = 200

[alerts]
use_defaults = true         # built-in rules; same id below replaces one

[[alerts.rules]]
id = "cpu-saturated"
metric = "cpu_pressure_pct"
threshold = 50.0
clear = 25.0
for_s = 120.0
cooldown_s = 600.0
```

Precedence: command-line flag > config file > built-in default.

## Validation and recovery
- Unknown keys are errors (typos are not silently ignored).
- `version` is required; a newer version than supported is refused with an
  "upgrade nysm" message.
- Durations and rules are range-checked; `clear` must be on the healthy
  side of `threshold`; rule ids must be unique.
- Files above 256 KiB are refused.
- If the file is invalid, every command except `config` prints a warning
  naming the file and problem, then runs on defaults. Recover by editing
  the file, or `nysm config init --force` to replace it.
