#!/usr/bin/env sh
# Install Now You See Me for the current user (default) or into PREFIX.
#   ./install.sh                     -> ~/.local
#   PREFIX=/opt/nysm ./install.sh    -> custom prefix (needs write access)
#   ./install.sh --autostart-tray    -> also start the tray at login (opt-in)
#   ./install.sh --service-unit      -> also install a systemd *user* unit (not enabled)
# Nothing is installed system-wide and nothing needs root unless PREFIX does.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-"$HOME/.local"}
autostart=0
unit=0
for a in "$@"; do
  case "$a" in
    --autostart-tray) autostart=1 ;;
    --service-unit) unit=1 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done
manifest="$prefix/share/nysm/installed-files"
mkdir -p "$prefix/bin" "$prefix/share/applications" "$prefix/share/nysm" "$prefix/share/doc/nysm"
: > "$manifest.tmp"
put() { # src dest mode
  install -m "$3" "$1" "$2"
  echo "$2" >> "$manifest.tmp"
}
for b in nysm nysm-tray nysm-desktop; do
  [ -f "$here/bin/$b" ] && put "$here/bin/$b" "$prefix/bin/$b" 0755
done
if [ -f "$here/bin/nysm-desktop" ]; then
  put "$here/share/applications/dev.nysm.NowYouSeeMe.desktop" "$prefix/share/applications/dev.nysm.NowYouSeeMe.desktop" 0644
fi
put "$here/share/applications/nysm-tray.desktop" "$prefix/share/applications/nysm-tray.desktop" 0644
for d in README.md THIRD-PARTY-LICENSES.txt; do
  put "$here/$d" "$prefix/share/doc/nysm/$d" 0644
done
put "$here/uninstall.sh" "$prefix/share/nysm/uninstall.sh" 0755
if [ "$autostart" = 1 ]; then
  mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
  sed "s|^Exec=nysm-tray|Exec=$prefix/bin/nysm-tray|" "$here/share/applications/nysm-tray.desktop" \
    > "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/nysm-tray.desktop"
  echo "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/nysm-tray.desktop" >> "$manifest.tmp"
fi
if [ "$unit" = 1 ]; then
  udir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  mkdir -p "$udir"
  "$prefix/bin/nysm" service unit > "$udir/nysm.service"
  echo "$udir/nysm.service" >> "$manifest.tmp"
  echo "systemd user unit written (not enabled): systemctl --user enable --now nysm"
fi
mv "$manifest.tmp" "$manifest"
echo "installed to $prefix (uninstall: $prefix/share/nysm/uninstall.sh)"
case ":$PATH:" in *":$prefix/bin:"*) ;; *) echo "note: add $prefix/bin to PATH";; esac
