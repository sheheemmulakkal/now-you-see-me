//! Pure counter arithmetic. Every rate in the product is computed here so
//! that frontends cannot invent their own formulas.

use std::time::Duration;

use crate::raw::{
    CpuTimes, DISKSTATS_SECTOR_BYTES, RawDisk, RawInterface, RawMemory, RawPsiLine, RawSwapActivity,
};
use crate::snapshot::{
    CpuBreakdown, DiskIo, MemoryUsage, NetworkRates, PressureWindow, SwapActivity, SwapUsage,
};

/// Shortest interval over which we will report a rate. Shorter intervals
/// amplify tick quantisation (Linux CPU counters advance in 10 ms ticks).
pub const MIN_RATE_INTERVAL: Duration = Duration::from_millis(100);

/// Difference of a monotonically increasing counter. `None` means the
/// counter went backwards (reset, device re-plugged, CPU re-onlined); the
/// caller must treat the sample as a fresh baseline, never as a huge rate.
pub fn counter_delta(prev: u64, cur: u64) -> Option<u64> {
    cur.checked_sub(prev)
}

pub fn per_second(delta: u64, elapsed: Duration) -> Option<f64> {
    if elapsed < MIN_RATE_INTERVAL {
        return None;
    }
    Some(delta as f64 / elapsed.as_secs_f64())
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 100.0).clamp(0.0, 100.0)
    }
}

/// CPU utilisation between two cumulative readings of the same CPU (or
/// the aggregate line). Returns `None` on counter reset or when no time
/// was accounted (e.g. identical readings within one tick).
pub fn cpu_breakdown(prev: &CpuTimes, cur: &CpuTimes) -> Option<CpuBreakdown> {
    let d = |a: u64, b: u64| counter_delta(a, b);
    let user = d(prev.user, cur.user)?;
    let nice = d(prev.nice, cur.nice)?;
    let system = d(prev.system, cur.system)?;
    let idle = d(prev.idle, cur.idle)?;
    // iowait is documented by the kernel as unreliable and may go backwards
    // on some kernels; clamp to zero rather than declaring a reset.
    let iowait = cur.iowait.saturating_sub(prev.iowait);
    let irq = d(prev.irq, cur.irq)?;
    let softirq = d(prev.softirq, cur.softirq)?;
    let steal = d(prev.steal, cur.steal)?;
    let total = user + nice + system + idle + iowait + irq + softirq + steal;
    if total == 0 {
        return None;
    }
    let busy = user + nice + system + irq + softirq;
    Some(CpuBreakdown {
        total_pct: pct(busy, total),
        user_pct: pct(user, total),
        nice_pct: pct(nice, total),
        system_pct: pct(system, total),
        irq_pct: pct(irq + softirq, total),
        iowait_pct: pct(iowait, total),
        steal_pct: pct(steal, total),
        idle_pct: pct(idle, total),
    })
}

/// Process CPU as a share of the whole machine (0–100).
/// `ticks` is the delta of utime+stime; `ticks_per_s` is `CLK_TCK`.
pub fn process_cpu_pct(
    ticks: u64,
    ticks_per_s: u64,
    elapsed: Duration,
    logical_cores: u32,
) -> Option<f64> {
    if elapsed < MIN_RATE_INTERVAL || ticks_per_s == 0 || logical_cores == 0 {
        return None;
    }
    let cpu_s = ticks as f64 / ticks_per_s as f64;
    let pct = cpu_s / elapsed.as_secs_f64() / logical_cores as f64 * 100.0;
    // Tick quantisation can overshoot slightly; never exceed the machine.
    Some(pct.clamp(0.0, 100.0))
}

/// PSI stall share over the interval, from cumulative `total` microseconds.
pub fn psi_interval_pct(prev: &RawPsiLine, cur: &RawPsiLine, elapsed: Duration) -> Option<f64> {
    if elapsed < MIN_RATE_INTERVAL {
        return None;
    }
    let stalled_us = counter_delta(prev.total_us, cur.total_us)?;
    Some((stalled_us as f64 / (elapsed.as_secs_f64() * 1e6) * 100.0).clamp(0.0, 100.0))
}

pub fn psi_window(
    line: &RawPsiLine,
    prev: Option<&RawPsiLine>,
    elapsed: Option<Duration>,
) -> PressureWindow {
    PressureWindow {
        avg10_pct: line.avg10,
        avg60_pct: line.avg60,
        avg300_pct: line.avg300,
        interval_pct: match (prev, elapsed) {
            (Some(p), Some(e)) => psi_interval_pct(p, line, e),
            _ => None,
        },
    }
}

