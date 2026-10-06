use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use nysm_core::Status;
use nysm_core::raw::{
    DiskKind, InterfaceKind, ProcessId, RawCgroup, RawCpu, RawDisk, RawInterface, RawLoad,
    RawMemory, RawPressure, RawProcess, RawProcessIo, RawSwapActivity,
};
use nysm_core::snapshot::{
    Battery, Fan, FilesystemSnapshot, HostInfo, MeasurementScope, SensorsSnapshot, Temperature,
};
use nysm_core::sockets::{Protocol, SocketEntry, SocketOwner, SocketState};

use super::parse;
use crate::error::{CResult, CollectError};
use crate::{FilesystemProvider, Platform, PressureResource, ProcessDetails, ProcessScan};

/// Reads a whole file into a reused buffer.
fn read_into(path: &str, buf: &mut String) -> CResult<()> {
    buf.clear();
    fs::File::open(path)
        .and_then(|mut f| f.read_to_string(buf))
        .map(|_| ())
        .map_err(|e| CollectError::from_io(path, &e))
}

fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub struct LinuxPlatform {
    buf: String,
    ticks_per_s: u64,
    page_size: u64,
    iface_kinds: HashMap<String, InterfaceKind>,
    disk_kinds: HashMap<String, DiskKind>,
    hw_disks: HashMap<(u32, u32), bool>,
    cg_paths: Vec<String>,
    cg_truncated: u32,
    cg_scans: u32,
    cg_rewalk: bool,
    cg_main: HashMap<String, Option<String>>,
    users: Option<HashMap<u32, String>>,
    scope: Option<MeasurementScope>,
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxPlatform {
    pub fn new() -> Self {
        // SAFETY: sysconf has no preconditions.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        LinuxPlatform {
            buf: String::with_capacity(64 * 1024),
            ticks_per_s: if ticks > 0 { ticks as u64 } else { 100 },
            page_size: if page > 0 { page as u64 } else { 4096 },
            iface_kinds: HashMap::new(),
            disk_kinds: HashMap::new(),
            hw_disks: HashMap::new(),
            cg_paths: Vec::new(),
            cg_truncated: 0,
            cg_scans: 0,
            cg_rewalk: false,
            cg_main: HashMap::new(),
            users: None,
            scope: None,
        }
    }

    /// Whole hardware disk (not a partition, dm/md or loop device). Cached.
    fn is_hardware_disk(&mut self, major: u32, minor: u32) -> bool {
        *self.hw_disks.entry((major, minor)).or_insert_with(|| {
            let base = format!("/sys/dev/block/{major}:{minor}");
            Path::new(&format!("{base}/device")).exists()
                && !Path::new(&format!("{base}/partition")).exists()
        })
    }

    fn read(&mut self, path: &str) -> CResult<&str> {
        read_into(path, &mut self.buf)?;
        Ok(&self.buf)
    }
}

fn classify_interface(name: &str) -> InterfaceKind {
    let base = format!("/sys/class/net/{name}");
    let has = |p: &str| Path::new(&format!("{base}/{p}")).exists();
    let if_type: Option<u32> = read_trim(format!("{base}/type")).and_then(|s| s.parse().ok());
    if name == "lo" || if_type == Some(772) {
        InterfaceKind::Loopback
    } else if has("bridge") {
        InterfaceKind::Bridge
    } else if has("tun_flags") || if_type == Some(65534) || name.starts_with("wg") {
        InterfaceKind::Tunnel
    } else if has("wireless") || has("phy80211") {
        InterfaceKind::Wireless
    } else if name.starts_with("veth") {
        InterfaceKind::VirtualEthernet
    } else if has("device") {
        InterfaceKind::Physical
    } else if Path::new(&base).exists() {
        InterfaceKind::Virtual
    } else {
        InterfaceKind::Unknown
    }
}

fn classify_disk(name: &str) -> DiskKind {
    if name.starts_with("loop") {
        return DiskKind::Loop;
    }
    if name.starts_with("zram") || name.starts_with("ram") {
        return DiskKind::Memory;
    }
    if name.starts_with("dm-") || name.starts_with("md") {
        return DiskKind::Stacked;
    }
    if Path::new(&format!("/sys/class/block/{name}/partition")).exists() {
        return DiskKind::Partition;
    }
    if Path::new(&format!("/sys/block/{name}/device")).exists() {
        DiskKind::Disk
    } else {
        DiskKind::Other
    }
}

