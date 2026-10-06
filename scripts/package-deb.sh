#!/usr/bin/env sh
# Build Debian packages from the release binaries (run scripts/package.sh first):
#   nysm          CLI + TUI + collector service (static, no GUI dependencies)
#   nysm-tray     tray indicator (static; needs a StatusNotifier host at runtime)
#   nysm-desktop  GTK4 desktop app (only if bin/nysm-desktop exists)
# Output: target/dist/deb/*.deb. Nothing is installed.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
host=$(rustc -vV | sed -n 's/^host: //p')
arch=${host%%-*}
case "$arch" in x86_64) debarch=amd64 ;; aarch64) debarch=arm64 ;; *) echo "unsupported arch $arch" >&2; exit 1 ;; esac
stage="target/dist/nysm-$version-$arch-linux"
[ -d "$stage" ] || { echo "run scripts/package.sh first" >&2; exit 1; }
out=target/dist/deb
rm -rf "$out" && mkdir -p "$out"
maint="Now You See Me maintainers <noreply@invalid>"

mkpkg() { # name description depends files...
  name=$1; desc=$2; deps=$3; shift 3
  d="$out/build/$name"
  mkdir -p "$d/DEBIAN" "$d/usr/share/doc/$name"
  for spec in "$@"; do # src:dest
    src=${spec%%:*}; dest=${spec#*:}
    mkdir -p "$d/$(dirname "$dest")"
    install -m "$( [ -x "$src" ] && echo 0755 || echo 0644 )" "$src" "$d/$dest"
  done
  cat > "$d/usr/share/doc/$name/copyright" <<COPY
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Now You See Me
Comment: Third-party components and their licences are listed in
 /usr/share/doc/$name/THIRD-PARTY-LICENSES.txt

Files: *
Copyright: 2026 Muhammed Sheheem and the Now You See Me contributors
License: MIT or Apache-2.0
 Full texts: /usr/share/doc/$name/LICENSE-MIT and
 /usr/share/doc/$name/LICENSE-APACHE
COPY
  chmod 0644 "$d/usr/share/doc/$name/copyright"
  install -m 0644 "$stage/THIRD-PARTY-LICENSES.txt" "$stage/LICENSE-MIT" "$stage/LICENSE-APACHE" "$d/usr/share/doc/$name/"
  size=$(du -sk "$d" | cut -f1)
  {
    echo "Package: $name"
    echo "Version: $version"
    echo "Architecture: $debarch"
    echo "Maintainer: $maint"
    echo "Installed-Size: $size"
    [ -n "$deps" ] && echo "Depends: $deps"
    echo "Section: utils"
    echo "Priority: optional"
    echo "Description: $desc"
  } > "$d/DEBIAN/control"
  dpkg-deb --root-owner-group --build "$d" "$out/${name}_${version}_${debarch}.deb" >/dev/null
  echo "built $out/${name}_${version}_${debarch}.deb"
}

mkpkg nysm "Resource monitor for engineers (CLI, TUI, collector)
 Precise, lightweight system monitor: CPU, memory, pressure, network,
 storage, processes, containers and services. No GUI dependencies." "" \
  "$stage/bin/nysm:usr/bin/nysm" "README.md:usr/share/doc/nysm/README.md"

mkpkg nysm-tray "Tray indicator for Now You See Me
 Live CPU/memory meter for StatusNotifier/AppIndicator trays. Autostart is
 not enabled by the package; copy the desktop file to ~/.config/autostart." "nysm (= $version)" \
  "$stage/bin/nysm-tray:usr/bin/nysm-tray" \
  "packaging/linux/nysm-tray.desktop:usr/share/applications/nysm-tray.desktop"

if [ -x "$stage/bin/nysm-desktop" ]; then
  mkpkg nysm-desktop "GTK4 desktop app for Now You See Me
 Overview, processes, containers and services, CPU, memory, network and
 storage pages with light and dark themes." "nysm (= $version), libgtk-4-1 (>= 4.12), libc6 (>= 2.39)" \
    "$stage/bin/nysm-desktop:usr/bin/nysm-desktop" \
    "packaging/linux/dev.nysm.NowYouSeeMe.desktop:usr/share/applications/dev.nysm.NowYouSeeMe.desktop"
fi
rm -rf "$out/build"
(cd "$out" && sha256sum ./*.deb > SHA256SUMS)
