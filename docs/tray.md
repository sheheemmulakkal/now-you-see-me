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
  storage used `87%` (the `/` filesystem, like `df`), disk activity `12%`
  (busiest disk), optionally disk read/write `R1.2M W340K`.
  Values are compact (bytes per second, binary prefixes; bits with
  `--rate-unit bits`) and every item keeps exactly the same width: numbers are
  filled to three digits and a period with figure/punctuation spaces (as wide
  as a digit and a period by definition; UI fonts have tabular digits), and
  narrow units (K, B) get a period-wide filler to match M. The filler goes at
  the end of the item, so values stay next to their icons. Measured on Ubuntu
  24.04: icon positions moved 0 px over 30 s. A `⚠` appears before the first value while an
  alert fires. The icons are symbolic, so the panel recolours them.
- **Names**: on by default, short names before values — `CPU 24%`,
  `RAM 9.9G`, `Disk 87%` (storage used on `/`), `I/O 2%` (disk activity);
  network keeps its arrows and read/write its R/W. Menu → *Show icons* /
  *Show names* (or desktop Settings) chooses icons, names or both (at least
  one stays on); saved as `display.tray_icons` / `display.tray_names`. With
  names only, Ubuntu still reserves the (empty) icon slot. Ubuntu shows
  no tray tooltips, so each item's menu starts with a heading saying what it
  shows. GNOME gives the right side of the bar about half the screen; with many
  items it shortens the longest labels with "…" — show fewer items or no names.
- **Compact strip (default on GNOME)**: all chosen parts are drawn into one
  wide SVG image shown by a single tray item. Ubuntu's host displays image
  files at least 1.5× wider than tall at their own width, so there is no
  per-item padding, no empty icon slot with names only, and no "…" truncation.
  Each value has a fixed slot for its widest text, so the image width never
  changes (measured: left edge on the same pixel for 20 s). The text is drawn by
  GNOME with the system font; slots use Ubuntu Sans metrics scaled by the text
  scaling factor, and the colour is white (GNOME's top bar is dark). Written to
  `$XDG_RUNTIME_DIR/nysm/icons/nysm-strip-{0,1}.svg`. Cost measured: gnome-shell
  3.2 % of a core with the strip vs 3.3 % with separate items (2.2 % with the
  tray paused). Menu → *One compact strip* or `display.tray_layout`
  (`auto` = strip on GNOME, `strip`, `items`) switches; other desktops use
  separate items.
- **Choose what to show**: menu → *Show in top bar* → CPU usage, Memory used,
  Network download / upload, Storage used (%), Disk activity (%), Disk read / write. At least
  one item always stays (the last one is greyed out). Applied at once and saved as `display.tray_items` (e.g. `"cpu,mem,net,disk"`);
  `--items cpu,mem,net,disk,diskio` overrides it for one run.
- **From the desktop app**: Settings (Ctrl+,) → *Top bar*: show or hide the
  tray now, start it at login (writes or removes
  `~/.config/autostart/nysm-tray.desktop`), and choose the items. A running
  tray picks up item changes from the config file within 2 s.
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