/// Memory headroom using the kernel's `MemAvailable` estimate. Returns
/// `None` when the required fields are absent (kernels < 3.14).
pub fn memory_usage(m: &RawMemory) -> Option<MemoryUsage> {
    let total = m.total?;
    let available = m.available?.min(total);
    let used = total - available;
    let cache = match (m.cached, m.buffers) {
        (Some(c), Some(b)) => Some(c + b),
        (Some(c), None) => Some(c),
        _ => None,
    };
    Some(MemoryUsage {
        total_bytes: total,
        available_bytes: available,
        used_bytes: used,
        used_pct: pct(used, total),
        free_bytes: m.free,
        cache_bytes: cache,
        reclaimable_slab_bytes: m.s_reclaimable,
        shared_bytes: m.shmem,
        dirty_bytes: m.dirty,
    })
}

pub fn swap_usage(m: &RawMemory) -> Option<SwapUsage> {
    let total = m.swap_total?;
    let free = m.swap_free?.min(total);
    Some(SwapUsage {
        total_bytes: total,
        used_bytes: total - free,
        free_bytes: free,
    })
}

pub fn swap_activity(
    prev: &RawSwapActivity,
    cur: &RawSwapActivity,
    page_size: u64,
    elapsed: Duration,
) -> Option<SwapActivity> {
    let pin = counter_delta(prev.pages_in, cur.pages_in)?;
    let pout = counter_delta(prev.pages_out, cur.pages_out)?;
    Some(SwapActivity {
        in_bytes_per_s: per_second(pin.saturating_mul(page_size), elapsed)?,
        out_bytes_per_s: per_second(pout.saturating_mul(page_size), elapsed)?,
    })
}

pub fn interface_rates(
    prev: &RawInterface,
    cur: &RawInterface,
    elapsed: Duration,
) -> Option<NetworkRates> {
    let rx = counter_delta(prev.rx_bytes, cur.rx_bytes)?;
    let tx = counter_delta(prev.tx_bytes, cur.tx_bytes)?;
    let rxp = counter_delta(prev.rx_packets, cur.rx_packets)?;
    let txp = counter_delta(prev.tx_packets, cur.tx_packets)?;
    let rx_bad = (cur.rx_errors + cur.rx_dropped).saturating_sub(prev.rx_errors + prev.rx_dropped);
    let tx_bad = (cur.tx_errors + cur.tx_dropped).saturating_sub(prev.tx_errors + prev.tx_dropped);
    Some(NetworkRates {
        rx_bytes_per_s: per_second(rx, elapsed)?,
        tx_bytes_per_s: per_second(tx, elapsed)?,
        rx_packets_per_s: per_second(rxp, elapsed)?,
        tx_packets_per_s: per_second(txp, elapsed)?,
        rx_errors_dropped: rx_bad,
        tx_errors_dropped: tx_bad,
    })
}

pub fn disk_io(prev: &RawDisk, cur: &RawDisk, elapsed: Duration) -> Option<DiskIo> {
    let reads = counter_delta(prev.reads, cur.reads)?;
    let writes = counter_delta(prev.writes, cur.writes)?;
    let sr = counter_delta(prev.sectors_read, cur.sectors_read)?;
    let sw = counter_delta(prev.sectors_written, cur.sectors_written)?;
    let read_ms = counter_delta(prev.read_ms, cur.read_ms)?;
    let write_ms = counter_delta(prev.write_ms, cur.write_ms)?;
    let io_ms = counter_delta(prev.io_ms, cur.io_ms)?;
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    Some(DiskIo {
        read_bytes_per_s: per_second(sr.saturating_mul(DISKSTATS_SECTOR_BYTES), elapsed)?,
        write_bytes_per_s: per_second(sw.saturating_mul(DISKSTATS_SECTOR_BYTES), elapsed)?,
        read_ops_per_s: per_second(reads, elapsed)?,
        write_ops_per_s: per_second(writes, elapsed)?,
        read_latency_ms: (reads > 0).then(|| read_ms as f64 / reads as f64),
        write_latency_ms: (writes > 0).then(|| write_ms as f64 / writes as f64),
        busy_pct: Some((io_ms as f64 / elapsed_ms * 100.0).clamp(0.0, 100.0)),
        in_flight: Some(cur.in_flight),
    })
}

