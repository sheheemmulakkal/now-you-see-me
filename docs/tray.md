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
./target/release/nysm-tray [--attach auto|never|require]
# opt-in autostart:
cp packaging/linux/nysm-tray.desktop ~/.config/autostart/
```

## What it shows
- **Icon**: a live meter, two bars — CPU (blue) and memory used (purple).
  Grey when stale or not yet collected; missing values are an empty
  track, never a zero level; red corner while an alert is firing.
- **Tooltip**: CPU, memory, network.
- **Menu**: CPU (with core count), memory (used/total/available), network
  ↓/↑, disk read/write, load and pressure (cpu/mem/io), firing alerts,
  data source, *Open monitor* (also on left click), *Quit*.

## Why no text in the bar
Ubuntu's indicator host can show a text label (`XAyatanaLabel`), but it
is an Ayatana extension that `ksni` does not implement and that KDE and
others ignore. The portable meter icon was chosen instead; the label is a
possible Ubuntu-only enhancement later.

## Data and cost
If no service is running the tray collects totals itself, and every 30 s
tries to attach to a service (verified: switches to "Data: collector
service" within 30 s of `nysm service run`). If the service stops, it
falls back to local collection again.

Attaches to `nysm service` with a *lite* subscription (totals only), or
collects locally (CPU/memory/network/disk totals only; no process scan,
no filesystem worker) if no service runs. Measured attached to the service (30 s): release build **0.03 %** of one
core and 5.7 MiB RSS (debug build: 0.53 %, 11 MiB).
