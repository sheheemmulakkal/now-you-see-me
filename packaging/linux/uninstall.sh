#!/usr/bin/env sh
# Remove exactly the files recorded at install time. Configuration
# (~/.config/nysm) and recordings are left alone unless --purge-config.
set -eu
prefix=${PREFIX:-"$HOME/.local"}
manifest="$prefix/share/nysm/installed-files"
purge=0
[ "${1:-}" = "--purge-config" ] && purge=1
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
while IFS= read -r f; do
  [ -n "$f" ] && rm -f -- "$f"
done < "$manifest"
rm -f -- "$manifest"
rmdir "$prefix/share/nysm" "$prefix/share/doc/nysm" 2>/dev/null || true
if [ "$purge" = 1 ]; then
  rm -rf -- "${XDG_CONFIG_HOME:-$HOME/.config}/nysm"
fi
echo "uninstalled from $prefix"