fn detect_scope(kernel: Option<&str>) -> MeasurementScope {
    if kernel.is_some_and(|k| k.to_ascii_lowercase().contains("microsoft")) {
        return MeasurementScope::Wsl;
    }
    if Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists() {
        return MeasurementScope::Container;
    }
    let cpu_flags_hv = fs::read_to_string("/proc/cpuinfo")
        .map(|s| {
            s.lines()
                .find(|l| l.starts_with("flags"))
                .is_some_and(|l| l.split_ascii_whitespace().any(|f| f == "hypervisor"))
        })
        .unwrap_or(false);
    let dmi = read_trim("/sys/class/dmi/id/sys_vendor")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let vm_vendor = [
        "qemu",
        "vmware",
        "innotek",
        "xen",
        "microsoft corporation",
        "amazon ec2",
        "google",
        "parallels",
    ]
    .iter()
    .any(|v| dmi.contains(v));
    if cpu_flags_hv || vm_vendor || Path::new("/sys/hypervisor/type").exists() {
        return MeasurementScope::VirtualMachine;
    }
    MeasurementScope::Host
}

impl Platform for LinuxPlatform {
    fn host_info(&mut self) -> HostInfo {
        let kernel = read_trim("/proc/sys/kernel/osrelease");
        let scope = *self
            .scope
            .get_or_insert_with(|| detect_scope(kernel.as_deref()));
        let (os_name, os_version) = fs::read_to_string("/etc/os-release")
            .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
            .map(|t| parse::os_release(&t))
            .unwrap_or((None, None));
        HostInfo {
            hostname: read_trim("/proc/sys/kernel/hostname"),
            os: os_name.unwrap_or_else(|| "Linux".into()),
            os_version,
            kernel,
            arch: std::env::consts::ARCH.into(),
            boot_id: read_trim("/proc/sys/kernel/random/boot_id"),
            uptime_s: read_trim("/proc/uptime").and_then(|s| {
                s.split_ascii_whitespace()
                    .next()
                    .and_then(|v| v.parse().ok())
            }),
            scope,
        }
    }

    fn clock_ticks_per_s(&self) -> u64 {
        self.ticks_per_s
    }

    fn page_size(&self) -> u64 {
        self.page_size
    }

    fn cpu_times(&mut self) -> CResult<RawCpu> {
        parse::stat(self.read("/proc/stat")?)
    }

    fn cpu_frequencies(&mut self, ids: &[u32]) -> Vec<CResult<f64>> {
        ids.iter()
            .map(|id| {
                let path = format!("/sys/devices/system/cpu/cpu{id}/cpufreq/scaling_cur_freq");
                let khz: u64 = read_into(&path, &mut self.buf).and_then(|_| {
                    self.buf
                        .trim()
                        .parse()
                        .map_err(|_| CollectError::Failed(format!("{path}: not a number")))
                })?;
                Ok(khz as f64 / 1000.0)
            })
            .collect()
    }

    fn load(&mut self) -> CResult<RawLoad> {
        parse::loadavg(self.read("/proc/loadavg")?)
    }

    fn memory(&mut self) -> CResult<RawMemory> {
        Ok(parse::meminfo(self.read("/proc/meminfo")?))
    }

    fn swap_activity(&mut self) -> CResult<RawSwapActivity> {
        parse::vmstat_swap(self.read("/proc/vmstat")?)
    }

    fn pressure(&mut self, resource: PressureResource) -> CResult<RawPressure> {
        let path = match resource {
            PressureResource::Cpu => "/proc/pressure/cpu",
            PressureResource::Memory => "/proc/pressure/memory",
            PressureResource::Io => "/proc/pressure/io",
        };
        match self.read(path) {
            Ok(t) => parse::pressure(t),
            // PSI needs CONFIG_PSI and may be disabled with psi=0 (EOPNOTSUPP).
            Err(CollectError::Failed(_)) | Err(CollectError::Unsupported(_)) => {
                Err(CollectError::Unsupported(
                    "PSI not available (kernel < 4.20, CONFIG_PSI off, or psi=0)".into(),
                ))
            }
            Err(e) => Err(e),
        }
    }

