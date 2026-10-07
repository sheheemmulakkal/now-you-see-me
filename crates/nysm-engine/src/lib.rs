//! The sampling engine turns raw platform readings into immutable
//! [`Snapshot`]s. It owns all counter state, so every frontend sees the
//! same rates computed by the same formulas.

mod fsworker;
pub mod live;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nysm_collect::{CollectError, Platform, PressureResource, ProcessDetails};
use nysm_core::history::{History, HistoryPoint, PinnedProcess, ProcessHistory};
use nysm_core::math;
use nysm_core::raw::{
    InterfaceKind, ProcessId, RawCpu, RawDisk, RawInterface, RawPressure, RawProcessIo,
    RawSwapActivity,
};
use nysm_core::snapshot::*;
use nysm_core::{Reading, Status};

use fsworker::{SlowWorker, WorkerStatus};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Nominal sampling interval; used for gap detection, not for sleeping.
    pub interval: Duration,
    pub processes: bool,
    pub process_interval: Duration,
    /// Per-container/service/app accounting (refreshed at `process_interval`).
    pub cgroups: bool,
    pub filesystems: bool,
    pub filesystem_interval: Duration,
    pub frequency: bool,
    /// Temperatures/fans/batteries on a worker thread.
    pub sensors: bool,
    pub sensor_interval: Duration,
    pub history_samples: usize,
    pub history_bytes: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            interval: Duration::from_secs(1),
            processes: false,
            process_interval: Duration::from_secs(2),
            cgroups: false,
            filesystems: true,
            filesystem_interval: Duration::from_secs(15),
            frequency: true,
            sensors: true,
            sensor_interval: Duration::from_secs(5),
            // 10 minutes at 1 s.
            history_samples: 600,
            history_bytes: 256 * 1024,
        }
    }
}

struct Prev {
    at: Instant,
    wall_ms: i64,
    cpu: Option<RawCpu>,
    swap: Option<RawSwapActivity>,
    psi: [Option<RawPressure>; 3],
    ifaces: HashMap<String, RawInterface>,
    disks: HashMap<String, RawDisk>,
}

struct ProcPrev {
    ticks: u64,
    io: Option<RawProcessIo>,
}

struct CgPrev {
    cpu_usec: Option<u64>,
    io: (Option<u64>, Option<u64>),
    mem_psi: Option<u64>,
    cpu_psi: Option<u64>,
}

#[derive(Default)]
struct CgState {
    last_at: Option<Instant>,
    prev: HashMap<String, CgPrev>,
    table: Option<Arc<CgroupTable>>,
}

#[derive(Default)]
struct ProcState {
    last_at: Option<Instant>,
    /// The last scan was the first: rates need a second one, so take it at
    /// the next tick instead of a full process interval later.
    quick: bool,
    prev: HashMap<ProcessId, ProcPrev>,
    table: Option<Arc<ProcessTable>>,
}

pub struct Engine {
    platform: Box<dyn Platform>,
    cfg: EngineConfig,
    seq: u64,
    host: HostInfo,
    host_at: Instant,
    prev: Option<Prev>,
    procs: ProcState,
    cgs: CgState,
    fs: Option<SlowWorker<Vec<FilesystemSnapshot>>>,
    sensors: Option<SlowWorker<SensorsSnapshot>>,
    /// Per mount point: (refresh time, used bytes), bounded.
    fs_growth: HashMap<String, std::collections::VecDeque<(i64, u64)>>,
    history: History,
    pins: ProcessHistory,
}

fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn reset_reading<T>() -> Reading<T> {
    Reading::missing(
        Status::WarmingUp,
        "counter reset or first sample; re-baselining",
    )
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Self::with_platform(nysm_collect::native(), cfg)
    }

    pub fn with_platform(mut platform: Box<dyn Platform>, cfg: EngineConfig) -> Self {
        let host = platform.host_info();
        let fs = cfg.filesystems.then(|| {
            let mut provider = platform.filesystem_provider();
            SlowWorker::spawn(
                "nysm-fs",
                "filesystem",
                Box::new(move || provider.filesystems()),
                cfg.filesystem_interval,
            )
        });
        let sensors = cfg.sensors.then(|| {
            SlowWorker::spawn(
                "nysm-sensors",
                "sensor",
                platform.sensors_provider(),
                cfg.sensor_interval,
            )
        });
        let history = History::new(cfg.history_samples, cfg.history_bytes);
        // Pinned history spans the same window as global history.
        let window = cfg.interval.as_secs_f64() * cfg.history_samples as f64;
        let pin_points =
            (window / cfg.process_interval.max(cfg.interval).as_secs_f64()).ceil() as usize;
        let pins = ProcessHistory::new(ProcessHistory::DEFAULT_MAX_PINS, pin_points.max(1));
        Engine {
            platform,
            cfg,
            seq: 0,
            host,
            host_at: Instant::now(),
            prev: None,
            procs: ProcState::default(),
            cgs: CgState::default(),
            fs,
            sensors,
            fs_growth: HashMap::new(),
            history,
            pins,
        }
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// Keep detailed history for a process (bounded number of pins).
    pub fn pin(&mut self, id: ProcessId, name: String) -> Result<(), String> {
        self.pins.pin(id, name)
    }

    pub fn unpin(&mut self, id: &ProcessId) {
        self.pins.unpin(id);
    }

    pub fn pinned(&self) -> &[PinnedProcess] {
        self.pins.pinned()
    }

    pub fn set_processes(&mut self, on: bool) {
        self.cfg.processes = on;
        if !on {
            self.procs = ProcState::default();
        }
    }

    pub fn set_cgroups(&mut self, on: bool) {
        self.cfg.cgroups = on;
        if !on {
            self.cgs = CgState::default();
        }
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.cfg.interval = interval;
    }

    pub fn source(&self, metric_id: &str) -> &'static str {
        self.platform.source(metric_id)
    }

    /// Wait (bounded) for slow providers' first results; for one-shot use.
    pub fn wait_slow_providers(&self, timeout: Duration) {
        if let Some(fs) = &self.fs {
            fs.wait_first(timeout);
        }
        if let Some(s) = &self.sensors {
            s.wait_first(timeout);
        }
    }

    pub fn process_details(
        &mut self,
        pid: u32,
        start_ticks: Option<u64>,
        include_cmdline: bool,
    ) -> Result<ProcessDetails, CollectError> {
        self.platform
            .process_details(pid, start_ticks, include_cmdline)
    }

    /// On-demand socket table (never part of periodic sampling).
    pub fn sockets(
        &mut self,
        resolve_owners: bool,
    ) -> Result<Vec<nysm_core::sockets::SocketEntry>, CollectError> {
        self.platform.sockets(resolve_owners)
    }

    pub fn user_name(&mut self, uid: u32) -> Option<String> {
        self.platform.user_name(uid)
    }

    /// Take one sample. Never blocks on slow providers.
    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let now_wall = wall_ms();
        if now.duration_since(self.host_at) > Duration::from_secs(60) {
            self.host = self.platform.host_info();
            self.host_at = now;
        } else if let Some(u) = self.host.uptime_s.as_mut() {
            // Cheap uptime update without re-reading host files.
            if let Some(p) = &self.prev {
                *u += now.duration_since(p.at).as_secs_f64();
            }
        }
        let (elapsed, gap) = match &self.prev {
            Some(p) => {
                let e = now.duration_since(p.at);
                let wall_e = (now_wall - p.wall_ms).max(0) as u64;
                // Suspend: the wall clock advanced far more than monotonic time,
                // or monotonic time itself jumped well past the cadence.
                let suspended = wall_e > e.as_millis() as u64 + 2000;
                let stalled = e > self.cfg.interval * 3 + Duration::from_secs(1);
                (Some(e), suspended || stalled)
            }
            None => (None, false),
        };

        let mut next = Prev {
            at: now,
            wall_ms: now_wall,
            cpu: None,
            swap: None,
            psi: [None, None, None],
            ifaces: HashMap::new(),
            disks: HashMap::new(),
        };

        let cpu = self.sample_cpu(elapsed, &mut next);
        let memory = self.sample_memory(elapsed, &mut next);
        let network = self.sample_network(elapsed, &mut next);
        let storage = self.sample_storage(elapsed, &mut next);
        let cores = cpu.logical_cores.value.unwrap_or(0);
        let cgroups = if self.cfg.cgroups {
            self.sample_cgroups(now, now_wall, cores)
        } else {
            None
        };
        let processes = if self.cfg.processes {
            self.sample_processes(now, now_wall, cores)
        } else {
            None
        };

        self.prev = Some(next);
        self.seq += 1;
        let snap = Snapshot {
            schema_version: nysm_core::SCHEMA_VERSION,
            producer: nysm_core::brand::PRODUCER.into(),
            seq: self.seq,
            timestamp_ms: now_wall,
            interval_ms: elapsed.map(|e| e.as_millis() as u64),
            gap_before: gap,
            host: self.host.clone(),
            cpu,
            memory,
            network,
            storage,
            limits: self
                .platform
                .own_limits()
                .map_or_else(Into::into, Reading::ok),
            sensors: self.sensor_reading(),
            processes,
            cgroups,
        };
        self.history.push(HistoryPoint::from_snapshot(&snap));
        snap
    }

    fn psi(
        &mut self,
        res: PressureResource,
        idx: usize,
        elapsed: Option<Duration>,
        next: &mut Prev,
    ) -> Reading<Pressure> {
        match self.platform.pressure(res) {
            Ok(raw) => {
                let prev = self.prev.as_ref().and_then(|p| p.psi[idx]);
                let p = Pressure {
                    some: math::psi_window(&raw.some, prev.as_ref().map(|p| &p.some), elapsed),
                    full: raw.full.map(|f| {
                        math::psi_window(&f, prev.as_ref().and_then(|p| p.full.as_ref()), elapsed)
                    }),
                };
                next.psi[idx] = Some(raw);
                Reading::ok(p)
            }
            Err(e) => e.into(),
        }
    }

    fn sample_cpu(&mut self, elapsed: Option<Duration>, next: &mut Prev) -> CpuSnapshot {
        let load = self.platform.load().map(|l| LoadAverage {
            one: l.one,
            five: l.five,
            fifteen: l.fifteen,
            runnable: l.runnable,
            tasks: l.tasks,
        });
        let pressure = self.psi(PressureResource::Cpu, 0, elapsed, next);
        let raw = match self.platform.cpu_times() {
            Ok(r) => r,
            Err(e) => {
                return CpuSnapshot {
                    logical_cores: e.clone().into(),
                    usage: e.into(),
                    per_core: Vec::new(),
                    load: load.map_or_else(Into::into, Reading::ok),
                    pressure,
                };
            }
        };
        let prev = self.prev.as_ref().and_then(|p| p.cpu.as_ref());
        let usage = match prev {
            None => Reading::warming_up(),
            Some(p) => match math::cpu_breakdown(&p.total, &raw.total) {
                Some(b) => Reading::ok(b),
                None => reset_reading(),
            },
        };
        let ids: Vec<u32> = raw.per_cpu.iter().map(|(id, _)| *id).collect();
        let freqs = if self.cfg.frequency {
            self.platform.cpu_frequencies(&ids)
        } else {
            ids.iter()
                .map(|_| {
                    Err(CollectError::Unsupported(
                        "frequency collection disabled".into(),
                    ))
                })
                .collect()
        };
        let per_core = raw
            .per_cpu
            .iter()
            .zip(freqs)
            .map(|((id, t), f)| {
                let before = prev.and_then(|p| p.per_cpu.iter().find(|(pid, _)| pid == id));
                let usage_pct = match before {
                    None => Reading::warming_up(),
                    Some((_, b)) => match math::cpu_breakdown(b, t) {
                        Some(x) => Reading::ok(x.total_pct),
                        None => reset_reading(),
                    },
                };
                CoreSnapshot {
                    id: *id,
                    usage_pct,
                    frequency_mhz: f.map_or_else(Into::into, Reading::ok),
                }
            })
            .collect();
        let cores = raw.per_cpu.len() as u32;
        next.cpu = Some(raw);
        CpuSnapshot {
            logical_cores: if cores > 0 {
                Reading::ok(cores)
            } else {
                Reading::missing(Status::CollectionError, "no per-CPU lines")
            },
            usage,
            per_core,
            load: load.map_or_else(Into::into, Reading::ok),
            pressure,
        }
    }

    fn sample_memory(&mut self, elapsed: Option<Duration>, next: &mut Prev) -> MemorySnapshot {
        let pressure = self.psi(PressureResource::Memory, 1, elapsed, next);
        let (usage, swap) = match self.platform.memory() {
            Ok(m) => (
                math::memory_usage(&m).map_or_else(
                    || {
                        Reading::missing(
                            Status::Unsupported,
                            "MemTotal/MemAvailable not reported (kernel < 3.14?)",
                        )
                    },
                    Reading::ok,
                ),
                math::swap_usage(&m).map_or_else(
                    || Reading::missing(Status::Unsupported, "swap fields not reported"),
                    Reading::ok,
                ),
            ),
            Err(e) => (e.clone().into(), e.into()),
        };
        let swap_activity = match self.platform.swap_activity() {
            Ok(cur) => {
                let r = match (self.prev.as_ref().and_then(|p| p.swap.as_ref()), elapsed) {
                    (Some(p), Some(e)) => {
                        math::swap_activity(p, &cur, self.platform.page_size(), e)
                            .map_or_else(reset_reading, Reading::ok)
                    }
                    _ => Reading::warming_up(),
                };
                next.swap = Some(cur);
                r
            }
            Err(e) => e.into(),
        };
        MemorySnapshot {
            usage,
            swap,
            swap_activity,
            pressure,
        }
    }

    fn sample_network(&mut self, elapsed: Option<Duration>, next: &mut Prev) -> NetworkSnapshot {
        const PHYSICAL: &str = "up physical (ethernet/wireless) interfaces; excludes loopback, bridges, veth and tunnels";
        const FALLBACK: &str = "no physical interface is visible (container or VM), so all up non-loopback, non-bridge interfaces are counted";
        let raws = match self.platform.interfaces() {
            Ok(r) => r,
            Err(e) => {
                return NetworkSnapshot {
                    interfaces: e.clone().into(),
                    total: e.into(),
                    total_scope: PHYSICAL.into(),
                };
            }
        };
        // Default: physical links only, to avoid double counting. If none is
        // visible (e.g. inside a container, where eth0 is a veth), count the
        // container's own non-loopback interfaces instead and say so.
        let up = |r: &RawInterface| r.up != Some(false);
        let physical_visible = raws.iter().any(|r| r.kind.counted_by_default() && up(r));
        let counts = |r: &RawInterface| {
            if physical_visible {
                r.kind.counted_by_default() && up(r)
            } else {
                !matches!(r.kind, InterfaceKind::Loopback | InterfaceKind::Bridge) && up(r)
            }
        };
        let scope = if physical_visible { PHYSICAL } else { FALLBACK }.to_string();
        let mut total = NetworkRates::default();
        let mut total_ok = true;
        let mut any_counted = false;
        let mut out = Vec::with_capacity(raws.len());
        for r in raws {
            let counted = counts(&r);
            let prev = self.prev.as_ref().and_then(|p| p.ifaces.get(&r.name));
            let rates = match (prev, elapsed) {
                (Some(p), Some(e)) => {
                    math::interface_rates(p, &r, e).map_or_else(reset_reading, Reading::ok)
                }
                _ => Reading::warming_up(),
            };
            if counted {
                any_counted = true;
                match rates.live() {
                    Some(v) => total = math::sum_network([&total, v]),
                    None => total_ok = false,
                }
            }
            out.push(InterfaceSnapshot {
                name: r.name.clone(),
                kind: r.kind,
                up: r.up,
                counted_in_total: counted,
                rx_total_bytes: r.rx_bytes,
                tx_total_bytes: r.tx_bytes,
                rates,
            });
            next.ifaces.insert(r.name.clone(), r);
        }
        let total = if !any_counted {
            Reading::missing(Status::Unsupported, "no up non-loopback interfaces found")
        } else if total_ok {
            Reading::ok(total)
        } else {
            Reading::warming_up()
        };
        NetworkSnapshot {
            interfaces: Reading::ok(out),
            total,
            total_scope: scope,
        }
    }

    fn sample_storage(&mut self, elapsed: Option<Duration>, next: &mut Prev) -> StorageSnapshot {
        let io_pressure = self.psi(PressureResource::Io, 2, elapsed, next);
        let (devices, total_io) = match self.platform.disks() {
            Ok(raws) => {
                let mut out = Vec::with_capacity(raws.len());
                let mut counted_ios = Vec::new();
                let mut total_ok = true;
                for r in raws {
                    let counted = r.kind.counted_by_default();
                    let prev = self.prev.as_ref().and_then(|p| p.disks.get(&r.name));
                    let io = match (prev, elapsed) {
                        (Some(p), Some(e)) if p.major == r.major && p.minor == r.minor => {
                            math::disk_io(p, &r, e).map_or_else(reset_reading, Reading::ok)
                        }
                        (Some(_), Some(_)) => reset_reading(),
                        _ => Reading::warming_up(),
                    };
                    if counted {
                        match io.live() {
                            Some(v) => counted_ios.push(*v),
                            None => total_ok = false,
                        }
                    }
                    out.push(DiskSnapshot {
                        name: r.name.clone(),
                        kind: r.kind,
                        counted_in_total: counted,
                        io,
                    });
                    next.disks.insert(r.name.clone(), r);
                }
                let any = out.iter().any(|d| d.counted_in_total);
                let total = if !any {
                    Reading::missing(Status::Unsupported, "no physical disks visible")
                } else if total_ok {
                    Reading::ok(math::sum_disk_io(&counted_ios))
                } else {
                    Reading::warming_up()
                };
                (Reading::ok(out), total)
            }
            Err(e) => (e.clone().into(), e.into()),
        };
        let (filesystems, filesystems_age_ms) = match &self.fs {
            None => (
                Reading::missing(Status::Unsupported, "filesystem collection disabled"),
                None,
            ),
            Some(fs) => match fs.status() {
                WorkerStatus::NotYet => (Reading::warming_up(), None),
                WorkerStatus::Fresh(age, r) => (
                    r.map_or_else(Into::into, Reading::ok),
                    Some(age.as_millis() as u64),
                ),
                WorkerStatus::Stale(age, r, why) => (
                    match r {
                        Ok(v) => Reading::stale(v, why),
                        Err(_) => Reading::missing(Status::Stale, why),
                    },
                    Some(age.as_millis() as u64),
                ),
            },
        };
        // Growth trend from successive capacity refreshes (one sample per
        // refresh, keyed by when that refresh happened).
        let filesystems = match (filesystems_age_ms, filesystems) {
            (Some(age), mut r) => {
                let refreshed_at = wall_ms() - age as i64;
                if let Some(v) = r.value.as_mut() {
                    const MAX_POINTS: usize = 240; // ~1 h at 15 s
                    self.fs_growth
                        .retain(|k, _| v.iter().any(|f| &f.mount_point == k));
                    for f in v.iter_mut() {
                        let q = self.fs_growth.entry(f.mount_point.clone()).or_default();
                        // Same refresh seen again: allow a little clock jitter.
                        if q.back().is_none_or(|(t, _)| refreshed_at - t > 1000) {
                            if q.len() == MAX_POINTS {
                                q.pop_front();
                            }
                            q.push_back((refreshed_at, f.used_bytes));
                        }
                        let pts: Vec<(i64, u64)> = q.iter().copied().collect();
                        f.growth_bytes_per_hour = math::growth_per_hour(&pts);
                        f.full_in_hours = f
                            .growth_bytes_per_hour
                            .and_then(|g| math::hours_until_full(f.available_bytes, g));
                    }
                }
                r
            }
            (None, r) => r,
        };
        StorageSnapshot {
            devices,
            total_io,
            filesystems,
            filesystems_age_ms,
            io_pressure,
        }
    }

    fn sensor_reading(&self) -> Reading<SensorsSnapshot> {
        match &self.sensors {
            None => Reading::missing(Status::Unsupported, "sensor collection disabled"),
            Some(w) => match w.status() {
                WorkerStatus::NotYet => Reading::warming_up(),
                WorkerStatus::Fresh(_, r) => r.map_or_else(Into::into, Reading::ok),
                WorkerStatus::Stale(_, r, why) => match r {
                    Ok(v) => Reading::stale(v, why),
                    Err(_) => Reading::missing(Status::Stale, why),
                },
            },
        }
    }

    fn sample_cgroups(
        &mut self,
        now: Instant,
        now_wall: i64,
        cores: u32,
    ) -> Option<Arc<CgroupTable>> {
        let due = self
            .cgs
            .last_at
            .is_none_or(|t| now.duration_since(t) >= self.cfg.process_interval);
        if !due {
            return self.cgs.table.clone();
        }
        let scan = match self.platform.cgroups() {
            Ok(s) => s,
            Err(_) => return self.cgs.table.clone(),
        };
        let elapsed = self.cgs.last_at.map(|t| now.duration_since(t));
        let mut prev_map = HashMap::with_capacity(scan.groups.len());
        let mut groups = Vec::with_capacity(scan.groups.len());
        for g in scan.groups {
            let before = self.cgs.prev.get(&g.path);
            let rate = |p: Option<u64>, c: Option<u64>| -> Option<Option<f64>> {
                let (p, c, e) = (p?, c?, elapsed?);
                Some(math::counter_delta(p, c).and_then(|d| math::per_second(d, e)))
            };
            let cpu_pct = match (before.and_then(|b| b.cpu_usec), g.cpu_usage_usec, elapsed) {
                (_, None, _) => Reading::missing(
                    Status::Unsupported,
                    "cpu controller not enabled for this group",
                ),
                (Some(p), Some(c), Some(e)) => match math::counter_delta(p, c) {
                    Some(d) => math::cgroup_cpu_pct(d, e, cores)
                        .map_or_else(Reading::warming_up, Reading::ok),
                    None => reset_reading(),
                },
                _ => Reading::warming_up(),
            };
            let disk_io = match (g.io_read_bytes, g.io_write_bytes) {
                (Some(_), Some(_)) => match (
                    rate(before.and_then(|b| b.io.0), g.io_read_bytes),
                    rate(before.and_then(|b| b.io.1), g.io_write_bytes),
                ) {
                    (Some(Some(r)), Some(Some(w))) => Reading::ok(ProcessDiskIo {
                        read_bytes_per_s: r,
                        write_bytes_per_s: w,
                    }),
                    (Some(None), _) | (_, Some(None)) => reset_reading(),
                    _ => Reading::warming_up(),
                },
                _ => Reading::missing(
                    Status::Unsupported,
                    "io controller not enabled for this group",
                ),
            };
            let stall = |p: Option<u64>, c: Option<u64>| match (p, c, elapsed) {
                (_, None, _) => Reading::missing(Status::Unsupported, "no PSI for this group"),
                (Some(p), Some(c), Some(e)) => {
                    math::stall_pct(p, c, e).map_or_else(reset_reading, Reading::ok)
                }
                _ => Reading::warming_up(),
            };
            let memory_pressure_pct =
                stall(before.and_then(|b| b.mem_psi), g.memory_pressure_some_us);
            let cpu_pressure_pct = stall(before.and_then(|b| b.cpu_psi), g.cpu_pressure_some_us);
            prev_map.insert(
                g.path.clone(),
                CgPrev {
                    cpu_usec: g.cpu_usage_usec,
                    io: (g.io_read_bytes, g.io_write_bytes),
                    mem_psi: g.memory_pressure_some_us,
                    cpu_psi: g.cpu_pressure_some_us,
                },
            );
            groups.push(CgroupSnapshot {
                path: g.path,
                kind: g.kind,
                name: g.name,
                main_process: g.main_process,
                cpu_pct,
                cpu_limit_cores: g
                    .cpu_max
                    .and_then(|(q, p)| q.map(|q| q as f64 / p.max(1) as f64)),
                memory_bytes: g.memory_current,
                memory_max_bytes: g.memory_max,
                memory_high_bytes: g.memory_high,
                pids: g.pids_current,
                pids_max: g.pids_max,
                disk_io,
                memory_pressure_pct,
                cpu_pressure_pct,
            });
        }
        self.cgs.prev = prev_map;
        self.cgs.last_at = Some(now);
        let table = Arc::new(CgroupTable {
            timestamp_ms: now_wall,
            interval_ms: elapsed.map(|e| e.as_millis() as u64),
            logical_cores: cores,
            groups,
            truncated: scan.truncated,
        });
        self.cgs.table = Some(table.clone());
        Some(table)
    }

    fn sample_processes(
        &mut self,
        now: Instant,
        now_wall: i64,
        cores: u32,
    ) -> Option<Arc<ProcessTable>> {
        let due = self.procs.last_at.is_none_or(|t| {
            let since = now.duration_since(t);
            since >= self.cfg.process_interval
                || (self.procs.quick && since >= self.cfg.process_interval.min(self.cfg.interval))
        });
        if !due {
            return self.procs.table.clone();
        }
        let scan = match self.platform.processes() {
            Ok(s) => s,
            Err(_) => return self.procs.table.clone(),
        };
        let elapsed = self.procs.last_at.map(|t| now.duration_since(t));
        let tps = self.platform.clock_ticks_per_s();
        let mut prev_map = HashMap::with_capacity(scan.processes.len());
        let mut entries = Vec::with_capacity(scan.processes.len());
        for p in scan.processes {
            let ticks = p.utime_ticks + p.stime_ticks;
            let before = self.procs.prev.get(&p.id);
            let cpu_pct = match (before, elapsed) {
                (Some(b), Some(e)) => match math::counter_delta(b.ticks, ticks) {
                    Some(d) => math::process_cpu_pct(d, tps, e, cores)
                        .map_or_else(Reading::warming_up, Reading::ok),
                    None => reset_reading(),
                },
                _ => Reading::warming_up(),
            };
            let disk_io = match (&p.io, before.and_then(|b| b.io), elapsed) {
                (Err(st), _, _) => Reading::missing(
                    *st,
                    match st {
                        Status::PermissionDenied => "owned by another user",
                        _ => "not readable",
                    },
                ),
                (Ok(cur), Some(b), Some(e)) => {
                    match (
                        math::counter_delta(b.read_bytes, cur.read_bytes),
                        math::counter_delta(b.write_bytes, cur.write_bytes),
                    ) {
                        (Some(r), Some(w)) => {
                            match (math::per_second(r, e), math::per_second(w, e)) {
                                (Some(r), Some(w)) => Reading::ok(ProcessDiskIo {
                                    read_bytes_per_s: r,
                                    write_bytes_per_s: w,
                                }),
                                _ => Reading::warming_up(),
                            }
                        }
                        _ => reset_reading(),
                    }
                }
                (Ok(_), _, _) => Reading::warming_up(),
            };
            prev_map.insert(
                p.id.clone(),
                ProcPrev {
                    ticks,
                    io: p.io.ok(),
                },
            );
            let user = p.uid.and_then(|u| self.platform.user_name(u));
            entries.push(ProcessSnapshot {
                id: p.id,
                ppid: p.ppid,
                name: p.name,
                state: p.state,
                user,
                uid: p.uid,
                threads: p.threads,
                cpu_pct,
                rss_bytes: p.rss_bytes,
                virtual_bytes: p.virtual_bytes,
                cpu_time_s: ticks as f64 / tps.max(1) as f64,
                disk_io,
            });
        }
        self.procs.prev = prev_map;
        self.procs.last_at = Some(now);
        self.procs.quick = elapsed.is_none();
        let table = Arc::new(ProcessTable {
            timestamp_ms: now_wall,
            interval_ms: elapsed.map(|e| e.as_millis() as u64),
            logical_cores: cores,
            entries,
            unreadable: scan.unreadable,
        });
        self.pins.observe(&table);
        self.procs.table = Some(table.clone());
        Some(table)
    }
}

#[cfg(test)]
mod tests;
