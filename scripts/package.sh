#!/usr/bin/env sh
# Build a release tarball: target/dist/nysm-<version>-<arch>-linux.tar.gz (+ .sha256).
# nysm and nysm-tray are static musl binaries (run on any Linux of that
# architecture, no glibc version requirement). nysm-desktop is included only
# if it builds; it is dynamically linked against glibc and GTK >= 4.12.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
host=$(rustc -vV | sed -n 's/^host: //p')
arch=${host%%-*}
musl="$arch-unknown-linux-musl"
name="nysm-$version-$arch-linux"
stage="target/dist/$name"
rm -rf "$stage" && mkdir -p "$stage/bin" "$stage/share/applications"
rustup target add "$musl" >/dev/null 2>&1 || true
cargo build --release --locked --target "$musl" -p nysm-cli -p nysm-tray
install -m 0755 "target/$musl/release/nysm" "target/$musl/release/nysm-tray" "$stage/bin/"
if cargo build --release --locked -p nysm-desktop 2>/dev/null; then
  install -m 0755 target/release/nysm-desktop "$stage/bin/"
else
  echo "note: nysm-desktop not built (GTK development files missing?)" >&2
fi
cp packaging/linux/*.desktop "$stage/share/applications/"
mkdir -p "$stage/share/icons/hicolor/scalable/apps"
cp packaging/icons/dev.nysm.NowYouSeeMe.svg "$stage/share/icons/hicolor/scalable/apps/"
mkdir -p "$stage/share/metainfo"
cp packaging/linux/dev.nysm.NowYouSeeMe.metainfo.xml "$stage/share/metainfo/"
# Man pages (generated from the CLI definition, plus the tray and desktop
# pages) and shell completions.
mkdir -p "$stage/share/man/man1" "$stage/share/bash-completion/completions" \
  "$stage/share/zsh/site-functions" "$stage/share/fish/vendor_completions.d"
"$stage/bin/nysm" manpages "$stage/share/man/man1"
cp packaging/man/nysm-tray.1 packaging/man/nysm-desktop.1 "$stage/share/man/man1/"
"$stage/bin/nysm" completions bash > "$stage/share/bash-completion/completions/nysm"
"$stage/bin/nysm" completions zsh > "$stage/share/zsh/site-functions/_nysm"
"$stage/bin/nysm" completions fish > "$stage/share/fish/vendor_completions.d/nysm.fish"
cp packaging/linux/install.sh packaging/linux/uninstall.sh README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$stage/"
python3 scripts/third_party_licenses.py > "$stage/THIRD-PARTY-LICENSES.txt"
(cd target/dist && tar --owner=0 --group=0 --sort=name -czf "$name.tar.gz" "$name" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "built target/dist/$name.tar.gz"
cat "target/dist/$name.tar.gz.sha256"
