//! Pure parsers for Linux procfs text formats. Tolerant of unknown fields,
//! extra whitespace and missing optional fields; strict about the fields a
//! metric depends on.

use nysm_core::raw::{
    CpuTimes, RawCpu, RawLoad, RawMemory, RawPressure, RawPsiLine, RawSwapActivity,
};

use crate::error::{CResult, CollectError};

fn bad(what: &str, detail: impl std::fmt::Display) -> CollectError {
    CollectError::Failed(format!("malformed {what}: {detail}"))
}

fn parse_cpu_fields<'a>(mut it: impl Iterator<Item = &'a str>) -> Option<CpuTimes> {
    let mut next = || -> Option<u64> { it.next().map(|s| s.parse().ok()).unwrap_or(Some(0)) };
    // Fields beyond `idle` were added over kernel history; absent => 0.
    let user = next()?;
    let nice = next()?;
    let system = next()?;
    let idle = next()?;
    Some(CpuTimes {
        user,
        nice,
        system,
        idle,
        iowait: next()?,
        irq: next()?,
        softirq: next()?,
        steal: next()?,
        guest: next()?,
        guest_nice: next()?,
    })
}

/// `/proc/stat`: aggregate `cpu` line plus `cpuN` lines for online CPUs.
pub fn stat(text: &str) -> CResult<RawCpu> {
    let mut out = RawCpu::default();
    let mut saw_total = false;
    for line in text.lines() {
        let mut it = line.split_ascii_whitespace();
        let Some(key) = it.next() else { continue };
        let Some(rest) = key.strip_prefix("cpu") else {
            continue;
        };
        let required = line.split_ascii_whitespace().count() >= 5;
        if !required {
            return Err(bad("/proc/stat", format!("short cpu line {key:?}")));
        }
        let times = parse_cpu_fields(it)
            .ok_or_else(|| bad("/proc/stat", format!("non-numeric {key:?}")))?;
        if rest.is_empty() {
            out.total = times;
            saw_total = true;
        } else if let Ok(id) = rest.parse::<u32>() {
            out.per_cpu.push((id, times));
        }
    }
    if !saw_total {
        return Err(bad("/proc/stat", "no aggregate cpu line"));
    }
    Ok(out)
}

/// `/proc/loadavg`: `0.52 0.58 0.59 2/1093 12345`.
pub fn loadavg(text: &str) -> CResult<RawLoad> {
    let mut it = text.split_ascii_whitespace();
    let mut f = || -> CResult<f64> {
        it.next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("/proc/loadavg", text.trim()))
    };
    let (one, five, fifteen) = (f()?, f()?, f()?);
    let (runnable, tasks) = text
        .split_ascii_whitespace()
        .nth(3)
        .and_then(|s| s.split_once('/'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((0, 0));
    Ok(RawLoad {
        one,
        five,
        fifteen,
        runnable,
        tasks,
    })
}

/// `/proc/meminfo`. Values are in kibibytes despite the `kB` label.
pub fn meminfo(text: &str) -> RawMemory {
    let mut m = RawMemory::default();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let mut parts = rest.split_ascii_whitespace();
        let Some(Ok(v)) = parts.next().map(str::parse::<u64>) else {
            continue;
        };
        let bytes = match parts.next() {
            Some("kB") => v.saturating_mul(1024),
            None => v,
            Some(_) => continue,
        };
        let slot = match key.trim() {
            "MemTotal" => &mut m.total,
            "MemFree" => &mut m.free,
            "MemAvailable" => &mut m.available,
            "Buffers" => &mut m.buffers,
            "Cached" => &mut m.cached,
            "SReclaimable" => &mut m.s_reclaimable,
            "Shmem" => &mut m.shmem,
            "Dirty" => &mut m.dirty,
            "SwapTotal" => &mut m.swap_total,
            "SwapFree" => &mut m.swap_free,
            "SwapCached" => &mut m.swap_cached,
            _ => continue,
        };
        *slot = Some(bytes);
    }
    m
}

/// `/proc/vmstat` swap page counters.
pub fn vmstat_swap(text: &str) -> CResult<RawSwapActivity> {
    let (mut pin, mut pout) = (None, None);
    for line in text.lines() {
        let mut it = line.split_ascii_whitespace();
        match (it.next(), it.next().and_then(|v| v.parse::<u64>().ok())) {
            (Some("pswpin"), Some(v)) => pin = Some(v),
            (Some("pswpout"), Some(v)) => pout = Some(v),
            _ => {}
        }
    }
    match (pin, pout) {
        (Some(pages_in), Some(pages_out)) => Ok(RawSwapActivity {
            pages_in,
            pages_out,
        }),
        _ => Err(CollectError::Unsupported(
            "pswpin/pswpout not in /proc/vmstat".into(),
        )),
    }
}

