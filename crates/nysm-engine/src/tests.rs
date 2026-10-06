//! Engine tests driven by a scripted, test-only platform.

use std::sync::{Arc, Mutex};
use std::thread::sleep;

use nysm_collect::{CResult, FilesystemProvider, ProcessScan};
use nysm_core::raw::*;

use super::*;

#[derive(Default, Clone)]
struct Script {
    cpu: Option<RawCpu>,
    ifaces: Vec<RawInterface>,
    disks: Vec<RawDisk>,
    procs: Vec<RawProcess>,
    psi_supported: bool,
    cgroups: Vec<RawCgroup>,
}

struct Fake(Arc<Mutex<Script>>);

fn unsupported<T>() -> CResult<T> {
    Err(CollectError::Unsupported("fake".into()))
}

struct NoFs;
impl FilesystemProvider for NoFs {
    fn filesystems(&mut self) -> CResult<Vec<FilesystemSnapshot>> {
        unsupported()
    }
}

impl Platform for Fake {
    fn host_info(&mut self) -> HostInfo {
        HostInfo {
            hostname: None,
            os: "fake".into(),
            os_version: None,
            kernel: None,
            arch: "x".into(),
            boot_id: None,
            uptime_s: None,
            scope: MeasurementScope::Unknown,
        }
    }
    fn clock_ticks_per_s(&self) -> u64 {
        100
    }
    fn page_size(&self) -> u64 {
        4096
    }
    fn cpu_times(&mut self) -> CResult<RawCpu> {
        self.0
            .lock()
            .unwrap()
            .cpu
            .clone()
            .ok_or(CollectError::PermissionDenied("nope".into()))
    }
    fn cpu_frequencies(&mut self, ids: &[u32]) -> Vec<CResult<f64>> {
        ids.iter().map(|_| unsupported()).collect()
    }
    fn load(&mut self) -> CResult<RawLoad> {
        unsupported()
    }
    fn memory(&mut self) -> CResult<RawMemory> {
        Ok(RawMemory {
            total: Some(100),
            available: Some(75),
            ..Default::default()
        })
    }
    fn swap_activity(&mut self) -> CResult<RawSwapActivity> {
        unsupported()
    }
    fn pressure(&mut self, _: PressureResource) -> CResult<RawPressure> {
        if self.0.lock().unwrap().psi_supported {
            let l = RawPsiLine {
                avg10: 0.0,
                avg60: 0.0,
                avg300: 0.0,
                total_us: 0,
            };
            Ok(RawPressure {
                some: l,
                full: None,
            })
        } else {
            unsupported()
        }
    }
    fn interfaces(&mut self) -> CResult<Vec<RawInterface>> {
        Ok(self.0.lock().unwrap().ifaces.clone())
    }
    fn disks(&mut self) -> CResult<Vec<RawDisk>> {
        Ok(self.0.lock().unwrap().disks.clone())
    }
    fn cgroups(&mut self) -> CResult<nysm_collect::CgroupScan> {
        Ok(nysm_collect::CgroupScan {
            groups: self.0.lock().unwrap().cgroups.clone(),
            truncated: 0,
        })
    }
    fn own_limits(&mut self) -> CResult<nysm_core::snapshot::OwnLimits> {
        unsupported()
    }
    fn processes(&mut self) -> CResult<ProcessScan> {
        Ok(ProcessScan {
            processes: self.0.lock().unwrap().procs.clone(),
            unreadable: 0,
        })
    }
    fn process_details(&mut self, _: u32, _: Option<u64>, _: bool) -> CResult<ProcessDetails> {
        unsupported()
    }
    fn user_name(&mut self, _: u32) -> Option<String> {
        None
    }
    fn sockets(&mut self, _: bool) -> CResult<Vec<nysm_core::sockets::SocketEntry>> {
        unsupported()
    }
    fn sensors_provider(
        &self,
    ) -> Box<dyn FnMut() -> CResult<nysm_core::snapshot::SensorsSnapshot> + Send> {
        Box::new(unsupported)
    }
    fn filesystem_provider(&self) -> Box<dyn FilesystemProvider> {
        Box::new(NoFs)
    }
    fn source(&self, _: &str) -> &'static str {
        "fake"
    }
}

fn times(busy: u64, idle: u64) -> CpuTimes {
    CpuTimes {
        user: busy,
        idle,
        ..Default::default()
    }
}

fn cpu(cores: &[(u32, u64, u64)]) -> RawCpu {
    let mut total = CpuTimes::default();
    for (_, b, i) in cores {
        total.user += b;
        total.idle += i;
    }
    RawCpu {
        total,
        per_cpu: cores
            .iter()
            .map(|(id, b, i)| (*id, times(*b, *i)))
            .collect(),
    }
}

