# Desktop app (`nysm-desktop`, GTK4)

Status: **experimental**, implemented and rendered on Ubuntu 24.04
(GTK 4.14, GNOME 46) through a headless broadway display. Not yet used
interactively on a live session by a person; not packaged.

```sh
# needs GTK >= 4.12 development files (Ubuntu: libgtk-4-dev)
cargo build --release -p nysm-desktop
./target/release/nysm-desktop [--attach auto|never|require] [--page overview|processes|cpu|memory|network|storage]
```

It is not a default workspace member, so `cargo build` on a server never
needs GTK. It attaches to `nysm service` when running (shared sampling),
otherwise it runs an embedded collector.

Design follows the reference collage: B (calm overview with cards) for
the layout and D (quiet, warm light theme). Mockup labels that would
misstate a metric were corrected: pressure is never shown as usage, a
temperature is not shown until a sensor module exists, and per-volume
capacity is kept separate from per-device activity.

![Overview, dark](images/desktop-overview.png)
![Overview, light](images/desktop-overview-light.png)
![Storage, light](images/desktop-storage.png)

## Pages
- **Overview**: CPU, memory, network, disk with values, trends and detail
  lines (pressure, load), plus top processes by CPU. Alert banner when a
  rule is firing.
- **Processes**: filter (name/PID/user), sort (CPU, memory, disk I/O, name,
  PID), top 200 rows; activate a row for executable, working directory,
  cgroup and open file descriptors (command line never shown here).
- **Containers & services**: cgroup v2 groups (containers, services,
  apps) with kind filter and sort; collected only while the page is open.
- **CPU / Memory / Network / Storage**: breakdowns, per-core bars with
  frequency, memory categories and pressure history, interfaces, block
  devices, filesystems with capacity bars.

## Controls
- Sidebar with icons: Overview, CPU, Memory, Network, Storage, Processes.
- Header: live/stale indicator, alert count, time range (last 1/5/10
  minutes; history is 10 minutes by default), theme (system/light/dark;
  default from `display.theme` in the config, or `--theme`).

## Behaviour
- Hidden or minimised windows skip all updates; only the visible page is
  updated; the process list is rebuilt only when a new table arrives.
- Follows GNOME's light/dark preference (`org.gnome.desktop.interface
  color-scheme`) without libadwaita. No animations (stack transitions off).
- Charts break at gaps and missing samples, show scale and time span, and
  expose a text summary as their accessible label and tooltip. Series are
  labelled in a legend, not by colour alone.
- If an attached service disappears, it switches to an embedded collector
  and says so.

## Large text (checked 2026-10-06)
Rendered with `gtk-xft-dpi` at 1.5× (144 DPI): all text, cards and tables
scale and nothing overlaps; chart axis labels scale with DPI (10 px at
96 DPI). Known limitation: fixed column widths make the window's minimum
width grow to ~1480 px at 1.5× text, wider than small laptop screens.
Screen-reader (Orca) testing has not been done.

## Measured (reference machine, release, broadway, 30 s, embedded collector)
| visible page | CPU % of one core | RSS |
| --- | --- | --- |
| Overview | 2.8 | 47 MiB |
| Processes | 2.4 | 53 MiB |

Broadway (headless) rendering differs from X11/Wayland GL, so treat these
as indicative. The Processes page first cost 6.4 % because it rebuilt
~200 row widgets on every refresh; rows are now created once and only
their label texts are updated.

## Not yet done
- libadwaita (`libadwaita-1-dev` is not installed on the dev machine;
  installing it needs administrator rights); keyboard shortcuts beyond GTK
  defaults; settings dialog; screen-reader testing with Orca; HiDPI and
  large-text checks; packaging/installation of the `.desktop` file
  (`packaging/linux/`).