fn psi_line(rest: &str) -> Option<RawPsiLine> {
    let (mut a10, mut a60, mut a300, mut total) = (None, None, None, None);
    for kv in rest.split_ascii_whitespace() {
        let (k, v) = kv.split_once('=')?;
        match k {
            "avg10" => a10 = v.parse().ok(),
            "avg60" => a60 = v.parse().ok(),
            "avg300" => a300 = v.parse().ok(),
            "total" => total = v.parse().ok(),
            _ => {}
        }
    }
    Some(RawPsiLine {
        avg10: a10?,
        avg60: a60?,
        avg300: a300?,
        total_us: total?,
    })
}

/// `/proc/pressure/{cpu,memory,io}`.
pub fn pressure(text: &str) -> CResult<RawPressure> {
    let mut some = None;
    let mut full = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("some ") {
            some = psi_line(rest);
        } else if let Some(rest) = line.strip_prefix("full ") {
            full = psi_line(rest);
        }
    }
    let some = some.ok_or_else(|| bad("PSI file", "no `some` line"))?;
    // Kernels report an all-zero `full` line for CPU at the system level;
    // that is "not meaningful", not "zero stall".
    if full.is_some_and(|f| f.total_us == 0 && f.avg300 == 0.0) && some.total_us > 0 {
        full = None;
    }
    Ok(RawPressure { some, full })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetDevLine {
    pub name: String,
    /// rx: bytes packets errs drop fifo frame compressed multicast,
    /// tx: bytes packets errs drop fifo colls carrier compressed.
    pub rx: [u64; 8],
    pub tx: [u64; 8],
}