fn iface(name: &str, kind: InterfaceKind, rx: u64) -> RawInterface {
    RawInterface {
        name: name.into(),
        kind,
        up: Some(true),
        rx_bytes: rx,
        rx_packets: 0,
        rx_errors: 0,
        rx_dropped: 0,
        tx_bytes: 0,
        tx_packets: 0,
        tx_errors: 0,
        tx_dropped: 0,
    }
}

fn proc_(pid: u32, start: u64, ticks: u64) -> RawProcess {
    RawProcess {
        id: ProcessId {
            pid,
            start_ticks: start,
        },
        ppid: 1,
        name: format!("p{pid}"),
        state: 'R',
        uid: Some(0),
        utime_ticks: ticks,
        stime_ticks: 0,
        threads: 1,
        rss_bytes: 0,
        virtual_bytes: 0,
        io: Err(Status::PermissionDenied),
    }
}

fn engine(script: Script, procs: bool) -> (Engine, Arc<Mutex<Script>>) {
    let s = Arc::new(Mutex::new(script));
    let cfg = EngineConfig {
        processes: procs,
        process_interval: Duration::ZERO,
        filesystems: false,
        frequency: false,
        ..Default::default()
    };
    (Engine::with_platform(Box::new(Fake(s.clone())), cfg), s)
}

const STEP: Duration = Duration::from_millis(120);

#[test]
fn first_sample_warms_up_never_zero() {
    let (mut e, _) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            ..Default::default()
        },
        false,
    );
    let s = e.sample();
    assert_eq!(s.cpu.usage.status, Status::WarmingUp);
    assert!(s.cpu.usage.value.is_none());
    assert_eq!(s.cpu.logical_cores.value, Some(1));
    assert_eq!(s.memory.usage.live().unwrap().used_bytes, 25);
}

#[test]
fn cpu_usage_from_deltas_and_reset() {
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0), (1, 0, 0)])),
            ..Default::default()
        },
        false,
    );
    e.sample();
    sleep(STEP);
    s.lock().unwrap().cpu = Some(cpu(&[(0, 30, 10), (1, 10, 30)]));
    let snap = e.sample();
    let u = snap.cpu.usage.live().unwrap();
    assert!((u.total_pct - 50.0).abs() < 1e-9);
    assert_eq!(snap.cpu.per_core[0].usage_pct.live(), Some(&75.0));
    // Counter reset: go backwards => re-baseline, not a huge or zero rate.
    sleep(STEP);
    s.lock().unwrap().cpu = Some(cpu(&[(0, 1, 1), (1, 1, 1)]));
    let snap = e.sample();
    assert_eq!(snap.cpu.usage.status, Status::WarmingUp);
    sleep(STEP);
    s.lock().unwrap().cpu = Some(cpu(&[(0, 2, 2), (1, 2, 2)]));
    assert!(e.sample().cpu.usage.is_available());
}

#[test]
fn cpu_hotplug_new_core_warms_up_alone() {
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            ..Default::default()
        },
        false,
    );
    e.sample();
    sleep(STEP);
    s.lock().unwrap().cpu = Some(cpu(&[(0, 10, 10), (5, 100, 100)]));
    let snap = e.sample();
    assert_eq!(snap.cpu.logical_cores.value, Some(2));
    assert!(snap.cpu.per_core[0].usage_pct.is_available());
    assert_eq!(snap.cpu.per_core[1].id, 5);
    assert_eq!(snap.cpu.per_core[1].usage_pct.status, Status::WarmingUp);
}

#[test]
fn permission_denied_is_reported_not_zero() {
    let (mut e, _) = engine(Script::default(), false);
    let s = e.sample();
    assert_eq!(s.cpu.usage.status, Status::PermissionDenied);
    assert_eq!(s.cpu.pressure.status, Status::Unsupported);
}

