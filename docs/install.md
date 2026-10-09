# Install and uninstall

Status: release tarball and split `.deb` packages for Linux x86_64 and arm64,
published on [GitHub Releases](https://github.com/sheheemmulakkal/now-you-see-me/releases)
for every `v*` tag. The per-user install is tested in a throwaway HOME and on
the development machine (Ubuntu 24.04). Licensed MIT OR Apache-2.0; licence texts and the
third-party notices are included in every package.

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
`THIRD-PARTY-LICENSES.txt` (generated from `cargo metadata`), man pages
(`share/man/man1`), shell completions for bash, zsh and fish, AppStream
metadata for software centres, and the install/uninstall scripts. Built with the committed `Cargo.lock`.

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

The packages carry man pages, bash/zsh/fish completions, AppStream metadata
(`nysm-desktop`) and a Debian changelog, and pass `lintian` with no
findings.

Verified (2026-10-09) in clean containers: on Ubuntu 22.04 `nysm` and
`nysm-tray` install and run, and `nysm-desktop` is refused because 22.04
has GTK 4.6 (correct); on Ubuntu 24.04 and 26.04 all three install and run,
and purge leaves no files. Debian 12 behaves like Ubuntu 22.04 (GTK 4.8).

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

### What uninstall removes
`~/.local/share/nysm/uninstall.sh` removes the installed files, stops this
installation's tray and collector, removes the tray's login entry if it starts
this installation, and clears the tray images in the runtime directory. Open
desktop windows are reported, not closed. `--purge-config` also deletes
`~/.config/nysm`; `--purge` also deletes saved data in `~/.local/state/nysm`
(incident captures). Files you recorded with `nysm record -o FILE` are never
touched. Verified in a throwaway HOME: nothing left after `--purge`.

### Cleanup while running
- The collector removes its socket on exit and replaces a stale one on start.
- The tray removes its images when it quits or is stopped (logout, `kill`, the
  desktop app's switch) and when switching away from the strip.
- History, process pins and incident captures are bounded; watched processes
  that exited are dropped 15 minutes after exit.
- Interrupted config saves leave no temp files behind: stale
  `config.toml.tmp-PID` files are removed on the next save.