/// `/proc/net/dev` (two header lines, then `name: 16 counters`).
pub fn net_dev(text: &str) -> CResult<Vec<NetDevLine>> {
    let mut out = Vec::new();
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let nums: Vec<u64> = rest
            .split_ascii_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|e| bad("/proc/net/dev", e))?;
        if nums.len() < 16 {
            return Err(bad(
                "/proc/net/dev",
                format!("{} has {} fields", name.trim(), nums.len()),
            ));
        }
        let mut rx = [0; 8];
        let mut tx = [0; 8];
        rx.copy_from_slice(&nums[0..8]);
        tx.copy_from_slice(&nums[8..16]);
        out.push(NetDevLine {
            name: name.trim().to_string(),
            rx,
            tx,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskStatsLine {
    pub major: u32,
    pub minor: u32,
    pub name: String,
    /// The 11 classic fields (reads .. weighted io ms). Newer kernels append
    /// discard/flush fields, which are ignored here.
    pub fields: [u64; 11],
}

/// `/proc/diskstats`.
pub fn diskstats(text: &str) -> CResult<Vec<DiskStatsLine>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_ascii_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        if parts.len() < 14 {
            return Err(bad("/proc/diskstats", format!("{} fields", parts.len())));
        }
        let num = |s: &str| s.parse::<u64>().map_err(|e| bad("/proc/diskstats", e));
        let mut fields = [0u64; 11];
        for (i, f) in fields.iter_mut().enumerate() {
            *f = num(parts[3 + i])?;
        }
        out.push(DiskStatsLine {
            major: num(parts[0])? as u32,
            minor: num(parts[1])? as u32,
            name: parts[2].to_string(),
            fields,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub major: u32,
    pub minor: u32,
    pub root: String,
    pub mount_point: String,
    pub read_only: bool,
    pub fs_type: String,
    pub source: String,
}

/// Undo mountinfo's octal escaping (`\040` for space, etc).
pub fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 4 <= b.len()
            && b[i + 1] <= b'3'
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/proc/self/mountinfo`: `id parent maj:min root mountpoint opts [optional...] - fstype source superopts`.
pub fn mountinfo(text: &str) -> Vec<MountInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 6 || r.len() < 2 {
            continue;
        }
        let Some((maj, min)) = l[2].split_once(':') else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (maj.parse(), min.parse()) else {
            continue;
        };
        out.push(MountInfo {
            major,
            minor,
            root: unescape_octal(l[3]),
            mount_point: unescape_octal(l[4]),
            read_only: l[5].split(',').any(|o| o == "ro"),
            fs_type: r[0].to_string(),
            source: unescape_octal(r[1]),
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidStat {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub utime: u64,
    pub stime: u64,
    pub threads: u32,
    pub start_ticks: u64,
    pub vsize: u64,
    pub rss_pages: u64,
}

/// `/proc/<pid>/stat`. `comm` may contain spaces and parentheses, so it is
/// delimited by the first `(` and the *last* `)`.
pub fn pid_stat(text: &str) -> CResult<PidStat> {
    let open = text.find('(').ok_or_else(|| bad("pid stat", "no ("))?;
    let close = text.rfind(')').ok_or_else(|| bad("pid stat", "no )"))?;
    if close < open {
        return Err(bad("pid stat", "unbalanced comm"));
    }
    let pid = text[..open]
        .trim()
        .parse()
        .map_err(|e| bad("pid stat", e))?;
    let comm = text[open + 1..close].to_string();
    // Fields after comm, numbered from 3 (state) per proc(5).
    let rest: Vec<&str> = text[close + 1..].split_ascii_whitespace().collect();
    let field = |n: usize| -> CResult<u64> {
        rest.get(n - 3)
            .ok_or_else(|| bad("pid stat", format!("missing field {n}")))?
            .parse::<i64>()
            .map(|v| v.max(0) as u64)
            .map_err(|e| bad("pid stat", e))
    };
    Ok(PidStat {
        pid,
        comm,
        state: rest.first().and_then(|s| s.chars().next()).unwrap_or('?'),
        ppid: field(4)? as u32,
        utime: field(14)?,
        stime: field(15)?,
        threads: field(20)? as u32,
        start_ticks: field(22)?,
        vsize: field(23)?,
        rss_pages: field(24)?,
    })
}

/// `/proc/<pid>/io`: returns (read_bytes, write_bytes).
pub fn pid_io(text: &str) -> CResult<(u64, u64)> {
    let (mut r, mut w) = (None, None);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            match k {
                "read_bytes" => r = v.trim().parse().ok(),
                "write_bytes" => w = v.trim().parse().ok(),
                _ => {}
            }
        }
    }
    Ok((
        r.ok_or_else(|| bad("pid io", "read_bytes"))?,
        w.ok_or_else(|| bad("pid io", "write_bytes"))?,
    ))
}

/// `/etc/os-release`: returns (NAME or PRETTY_NAME, VERSION_ID).
pub fn os_release(text: &str) -> (Option<String>, Option<String>) {
    let mut pretty = None;
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
        match k.trim() {
            "PRETTY_NAME" => pretty = Some(v),
            "NAME" => name = Some(v),
            "VERSION_ID" => version = Some(v),
            _ => {}
        }
    }
    (pretty.or(name), version)
}

/// `/etc/passwd` → (uid, name). Ignores NSS sources (LDAP, sssd).
/// Soft "Max open files" from `/proc/<pid>/limits`; `u64::MAX` = unlimited.
pub fn limits_open_files(text: &str) -> Option<u64> {
    let rest = text
        .lines()
        .find_map(|l| l.strip_prefix("Max open files"))?;
    let soft = rest.split_whitespace().next()?;
    if soft == "unlimited" {
        Some(u64::MAX)
    } else {
        soft.parse().ok()
    }
}

/// `VmSwap` from `/proc/<pid>/status`, in bytes. Absent for kernel threads.
pub fn status_vm_swap(text: &str) -> Option<u64> {
    let rest = text.lines().find_map(|l| l.strip_prefix("VmSwap:"))?;
    let mut it = rest.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    match it.next() {
        Some("kB") | None => Some(n * 1024),
        _ => None,
    }
}

pub fn passwd(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split(':');
            let name = it.next()?;
            let _pw = it.next()?;
            let uid = it.next()?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_limits_and_swap() {
        let limits = "Limit                     Soft Limit           Hard Limit           Units     \n\
                      Max cpu time              unlimited            unlimited            seconds   \n\
                      Max open files            1024                 524288               files     \n";
        assert_eq!(limits_open_files(limits), Some(1024));
        assert_eq!(
            limits_open_files(
                "Max open files            unlimited            unlimited            files\n"
            ),
            Some(u64::MAX)
        );
        assert_eq!(limits_open_files("nothing"), None);
        let status = "Name:\tbash\nVmRSS:\t   5000 kB\nVmSwap:\t     12 kB\n";
        assert_eq!(status_vm_swap(status), Some(12 * 1024));
        assert_eq!(status_vm_swap("Name:\tkthreadd\n"), None);
    }

    const STAT: &str = "cpu  4705 356 584 3699 23 23 0 0 0 0\n\
cpu0 1393280 32966 572056 13343292 6130 0 17875 0 23933 0\n\
cpu3   100 0 50 1000 0 0 0 0 0 0\n\
intr 114930548 113199788 3 0 5 263 0 4 [...]\n\
ctxt 1990473\nbtime 1062191376\n";

    #[test]
    fn stat_parses_total_and_sparse_cores() {
        let r = stat(STAT).unwrap();
        assert_eq!(r.total.user, 4705);
        assert_eq!(r.total.idle, 3699);
        assert_eq!(r.per_cpu.len(), 2);
        assert_eq!(r.per_cpu[1].0, 3);
        assert_eq!(r.per_cpu[0].1.guest, 23933);
    }

    #[test]
    fn stat_old_kernel_without_steal_or_guest() {
        let r = stat("cpu 1 2 3 4\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(r.total.idle, 4);
        assert_eq!(r.total.steal, 0);
    }

    #[test]
    fn stat_malformed() {
        assert!(stat("cpu a b c d\n").is_err());
        assert!(stat("cpu 1 2\n").is_err());
        assert!(stat("intr 5\n").is_err());
        assert!(stat("").is_err());
    }

    #[test]
    fn loadavg_parses() {
        let l = loadavg("0.52 0.58 0.59 2/1093 12345\n").unwrap();
        assert_eq!(l.one, 0.52);
        assert_eq!((l.runnable, l.tasks), (2, 1093));
        assert!(loadavg("garbage").is_err());
    }

    #[test]
    fn meminfo_units_and_unknown_fields() {
        let m = meminfo(
            "MemTotal:       16000 kB\nMemFree:  100 kB\nMemAvailable:   8000 kB\n\
             HugePages_Total:       0\nNewFieldFrom2030: 7 kB\nCached: x kB\n",
        );
        assert_eq!(m.total, Some(16000 * 1024));
        assert_eq!(m.available, Some(8000 * 1024));
        assert_eq!(m.cached, None);
        assert_eq!(m.swap_total, None);
    }

    #[test]
    fn vmstat_swap_counters() {
        let s = vmstat_swap("nr_free_pages 1\npswpin 10\npswpout 20\n").unwrap();
        assert_eq!((s.pages_in, s.pages_out), (10, 20));
        assert!(matches!(
            vmstat_swap("x 1\n"),
            Err(CollectError::Unsupported(_))
        ));
    }

    #[test]
    fn psi_parses_and_drops_meaningless_full() {
        let p = pressure(
            "some avg10=1.50 avg60=0.32 avg300=0.15 total=29796010\n\
             full avg10=0.00 avg60=0.25 avg300=0.12 total=25411700\n",
        )
        .unwrap();
        assert_eq!(p.some.avg10, 1.5);
        assert_eq!(p.full.unwrap().total_us, 25411700);
        let cpu = pressure(
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=5\n\
             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        )
        .unwrap();
        assert!(cpu.full.is_none());
        assert!(pressure("nonsense").is_err());
    }

    #[test]
    fn net_dev_parses() {
        let t = "Inter-|   Receive |  Transmit\n face |bytes ...\n\
            lo: 108067745  128224    0    0    0     0          0         0 108067745  128224    0    0    0     0       0          0\n\
          eno1:1793736268 1615786    0  125    0     0          0    256339 227460492  535784    0    0    0     0       0          0\n";
        let v = net_dev(t).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].name, "eno1");
        assert_eq!(v[1].rx[0], 1793736268);
        assert_eq!(v[1].rx[3], 125);
        assert_eq!(v[1].tx[0], 227460492);
        assert!(net_dev("a\nb\nx: 1 2 3\n").is_err());
    }

    #[test]
    fn diskstats_parses_with_extra_fields() {
        let t = "   7       0 loop0 564 0 68838 503 0 0 0 0 0 488 503 0 0 0 0 0 0\n\
                 259       0 nvme0n1 100 5 2048 50 10 0 80 20 1 300 70\n";
        let v = diskstats(t).unwrap();
        assert_eq!(v[1].name, "nvme0n1");
        assert_eq!(v[1].fields[0], 100);
        assert_eq!(v[1].fields[8], 1);
        assert_eq!(v[1].fields[9], 300);
        assert!(diskstats("1 2 sda 1 2\n").is_err());
    }

    #[test]
    fn mountinfo_parses_optional_fields_and_escapes() {
        let t = "36 35 98:0 /mnt1 /mnt/my\\040disk rw,noatime master:1 - ext3 /dev/root rw,errors=continue\n\
                 29 33 0:26 / /proc rw,nosuid shared:13 - proc proc rw\n\
                 40 33 8:1 / /boot ro,relatime - vfat /dev/sda1 rw\n";
        let m = mountinfo(t);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].mount_point, "/mnt/my disk");
        assert_eq!((m[0].major, m[0].minor), (98, 0));
        assert_eq!(m[0].fs_type, "ext3");
        assert!(m[2].read_only);
        assert!(!m[0].read_only);
    }

    #[test]
    fn pid_stat_handles_hostile_comm() {
        let t = "1234 (evil) name (x) S) R 1 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 7 0 98765 1000000 300 18446744073709551615";
        let s = pid_stat(t).unwrap();
        assert_eq!(s.pid, 1234);
        assert_eq!(s.comm, "evil) name (x) S");
        assert_eq!(s.state, 'R');
        assert_eq!(s.ppid, 1);
        assert_eq!((s.utime, s.stime), (250, 50));
        assert_eq!(s.threads, 7);
        assert_eq!(s.start_ticks, 98765);
        assert_eq!(s.vsize, 1000000);
        assert_eq!(s.rss_pages, 300);
        assert!(pid_stat("1234 (truncated").is_err());
        assert!(pid_stat("1234 (x) R 1").is_err());
    }

    #[test]
    fn pid_io_parses() {
        let t = "rchar: 1\nwchar: 2\nsyscr: 3\nsyscw: 4\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n";
        assert_eq!(pid_io(t).unwrap(), (4096, 8192));
        assert!(pid_io("rchar: 1\n").is_err());
    }

    #[test]
    fn os_release_and_passwd() {
        let (n, v) = os_release(
            "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\n",
        );
        assert_eq!(n.as_deref(), Some("Ubuntu 24.04.3 LTS"));
        assert_eq!(v.as_deref(), Some("24.04"));
        let p = passwd(
            "root:x:0:0:root:/root:/bin/bash\n# comment\nbroken\nalice:x:1000:1000::/home/a:/bin/sh\n",
        );
        assert_eq!(
            p,
            vec![(0, "root".to_string()), (1000, "alice".to_string())]
        );
    }
}

/// One row of `/proc/net/{tcp,tcp6,udp,udp6}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetSocketLine {
    pub local: (std::net::IpAddr, u16),
    pub remote: (std::net::IpAddr, u16),
    pub state: u8,
    pub uid: u32,
    pub inode: u64,
}

/// Kernel prints each 32-bit address word as hex of its in-memory value,
/// so converting with native byte order recovers the network-order bytes.
fn hex_addr(s: &str) -> Option<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let word = |w: &str| u32::from_str_radix(w, 16).ok().map(u32::to_ne_bytes);
    match s.len() {
        8 => Some(IpAddr::V4(Ipv4Addr::from(word(s)?))),
        32 => {
            let mut b = [0u8; 16];
            for i in 0..4 {
                b[i * 4..i * 4 + 4].copy_from_slice(&word(&s[i * 8..i * 8 + 8])?);
            }
            let v6 = Ipv6Addr::from(b);
            // Show IPv4-mapped addresses as plain IPv4.
            Some(v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4))
        }
        _ => None,
    }
}

fn hex_endpoint(s: &str) -> Option<(std::net::IpAddr, u16)> {
    let (a, p) = s.split_once(':')?;
    Some((hex_addr(a)?, u16::from_str_radix(p, 16).ok()?))
}

pub fn net_sockets(text: &str) -> CResult<Vec<NetSocketLine>> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_ascii_whitespace().collect();
        if f.is_empty() {
            continue;
        }
        if f.len() < 10 {
            return Err(bad("/proc/net socket table", format!("{} fields", f.len())));
        }
        let parsed = (|| {
            Some(NetSocketLine {
                local: hex_endpoint(f[1])?,
                remote: hex_endpoint(f[2])?,
                state: u8::from_str_radix(f[3], 16).ok()?,
                uid: f[7].parse().ok()?,
                inode: f[9].parse().ok()?,
            })
        })();
        out.push(parsed.ok_or_else(|| bad("/proc/net socket table", line.trim()))?);
    }
    Ok(out)
}

