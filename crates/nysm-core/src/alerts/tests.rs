use super::*;
use crate::snapshot::*;

fn missing<T>() -> Reading<T> {
    Reading::missing(Status::Unsupported, "test")
}

fn pressure(v: Option<f64>) -> Reading<Pressure> {
    match v {
        Some(v) => Reading::ok(Pressure {
            some: PressureWindow {
                avg10_pct: 0.0,
                avg60_pct: 0.0,
                avg300_pct: 0.0,
                interval_pct: Some(v),
            },
            full: None,
        }),
        None => Reading::missing(Status::CollectionError, "gone"),
    }
}

fn fs(mount: &str, used_pct: f64, ro: bool) -> FilesystemSnapshot {
    FilesystemSnapshot {
        mount_point: mount.into(),
        source: "/dev/x".into(),
        fs_type: "ext4".into(),
        read_only: ro,
        total_bytes: 100,
        used_bytes: used_pct as u64,
        available_bytes: 100 - used_pct as u64,
        used_pct,
        also_mounted_at: vec![],
    }
}

fn snap(seq: u64, mem_psi: Option<f64>, fss: Vec<FilesystemSnapshot>, gap: bool) -> Snapshot {
    Snapshot {
        schema_version: 1,
        producer: "t".into(),
        seq,
        timestamp_ms: seq as i64 * 1000,
        interval_ms: Some(1000),
        gap_before: gap,
        host: HostInfo {
            hostname: None,
            os: "t".into(),
            os_version: None,
            kernel: None,
            arch: "x".into(),
            boot_id: None,
            uptime_s: None,
            scope: MeasurementScope::Host,
        },
        cpu: CpuSnapshot {
            logical_cores: missing(),
            usage: missing(),
            per_core: vec![],
            load: missing(),
            pressure: missing(),
        },
        memory: MemorySnapshot {
            usage: missing(),
            swap: missing(),
            swap_activity: missing(),
            pressure: pressure(mem_psi),
        },
        network: NetworkSnapshot {
            interfaces: missing(),
            total: missing(),
            total_scope: String::new(),
        },
        storage: StorageSnapshot {
            devices: missing(),
            total_io: missing(),
            filesystems: Reading::ok(fss),
            filesystems_age_ms: None,
            io_pressure: missing(),
        },
        limits: Reading::missing(Status::Unsupported, "test"),
        sensors: Reading::missing(Status::Unsupported, "test"),
        processes: None,
        cgroups: None,
    }
}

fn mem_rule(for_s: f64, cooldown_s: f64) -> Rule {
    Rule {
        id: "mem".into(),
        metric: AlertMetric::MemoryPressurePct,
        direction: Direction::Above,
        threshold: 10.0,
        clear: 5.0,
        for_s,
        cooldown_s,
        enabled: true,
    }
}

/// Feed memory-pressure values one second apart; return event kinds by step.
fn run(rule: Rule, values: &[Option<f64>]) -> (Vec<(usize, AlertEventKind)>, AlertEngine) {
    let mut e = AlertEngine::new(vec![rule]);
    let mut out = vec![];
    for (i, v) in values.iter().enumerate() {
        for ev in e.evaluate(&snap(i as u64, *v, vec![], false)) {
            out.push((i, ev.kind));
        }
    }
    (out, e)
}

#[test]
fn fires_only_after_sustained_breach() {
    // Breach at steps 1..=3 (3 s) with for_s=3: fires on the 4th breached sample.
    let (ev, _) = run(
        mem_rule(3.0, 0.0),
        &[Some(1.0), Some(20.0), Some(20.0), Some(20.0), Some(20.0)],
    );
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].0, 4);
    assert!(matches!(ev[0].1, AlertEventKind::Fired { sustained_s, .. } if sustained_s == 3.0));
}

#[test]
fn short_spikes_do_not_fire() {
    let (ev, e) = run(
        mem_rule(3.0, 0.0),
        &[
            Some(50.0),
            Some(50.0),
            Some(1.0),
            Some(50.0),
            Some(50.0),
            Some(1.0),
        ],
    );
    assert!(ev.is_empty());
    assert!(e.active().is_empty());
}

#[test]
fn hysteresis_requires_clear_threshold() {
    let (ev, _) = run(
        mem_rule(0.0, 0.0),
        &[Some(20.0), Some(8.0), Some(9.9), Some(5.0)],
    );
    let kinds: Vec<_> = ev
        .iter()
        .map(|(i, k)| (*i, std::mem::discriminant(k)))
        .collect();
    assert_eq!(kinds.len(), 2);
    assert_eq!(ev[0].0, 0);
    // 8.0 and 9.9 are below threshold but above clear: still firing.
    assert_eq!(ev[1].0, 3);
    assert!(matches!(ev[1].1, AlertEventKind::Resolved { firing_s, .. } if firing_s == 3.0));
}