    fn interfaces(&mut self) -> CResult<Vec<RawInterface>> {
        let lines = parse::net_dev(self.read("/proc/net/dev")?)?;
        // Drop cache entries for interfaces that disappeared (hotplug).
        self.iface_kinds
            .retain(|k, _| lines.iter().any(|l| &l.name == k));
        Ok(lines
            .into_iter()
            .map(|l| {
                let kind = *self
                    .iface_kinds
                    .entry(l.name.clone())
                    .or_insert_with(|| classify_interface(&l.name));
                let up = match read_trim(format!("/sys/class/net/{}/operstate", l.name)).as_deref()
                {
                    Some("up") => Some(true),
                    Some("down") | Some("lowerlayerdown") | Some("notpresent") => Some(false),
                    _ => None,
                };
                RawInterface {
                    name: l.name,
                    kind,
                    up,
                    rx_bytes: l.rx[0],
                    rx_packets: l.rx[1],
                    rx_errors: l.rx[2],
                    rx_dropped: l.rx[3],
                    tx_bytes: l.tx[0],
                    tx_packets: l.tx[1],
                    tx_errors: l.tx[2],
                    tx_dropped: l.tx[3],
                }
            })
            .collect())
    }

    fn disks(&mut self) -> CResult<Vec<RawDisk>> {
        let lines = parse::diskstats(self.read("/proc/diskstats")?)?;
        self.disk_kinds
            .retain(|k, _| lines.iter().any(|l| &l.name == k));
        Ok(lines
            .into_iter()
            .map(|l| {
                let kind = *self
                    .disk_kinds
                    .entry(l.name.clone())
                    .or_insert_with(|| classify_disk(&l.name));
                let f = l.fields;
                RawDisk {
                    name: l.name,
                    major: l.major,
                    minor: l.minor,
                    kind,
                    reads: f[0],
                    sectors_read: f[2],
                    read_ms: f[3],
                    writes: f[4],
                    sectors_written: f[6],
                    write_ms: f[7],
                    in_flight: f[8],
                    io_ms: f[9],
                    weighted_io_ms: f[10],
                }
            })
            .collect())
    }