/// `socket:[12345]` → 12345.
pub fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

#[cfg(test)]
mod socket_tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn parses_ipv4_and_ipv6_tables() {
        let v4 = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
           0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 270557 1 0000000000000000 100 0 0 10 0\n";
        let v = net_sockets(v4).unwrap();
        if cfg!(target_endian = "little") {
            assert_eq!(v[0].local, ("127.0.0.1".parse::<IpAddr>().unwrap(), 3000));
        }
        assert_eq!((v[0].state, v[0].uid, v[0].inode), (0x0A, 1000, 270557));

        let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
           0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 9 1 0\n\
           1: 0000000000000000FFFF00000100007F:0050 0000000000000000FFFF00000100007F:D431 01 00000000:00000000 00:00000000 00000000     0        0 10 1 0\n";
        let v = net_sockets(v6).unwrap();
        if cfg!(target_endian = "little") {
            assert_eq!(v[0].local.0, "::1".parse::<IpAddr>().unwrap());
            assert_eq!(v[1].local.0, "127.0.0.1".parse::<IpAddr>().unwrap());
        }
        assert_eq!(v[0].local.1, 8080);
        assert_eq!(v[1].remote.1, 0xD431);
    }

    #[test]
    fn malformed_rows_are_errors() {
        assert!(net_sockets("header\n 0: zz:0 00000000:0000 0A 0 0 0 0 0 1\n").is_err());
        assert!(net_sockets("header\n 0: short\n").is_err());
        assert!(net_sockets("header only\n").unwrap().is_empty());
    }

    #[test]
    fn socket_links() {
        assert_eq!(socket_inode("socket:[3486886]"), Some(3486886));
        assert_eq!(socket_inode("pipe:[3486886]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }
}

// ------------------------------------------------------------ cgroup v2

/// Value of `key` in a flat-keyed file such as `cpu.stat`.
pub fn cgroup_keyed(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let mut it = l.split_ascii_whitespace();
        (it.next()? == key)
            .then(|| it.next()?.parse().ok())
            .flatten()
    })
}

