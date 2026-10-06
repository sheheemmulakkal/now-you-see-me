# Platform support

Tiers per ADR 0007. "Built" means compiled; only "Tested" means the binary
was run on that platform with real data.

| Platform | Arch | CLI | TUI | Desktop | Panel | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Ubuntu 24.04 (kernel 7.0) | x86_64 | ✅ tested | ✅ tested (pty harness, TestBackend) | 🟡 experimental (rendered headless via broadway) | planned (M3) | Tier 1 |
| Other Linux distributions | x86_64 | static musl build runs on Debian 12 and Alpine (in containers, host kernel 7.0); other distros/kernels untested | expected | — | — | partial |
| Linux | aarch64 (incl. Raspberry Pi 64-bit) | type-checks (`cargo check`) | type-checks | — | — | Tier 2 target; not run |
| macOS | x86_64 / arm64 | type-checks; all metrics `unsupported` | type-checks | not planned yet | — | Tier 3 |
| Windows | x86_64 | type-checks; all metrics `unsupported` | type-checks | not planned yet | — | Tier 3 |
| Containers (Docker, unprivileged) | x86_64 | ✅ tested on Debian 12 and Alpine 3.21 images (static musl build): scope, own limits, network, process list | TUI not tested in a container | — | — | Tier 1 for the CLI |
| VM guests / WSL | — | scope detected and labelled | same | — | — | not tested |
| BSD, Android, iOS | — | — | — | — | — | not started |

## Requirements (Linux)

- Kernel ≥ 3.14 for `MemAvailable` (otherwise memory usage is `unsupported`).
- PSI needs kernel ≥ 4.20 with `CONFIG_PSI` and not booted with `psi=0`.
- `/proc` must be mounted. With `hidepid=` other users' processes are
  hidden (reported by `nysm doctor`).
- No GUI libraries, D-Bus, systemd, or root needed. Verified: the
  dependency tree of `nysm-cli` contains no GTK/X11/Wayland/D-Bus/systemd
  crates.

## Binary compatibility
Release builds of `nysm`/`nysm-tray` are static musl binaries. A plain
`cargo build` on Ubuntu 24.04 produces glibc binaries that require
glibc ≥ 2.39 and will not start on Debian 12 / Ubuntu 22.04.

## Verification gaps

- No ARM64 hardware or macOS/Windows machines have been used yet.
- Not yet tested in a VM guest, WSL, Podman, or Kubernetes.
- Not tested on distributions other than Ubuntu 24.04.