#[test]
fn cooldown_suppresses_renotification() {
    let (ev, e) = run(
        mem_rule(0.0, 10.0),
        &[Some(20.0), Some(1.0), Some(20.0), Some(1.0)],
    );
    assert_eq!(ev.len(), 2, "{ev:?}"); // fired + resolved; refire inside cooldown is silent
    // The silent refire resolved silently too; state is Ok again.
    assert!(e.active().is_empty());
    let (ev, _) = run(
        mem_rule(0.0, 1.0),
        &[Some(20.0), Some(1.0), Some(1.0), Some(20.0)],
    );
    assert_eq!(ev.len(), 3, "after cooldown it notifies again: {ev:?}");
}

#[test]
fn missing_data_never_counts_as_breach_or_recovery() {
    // Pending breach interrupted by missing data resets the timer.
    let (ev, _) = run(
        mem_rule(2.0, 0.0),
        &[Some(20.0), None, Some(20.0), Some(20.0)],
    );
    assert!(ev.is_empty(), "{ev:?}");
    // Firing alert with missing data stays firing, flagged; then restored.
    let (ev, e) = run(mem_rule(0.0, 0.0), &[Some(20.0), None, None]);
    assert_eq!(ev.len(), 2, "one DataMissing only: {ev:?}");
    assert!(matches!(
        ev[1].1,
        AlertEventKind::DataMissing {
            status: Status::CollectionError
        }
    ));
    assert!(matches!(
        e.active()[0].state,
        AlertState::Firing {
            data_missing: true,
            ..
        }
    ));
    let (ev, _) = run(mem_rule(0.0, 0.0), &[Some(20.0), None, Some(1.0)]);
    assert!(matches!(ev[2].1, AlertEventKind::DataRestored));
    assert!(matches!(ev[3].1, AlertEventKind::Resolved { .. }));
}

#[test]
fn gaps_reset_pending_timers() {
    let mut e = AlertEngine::new(vec![mem_rule(2.0, 0.0)]);
    assert!(e.evaluate(&snap(0, Some(20.0), vec![], false)).is_empty());
    assert!(e.evaluate(&snap(1, Some(20.0), vec![], false)).is_empty());
    // Without the gap, step 2 would fire (2 s sustained). After a suspend
    // the breach restarts at step 2 and needs 2 more observed seconds.
    assert!(e.evaluate(&snap(2, Some(20.0), vec![], true)).is_empty());
    assert!(e.evaluate(&snap(3, Some(20.0), vec![], false)).is_empty());
    assert_eq!(e.evaluate(&snap(4, Some(20.0), vec![], false)).len(), 1);
}

#[test]
fn filesystem_rules_track_each_mount_and_skip_read_only() {
    let rule = Rule {
        for_s: 0.0,
        ..default_rules()
            .into_iter()
            .find(|r| r.id == "filesystem-nearly-full")
            .unwrap()
    };
    let mut e = AlertEngine::new(vec![rule]);
    let ev = e.evaluate(&snap(
        0,
        None,
        vec![
            fs("/", 96.0, false),
            fs("/data", 50.0, false),
            fs("/snap/x", 100.0, true),
        ],
        false,
    ));
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].target.as_deref(), Some("/"));
    assert!(ev[0].describe().contains("filesystem used on / at 96.0%"));
    assert_eq!(e.firing_count(), 1);
}

#[test]
fn rule_validation() {
    assert!(mem_rule(1.0, 0.0).validate().is_ok());
    assert!(
        Rule {
            clear: 20.0,
            ..mem_rule(1.0, 0.0)
        }
        .validate()
        .is_err()
    );
    assert!(
        Rule {
            for_s: -1.0,
            ..mem_rule(1.0, 0.0)
        }
        .validate()
        .is_err()
    );
    assert!(
        Rule {
            threshold: f64::NAN,
            ..mem_rule(1.0, 0.0)
        }
        .validate()
        .is_err()
    );
    let below = Rule {
        direction: Direction::Below,
        threshold: 5.0,
        clear: 10.0,
        ..mem_rule(1.0, 0.0)
    };
    assert!(below.validate().is_ok());
    assert!(default_rules().iter().all(|r| r.validate().is_ok()));
}

#[test]
fn cpu_temperature_rule_uses_degrees() {
    use crate::snapshot::{SensorClass, SensorsSnapshot, Temperature};
    let rule = Rule {
        id: "hot".into(),
        metric: AlertMetric::CpuTemperatureC,
        direction: Direction::Above,
        threshold: 85.0,
        clear: 75.0,
        for_s: 0.0,
        cooldown_s: 0.0,
        enabled: true,
    };
    let mut e = AlertEngine::new(vec![rule.clone()]);
    let mut s = snap(0, None, vec![], false);
    s.sensors = Reading::ok(SensorsSnapshot {
        temperatures: vec![Temperature {
            chip: "coretemp".into(),
            label: "Package id 0".into(),
            class: SensorClass::Cpu,
            celsius: 91.0,
            high_celsius: Some(80.0),
            critical_celsius: Some(100.0),
        }],
        ..Default::default()
    });
    let ev = e.evaluate(&s);
    assert_eq!(ev.len(), 1);
    assert!(
        ev[0]
            .describe()
            .contains("CPU temperature at 91.0 °C stayed beyond 85.0 °C"),
        "{}",
        ev[0].describe()
    );
    // No sensors: never fires.
    let mut e = AlertEngine::new(vec![rule]);
    assert!(e.evaluate(&snap(1, None, vec![], false)).is_empty());
}