/// `memory.max`-style value: `max` = unlimited (`Some(None)`).
pub fn cgroup_limit(text: &str) -> Option<Option<u64>> {
    match text.trim() {
        "max" => Some(None),
        v => v.parse().ok().map(Some),
    }
}

/// `cpu.max`: "quota period" with quota possibly `max`.
pub fn cgroup_cpu_max(text: &str) -> Option<(Option<u64>, u64)> {
    let mut it = text.split_ascii_whitespace();
    let quota = match it.next()? {
        "max" => None,
        q => Some(q.parse().ok()?),
    };
    Some((quota, it.next()?.parse().ok()?))
}

/// `io.stat`: sum rbytes/wbytes over devices accepted by `include(maj, min)`.
pub fn cgroup_io_stat(text: &str, mut include: impl FnMut(u32, u32) -> bool) -> (u64, u64) {
    let (mut r, mut w) = (0u64, 0u64);
    for line in text.lines() {
        let mut it = line.split_ascii_whitespace();
        let Some((maj, min)) = it.next().and_then(|d| d.split_once(':')) else {
            continue;
        };
        let (Ok(maj), Ok(min)) = (maj.parse::<u32>(), min.parse::<u32>()) else {
            continue;
        };
        if !include(maj, min) {
            continue;
        }
        for kv in it {
            match kv.split_once('=') {
                Some(("rbytes", v)) => r = r.saturating_add(v.parse().unwrap_or(0)),
                Some(("wbytes", v)) => w = w.saturating_add(v.parse().unwrap_or(0)),
                _ => {}
            }
        }
    }
    (r, w)
}

