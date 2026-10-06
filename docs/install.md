# Install and uninstall

Status: release tarball for Linux x86_64 built and its install/uninstall
tested in a throwaway HOME on Ubuntu 24.04 (2026-10-06). No `.deb` yet.
Not for public distribution until the licence is chosen (ADR 0008).

## Build the package
```sh
scripts/package.sh
# -> target/dist/nysm-<version>-<arch>-linux.tar.gz and .sha256
```
Contains `bin/nysm` (CLI+TUI) and `bin/nysm-tray` as **static musl
binaries** (no glibc version requirement; verified running on Ubuntu
24.04, Debian 12 and Alpine 3.21), `bin/nysm-desktop` (only if GTK ≥ 4.12
dev files were available; dynamically linked, needs glibc ≥ 2.39 when built
on Ubuntu 24.04 and GTK ≥ 4.12 at runtime), `.desktop` files, `README.md`,
`THIRD-PARTY-LICENSES.txt` (generated from `cargo metadata`), and the
install/uninstall scripts. Built with the committed `Cargo.lock`.

## Debian/Ubuntu packages
```sh
scripts/package.sh && scripts/package-deb.sh
# -> target/dist/deb/{nysm,nysm-tray,nysm-desktop}_<ver>_amd64.deb + SHA256SUMS
sudo apt install ./target/dist/deb/nysm_*.deb            # CLI/TUI/service only (servers)
sudo apt install ./target/dist/deb/nysm-tray_*.deb       # + tray
sudo apt install ./target/dist/deb/nysm-desktop_*.deb    # + GTK app (Ubuntu 24.04+)
sudo apt purge nysm-desktop nysm-tray nysm
```
Split so that a server installs only `nysm` (static, no GUI or D-Bus
dependencies). `nysm-desktop` depends on `libgtk-4-1 (>= 4.12)` and
`libc6 (>= 2.39)`. No package enables autostart or a service.

Verified (2026-10-06) in an unprivileged Debian 12 container: `nysm` and
`nysm-tray` install and run; `nysm-desktop` is refused by dpkg because
Debian 12 has GTK 4.8 and glibc 2.36 (correct); purge leaves no files.
Not yet installed on a real Ubuntu host (requires sudo).

## Install (per user, no root)
```sh
sha256sum -c nysm-*.tar.gz.sha256          # verify first
tar xzf nysm-*.tar.gz && cd nysm-*/
./install.sh                     # -> ~/.local/bin, ~/.local/share/...
./install.sh --autostart-tray    # opt-in: start the tray at login
./install.sh --service-unit      # opt-in: write a systemd user unit (not enabled)
PREFIX=/opt/nysm ./install.sh    # another prefix (needs write access there)
```
Every installed file is recorded in `~/.local/share/nysm/installed-files`.
Servers can simply copy `bin/nysm`; it has no GUI or D-Bus dependencies.

## Uninstall
```sh
~/.local/share/nysm/uninstall.sh                 # removes exactly the recorded files
~/.local/share/nysm/uninstall.sh --purge-config  # also ~/.config/nysm
```
It disables the systemd user unit only if this installation wrote it, and
stops a running collector only if that process runs this installation's
`nysm` binary. Recordings you created are never touched.

Verified: install with `--autostart-tray --service-unit` placed 11 files;
uninstall removed all of them (0 left); a collector started from another
binary kept running.
