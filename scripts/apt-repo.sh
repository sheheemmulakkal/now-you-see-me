#!/usr/bin/env sh
# Add .deb packages to the signed APT repository served by GitHub Pages and
# regenerate its indexes and signatures.
#
#   scripts/apt-repo.sh DEB_DIR SITE_DIR
#
# DEB_DIR   directory with the release's *.deb files (amd64 and/or arm64)
# SITE_DIR  checkout of the gh-pages branch; the repository lives in
#           SITE_DIR/apt (older versions stay in the pool)
#
# Signing key: imported from $APT_SIGNING_KEY (armored private key, as in
# CI), else the first secret key in $GNUPGHOME / the default keyring.
# Needs apt-ftparchive (apt-utils) and gpg.
set -eu
debs=$(cd "$1" && pwd)
site=$(cd "$2" && pwd)
repo="$site/apt"

if [ -n "${APT_SIGNING_KEY:-}" ]; then
  GNUPGHOME=$(mktemp -d)
  export GNUPGHOME
  trap 'rm -rf "$GNUPGHOME"' EXIT
  printf '%s\n' "$APT_SIGNING_KEY" | gpg --batch --quiet --import
fi
fpr=$(gpg --list-secret-keys --with-colons | awk -F: '/^fpr/ {print $10; exit}')
[ -n "$fpr" ] || { echo "no signing key available" >&2; exit 1; }

mkdir -p "$repo/pool/main/n/nysm"
cp "$debs"/*.deb "$repo/pool/main/n/nysm/"
cd "$repo"
for arch in amd64 arm64; do
  d="dists/stable/main/binary-$arch"
  mkdir -p "$d"
  apt-ftparchive --arch "$arch" packages pool > "$d/Packages"
  gzip -9nkf "$d/Packages"
done
apt-ftparchive \
  -o APT::FTPArchive::Release::Origin="Now You See Me" \
  -o APT::FTPArchive::Release::Label="Now You See Me" \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
  -o APT::FTPArchive::Release::Components=main \
  -o APT::FTPArchive::Release::Description="Now You See Me resource monitor" \
  release dists/stable > Release.tmp
mv Release.tmp dists/stable/Release
gpg --batch --yes --local-user "$fpr" --clearsign -o dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$fpr" --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
gpg --export "$fpr" > nysm-archive-keyring.gpg
gpg --armor --export "$fpr" > nysm-archive-keyring.asc
touch "$site/.nojekyll" # serve files as they are
echo "APT repository updated in $repo (signed by $fpr)"
ls pool/main/n/nysm