/// Undo systemd unit-name escaping (`\x2d` → `-`).
pub fn systemd_unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1] == b'x'
            && let Ok(v) = u8::from_str_radix(&s[i + 2..i + 4], 16)
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn short_id(s: &str) -> String {
    s.chars().take(12).collect()
}

/// Classify a cgroup path (relative to the root) into kind + display name.
pub fn cgroup_classify(path: &str) -> (nysm_core::raw::CgroupKind, String) {
    use nysm_core::raw::CgroupKind as K;
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let unescaped = systemd_unescape(leaf);
    let unit = unescaped
        .trim_end_matches(".scope")
        .trim_end_matches(".service");
    for (prefix, runtime) in [
        ("docker-", "docker"),
        ("libpod-", "podman"),
        ("cri-containerd-", "containerd"),
        ("crio-", "cri-o"),
    ] {
        if let Some(id) = unit.strip_prefix(prefix)
            && id.len() >= 12
            && id.chars().all(|c| c.is_ascii_hexdigit())
        {
            return (K::Container, format!("{runtime} {}", short_id(id)));
        }
    }
    if path.contains("kubepods") {
        return (
            K::Container,
            format!("k8s {}", short_id(unit.trim_start_matches("pod"))),
        );
    }
    if path.contains("machine.slice/") {
        return (K::Machine, unit.to_string());
    }
    if path.starts_with("user.slice/") && path.contains("/user@") {
        // e.g. app-gnome-firefox-1234.scope, app-org.gnome.Terminal.slice/vte-spawn-….scope
        let mut name = unit.strip_prefix("app-").unwrap_or(unit).to_string();
        // Snap apps: snap.<snap>.<app>-<uuid>.scope → "<app> (snap)".
        if let Some(rest) = name.strip_prefix("snap.") {
            let app = rest
                .split('-')
                .next()
                .unwrap_or(rest)
                .rsplit('.')
                .next()
                .unwrap_or(rest);
            return (K::UserApp, format!("{app} (snap)"));
        }
        if let Some((head, tail)) = name.rsplit_once('-')
            && tail.chars().all(|c| c.is_ascii_digit() || c == '@')
            && !tail.is_empty()
        {
            name = head.to_string();
        }
        if name.starts_with("vte-spawn") || name.starts_with("tmux-spawn") {
            // Terminal tabs; the shell shows up as the group's main process.
            name = "terminal tab".to_string();
        }
        return (K::UserApp, name);
    }
    if path.starts_with("system.slice/") && leaf.ends_with(".service") {
        return (K::Service, unit.to_string());
    }
    (K::Other, unit.to_string())
}

#[cfg(test)]
mod cgroup_tests {
    use super::*;
    use nysm_core::raw::CgroupKind as K;

