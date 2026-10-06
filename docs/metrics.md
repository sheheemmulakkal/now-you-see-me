# Metric definitions (Linux adapter)

Every metric is a `Reading` with a `status`: `available`, `warming_up`
(needs a second sample), `unsupported`, `permission_denied`, `stale`,
`collection_error`. Units are in JSON field names (`_pct`, `_bytes`,
`_bytes_per_s`, `_ms`). Rates use monotonic elapsed time; the minimum rate
interval is 100 ms. A counter that decreases is treated as a reset and the
reading becomes `warming_up` for that sample.

| id | source | definition |
| --- | --- | --- |
| `cpu.usage` | `/proc/stat` `cpu` line | busy = user+nice+system+irq+softirq; total = busy+idle+iowait+steal. `total_pct = Δbusy/Δtotal·100`. guest/guest_nice are already in user/nice and not added. |
| `cpu.per_core` | `/proc/stat` `cpuN` | same per logical CPU; matched by kernel id, so hot-plugged CPUs warm up independently |
| `cpu.logical_cores` | count of `cpuN` lines | online logical CPUs at this sample |
| `cpu.frequency` | `cpufreq/scaling_cur_freq` | kHz → MHz; instantaneous, may be `unsupported` in VMs |
| `cpu.load` | `/proc/loadavg` | 1/5/15-min run-queue averages (Linux includes uninterruptible tasks). Not a percentage; shown with core count. |
| `cpu.pressure`, `memory.pressure`, `io.pressure` | `/proc/pressure/*` | `some`/`full` avg10/60/300 as reported, plus `interval_pct = Δtotal_us / Δt`. CPU `full` lines that are all-zero at system level are dropped as meaningless. |
| `memory.usage` | `/proc/meminfo` | used = MemTotal − MemAvailable; cache = Cached + Buffers; also SReclaimable, Shmem, Dirty, MemFree |
| `memory.swap` | `/proc/meminfo` | used = SwapTotal − SwapFree |
| `memory.swap_activity` | `/proc/vmstat` pswpin/pswpout | pages/s × page size |
| `network.interfaces` | `/proc/net/dev`, `/sys/class/net` | Δbytes/Δt, Δpackets/Δt, errors+drops in interval. Kind from sysfs: loopback, bridge (`bridge/`), tunnel (`tun_flags`, type 65534, `wg*`), wireless, veth, physical (`device` link), virtual. |
| network total | — | sum of `counted_in_total` interfaces: physical/wireless that are not `down`. Includes LAN traffic; not internet throughput or link capacity. |
| `storage.devices` | `/proc/diskstats`, `/sys/block` | bytes = Δsectors × 512 (diskstats sectors are always 512 B); ops/s; latency = Δ(read_ms)/Δreads (queue+service, ms resolution); in-flight % = Δio_ms/Δt (not saturation for parallel devices) |
| disk total | — | sum over kind `disk` (whole hardware devices); excludes partitions, dm/md, loop, zram. Latency/busy are not summed. |
| `storage.filesystems` | `/proc/self/mountinfo` + `statvfs` | total = f_blocks·f_frsize; used = total − f_bfree·f_frsize (includes root-reserved); available = f_bavail·f_frsize (what a user can allocate); `used_pct = used/(used+available)` like `df`. Pseudo filesystems (proc, sysfs, tmpfs, overlay except `/`, squashfs, …) are skipped; duplicate mounts of one device are folded into `also_mounted_at`. Refreshed every 15 s on a worker thread. |
| `process.list` | `/proc/<pid>/stat`, owner of `/proc/<pid>` | identity = (pid, starttime ticks); `cpu_pct = Δ(utime+stime)/CLK_TCK / Δt / cores · 100`, clamped to 0–100; RSS = stat rss × page size; `cpu_time_s` cumulative |
| `process.disk_io` | `/proc/<pid>/io` read_bytes/write_bytes | bytes the process caused to be read from / written to storage (write includes later writeback); `permission_denied` for other users' processes unless privileged |

## Sensors (`snapshot.sensors`)

Read on a worker thread every 5 s (some drivers query the device, e.g.
NVMe), so a slow sensor can never stall sampling; old data is `stale`.

| item | source | notes |
| --- | --- | --- |
| temperatures | `/sys/class/hwmon/*/temp*_input`, `_label`, `_max`, `_crit` | millidegrees → °C. Thresholds of 0 mean "not set" and are dropped; unreadable thresholds (EIO) are absent; values outside −60…200 °C are discarded. Class from the driver: `coretemp`/`k10temp` → CPU, `nvme`/`drivetemp` → storage, `jc42` → memory, `pch_*` → chipset, GPU drivers → GPU. |
| CPU temperature | — | the package sensor (`Package id 0`, `Tctl`, `Tdie`) if present, else the hottest CPU-class sensor |
| fans | `fan*_input` | RPM; 0 is a valid reading (fan stopped) |
| batteries | `/sys/class/power_supply/*` with `type = Battery` | capacity %, kernel status, power from `power_now` or `current_now × voltage_now` |

| GPUs | `/sys/class/drm/cardN` | one entry per card (connectors ignored). i915/xe: `gt_act_freq_mhz` (else `gt_cur_freq_mhz`) and `gt_max_freq_mhz`. amdgpu: `gpu_busy_percent`, `mem_info_vram_used/total`, current level of `pp_dpm_sclk`. Utilisation for Intel needs perf PMU access and is reported as not exposed; NVIDIA (NVML) is not implemented. amdgpu parsing is fixture-tested only (no AMD GPU on the reference machine). |

A temperature above the driver's `high` value is shown as such; no
claim of thermal throttling is made from temperature or frequency alone.
VMs and containers usually expose no sensors (`unsupported`).

## Platform and scope notes

- **Containers**: `/proc/stat`, `/proc/meminfo` etc. usually describe the
  host kernel, while the process list is namespaced. `host.scope` says
  `container`. `snapshot.limits` reports the container's own cgroup:
  memory used vs `memory.max`/`memory.high`, CPU quota in cores
  (`cpu.max`) and tasks vs `pids.max`; it is `unsupported` in the root
  cgroup (a host). Network: when no physical interface is visible (a
  container's `eth0` is a veth), the total counts all up non-loopback,
  non-bridge interfaces and `total_scope` says so. Sensors are read from
  the host's `/sys` and describe host hardware.
- **VMs**: detected via the `hypervisor` CPU flag or DMI vendor; values
  describe the guest. `steal` shows time the hypervisor withheld.
- **WSL**: detected from the kernel release; values describe the WSL VM.
- `iowait` is idle time with I/O outstanding on that CPU. It is not CPU
  work and does not identify a process waiting on disk.
- Summing process RSS over-counts shared pages; it is not machine memory use.
- Process CPU on the one-core scale is `cpu_pct × logical_cores`.

## Differences from common tools

- `top`'s `%CPU` per process uses the one-core scale by default;
  `nysm processes --per-core` matches it.
- `free`'s "used" (total − free − buff/cache) differs from ours
  (total − available); ours answers "how much can a new workload get".
- `iostat` `%util` equals our in-flight %, with the same caveat.