/// Sum device I/O. Latency/busy are not additive across devices, so the
/// aggregate reports throughput/IOPS only.
pub fn sum_disk_io<'a>(items: impl IntoIterator<Item = &'a DiskIo>) -> DiskIo {
    let mut t = DiskIo::default();
    for d in items {
        t.read_bytes_per_s += d.read_bytes_per_s;
        t.write_bytes_per_s += d.write_bytes_per_s;
        t.read_ops_per_s += d.read_ops_per_s;
        t.write_ops_per_s += d.write_ops_per_s;
        // Busy time does not add up across disks: report the busiest one.
        t.busy_pct = match (t.busy_pct, d.busy_pct) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }
    t
}

pub fn sum_network<'a>(items: impl IntoIterator<Item = &'a NetworkRates>) -> NetworkRates {
    let mut t = NetworkRates::default();
    for r in items {
        t.rx_bytes_per_s += r.rx_bytes_per_s;
        t.tx_bytes_per_s += r.tx_bytes_per_s;
        t.rx_packets_per_s += r.rx_packets_per_s;
        t.tx_packets_per_s += r.tx_packets_per_s;
        t.rx_errors_dropped += r.rx_errors_dropped;
        t.tx_errors_dropped += r.tx_errors_dropped;
    }
    t
}

/// Minimum observation window before a growth trend is reported.
pub const MIN_GROWTH_WINDOW_MS: i64 = 120_000;