    #[test]
    fn keyed_limits_and_cpu_max() {
        let stat = "usage_usec 73771023\nuser_usec 62007748\nsystem_usec 11763274\n";
        assert_eq!(cgroup_keyed(stat, "usage_usec"), Some(73771023));
        assert_eq!(cgroup_keyed(stat, "nope"), None);
        assert_eq!(cgroup_limit("max\n"), Some(None));
        assert_eq!(cgroup_limit("536870912\n"), Some(Some(536870912)));
        assert_eq!(cgroup_limit("garbage"), None);
        assert_eq!(cgroup_cpu_max("max 100000\n"), Some((None, 100000)));
        assert_eq!(cgroup_cpu_max("50000 100000"), Some((Some(50000), 100000)));
        assert_eq!(cgroup_cpu_max("x"), None);
    }

    #[test]
    fn io_stat_sums_selected_devices_only() {
        let t = "251:0 rbytes=1000 wbytes=2000 rios=1 wios=2 dbytes=0 dios=0\n\
                 259:0 rbytes=10 wbytes=20 rios=1 wios=1\n\
                 253:0 rbytes=10 wbytes=20\n";
        assert_eq!(cgroup_io_stat(t, |maj, _| maj == 259), (10, 20));
        assert_eq!(cgroup_io_stat(t, |_, _| true), (1020, 2040));
        assert_eq!(cgroup_io_stat("", |_, _| true), (0, 0));
    }

    #[test]
    fn classification() {
        let id = "0060944a9384f51a8b108799e4b04e7c6609706420f95cf699d73d27ba05baa8";
        assert_eq!(
            cgroup_classify(&format!("system.slice/docker-{id}.scope")),
            (K::Container, "docker 0060944a9384".into())
        );
        assert_eq!(
            cgroup_classify("machine.slice/libpod-abcdefabcdef0123.scope").0,
            K::Container
        );
        assert_eq!(
            cgroup_classify("system.slice/cron.service"),
            (K::Service, "cron".into())
        );
        assert_eq!(
            cgroup_classify(
                "user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-firefox-4242.scope"
            ),
            (K::UserApp, "gnome-firefox".into())
        );
        assert_eq!(
            cgroup_classify(
                "user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/vte-spawn-48b6.scope"
            ),
            (K::UserApp, "terminal tab".into())
        );
        assert_eq!(
            cgroup_classify("machine.slice/machine-qemu-1-win.scope").0,
            K::Machine
        );
        assert_eq!(cgroup_classify("init.scope"), (K::Other, "init".into()));
        let u = "user.slice/user-1000.slice/user@1000.service/app.slice";
        assert_eq!(
            cgroup_classify(&format!("{u}/app-gnome-google\\x2dchrome-1234.scope")).1,
            "gnome-google-chrome"
        );
        assert_eq!(
            cgroup_classify(&format!(
                "{u}/snap.code.code-433a466f-31e2-4c1d-9a8e-0b9c1d2e3f40.scope"
            ))
            .1,
            "code (snap)"
        );
        assert_eq!(systemd_unescape("a\\x2db\\x"), "a-b\\x");
    }
}

// ------------------------------------------------------------ hwmon

/// One hwmon temperature channel from raw sysfs strings (millidegrees).
/// Returns None for values the kernel reports but that cannot be real.
pub fn hwmon_temperature(
    input: &str,
    max: Option<&str>,
    crit: Option<&str>,
) -> Option<(f64, Option<f64>, Option<f64>)> {
    let c = |s: &str| s.trim().parse::<i64>().ok().map(|m| m as f64 / 1000.0);
    let v = c(input)?;
    if !(-60.0..=200.0).contains(&v) {
        return None;
    }
    // Some drivers report 0 for "no threshold".
    let threshold = |s: Option<&str>| s.and_then(c).filter(|t| *t > 0.0 && *t < 250.0);
    Some((v, threshold(max), threshold(crit)))
}

pub fn sensor_class(chip: &str, label: &str) -> nysm_core::snapshot::SensorClass {
    use nysm_core::snapshot::SensorClass as C;
    let chip = chip.to_ascii_lowercase();
    match chip.as_str() {
        "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" | "cpu-thermal" | "x86_pkg_temp" => {
            C::Cpu
        }
        "amdgpu" | "nouveau" | "radeon" | "i915" | "xe" => C::Gpu,
        "nvme" | "drivetemp" => C::Storage,
        "jc42" | "spd5118" => C::Memory,
        c if c.starts_with("pch_") => C::Chipset,
        _ if label.to_ascii_lowercase().contains("cpu") => C::Cpu,
        _ => C::Other,
    }
}

#[cfg(test)]
mod hwmon_tests {
    use super::*;
    use nysm_core::snapshot::SensorClass as C;