    fn processes(&mut self) -> CResult<ProcessScan> {
        let dir = fs::read_dir("/proc").map_err(|e| CollectError::from_io("/proc", &e))?;
        let mut processes = Vec::with_capacity(512);
        let mut unreadable = 0;
        let mut path = String::with_capacity(32);
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            path.clear();
            path.push_str("/proc/");
            path.push_str(name.to_str().unwrap_or_default());
            let uid = fs::metadata(&path).ok().map(|m| m.uid());
            let base_len = path.len();
            path.push_str("/stat");
            let stat =
                match read_into(&path, &mut self.buf).and_then(|_| parse::pid_stat(&self.buf)) {
                    Ok(s) => s,
                    // Exited between readdir and read: not an error.
                    Err(CollectError::Unsupported(_)) | Err(CollectError::Gone) => continue,
                    Err(_) => {
                        unreadable += 1;
                        continue;
                    }
                };
            path.truncate(base_len);
            path.push_str("/io");
            let io = match read_into(&path, &mut self.buf) {
                Ok(()) => parse::pid_io(&self.buf)
                    .map(|(r, w)| RawProcessIo {
                        read_bytes: r,
                        write_bytes: w,
                    })
                    .map_err(|e| e.status()),
                Err(e) => Err(e.status()),
            };
            processes.push(RawProcess {
                id: ProcessId {
                    pid,
                    start_ticks: stat.start_ticks,
                },
                ppid: stat.ppid,
                name: stat.comm,
                state: stat.state,
                uid,
                utime_ticks: stat.utime,
                stime_ticks: stat.stime,
                threads: stat.threads,
                rss_bytes: stat.rss_pages.saturating_mul(self.page_size),
                virtual_bytes: stat.vsize,
                io,
            });
        }
        Ok(ProcessScan {
            processes,
            unreadable,
        })
    }

    fn own_limits(&mut self) -> CResult<nysm_core::snapshot::OwnLimits> {
        // Inside a container /sys/fs/cgroup is the container's own cgroup
        // and has these files; on a host it is the hierarchy root, which
        // has no limits of its own.
        const ROOT: &str = "/sys/fs/cgroup";
        if !Path::new(&format!("{ROOT}/memory.max")).exists()
            && !Path::new(&format!("{ROOT}/cpu.max")).exists()
        {
            return Err(CollectError::Unsupported(
                "not running in a container (no container limits apply)".into(),
            ));
        }
        let read = |f: &str| fs::read_to_string(format!("{ROOT}/{f}")).ok();
        Ok(nysm_core::snapshot::OwnLimits {
            memory_bytes: read("memory.current").and_then(|t| t.trim().parse().ok()),
            memory_max_bytes: read("memory.max")
                .and_then(|t| parse::cgroup_limit(&t))
                .flatten(),
            memory_high_bytes: read("memory.high")
                .and_then(|t| parse::cgroup_limit(&t))
                .flatten(),
            cpu_limit_cores: read("cpu.max")
                .and_then(|t| parse::cgroup_cpu_max(&t))
                .and_then(|(q, p)| q.map(|q| q as f64 / p.max(1) as f64)),
            pids: read("pids.current").and_then(|t| t.trim().parse().ok()),
            pids_max: read("pids.max")
                .and_then(|t| parse::cgroup_limit(&t))
                .flatten(),
        })
    }

    fn cgroups(&mut self) -> CResult<crate::CgroupScan> {
        const ROOT: &str = "/sys/fs/cgroup";
        if !Path::new(&format!("{ROOT}/cgroup.controllers")).exists() {
            return Err(CollectError::Unsupported(
                "cgroup v2 unified hierarchy not mounted at /sys/fs/cgroup".into(),
            ));
        }
        // Re-walk the tree only every few scans (or when a known group
        // vanished); reading counters of known groups is the cheap part.
        if self.cg_paths.is_empty()
            || self.cg_rewalk
            || self.cg_scans.is_multiple_of(CGROUP_REWALK_EVERY)
        {
            let (paths, truncated) = walk_cgroups(ROOT);
            self.cg_paths = paths;
            self.cg_truncated = truncated;
            self.cg_rewalk = false;
            self.cg_main.retain(|k, _| self.cg_paths.contains(k));
        }
        self.cg_scans = self.cg_scans.wrapping_add(1);
        let found = self.cg_paths.clone();
        let truncated = self.cg_truncated;
        let mut groups = Vec::with_capacity(found.len());
        for rel in found {
            let base = format!("{ROOT}/{rel}");
            let read = |f: &str| fs::read_to_string(format!("{base}/{f}")).ok();
            // A group that disappeared triggers a fresh walk next time;
            // groups with no processes (finished scopes) are skipped.
            let pids_current: Option<u64> = match read("pids.current") {
                Some(t) => t.trim().parse().ok(),
                None if !Path::new(&base).exists() => {
                    self.cg_rewalk = true;
                    continue;
                }
                None => None,
            };
            if pids_current == Some(0) {
                continue;
            }
            let (kind, name) = parse::cgroup_classify(&rel);
            let main_process = match self.cg_main.get(&rel) {
                Some(m) => m.clone(),
                None => {
                    let m = read("cgroup.procs")
                        .and_then(|t| t.lines().next().map(str::to_string))
                        .and_then(|pid| read_trim(format!("/proc/{pid}/comm")));
                    self.cg_main.insert(rel.clone(), m.clone());
                    m
                }
            };
            let (io_r, io_w) = match read("io.stat") {
                Some(t) => {
                    let (r, w) =
                        parse::cgroup_io_stat(&t, |maj, min| self.is_hardware_disk(maj, min));
                    (Some(r), Some(w))
                }
                None => (None, None),
            };
            let psi = |f: &str| {
                read(f)
                    .and_then(|t| parse::pressure(&t).ok())
                    .map(|p| p.some.total_us)
            };
            groups.push(RawCgroup {
                path: rel.clone(),
                kind,
                name,
                main_process,
                cpu_usage_usec: read("cpu.stat")
                    .and_then(|t| parse::cgroup_keyed(&t, "usage_usec")),
                cpu_max: read("cpu.max").and_then(|t| parse::cgroup_cpu_max(&t)),
                memory_current: read("memory.current").and_then(|t| t.trim().parse().ok()),
                memory_max: read("memory.max")
                    .and_then(|t| parse::cgroup_limit(&t))
                    .flatten(),
                memory_high: read("memory.high")
                    .and_then(|t| parse::cgroup_limit(&t))
                    .flatten(),
                pids_current,
                pids_max: read("pids.max")
                    .and_then(|t| parse::cgroup_limit(&t))
                    .flatten(),
                io_read_bytes: io_r,
                io_write_bytes: io_w,
                memory_pressure_some_us: psi("memory.pressure"),
                cpu_pressure_some_us: psi("cpu.pressure"),
            });
        }
        Ok(crate::CgroupScan { groups, truncated })
    }

    fn process_details(
        &mut self,
        pid: u32,
        start_ticks: Option<u64>,
        include_cmdline: bool,
    ) -> CResult<ProcessDetails> {
        let base = format!("/proc/{pid}");
        let stat = read_into(&format!("{base}/stat"), &mut self.buf)
            .and_then(|_| parse::pid_stat(&self.buf))
            .map_err(|e| match e {
                CollectError::Unsupported(_) => CollectError::Gone,
                e => e,
            })?;
        // Guard against PID reuse between listing and inspecting.
        if start_ticks.is_some_and(|t| t != stat.start_ticks) {
            return Err(CollectError::Gone);
        }
        let link = |p: &str| {
            let path = format!("{base}/{p}");
            fs::read_link(&path)
                .map(|l| l.to_string_lossy().into_owned())
                .map_err(|e| CollectError::from_io(&path, &e))
        };
        let cmdline = include_cmdline.then(|| {
            let path = format!("{base}/cmdline");
            fs::read(&path)
                .map(|b| {
                    b.split(|c| *c == 0)
                        .filter(|s| !s.is_empty())
                        .map(|s| String::from_utf8_lossy(s).into_owned())
                        .collect()
                })
                .map_err(|e| CollectError::from_io(&path, &e))
        });
        let cgroup = {
            let path = format!("{base}/cgroup");
            fs::read_to_string(&path)
                .map_err(|e| CollectError::from_io(&path, &e))
                .map(|t| {
                    // Prefer the unified (v2) hierarchy line `0::/path`.
                    t.lines()
                        .find_map(|l| l.strip_prefix("0::"))
                        .or_else(|| t.lines().next().and_then(|l| l.splitn(3, ':').nth(2)))
                        .unwrap_or("")
                        .to_string()
                })
        };
        let open_fds = {
            let path = format!("{base}/fd");
            fs::read_dir(&path)
                .map(|d| d.count() as u32)
                .map_err(|e| CollectError::from_io(&path, &e))
        };
        let read_parsed = |file: &str, f: fn(&str) -> Option<u64>, what: &str| {
            let path = format!("{base}/{file}");
            fs::read_to_string(&path)
                .map_err(|e| CollectError::from_io(&path, &e))
                .and_then(|t| {
                    f(&t).ok_or_else(|| CollectError::Unsupported(format!("no {what} in {path}")))
                })
        };
        Ok(ProcessDetails {
            exe: link("exe"),
            cwd: link("cwd"),
            cmdline,
            cgroup,
            open_fds,
            fd_limit: read_parsed("limits", parse::limits_open_files, "open-files limit"),
            swap_bytes: read_parsed("status", parse::status_vm_swap, "VmSwap"),
        })
    }

    fn user_name(&mut self, uid: u32) -> Option<String> {
        let users = self.users.get_or_insert_with(|| {
            fs::read_to_string("/etc/passwd")
                .map(|t| parse::passwd(&t).into_iter().collect())
                .unwrap_or_default()
        });
        users.get(&uid).cloned()
    }

    fn sockets(&mut self, resolve_owners: bool) -> CResult<Vec<SocketEntry>> {
        let tables = [
            ("/proc/net/tcp", Protocol::Tcp),
            ("/proc/net/tcp6", Protocol::Tcp),
            ("/proc/net/udp", Protocol::Udp),
            ("/proc/net/udp6", Protocol::Udp),
        ];
        let mut lines = Vec::new();
        let mut any = false;
        for (path, proto) in tables {
            match read_into(path, &mut self.buf) {
                Ok(()) => {
                    any = true;
                    lines.extend(
                        parse::net_sockets(&self.buf)?
                            .into_iter()
                            .map(|l| (proto, l)),
                    );
                }
                // IPv6 may be disabled; a missing table is not an error.
                Err(CollectError::Unsupported(_)) => {}
                Err(e) => return Err(e),
            }
        }
        if !any {
            return Err(CollectError::Unsupported(
                "no /proc/net socket tables".into(),
            ));
        }
        let owners = if resolve_owners {
            socket_owners()
        } else {
            HashMap::new()
        };
        // SAFETY: geteuid cannot fail.
        let me = unsafe { libc::geteuid() };
        Ok(lines
            .into_iter()
            .map(|(protocol, l)| {
                let owners = owners.get(&l.inode).cloned().unwrap_or_default();
                let (owner_status, owner_reason) = if !resolve_owners {
                    (
                        Status::Unsupported,
                        Some("owner lookup not requested".to_string()),
                    )
                } else if !owners.is_empty() {
                    (Status::Available, None)
                } else if l.inode == 0 {
                    (
                        Status::Unsupported,
                        Some("held by the kernel (e.g. TIME_WAIT); no process owns it".into()),
                    )
                } else if l.uid != me && me != 0 {
                    (
                        Status::PermissionDenied,
                        Some(format!(
                            "socket uid {}: owner visible only to that user or root/CAP_SYS_PTRACE",
                            l.uid
                        )),
                    )
                } else {
                    (
                        Status::Unsupported,
                        Some(
                            "no visible process holds it (exited, or another PID namespace)".into(),
                        ),
                    )
                };
                let state = match protocol {
                    Protocol::Tcp => SocketState::from_tcp(l.state),
                    Protocol::Udp => SocketState::from_udp(l.state),
                };
                SocketEntry {
                    protocol,
                    local_addr: l.local.0,
                    local_port: l.local.1,
                    remote_addr: l.remote.0,
                    remote_port: l.remote.1,
                    state,
                    uid: l.uid,
                    inode: l.inode,
                    owners,
                    owner_status,
                    owner_reason,
                }
            })
            .collect())
    }

    fn sensors_provider(&self) -> Box<dyn FnMut() -> CResult<SensorsSnapshot> + Send> {
        Box::new(read_sensors)
    }

    fn filesystem_provider(&self) -> Box<dyn FilesystemProvider> {
        Box::new(LinuxFilesystems)
    }

    fn source(&self, id: &str) -> &'static str {
        match id {
            "cpu.usage" | "cpu.logical_cores" => "/proc/stat",
            "cpu.frequency" => "/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq",
            "cpu.load" => "/proc/loadavg",
            "cpu.pressure" => "/proc/pressure/cpu",
            "memory.usage" | "memory.swap" => "/proc/meminfo",
            "memory.swap_activity" => "/proc/vmstat",
            "memory.pressure" => "/proc/pressure/memory",
            "network.interfaces" => "/proc/net/dev + /sys/class/net",
            "storage.devices" => "/proc/diskstats + /sys/block",
            "storage.filesystems" => "/proc/self/mountinfo + statvfs(3)",
            "io.pressure" => "/proc/pressure/io",
            "process.list" => "/proc/<pid>/stat",
            "process.disk_io" => "/proc/<pid>/io",
            "groups.cgroups" => "/sys/fs/cgroup (cgroup v2)",
            "sensors.temperature" | "sensors.fans" => "/sys/class/hwmon",
            "sensors.battery" => "/sys/class/power_supply",
            "gpu.usage" | "gpu.frequency" | "gpu.vram" => "/sys/class/drm/card*",
            _ => "unknown",
        }
    }
}