/// Trend of used bytes over time, in bytes per hour: the Theil–Sen
/// estimator (median of pairwise slopes), so a one-off step such as a
/// cleanup or a single large download does not dominate the rate.
/// `samples` are (unix ms, used bytes), oldest first.
pub fn growth_per_hour(samples: &[(i64, u64)]) -> Option<f64> {
    let (first, last) = (samples.first()?, samples.last()?);
    if samples.len() < 3 || last.0 - first.0 < MIN_GROWTH_WINDOW_MS {
        return None;
    }
    let mut slopes = Vec::with_capacity(samples.len() * (samples.len() - 1) / 2);
    for (i, (ta, ua)) in samples.iter().enumerate() {
        for (tb, ub) in &samples[i + 1..] {
            if tb > ta {
                let hours = (tb - ta) as f64 / 3_600_000.0;
                slopes.push((*ub as f64 - *ua as f64) / hours);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    slopes.sort_by(f64::total_cmp);
    let m = slopes.len() / 2;
    Some(if slopes.len().is_multiple_of(2) {
        (slopes[m - 1] + slopes[m]) / 2.0
    } else {
        slopes[m]
    })
}

/// Hours until `available` is used up at `rate` bytes/hour (None if not filling).
pub fn hours_until_full(available: u64, rate: f64) -> Option<f64> {
    (rate > 0.0).then(|| available as f64 / rate)
}

/// cgroup CPU as a share of the whole machine from `usage_usec` deltas.
pub fn cgroup_cpu_pct(delta_usec: u64, elapsed: Duration, logical_cores: u32) -> Option<f64> {
    if elapsed < MIN_RATE_INTERVAL || logical_cores == 0 {
        return None;
    }
    let pct = delta_usec as f64 / 1e6 / elapsed.as_secs_f64() / logical_cores as f64 * 100.0;
    Some(pct.clamp(0.0, 100.0))
}

/// Stall share over the interval from cumulative PSI microseconds.
pub fn stall_pct(prev_us: u64, cur_us: u64, elapsed: Duration) -> Option<f64> {
    if elapsed < MIN_RATE_INTERVAL {
        return None;
    }
    let d = counter_delta(prev_us, cur_us)?;
    Some((d as f64 / (elapsed.as_secs_f64() * 1e6) * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod growth_tests {
    use super::*;

    #[test]
    fn growth_needs_window_and_fits_trend() {
        let mb = 1_000_000u64;
        // 1 MB per minute for 5 minutes = 60 MB/hour.
        let s: Vec<(i64, u64)> = (0..=5)
            .map(|m| (m * 60_000, 1000 * mb + m as u64 * mb))
            .collect();
        let g = growth_per_hour(&s).unwrap();
        assert!((g - 60.0 * mb as f64).abs() < 1.0, "{g}");
        assert_eq!(hours_until_full(120 * mb, g).map(|h| h.round()), Some(2.0));
        // Too short a window: no trend.
        assert!(growth_per_hour(&s[..2]).is_none());
        assert!(growth_per_hour(&[(0, 1), (60_000, 2), (90_000, 3)]).is_none());
        // Shrinking usage: negative rate, never "full".
        let down: Vec<(i64, u64)> = (0..=5)
            .map(|m| (m * 60_000, 1000 * mb - m as u64 * mb))
            .collect();
        assert!(growth_per_hour(&down).unwrap() < 0.0);
        // Steady growth with one large cleanup in the middle: the trend
        // stays the steady rate instead of turning sharply negative.
        let step: Vec<(i64, u64)> = (0..30)
            .map(|i| {
                let freed = if i >= 10 { 2000 * mb } else { 0 };
                (i * 5_000, 10_000 * mb + i as u64 * 5 * mb - freed)
            })
            .collect();
        let g = growth_per_hour(&step).unwrap();
        assert!((g - 3600.0 * mb as f64).abs() < 1.0, "{g}");
        assert_eq!(hours_until_full(10, -5.0), None);
    }
}

#[cfg(test)]
mod cgroup_math_tests {
    use super::*;

    #[test]
    fn cgroup_cpu_and_stall() {
        // 2 CPU-seconds over 1 s on 4 cores = 50 % of machine.
        assert_eq!(
            cgroup_cpu_pct(2_000_000, Duration::from_secs(1), 4),
            Some(50.0)
        );
        assert_eq!(cgroup_cpu_pct(1, Duration::from_millis(10), 4), None);
        assert_eq!(stall_pct(0, 250_000, Duration::from_secs(1)), Some(25.0));
        assert_eq!(stall_pct(10, 5, Duration::from_secs(1)), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{DiskKind, InterfaceKind};

    fn cpu(user: u64, system: u64, idle: u64, iowait: u64, steal: u64, guest: u64) -> CpuTimes {
        CpuTimes {
            user,
            system,
            idle,
            iowait,
            steal,
            guest,
            ..Default::default()
        }
    }

    const S1: Duration = Duration::from_secs(1);

    #[test]
    fn counter_reset_is_not_a_rate() {
        assert_eq!(counter_delta(10, 15), Some(5));
        assert_eq!(counter_delta(15, 10), None);
        assert_eq!(counter_delta(u64::MAX - 1, 2), None);
    }

    #[test]
    fn rates_need_a_minimum_interval() {
        assert_eq!(per_second(100, Duration::from_millis(10)), None);
        assert_eq!(per_second(100, Duration::from_millis(500)), Some(200.0));
    }

    #[test]
    fn cpu_half_busy() {
        let a = cpu(100, 100, 800, 0, 0, 0);
        let b = cpu(150, 150, 900, 0, 0, 0);
        let r = cpu_breakdown(&a, &b).unwrap();
        assert!((r.total_pct - 50.0).abs() < 1e-9);
        assert!((r.user_pct - 25.0).abs() < 1e-9);
        assert!((r.idle_pct - 50.0).abs() < 1e-9);
    }

    #[test]
    fn guest_time_is_not_double_counted() {
        // 100 ticks of user time, all of it guest. Total must still be 100%.
        let a = cpu(0, 0, 0, 0, 0, 0);
        let b = cpu(100, 0, 0, 0, 0, 100);
        let r = cpu_breakdown(&a, &b).unwrap();
        assert!((r.total_pct - 100.0).abs() < 1e-9);
    }

    #[test]
    fn iowait_and_steal_are_not_busy() {
        let a = cpu(0, 0, 0, 0, 0, 0);
        let b = cpu(25, 0, 25, 25, 25, 0);
        let r = cpu_breakdown(&a, &b).unwrap();
        assert!((r.total_pct - 25.0).abs() < 1e-9);
        assert!((r.iowait_pct - 25.0).abs() < 1e-9);
        assert!((r.steal_pct - 25.0).abs() < 1e-9);
    }

    #[test]
    fn iowait_regression_is_tolerated_but_other_resets_are_not() {
        let a = cpu(10, 10, 10, 50, 0, 0);
        let b = cpu(20, 20, 20, 40, 0, 0);
        assert!(cpu_breakdown(&a, &b).is_some());
        let c = cpu(5, 20, 20, 50, 0, 0);
        assert!(cpu_breakdown(&a, &c).is_none());
    }

    #[test]
    fn no_elapsed_ticks_is_warming_up_not_zero() {
        let a = cpu(10, 10, 10, 0, 0, 0);
        assert!(cpu_breakdown(&a, &a).is_none());
    }

    #[test]
    fn process_cpu_normalised_to_machine() {
        // 2 seconds of CPU over 1 second on 4 cores = 50% of machine.
        let p = process_cpu_pct(200, 100, S1, 4).unwrap();
        assert!((p - 50.0).abs() < 1e-9);
        // Elapsed-time variation: same ticks over 2 s is half.
        let p = process_cpu_pct(200, 100, Duration::from_secs(2), 4).unwrap();
        assert!((p - 25.0).abs() < 1e-9);
        // Overshoot from tick quantisation is clamped.
        assert_eq!(process_cpu_pct(500, 100, S1, 4), Some(100.0));
        assert_eq!(process_cpu_pct(1, 100, Duration::from_millis(1), 4), None);
    }

    #[test]
    fn memory_used_is_total_minus_available() {
        let m = RawMemory {
            total: Some(16_000),
            available: Some(12_000),
            free: Some(2_000),
            cached: Some(9_000),
            buffers: Some(500),
            ..Default::default()
        };
        let u = memory_usage(&m).unwrap();
        assert_eq!(u.used_bytes, 4_000);
        assert!((u.used_pct - 25.0).abs() < 1e-9);
        assert_eq!(u.cache_bytes, Some(9_500));
        // Without MemAvailable we refuse to guess.
        let old = RawMemory {
            total: Some(1),
            free: Some(1),
            ..Default::default()
        };
        assert!(memory_usage(&old).is_none());
    }

    #[test]
    fn swap_usage_and_activity() {
        let m = RawMemory {
            swap_total: Some(100),
            swap_free: Some(40),
            ..Default::default()
        };
        assert_eq!(swap_usage(&m).unwrap().used_bytes, 60);
        let a = RawSwapActivity {
            pages_in: 10,
            pages_out: 0,
        };
        let b = RawSwapActivity {
            pages_in: 20,
            pages_out: 5,
        };
        let r = swap_activity(&a, &b, 4096, S1).unwrap();
        assert_eq!(r.in_bytes_per_s, 40960.0);
        assert_eq!(r.out_bytes_per_s, 20480.0);
    }

    #[test]
    fn psi_interval_from_totals() {
        let a = RawPsiLine {
            avg10: 0.0,
            avg60: 0.0,
            avg300: 0.0,
            total_us: 1_000_000,
        };
        let b = RawPsiLine {
            total_us: 1_250_000,
            ..a
        };
        assert_eq!(psi_interval_pct(&a, &b, S1), Some(25.0));
        assert_eq!(psi_interval_pct(&b, &a, S1), None);
    }

    fn iface(rx: u64, tx: u64) -> RawInterface {
        RawInterface {
            name: "eth0".into(),
            kind: InterfaceKind::Physical,
            up: Some(true),
            rx_bytes: rx,
            rx_packets: rx / 100,
            rx_errors: 0,
            rx_dropped: 0,
            tx_bytes: tx,
            tx_packets: tx / 100,
            tx_errors: 0,
            tx_dropped: 0,
        }
    }

    #[test]
    fn network_rates_and_reset() {
        let r =
            interface_rates(&iface(1000, 0), &iface(3000, 500), Duration::from_secs(2)).unwrap();
        assert_eq!(r.rx_bytes_per_s, 1000.0);
        assert_eq!(r.tx_bytes_per_s, 250.0);
        assert!(interface_rates(&iface(3000, 0), &iface(10, 0), S1).is_none());
    }

    fn disk(reads: u64, sectors: u64, read_ms: u64, io_ms: u64) -> RawDisk {
        RawDisk {
            name: "sda".into(),
            major: 8,
            minor: 0,
            kind: DiskKind::Disk,
            reads,
            sectors_read: sectors,
            read_ms,
            writes: 0,
            sectors_written: 0,
            write_ms: 0,
            in_flight: 0,
            io_ms,
            weighted_io_ms: 0,
        }
    }

    #[test]
    fn disk_io_latency_and_busy() {
        let r = disk_io(&disk(0, 0, 0, 0), &disk(100, 2048, 50, 250), S1).unwrap();
        assert_eq!(r.read_bytes_per_s, 1_048_576.0);
        assert_eq!(r.read_ops_per_s, 100.0);
        assert_eq!(r.read_latency_ms, Some(0.5));
        assert_eq!(r.write_latency_ms, None);
        assert_eq!(r.busy_pct, Some(25.0));
        let mut other = r;
        other.busy_pct = Some(60.0);
        assert_eq!(
            sum_disk_io([&r, &other]).busy_pct,
            Some(60.0),
            "busiest disk"
        );
    }
}