    #[test]
    fn temperatures_and_thresholds() {
        assert_eq!(
            hwmon_temperature("54850\n", Some("76850"), Some("78850")),
            Some((54.85, Some(76.85), Some(78.85)))
        );
        // jc42 reports 0 for unset thresholds.
        assert_eq!(
            hwmon_temperature("51000", Some("0"), Some("0")),
            Some((51.0, None, None))
        );
        // Unreadable threshold (EIO) is simply absent.
        assert_eq!(
            hwmon_temperature("62850", None, None),
            Some((62.85, None, None))
        );
        // Nonsense values are dropped, never shown.
        assert_eq!(hwmon_temperature("-273150", None, None), None);
        assert_eq!(hwmon_temperature("garbage", None, None), None);
    }

    #[test]
    fn classes() {
        assert_eq!(sensor_class("coretemp", "Package id 0"), C::Cpu);
        assert_eq!(sensor_class("nvme", "Composite"), C::Storage);
        assert_eq!(sensor_class("pch_skylake", ""), C::Chipset);
        assert_eq!(sensor_class("jc42", ""), C::Memory);
        assert_eq!(sensor_class("amdgpu", "edge"), C::Gpu);
        assert_eq!(sensor_class("acpitz", ""), C::Other);
    }
}

// ------------------------------------------------------------ GPU (DRM)

/// Files read for one DRM card, by name (relative to `/sys/class/drm/cardN`).
pub type CardFiles<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Build a GPU record from a card's sysfs files. Supports i915/xe
/// (frequency) and amdgpu (busy %, VRAM, current sclk).
pub fn drm_gpu(card: &str, driver: &str, read: CardFiles) -> nysm_core::snapshot::Gpu {
    let num = |f: &str| read(f).and_then(|v| v.trim().parse::<f64>().ok());
    let (mut freq, mut max) = (
        num("gt_act_freq_mhz")
            .filter(|v| *v > 0.0)
            .or(num("gt_cur_freq_mhz")),
        num("gt_max_freq_mhz"),
    );
    if freq.is_none() {
        // amdgpu: "0: 500Mhz\n1: 2100Mhz *" — the starred level is current.
        if let Some(t) = read("device/pp_dpm_sclk") {
            let mhz = |l: &str| {
                l.split_whitespace().nth(1).and_then(|v| {
                    v.trim_end_matches("Mhz")
                        .trim_end_matches("MHz")
                        .parse::<f64>()
                        .ok()
                })
            };
            freq = t
                .lines()
                .find(|l| l.trim_end().ends_with('*'))
                .and_then(mhz);
            max = t.lines().filter_map(mhz).reduce(f64::max);
        }
    }
    nysm_core::snapshot::Gpu {
        card: card.into(),
        driver: driver.into(),
        busy_pct: num("device/gpu_busy_percent").filter(|v| (0.0..=100.0).contains(v)),
        frequency_mhz: freq,
        max_frequency_mhz: max,
        vram_used_bytes: num("device/mem_info_vram_used").map(|v| v as u64),
        vram_total_bytes: num("device/mem_info_vram_total").map(|v| v as u64),
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use std::collections::HashMap;

    fn files(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn intel_frequency_only() {
        let f = files(&[
            ("gt_act_freq_mhz", "0\n"),
            ("gt_cur_freq_mhz", "1050\n"),
            ("gt_max_freq_mhz", "1100\n"),
        ]);
        let g = drm_gpu("card1", "i915", &|n| f.get(n).cloned());
        assert_eq!(g.frequency_mhz, Some(1050.0));
        assert_eq!(g.max_frequency_mhz, Some(1100.0));
        assert_eq!(
            g.busy_pct, None,
            "i915 utilisation is not exposed without perf"
        );
    }

    #[test]
    fn amdgpu_busy_vram_and_sclk() {
        let f = files(&[
            ("device/gpu_busy_percent", "37\n"),
            ("device/mem_info_vram_used", "1073741824\n"),
            ("device/mem_info_vram_total", "8589934592\n"),
            (
                "device/pp_dpm_sclk",
                "0: 500Mhz\n1: 1800Mhz *\n2: 2100Mhz\n",
            ),
        ]);
        let g = drm_gpu("card0", "amdgpu", &|n| f.get(n).cloned());
        assert_eq!(g.busy_pct, Some(37.0));
        assert_eq!(g.vram_used_bytes, Some(1 << 30));
        assert_eq!(g.frequency_mhz, Some(1800.0));
        assert_eq!(g.max_frequency_mhz, Some(2100.0));
        let bad = files(&[("device/gpu_busy_percent", "250")]);
        assert_eq!(
            drm_gpu("c", "amdgpu", &|n| bad.get(n).cloned()).busy_pct,
            None
        );
    }
}