/// Map socket inode → processes holding it, scanning `/proc/<pid>/fd` of
/// every process we may inspect. Unreadable fd directories are skipped.
fn socket_owners() -> HashMap<u64, Vec<SocketOwner>> {
    let mut map: HashMap<u64, Vec<SocketOwner>> = HashMap::new();
    let Ok(dir) = fs::read_dir("/proc") else {
        return map;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        let mut inodes: Vec<u64> = fds
            .flatten()
            .filter_map(|fd| fs::read_link(fd.path()).ok())
            .filter_map(|l| l.to_str().and_then(parse::socket_inode))
            .collect();
        if inodes.is_empty() {
            continue;
        }
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat"))
            .map_err(|_| ())
            .and_then(|t| parse::pid_stat(&t).map_err(|_| ()))
        else {
            continue;
        };
        inodes.sort_unstable();
        inodes.dedup();
        let owner = SocketOwner {
            id: ProcessId {
                pid,
                start_ticks: stat.start_ticks,
            },
            name: stat.comm,
        };
        for ino in inodes {
            map.entry(ino).or_default().push(owner.clone());
        }
    }
    map
}

/// hwmon temperatures/fans and power_supply batteries.
fn read_sensors() -> CResult<SensorsSnapshot> {
    let mut out = SensorsSnapshot::default();
    let mut chips: Vec<_> = fs::read_dir("/sys/class/hwmon")
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    chips.sort();
    for dir in chips {
        let chip = read_trim(dir.join("name")).unwrap_or_else(|| "unknown".into());
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        files.sort();
        for f in &files {
            if let Some(n) = f
                .strip_prefix("temp")
                .and_then(|r| r.strip_suffix("_input"))
            {
                let input = fs::read_to_string(dir.join(f)).ok();
                let max = fs::read_to_string(dir.join(format!("temp{n}_max"))).ok();
                let crit = fs::read_to_string(dir.join(format!("temp{n}_crit"))).ok();
                let Some((c, high, critical)) = input
                    .as_deref()
                    .and_then(|i| parse::hwmon_temperature(i, max.as_deref(), crit.as_deref()))
                else {
                    continue;
                };
                let label = read_trim(dir.join(format!("temp{n}_label")))
                    .unwrap_or_else(|| format!("temp{n}"));
                out.temperatures.push(Temperature {
                    class: parse::sensor_class(&chip, &label),
                    chip: chip.clone(),
                    label,
                    celsius: c,
                    high_celsius: high,
                    critical_celsius: critical,
                });
            } else if let Some(n) = f.strip_prefix("fan").and_then(|r| r.strip_suffix("_input"))
                && let Some(rpm) = read_trim(dir.join(f)).and_then(|v| v.parse().ok())
            {
                {
                    let label = read_trim(dir.join(format!("fan{n}_label")))
                        .unwrap_or_else(|| format!("fan{n}"));
                    out.fans.push(Fan {
                        chip: chip.clone(),
                        label,
                        rpm,
                    });
                }
            }
        }
    }
    if let Ok(d) = fs::read_dir("/sys/class/power_supply") {
        for e in d.flatten() {
            let p = e.path();
            if read_trim(p.join("type")).as_deref() != Some("Battery") {
                continue;
            }
            let num = |f: &str| read_trim(p.join(f)).and_then(|v| v.parse::<f64>().ok());
            let power = num("power_now").map(|uw| uw / 1e6).or_else(|| {
                match (num("current_now"), num("voltage_now")) {
                    (Some(ua), Some(uv)) => Some(ua * uv / 1e12),
                    _ => None,
                }
            });
            out.batteries.push(Battery {
                name: e.file_name().to_string_lossy().into_owned(),
                capacity_pct: num("capacity"),
                status: read_trim(p.join("status")).unwrap_or_else(|| "Unknown".into()),
                power_watts: power,
            });
        }
    }
    // GPUs: one entry per DRM card (not connectors like card1-DP-1).
    if let Ok(d) = fs::read_dir("/sys/class/drm") {
        let mut cards: Vec<_> = d
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                n.starts_with("card") && n[4..].chars().all(|c| c.is_ascii_digit()) && n.len() > 4
            })
            .collect();
        cards.sort();
        for card in cards {
            let base = Path::new("/sys/class/drm").join(&card);
            let driver = fs::read_link(base.join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "unknown".into());
            let read = |f: &str| fs::read_to_string(base.join(f)).ok();
            out.gpus.push(parse::drm_gpu(&card, &driver, &read));
        }
    }
    if out.temperatures.is_empty()
        && out.fans.is_empty()
        && out.batteries.is_empty()
        && out.gpus.is_empty()
    {
        return Err(CollectError::Unsupported(
            "no hwmon sensors or batteries exposed (common in VMs and containers)".into(),
        ));
    }
    Ok(out)
}

