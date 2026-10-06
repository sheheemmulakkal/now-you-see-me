use std::io::Cursor;
use std::path::PathBuf;

use nysm_core::Reading;
use nysm_core::snapshot::*;

use super::*;
use crate::format::*;
use crate::reader::{self, ReadError};
use crate::writer::{RecordingWriter, WriteOutcome};

fn host() -> HostInfo {
    HostInfo {
        hostname: Some("h".into()),
        os: "test".into(),
        os_version: None,
        kernel: None,
        arch: "x".into(),
        boot_id: Some("b".into()),
        uptime_s: None,
        scope: MeasurementScope::Host,
    }
}

fn missing<T>() -> Reading<T> {
    Reading::missing(nysm_core::Status::Unsupported, "test")
}

pub(crate) fn snap(seq: u64, cpu: Option<f64>, rx: Option<f64>, interval_ms: u64) -> Snapshot {
    Snapshot {
        schema_version: 1,
        producer: "t".into(),
        seq,
        timestamp_ms: seq as i64 * interval_ms as i64,
        interval_ms: Some(interval_ms),
        gap_before: false,
        host: host(),
        cpu: CpuSnapshot {
            logical_cores: Reading::ok(4),
            usage: cpu.map_or_else(Reading::warming_up, |c| {
                Reading::ok(CpuBreakdown {
                    total_pct: c,
                    user_pct: c,
                    nice_pct: 0.0,
                    system_pct: 0.0,
                    irq_pct: 0.0,
                    iowait_pct: 0.0,
                    steal_pct: 0.0,
                    idle_pct: 100.0 - c,
                })
            }),
            per_core: vec![],
            load: missing(),
            pressure: missing(),
        },
        memory: MemorySnapshot {
            usage: missing(),
            swap: missing(),
            swap_activity: missing(),
            pressure: missing(),
        },
        network: NetworkSnapshot {
            interfaces: missing(),
            total: rx.map_or_else(missing, |r| {
                Reading::ok(NetworkRates {
                    rx_bytes_per_s: r,
                    ..Default::default()
                })
            }),
            total_scope: String::new(),
        },
        storage: StorageSnapshot {
            devices: missing(),
            total_io: missing(),
            filesystems: missing(),
            filesystems_age_ms: None,
            io_pressure: missing(),
        },
        limits: Reading::missing(nysm_core::Status::Unsupported, "test"),
        sensors: Reading::missing(nysm_core::Status::Unsupported, "test"),
        processes: None,
        cgroups: None,
    }
}

