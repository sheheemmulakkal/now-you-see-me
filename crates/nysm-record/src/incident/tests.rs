use std::time::Duration;

use nysm_core::alerts::{AlertEvent, AlertEventKind, AlertMetric};

use super::*;
use crate::compare::tests::snap;
use crate::reader;

fn cfg(dir: &Path, max_files: usize, max_total_bytes: u64) -> IncidentConfig {
    IncidentConfig {
        dir: dir.to_path_buf(),
        pre: Duration::from_secs(3),
        post: Duration::from_secs(2),
        interval: Duration::from_secs(1),
        max_files,
        max_total_bytes,
    }
}

fn fired(seq: u64, rule: &str) -> AlertEvent {
    AlertEvent {
        rule: rule.into(),
        metric: AlertMetric::MemoryPressurePct,
        target: None,
        timestamp_ms: seq as i64 * 1000,
        kind: AlertEventKind::Fired {
            value: 20.0,
            threshold: 10.0,
            sustained_s: 30.0,
        },
    }
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nysm-inc-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn captures_pre_and_post_window_with_event() {
    let d = tmpdir("window");
    let mut r = IncidentRecorder::new(cfg(&d, 10, 1 << 30)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&d).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    // 10 quiet samples: only the last 3+1 are kept as the pre-window.
    for i in 1..=10 {
        assert!(
            r.observe(&snap(i, Some(1.0), None, 1000), &[])
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(r.buffered_samples(), 4);
    // Alert fires at sample 11; the capture then needs 2 more samples.
    assert!(
        r.observe(
            &snap(11, Some(90.0), None, 1000),
            &[fired(11, "memory-pressure")]
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        r.observe(&snap(12, Some(90.0), None, 1000), &[])
            .unwrap()
            .is_empty()
    );
    let done = r.observe(&snap(13, Some(90.0), None, 1000), &[]).unwrap();
    assert_eq!(done.len(), 1);
    let mut seqs = Vec::new();
    let c = reader::read(&done[0], reader::DEFAULT_READ_LIMIT, |s| {
        seqs.push(s.snapshot.seq)
    })
    .unwrap();
    assert_eq!(seqs, vec![8, 9, 10, 11, 12, 13]);
    assert!(
        matches!(c.header.target, Target::Incident { ref rule, .. } if rule == "memory-pressure")
    );
    assert!(
        c.events
            .iter()
            .any(|e| matches!(e.kind, EventKind::AlertFired { .. }))
    );
    assert!(
        c.end
            .is_some_and(|e| !e.truncated && e.dropped_samples == 0)
    );
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn skipped_samples_are_counted_and_shutdown_truncates() {
    let d = tmpdir("drop");
    // Long post-window so the capture is still open at shutdown.
    let mut r = IncidentRecorder::new(IncidentConfig {
        post: Duration::from_secs(60),
        ..cfg(&d, 10, 1 << 30)
    })
    .unwrap();
    r.observe(&snap(1, Some(1.0), None, 1000), &[fired(1, "a")])
        .unwrap();
    // Caller fell behind: seq jumps 1 -> 4.
    r.observe(&snap(4, Some(1.0), None, 1000), &[]).unwrap();
    let done = r.finish_all(5000).unwrap();
    assert_eq!(done.len(), 1);
    let c = reader::read(&done[0], reader::DEFAULT_READ_LIMIT, |_| {}).unwrap();
    let end = c.end.unwrap();
    assert!(end.truncated);
    assert_eq!(end.dropped_samples, 2);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn retention_keeps_newest_within_limits_and_dedupes_open_rules() {
    let d = tmpdir("retention");
    let mut r = IncidentRecorder::new(IncidentConfig {
        post: Duration::ZERO,
        ..cfg(&d, 2, 1 << 30)
    })
    .unwrap();
    for i in 1..=4u64 {
        // Same rule twice in one sample: only one capture.
        r.observe(
            &snap(i, Some(1.0), None, 1000),
            &[fired(i, "x"), fired(i, "x")],
        )
        .unwrap();
    }
    let files = list(&d).unwrap();
    assert_eq!(files.len(), 2, "{files:?}");
    assert_eq!(files[0].created_ms, 4000);
    assert_eq!(files[1].created_ms, 3000);
    std::fs::remove_dir_all(&d).unwrap();
}
