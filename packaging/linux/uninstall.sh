#!/usr/bin/env sh
# Remove exactly the files recorded at install time, stop this install's
# tray and collector, and remove the tray's login entry if it points here.
#   --purge-config  also delete the configuration (~/.config/nysm)
#   --purge         also delete configuration and saved data
#                   (~/.local/state/nysm: incident captures)
# Recordings you made with `nysm record -o FILE` are yours and never touched.
set -eu
prefix=${PREFIX:-"$HOME/.local"}
manifest="$prefix/share/nysm/installed-files"
purge_config=0
purge_state=0
case "${1:-}" in
  --purge-config) purge_config=1 ;;
  --purge) purge_config=1; purge_state=1 ;;
  "") ;;
  *) echo "usage: uninstall.sh [--purge-config|--purge]" >&2; exit 2 ;;
esac
if [ ! -f "$manifest" ]; then
  echo "no install manifest at $manifest" >&2
  exit 1
fi
# Disable the user unit only if this install wrote it.
unit_file="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/nysm.service"
if command -v systemctl >/dev/null 2>&1 && grep -qx "$unit_file" "$manifest"; then
  systemctl --user disable --now nysm.service 2>/dev/null || true
fi
# Stop the collector only if it runs *this* installation's binary.
rt="${NYSM_RUNTIME_DIR:-${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/nysm}}"
if [ -n "$rt" ] && [ -f "$rt/collector.lock" ]; then
  pid=$(cat "$rt/collector.lock" 2>/dev/null || true)
  if [ -n "$pid" ] && [ "$(readlink "/proc/$pid/exe" 2>/dev/null)" = "$prefix/bin/nysm" ]; then
    "$prefix/bin/nysm" service stop >/dev/null 2>&1 || true
  fi
fi
# Stop trays running this installation's binary (also after it was replaced).
for d in /proc/[0-9]*; do
  exe=$(readlink "$d/exe" 2>/dev/null || true)
  case "$exe" in
    "$prefix/bin/nysm-tray"|"$prefix/bin/nysm-tray (deleted)") kill "${d#/proc/}" 2>/dev/null || true ;;
  esac
done
# The desktop app's "Start at login" switch writes this entry; remove it
# only if it starts this installation's tray.
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/nysm-tray.desktop"
if [ -f "$autostart" ] && grep -qx "Exec=$prefix/bin/nysm-tray" "$autostart"; then
  rm -f -- "$autostart"
fi
# Tray images in the runtime directory (also cleared at logout).
[ -n "${XDG_RUNTIME_DIR:-}" ] && rm -rf -- "$XDG_RUNTIME_DIR/nysm/icons"
for d in /proc/[0-9]*; do
  case "$(readlink "$d/exe" 2>/dev/null || true)" in
    "$prefix/bin/nysm-desktop"*) echo "note: a Now You See Me window is still open; close it" ;;
  esac
done | sort -u
while IFS= read -r f; do
  [ -n "$f" ] && rm -f -- "$f"
done < "$manifest"
rm -f -- "$manifest"
rmdir "$prefix/share/nysm" "$prefix/share/doc/nysm" 2>/dev/null || true
if [ "$purge_config" = 1 ]; then
  rm -rf -- "${XDG_CONFIG_HOME:-$HOME/.config}/nysm"
  echo "removed configuration"
fi
if [ "$purge_state" = 1 ]; then
  rm -rf -- "${XDG_STATE_HOME:-$HOME/.local/state}/nysm"
  echo "removed saved data (incident captures)"
fi
echo "uninstalled from $prefix"
