# Tray indicator (`nysm-tray`) — default top-bar component

Status: **implemented and confirmed on Ubuntu 24.04 GNOME 46 (X11)**: icon
visible and updating, menu values correct (checked by the user and over
D-Bus).

Uses the StatusNotifierItem / AppIndicator protocol through `ksni`
(pure Rust over D-Bus; no GTK, no C libraries, no tokio). Works wherever a
StatusNotifier host exists: Ubuntu's GNOME (built-in AppIndicator
support), KDE Plasma, XFCE, Cinnamon, MATE, Budgie, LXQt. Plain GNOME
without the AppIndicator extension (Fedora, Arch, Debian GNOME) shows no
tray; there, use the optional GNOME extension or `nysm-desktop`.

```sh
cargo build --release -p nysm-tray
./target/release/nysm-tray [--attach auto|never|require] [--items cpu,mem,net,disk] [--meter] [--no-label]
# opt-in autostart:
cp packaging/linux/nysm-tray.desktop ~/.config/autostart/
```

## What it shows
- **Top bar (default)**: one item per metric, each a symbolic icon with
  its value next to it — CPU `24%`, memory `6.2G`, network `↓1.8M ↑240K`,
  disk activity `12%` (busiest disk), optionally disk read/write `R1.2M W340K`.
  Values are compact (bytes per second, binary prefixes; bits with
  `--rate-unit bits`) and fixed-width, and each item keeps its widest width,
  so neighbours do not jump. A `⚠` appears before the first value while an
  alert fires. The icons are symbolic, so the panel recolours them.
- **Choose what to show**: menu → *Show in top bar* → CPU usage, Memory used,
  Network download / upload, Disk activity (%), Disk read / write. Applied at
  once and saved as `display.tray_items` (e.g. `"cpu,mem,net,disk"`);
  `--items cpu,mem,net,disk,diskio` overrides it for one run.
- **`--meter`**: a single item instead: a live two-bar icon (CPU blue,
  memory purple; grey when stale, red corner on alerts) followed by
  `24% · 6.2G · ↓1.8 MiB/s ↑240 KiB/s`.
- **`--no-label`**: icons only (for hosts or users that want no text).
- **Tooltip**: the same full summary on every item (CPU, memory, network,
  disk, load and pressure, temperatures, firing alerts).
- **Menu**: CPU (with core count), memory (used/total/available), network
  ↓/↑, disk read/write, load and pressure (cpu/mem/io), firing alerts,
  data source, *Open monitor* (also on left click), *Quit*.

## Text in the bar
The values next to the icons use the Ayatana label extension
(`XAyatanaLabel`), which Ubuntu's AppIndicator host shows. Upstream `ksni`
does not implement it, so the workspace uses a vendored ksni 0.3.6 with that
one addition (`vendor/ksni/PATCHED.md`). Hosts without the extension (KDE,
XFCE) ignore it and show the icons only, with values in the tooltip and menu.
Verified on Ubuntu 24.04 GNOME 46 (X11).

The tray waits for a tray host instead of exiting: at login it may start
before the panel, and GNOME removes the host while the screen is locked. Items
re-register when the host returns.

## Data and cost
If no service is running the tray collects totals itself, and every 30 s
tries to attach to a service (verified: switches to "Data: collector
service" within 30 s of `nysm service run`). If the service stops, it
falls back to local collection again.

Attaches to `nysm service` with a *lite* subscription (totals only), or
collects locally (CPU/memory/network/disk totals only; no process scan,
no filesystem worker) if no service runs. Measured attached to the service (30 s): release build **0.03 %** of one
core and 5.7 MiB RSS (debug build: 0.53 %, 11 MiB).
