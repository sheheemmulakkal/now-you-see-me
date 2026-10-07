#!/usr/bin/env bash
# Capture real assets for the demo video, without opening anything on the
# user's screen: desktop pages and demo scenes render on a headless
# broadway display; the tray strip comes from a private tray instance on
# its own D-Bus session.
#
#   scripts/demo/capture.sh [OUT_DIR]      (default target/demo/capture)
#
# Needs: gtk4-broadwayd, dbus-run-session, python3 with gi (GdkPixbuf), an
# installed build in ~/.local/bin (or NYSM_BIN), and ideally a running
# collector service (`nysm service run`) for minutes of chart history.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
out=${1:-target/demo/capture}
bin=${NYSM_BIN:-$HOME/.local/bin}
mkdir -p "$out"
cfg="$out/demo.toml"
printf 'version = 1\n[display]\ncontainer_names = true\n' > "$cfg"

display=:31
gtk4-broadwayd "$display" >/dev/null 2>&1 &
bw=$!
trap 'kill $bw 2>/dev/null || true' EXIT
sleep 2
shot() { # theme page_or_demo out [extra args]
  local theme=$1 what=$2 file=$3; shift 3
  local gtk_theme=Yaru-dark
  [ "$theme" = light ] && gtk_theme=Yaru
  NYSM_CONFIG="$cfg" GTK_THEME=$gtk_theme GDK_BACKEND=broadway BROADWAY_DISPLAY=$display \
    "$bin/nysm-desktop" --theme "$theme" $what --screenshot "$out/$file" "$@" >/dev/null 2>&1
  echo "  $file"
}

echo "desktop pages"
for pg in overview cpu memory network storage groups about; do
  shot dark "--page $pg" "page-$pg.png" --delay 8s
done
shot light "--page overview" page-overview-light.png --delay 8s

echo "demo scenes"
# Watch uses a private collector so pins never touch the user's service.
shot dark "--demo watch" scene-watch.png --attach never --delay 45s
shot dark "--demo group" scene-group.png --delay 10s
shot dark "--demo settings" scene-settings.png --attach never --delay 4s

echo "terminal UI"
cargo run -q --release -p nysm-tui --example screens -- 120 34 --cells --warm 40 > "$out/tui.jsonl"

echo "tray strip"
for v in names icons; do
  rt="$out/rt-$v"
  mkdir -p "$rt" && chmod 700 "$rt"
  n=true; [ $v = icons ] && n=false
  printf 'version = 1\n[display]\ntray_items = "cpu,mem,net,storage,disk,diskio"\ntray_names = %s\ntray_icons = true\ntray_layout = "strip"\n' $n > "$out/tray-$v.toml"
  XDG_RUNTIME_DIR="$rt" NYSM_CONFIG="$out/tray-$v.toml" XDG_CURRENT_DESKTOP=GNOME \
    timeout 12 dbus-run-session -- "$bin/nysm-tray" --attach never >/dev/null 2>&1 || true
  cp "$(ls -t "$rt"/nysm/icons/nysm-strip-*.svg | head -1)" "$out/strip-$v.svg" 2>/dev/null || true
done
python3 - "$out" <<'EOF'
import sys, gi
gi.require_version('GdkPixbuf', '2.0')
from gi.repository import GdkPixbuf
out = sys.argv[1]
for v in ['names', 'icons']:
    pb = GdkPixbuf.Pixbuf.new_from_file_at_scale(f'{out}/strip-{v}.svg', -1, 72, True)
    pb.savev(f'{out}/strip-{v}@3x.png', 'png', [], [])
pb = GdkPixbuf.Pixbuf.new_from_file_at_scale('packaging/icons/dev.nysm.NowYouSeeMe.svg', 512, 512, True)
pb.savev(f'{out}/logo.png', 'png', [], [])
EOF

echo "command line"
host=$(hostname)
"$bin/nysm" summary --top 0 --color never | sed "s/$host/workstation/g" \
  | grep -v "not in total" | head -16 > "$out/cli-summary.txt"
"$bin/nysm" net check github.com --count 3 --color never > "$out/cli-net.txt" 2>&1 || true
echo "done: $out"