/// Re-walk the cgroup tree every N scans (new groups appear within N
/// process intervals; known groups are read every scan).
const CGROUP_REWALK_EVERY: u32 = 5;

/// Find cgroup leaf units of interest, with explicit bounds: cgroup trees
/// can be large (k8s nodes), so the walk is never unbounded.
fn walk_cgroups(root: &str) -> (Vec<String>, u32) {
    const MAX_DEPTH: usize = 8;
    const MAX_VISITED: usize = 5000;
    const MAX_GROUPS: usize = 1000;
    let mut found = Vec::new();
    let mut visited = 0usize;
    let mut truncated = 0u32;
    let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];
    while let Some((rel, depth)) = stack.pop() {
        let dir = if rel.is_empty() {
            root.to_string()
        } else {
            format!("{root}/{rel}")
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            visited += 1;
            if visited > MAX_VISITED {
                break;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let is_unit = name.ends_with(".scope") || name.ends_with(".service");
            // The per-user manager is a container of app units, not a leaf.
            let is_user_manager = name.starts_with("user@") && name.ends_with(".service");
            if is_unit && !is_user_manager {
                if found.len() < MAX_GROUPS {
                    found.push(child);
                } else {
                    truncated += 1;
                }
            } else if depth + 1 < MAX_DEPTH {
                stack.push((child, depth + 1));
            }
        }
    }
    (found, truncated)
}

