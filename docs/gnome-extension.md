# GNOME Shell panel extension (optional)

The default top-bar component is the tray indicator (docs/tray.md). This
extension is an optional, richer panel for GNOME users who have user
extensions enabled. It is not installed by default.

Status: **implemented, not yet loaded into a live GNOME Shell.** Target:
GNOME Shell 46 (Ubuntu 24.04). Other versions are not declared in
`metadata.json` until tested.

The extension only renders data from the per-user collector
(`nysm service run`) using a *lite* subscription (totals only, ~1 frame per
interval). It never collects metrics inside GNOME Shell, never spawns the
collector, and releases its socket, timers and signal handlers in
`disable()`.

## Panel and popover
- Panel: `CPU 23%  MEM 64%  ↓1.2MiB ↑34KiB` (units always explicit;
  arrows mean per second). Fixed-width, tabular figures so it does not
  jump. Each metric can be hidden; *compact* drops the labels; bits or
  bytes for network.
- Values turn grey/italic when stale (no update for 3 intervals + 1 s) or
  when the collector is not reachable; they are never shown as live.
- Panel text turns red while an alert rule is firing.
- Popover: CPU, memory, swap, network ↓/↑, disk read/write, load with core
  count, pressure cpu/mem/io; 60-sample sparklines (gaps not bridged);
  connection status; *Open monitor* (launches `nysm-desktop`).

## Build / install (opt-in, user-level)
```sh
scripts/extension-pack.sh
gnome-extensions install --force target/extension/nysm@nysm.dev.shell-extension.zip
# X11: Alt+F2, r — Wayland: log out and in
gnome-extensions enable nysm@nysm.dev
nysm service run &        # or install the systemd user unit: nysm service unit
```
Uninstall: `gnome-extensions uninstall nysm@nysm.dev`.

## Verified
- `gjs -m gnome-extension/test/format_test.js`: unit formatting.
- `gjs -m gnome-extension/test/client_test.js` against a running collector:
  handshake, lite updates (no per-core/interface rows), history.
- `gjs -m gnome-extension/test/reconnect_test.js`: starts with no
  collector, backs off (2 s, 4 s, …, max 30 s), connects when it appears.
- Syntax of `extension.js`/`prefs.js` (`node --check`), schema compiles
  with `glib-compile-schemas --strict`, `gnome-extensions pack` succeeds.

## Live-shell attempt (2026-10-06)
Installed and enabled on the reference machine, it stayed `INITIALIZED`
because that desktop has `disable-user-extensions = true` (which blocks all
home-directory extensions). It was uninstalled again and the user's
enabled-extensions list restored. This is why the tray became the default.

## Not verified yet
- Loading in GNOME Shell (enable/disable cycles, looking for leaked
  signals in `journalctl --user`), visual appearance, prefs dialog,
  keyboard navigation of the popover, GNOME 47+.
