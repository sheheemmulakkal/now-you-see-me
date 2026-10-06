#!/usr/bin/env sh
# Build an installable zip of the GNOME Shell extension (compiles schemas).
# Usage: scripts/extension-pack.sh [out-dir]   (default: target/extension)
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$here/target/extension"}
mkdir -p "$out"
gnome-extensions pack --force --out-dir="$out" \
  --extra-source=client.js --extra-source=format.js \
  "$here/gnome-extension/nysm@nysm.dev"
echo "built $out/nysm@nysm.dev.shell-extension.zip"
echo "install (user-level, opt-in): gnome-extensions install --force $out/nysm@nysm.dev.shell-extension.zip"
echo "then log out/in (Wayland) or restart the shell (X11: Alt+F2, r), and: gnome-extensions enable nysm@nysm.dev"
