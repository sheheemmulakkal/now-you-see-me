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
maint="Muhammed Sheheem <shaimonsheheem@gmail.com>"
# Reproducible dates: the last commit's time.
date_r=$(date -R -d "@$(git log -1 --format=%ct 2>/dev/null || date +%s)")

mkpkg() { # name description depends files...
  name=$1; desc=$2; deps=$3; shift 3
  d="$out/build/$name"
  mkdir -p "$d/DEBIAN" "$d/usr/share/doc/$name"
  for spec in "$@"; do # src:dest
    src=${spec%%:*}; dest=${spec#*:}
    mkdir -p "$d/$(dirname "$dest")"
    install -m "$( [ -x "$src" ] && echo 0755 || echo 0644 )" "$src" "$d/$dest"
    case "$dest" in
      usr/bin/*)
        strip --strip-all "$d/$dest"
        # Static PIE binaries look like shared libraries to lintian.
        if file "$d/$dest" | grep -q 'static-pie'; then
          mkdir -p "$d/usr/share/lintian/overrides"
          echo "$name: shared-library-lacks-prerequisites [$dest]" >> "$d/usr/share/lintian/overrides/$name"
        fi ;;
      usr/share/man/*) gzip -9n "$d/$dest" ;;
    esac
  done
  # Debian changelog (native package): points to the project changelog.
  printf '%s (%s) stable; urgency=medium\n\n  * Release %s. Changes are listed in CHANGELOG.md:\n    https://github.com/sheheemmulakkal/now-you-see-me/releases\n\n -- %s  %s\n' \
    "$name" "$version" "$version" "$maint" "$date_r" | gzip -9n > "$d/usr/share/doc/$name/changelog.gz"
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
  find "$d" -type d -exec chmod 0755 {} +
  find "$d" -type f ! -path "*/DEBIAN/*" ! -path "$d/usr/bin/*" -exec chmod 0644 {} +
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

man_nysm=""
for m in "$stage"/share/man/man1/nysm*.1; do
  case "$(basename "$m")" in nysm-tray.1|nysm-desktop.1) continue ;; esac
  man_nysm="$man_nysm $m:usr/share/man/man1/$(basename "$m")"
done
# shellcheck disable=SC2086 # man_nysm is a list of src:dest pairs
mkpkg nysm "Resource monitor for engineers (CLI, TUI, collector)
 Precise, lightweight system monitor: CPU, memory, pressure, network,
 storage, processes, containers and services. No GUI dependencies." "" \
  "$stage/bin/nysm:usr/bin/nysm" "README.md:usr/share/doc/nysm/README.md" \
  "packaging/icons/dev.nysm.NowYouSeeMe.svg:usr/share/icons/hicolor/scalable/apps/dev.nysm.NowYouSeeMe.svg" \
  "$stage/share/bash-completion/completions/nysm:usr/share/bash-completion/completions/nysm" \
  "$stage/share/zsh/site-functions/_nysm:usr/share/zsh/vendor-completions/_nysm" \
  "$stage/share/fish/vendor_completions.d/nysm.fish:usr/share/fish/vendor_completions.d/nysm.fish" \
  $man_nysm

mkpkg nysm-tray "Top-bar indicator for Now You See Me
 Live CPU, memory, network, storage and disk values in the top bar
 (StatusNotifier/AppIndicator). Autostart is not enabled by the package;
 turn on \"Start at login\" in the desktop app's settings." "nysm (= $version)" \
  "$stage/bin/nysm-tray:usr/bin/nysm-tray" \
  "$stage/share/man/man1/nysm-tray.1:usr/share/man/man1/nysm-tray.1" \
  "packaging/linux/nysm-tray.desktop:usr/share/applications/nysm-tray.desktop"

if [ -x "$stage/bin/nysm-desktop" ]; then
  mkpkg nysm-desktop "GTK4 desktop app for Now You See Me
 Overview, processes, containers and services, CPU, memory, network and
 storage pages with light and dark themes." "nysm (= $version), libgtk-4-1 (>= 4.12), libc6 (>= 2.39)" \
    "$stage/bin/nysm-desktop:usr/bin/nysm-desktop" \
    "$stage/share/man/man1/nysm-desktop.1:usr/share/man/man1/nysm-desktop.1" \
    "$stage/share/metainfo/dev.nysm.NowYouSeeMe.metainfo.xml:usr/share/metainfo/dev.nysm.NowYouSeeMe.metainfo.xml" \
    "packaging/linux/dev.nysm.NowYouSeeMe.desktop:usr/share/applications/dev.nysm.NowYouSeeMe.desktop"
fi
rm -rf "$out/build"
(cd "$out" && sha256sum ./*.deb > SHA256SUMS)