/// Filesystem types that never represent user-visible storage capacity.
const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "securityfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "debugfs",
    "tracefs",
    "mqueue",
    "hugetlbfs",
    "configfs",
    "fusectl",
    "binfmt_misc",
    "autofs",
    "efivarfs",
    "nsfs",
    "ramfs",
    "rpc_pipefs",
    "selinuxfs",
    "squashfs",
    "overlay",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.snapfuse",
    "fuse.lxcfs",
    "nfsd",
    "devfs",
    "zramfs",
];

struct LinuxFilesystems;

// statvfs field widths vary by target (u32 on some 32-bit ABIs), so the
// `as u64` casts are needed even where clippy sees them as no-ops.
#[allow(clippy::unnecessary_cast)]
impl FilesystemProvider for LinuxFilesystems {
    fn filesystems(&mut self) -> CResult<Vec<FilesystemSnapshot>> {
        let text = fs::read_to_string("/proc/self/mountinfo")
            .map_err(|e| CollectError::from_io("/proc/self/mountinfo", &e))?;
        let mounts = parse::mountinfo(&text);
        let mut by_dev: Vec<((u32, u32), FilesystemSnapshot)> = Vec::new();
        for m in mounts {
            // An overlay root (containers) is the only overlay worth showing.
            let pseudo = PSEUDO_FS.contains(&m.fs_type.as_str())
                && !(m.fs_type == "overlay" && m.mount_point == "/");
            if pseudo {
                continue;
            }
            let dev = (m.major, m.minor);
            if let Some((_, existing)) = by_dev.iter_mut().find(|(d, _)| *d == dev) {
                existing.also_mounted_at.push(m.mount_point);
                continue;
            }
            let Ok(c) = CString::new(m.mount_point.as_bytes()) else {
                continue;
            };
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            // SAFETY: valid NUL-terminated path and out-pointer. This call may
            // block on network filesystems; it runs on a worker thread.
            if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
                continue;
            }
            let frsize = st.f_frsize as u64;
            let total = (st.f_blocks as u64).saturating_mul(frsize);
            if total == 0 {
                continue;
            }
            let free = (st.f_bfree as u64).saturating_mul(frsize);
            let avail = (st.f_bavail as u64).saturating_mul(frsize).min(free);
            let used = total.saturating_sub(free);
            let denom = used + avail;
            by_dev.push((
                dev,
                FilesystemSnapshot {
                    mount_point: m.mount_point,
                    source: m.source,
                    fs_type: m.fs_type,
                    read_only: m.read_only || (st.f_flag & libc::ST_RDONLY) != 0,
                    total_bytes: total,
                    used_bytes: used,
                    available_bytes: avail,
                    used_pct: if denom == 0 {
                        0.0
                    } else {
                        used as f64 / denom as f64 * 100.0
                    },
                    also_mounted_at: Vec::new(),
                    growth_bytes_per_hour: None,
                    full_in_hours: None,
                },
            ));
        }
        Ok(by_dev.into_iter().map(|(_, f)| f).collect())
    }
}