#[test]
fn network_total_excludes_virtual_interfaces_and_handles_hotplug() {
    let base = vec![
        iface("eth0", InterfaceKind::Physical, 0),
        iface("lo", InterfaceKind::Loopback, 0),
        iface("docker0", InterfaceKind::Bridge, 0),
    ];
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            ifaces: base,
            ..Default::default()
        },
        false,
    );
    e.sample();
    sleep(STEP);
    s.lock().unwrap().ifaces = vec![
        iface("eth0", InterfaceKind::Physical, 1000),
        iface("lo", InterfaceKind::Loopback, 50_000),
        iface("docker0", InterfaceKind::Bridge, 50_000),
        iface("wlan0", InterfaceKind::Wireless, 7),
    ];
    let snap = e.sample();
    let ifs = snap.network.interfaces.live().unwrap();
    assert!(
        !ifs.iter()
            .find(|i| i.name == "lo")
            .unwrap()
            .counted_in_total
    );
    // wlan0 just appeared: warming up, so the total is honestly warming up.
    assert_eq!(snap.network.total.status, Status::WarmingUp);
    sleep(STEP);
    s.lock().unwrap().ifaces = vec![
        iface("eth0", InterfaceKind::Physical, 2000),
        iface("wlan0", InterfaceKind::Wireless, 7),
    ];
    let snap = e.sample();
    let total = snap.network.total.live().unwrap();
    let eth = snap.network.interfaces.live().unwrap()[0]
        .rates
        .live()
        .unwrap()
        .rx_bytes_per_s;
    assert!((total.rx_bytes_per_s - eth).abs() < 1e-6);
    assert!(eth > 0.0);
}

#[test]
fn process_cpu_and_pid_reuse() {
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0), (1, 0, 0)])),
            procs: vec![proc_(10, 500, 0)],
            ..Default::default()
        },
        true,
    );
    let first = e.sample();
    assert_eq!(
        first.processes.as_ref().unwrap().entries[0].cpu_pct.status,
        Status::WarmingUp
    );
    sleep(Duration::from_millis(500));
    s.lock().unwrap().procs = vec![proc_(10, 500, 50)];
    let snap = e.sample();
    let p = &snap.processes.as_ref().unwrap().entries[0];
    // 0.5 s CPU over ~0.5 s on 2 cores ≈ 50% of the machine.
    let pct = *p.cpu_pct.live().unwrap();
    assert!(pct > 35.0 && pct <= 50.0, "{pct}");
    assert_eq!(p.disk_io.status, Status::PermissionDenied);
    // Same PID, different start time: a new process, must not inherit ticks.
    sleep(STEP);
    s.lock().unwrap().procs = vec![proc_(10, 999, 10_000)];
    let snap = e.sample();
    assert_eq!(
        snap.processes.as_ref().unwrap().entries[0].cpu_pct.status,
        Status::WarmingUp
    );
    // Process exited: simply absent.
    sleep(STEP);
    s.lock().unwrap().procs = vec![];
    assert!(e.sample().processes.unwrap().entries.is_empty());
}

#[test]
fn history_is_bounded_and_records_missing_as_none() {
    let s = Arc::new(Mutex::new(Script {
        cpu: Some(cpu(&[(0, 0, 0)])),
        ..Default::default()
    }));
    let cfg = EngineConfig {
        filesystems: false,
        history_samples: 3,
        ..Default::default()
    };
    let mut e = Engine::with_platform(Box::new(Fake(s)), cfg);
    for _ in 0..5 {
        e.sample();
    }
    assert_eq!(e.history().len(), 3);
    assert!(e.history().iter().all(|p| p.cpu_pct.is_none()));
}

#[test]
fn long_pause_marks_gap() {
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            ..Default::default()
        },
        false,
    );
    e.set_interval(Duration::from_millis(10));
    e.sample();
    sleep(Duration::from_millis(1100));
    s.lock().unwrap().cpu = Some(cpu(&[(0, 1, 1)]));
    let snap = e.sample();
    assert!(snap.gap_before);
    // Rates remain valid over the true elapsed interval.
    assert!(snap.cpu.usage.is_available());
}

#[test]
fn live_collector_coalesces_and_stops() {
    let s = Arc::new(Mutex::new(Script {
        cpu: Some(cpu(&[(0, 0, 0)])),
        ..Default::default()
    }));
    let cfg = EngineConfig {
        filesystems: false,
        interval: Duration::from_millis(20),
        ..Default::default()
    };
    let live = live::Live::spawn(Engine::with_platform(Box::new(Fake(s)), cfg));
    let a = live.wait_newer(0, Duration::from_secs(2)).unwrap();
    // A consumer that sleeps sees a later seq, not a backlog.
    sleep(Duration::from_millis(150));
    let b = live.wait_newer(a.seq, Duration::from_secs(2)).unwrap();
    assert!(b.seq > a.seq + 1);
    assert!(live.with_history(|h| h.len()) >= 2);
    let t = Instant::now();
    drop(live);
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn pinned_process_history_follows_identity() {
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            procs: vec![proc_(10, 500, 0), proc_(11, 500, 0)],
            ..Default::default()
        },
        true,
    );
    e.pin(
        ProcessId {
            pid: 10,
            start_ticks: 500,
        },
        "p10".into(),
    )
    .unwrap();
    for i in 1..=3 {
        sleep(STEP);
        s.lock().unwrap().procs = vec![proc_(10, 500, i * 5), proc_(11, 500, 0)];
        e.sample();
    }
    let p = &e.pinned()[0];
    assert_eq!(p.points.len(), 3);
    assert!(p.points.iter().skip(1).all(|x| x.cpu_pct.is_some()));
    // Process exits: history is kept and marked.
    s.lock().unwrap().procs = vec![proc_(11, 500, 0)];
    sleep(STEP);
    e.sample();
    assert!(e.pinned()[0].exited);
    assert_eq!(e.pinned()[0].points.len(), 3);
}