fn header(target: Target) -> Header {
    Header {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        producer: "t".into(),
        schema_version: 1,
        created_ms: 0,
        host: host(),
        interval_ms: 1000,
        target,
        label: None,
    }
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("nysm-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn sample(seq: u64, cpu: Option<f64>, rx: Option<f64>) -> Sample {
    Sample {
        snapshot: snap(seq, cpu, rx, 1000),
        top: vec![],
        tree: None,
    }
}

fn summarize(path: &std::path::Path) -> Summary {
    let mut s = Summarizer::default();
    let c = reader::read(path, reader::DEFAULT_READ_LIMIT, |x| s.add(x)).unwrap();
    let h = c.header.clone();
    s.finish(&h, &c)
}

#[test]
fn round_trip_with_stats_and_totals() {
    let p = tmp("rt");
    let mut w = RecordingWriter::create(&p, &header(Target::Window), 1 << 20, false).unwrap();
    for (i, cpu) in [Some(10.0), None, Some(30.0), Some(20.0)]
        .into_iter()
        .enumerate()
    {
        w.sample(sample(i as u64, cpu, Some(1000.0))).unwrap();
    }
    w.event(Event {
        timestamp_ms: 5,
        kind: EventKind::CommandStarted,
    })
    .unwrap();
    w.finish(End {
        timestamp_ms: 9,
        samples: 0,
        dropped_samples: 0,
        truncated: false,
        command: None,
    })
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // Refuses to overwrite by default.
    assert!(RecordingWriter::create(&p, &header(Target::Window), 1 << 20, false).is_err());
    let s = summarize(&p);
    assert_eq!(s.samples, 4);
    assert_eq!(s.cpu_pct.samples, 3);
    assert_eq!(s.cpu_pct.missing, 1);
    assert_eq!(s.cpu_pct.mean, Some(20.0));
    assert_eq!(s.cpu_pct.p95, Some(30.0));
    assert_eq!(s.net_rx.bytes, 4000.0);
    assert!(!s.truncated);
    std::fs::remove_file(&p).unwrap();
}

#[test]
fn size_limit_truncates_with_accounting() {
    let p = tmp("limit");
    let mut w = RecordingWriter::create(&p, &header(Target::Window), 6000, false).unwrap();
    let mut written = 0;
    for i in 0..50 {
        match w.sample(sample(i, Some(1.0), None)).unwrap() {
            WriteOutcome::Written => written += 1,
            WriteOutcome::LimitReached => break,
        }
    }
    assert!(written > 0 && written < 50);
    w.finish(End {
        timestamp_ms: 0,
        samples: 0,
        dropped_samples: 0,
        truncated: false,
        command: None,
    })
    .unwrap();
    assert!(std::fs::metadata(&p).unwrap().len() <= 6000);
    let s = summarize(&p);
    assert!(s.truncated);
    assert_eq!(s.dropped_samples, 1);
    std::fs::remove_file(&p).unwrap();
}

fn body(lines: &[String]) -> Cursor<Vec<u8>> {
    Cursor::new(lines.join("\n").into_bytes())
}

fn header_line() -> String {
    serde_json::to_string(&Record::Header(Box::new(header(Target::Window)))).unwrap()
}

fn sample_line(i: u64) -> String {
    serde_json::to_string(&Record::Sample(Box::new(sample(i, Some(5.0), None)))).unwrap()
}

#[test]
fn truncated_final_line_is_tolerated_and_flagged() {
    let mut partial = sample_line(2);
    partial.truncate(partial.len() / 2);
    let mut n = 0;
    let c = reader::read_from(body(&[header_line(), sample_line(1), partial]), |_| n += 1).unwrap();
    assert_eq!(n, 1);
    assert!(c.end.is_none());
    assert_eq!(c.warnings.len(), 2);
}

#[test]
fn corrupt_middle_line_is_an_error() {
    let r = reader::read_from(
        body(&[header_line(), "{garbage".into(), sample_line(1)]),
        |_| {},
    );
    assert!(matches!(r, Err(ReadError::Corrupt { line: 2, .. })));
}

#[test]
fn newer_versions_and_foreign_files_are_rejected() {
    let mut h = serde_json::to_value(Record::Header(Box::new(header(Target::Window)))).unwrap();
    h["format_version"] = 99.into();
    let r = reader::read_from(body(&[h.to_string()]), |_| {});
    assert!(matches!(
        r,
        Err(ReadError::UnsupportedVersion { found: 99, .. })
    ));
    assert!(matches!(
        reader::read_from(body(&["{\"a\":1}".into()]), |_| {}),
        Err(ReadError::NotARecording(_))
    ));
    assert!(matches!(
        reader::read_from(body(&["not json".into()]), |_| {}),
        Err(ReadError::NotARecording(_))
    ));
    assert!(matches!(
        reader::read_from(Cursor::new(Vec::new()), |_| {}),
        Err(ReadError::NotARecording(_))
    ));
}

#[test]
fn read_limit_is_enforced() {
    let p = tmp("big");
    std::fs::write(&p, vec![b'x'; 2048]).unwrap();
    assert!(matches!(
        reader::read(&p, 1024, |_| {}),
        Err(ReadError::TooLarge { .. })
    ));
    std::fs::remove_file(&p).unwrap();
}

fn summary_with(cpu: f64, cmd_cpu: f64, n: u64) -> Summary {
    let mut s = Summarizer::default();
    for i in 0..n {
        s.add(&sample(i, Some(cpu), Some(100.0)));
    }
    let h = header(Target::Command {
        program: "make".into(),
        args: None,
        root_pid: 1,
    });
    let c = reader::Contents {
        header: h.clone(),
        end: Some(End {
            timestamp_ms: 0,
            samples: n,
            dropped_samples: 0,
            truncated: false,
            command: Some(CommandResult {
                exit_code: Some(0),
                signal: None,
                wall_s: 10.0,
                user_cpu_s: Some(cmd_cpu),
                system_cpu_s: Some(0.0),
                max_single_process_rss_bytes: Some(1000),
            }),
        }),
        events: vec![],
        warnings: vec![],
    };
    s.finish(&h, &c)
}

#[test]
fn comparison_reports_changes_beyond_noise_only() {
    let a = summary_with(50.0, 40.0, 20);
    let b = summary_with(51.0, 30.0, 20);
    let c = compare(&a, &b);
    let cpu_time = c
        .rows
        .iter()
        .find(|r| r.metric == "command CPU time (user+sys)")
        .unwrap();
    assert!(cpu_time.exact);
    assert_eq!(cpu_time.change_pct, Some(-25.0));
    assert!(
        c.observations
            .iter()
            .any(|o| o.contains("CPU time") && o.contains("25% lower") && o.contains("exact"))
    );
    // 2% change in system CPU is within noise.
    assert!(
        !c.observations
            .iter()
            .any(|o| o.starts_with("system CPU mean"))
    );
    assert!(c.caveats[0].contains("do not establish cause"));
    let same = compare(&a, &a);
    assert!(same.observations[0].starts_with("No metric changed"));
    let short = compare(&summary_with(1.0, 1.0, 3), &a);
    assert!(short.caveats.iter().any(|c| c.contains("Fewer than 10")));
}

#[test]
fn compaction_keeps_counted_and_busiest_rows_only() {
    use nysm_core::raw::InterfaceKind;
    let mut s = snap(1, Some(1.0), Some(1.0), 1000);
    let iface = |name: &str, counted: bool, rate: f64| InterfaceSnapshot {
        name: name.into(),
        kind: InterfaceKind::Virtual,
        up: Some(true),
        counted_in_total: counted,
        rx_total_bytes: 0,
        tx_total_bytes: 0,
        rates: Reading::ok(NetworkRates {
            rx_bytes_per_s: rate,
            ..Default::default()
        }),
    };
    let mut rows = vec![iface("eth0", true, 0.0)];
    for i in 0..20 {
        rows.push(iface(&format!("v{i}"), false, i as f64));
    }
    s.network.interfaces = Reading::ok(rows);
    compact_snapshot(&mut s);
    let kept: Vec<String> = s
        .network
        .interfaces
        .value
        .unwrap()
        .into_iter()
        .map(|i| i.name)
        .collect();
    assert_eq!(kept.len(), 1 + MAX_EXTRA_ROWS);
    assert_eq!(kept[0], "eth0", "counted rows are kept even when idle");
    assert!(kept.contains(&"v19".to_string()) && !kept.contains(&"v0".to_string()));
}