fn cg(usec: Option<u64>, io: u64) -> RawCgroup {
    RawCgroup {
        path: "system.slice/docker-abc.scope".into(),
        kind: CgroupKind::Container,
        name: "docker abc".into(),
        main_process: Some("nginx".into()),
        cpu_usage_usec: usec,
        cpu_max: Some((Some(50_000), 100_000)),
        memory_current: Some(100),
        memory_max: Some(1000),
        memory_high: None,
        pids_current: Some(3),
        pids_max: None,
        io_read_bytes: Some(io),
        io_write_bytes: Some(0),
        memory_pressure_some_us: None,
        cpu_pressure_some_us: Some(0),
    }
}

#[test]
fn cgroup_rates_limits_and_states() {
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0), (1, 0, 0)])),
            cgroups: vec![cg(Some(0), 0)],
            ..Default::default()
        },
        false,
    );
    e.set_cgroups(true);
    let first = e.sample();
    let g = &first.cgroups.as_ref().unwrap().groups[0];
    assert_eq!(g.cpu_pct.status, Status::WarmingUp);
    assert_eq!(g.cpu_limit_cores, Some(0.5));
    assert_eq!(g.memory_pressure_pct.status, Status::Unsupported);
    sleep(Duration::from_millis(500));
    s.lock().unwrap().cgroups = vec![cg(Some(500_000), 1_000_000)];
    let snap = e.sample();
    let g = &snap.cgroups.as_ref().unwrap().groups[0];
    // 0.5 CPU-s over ~0.5 s on 2 cores ≈ 50 % of machine.
    let pct = *g.cpu_pct.live().unwrap();
    assert!(pct > 35.0 && pct <= 50.0, "{pct}");
    assert!(g.disk_io.live().unwrap().read_bytes_per_s > 0.0);
    // Counter went backwards (group recreated): re-baseline, not a spike.
    sleep(STEP);
    s.lock().unwrap().cgroups = vec![cg(Some(10), 0)];
    let snap = e.sample();
    assert_eq!(
        snap.cgroups.as_ref().unwrap().groups[0].cpu_pct.status,
        Status::WarmingUp
    );
    // Controller missing: unsupported, never zero.
    s.lock().unwrap().cgroups = vec![cg(None, 0)];
    sleep(STEP);
    let snap = e.sample();
    assert_eq!(
        snap.cgroups.as_ref().unwrap().groups[0].cpu_pct.status,
        Status::Unsupported
    );
    // Unsubscribed: no table at all.
    e.set_cgroups(false);
    assert!(e.sample().cgroups.is_none());
}

#[test]
fn container_network_falls_back_to_veth_when_no_physical_interface() {
    let ifs = |rx| {
        vec![
            iface("lo", InterfaceKind::Loopback, rx),
            iface("eth0", InterfaceKind::VirtualEthernet, rx),
        ]
    };
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            ifaces: ifs(0),
            ..Default::default()
        },
        false,
    );
    e.sample();
    sleep(STEP);
    s.lock().unwrap().ifaces = ifs(1000);
    let snap = e.sample();
    let eth0 = snap
        .network
        .interfaces
        .live()
        .unwrap()
        .iter()
        .find(|i| i.name == "eth0")
        .unwrap()
        .clone();
    assert!(eth0.counted_in_total);
    assert!(snap.network.total.is_available());
    assert!(snap.network.total_scope.contains("no physical interface"));
    // With a physical interface present, veths stay excluded.
    let (mut e, s) = engine(
        Script {
            cpu: Some(cpu(&[(0, 0, 0)])),
            ifaces: ifs(0),
            ..Default::default()
        },
        false,
    );
    s.lock()
        .unwrap()
        .ifaces
        .push(iface("eno1", InterfaceKind::Physical, 0));
    let snap = e.sample();
    assert!(
        !snap
            .network
            .interfaces
            .live()
            .unwrap()
            .iter()
            .find(|i| i.name == "eth0")
            .unwrap()
            .counted_in_total
    );
}
