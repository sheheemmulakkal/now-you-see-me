//! End-to-end tests of the collector service over a real Unix socket.
#![cfg(target_os = "linux")]

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nysm_engine::EngineConfig;
use nysm_ipc::client::RemoteLive;
use nysm_ipc::protocol::*;
use nysm_ipc::server::{self, ServeError, ServerOptions};
use nysm_ipc::{paths, source};

fn dir(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("nysm-ipc-{}-{name}", std::process::id()))
        .join("rt")
}

fn opts(stop: Arc<AtomicBool>) -> ServerOptions {
    ServerOptions {
        engine: EngineConfig {
            interval: Duration::from_millis(200),
            filesystems: false,
            ..Default::default()
        },
        rules: vec![],
        idle_exit: None,
        max_clients: 4,
        log_alerts: false,
        handle_signals: false,
        stop,
        incidents: None,
    }
}

struct Server {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Result<(), ServeError>>>,
    dir: PathBuf,
}

impl Server {
    fn start(name: &str) -> Server {
        let d = dir(name);
        let stop = Arc::new(AtomicBool::new(false));
        let (d2, s2) = (d.clone(), stop.clone());
        let handle = thread::spawn(move || server::run(opts(s2), &d2));
        let t = Instant::now();
        while UnixStream::connect(paths::socket_path(&d)).is_err() {
            assert!(t.elapsed() < Duration::from_secs(5), "server did not start");
            thread::sleep(Duration::from_millis(20));
        }
        Server {
            stop,
            handle: Some(handle),
            dir: d,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_dir_all(self.dir.parent().unwrap());
    }
}

#[test]
fn clients_share_one_collector_and_receive_complete_history() {
    let srv = Server::start("share");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(paths::socket_path(&srv.dir))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    let a = RemoteLive::connect(&srv.dir, "test-a", Subscribe::default()).unwrap();
    let b = RemoteLive::connect(
        &srv.dir,
        "test-b",
        Subscribe {
            processes: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut seq = 0;
    for _ in 0..4 {
        seq = a
            .wait_newer(seq, Duration::from_secs(3))
            .expect("update")
            .seq;
    }
    // Same producer seq space: both clients see the one collector's samples.
    let sb = b.wait_newer(0, Duration::from_secs(3)).unwrap();
    assert!(sb.seq >= 1);
    // Process tables only for the subscribed client (may need one process interval).
    let t = Instant::now();
    while b.latest().unwrap().processes.is_none() {
        assert!(t.elapsed() < Duration::from_secs(5));
        b.wait_newer(b.latest().unwrap().seq, Duration::from_secs(3));
    }
    assert!(a.latest().unwrap().processes.is_none());
    // History has no holes even if snapshots were coalesced.
    let seqs: Vec<u64> = a.with_state(|s| s.history.iter().map(|p| p.seq).collect());
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
    // Details round trip for our own process.
    let me = std::process::id();
    let rx = b.request_details(me, None);
    let d = rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assert!(d.exe.is_ok() && d.cmdline.is_none());
}

#[test]
fn second_instance_is_refused_and_stale_socket_is_replaced() {
    let srv = Server::start("single");
    let err = server::run(opts(Arc::new(AtomicBool::new(true))), &srv.dir).unwrap_err();
    assert!(matches!(err, ServeError::AlreadyRunning(_)));
    drop(srv);
    // A leftover socket file without a running server must not block a new one.
    let d = dir("stale");
    paths::ensure_private_dir(&d).unwrap();
    std::fs::write(paths::socket_path(&d), "").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (d2, s2) = (d.clone(), stop.clone());
    let h = thread::spawn(move || server::run(opts(s2), &d2));
    let t = Instant::now();
    while RemoteLive::connect(&d, "t", Subscribe::default()).is_err() {
        assert!(t.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(d.parent().unwrap());
}

#[test]
fn version_mismatch_and_garbage_are_rejected_cleanly() {
    let srv = Server::start("version");
    let mut s = UnixStream::connect(paths::socket_path(&srv.dir)).unwrap();
    write_frame(
        &mut s,
        &ClientMsg::Hello {
            protocol_version: 99,
            client: "x".into(),
            subscribe: Subscribe::default(),
        },
    )
    .unwrap();
    match read_frame::<ServerMsg, _>(&mut s).unwrap() {
        ServerMsg::Error { message } => assert!(message.contains("unsupported")),
        other => panic!("{other:?}"),
    }
    // Garbage frame: the server drops the connection without affecting others.
    let mut g = UnixStream::connect(paths::socket_path(&srv.dir)).unwrap();
    use std::io::Write;
    g.write_all(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3]).unwrap();
    let ok = RemoteLive::connect(&srv.dir, "after-garbage", Subscribe::default()).unwrap();
    assert!(ok.wait_newer(0, Duration::from_secs(3)).is_some());
}

#[test]
fn stalled_client_does_not_delay_others() {
    let srv = Server::start("slow");
    // A client that subscribes to process tables and never reads.
    let mut stalled = UnixStream::connect(paths::socket_path(&srv.dir)).unwrap();
    write_frame(
        &mut stalled,
        &ClientMsg::Hello {
            protocol_version: 1,
            client: "stalled".into(),
            subscribe: Subscribe {
                processes: true,
                ..Default::default()
            },
        },
    )
    .unwrap();
    let live = RemoteLive::connect(&srv.dir, "live", Subscribe::default()).unwrap();
    let first = live.wait_newer(0, Duration::from_secs(3)).unwrap().seq;
    thread::sleep(Duration::from_secs(3));
    let later = live.wait_newer(first, Duration::from_secs(3)).unwrap().seq;
    // 200 ms interval over ~3 s: expect ~15 samples; allow generous slack.
    assert!(later - first >= 10, "collector stalled: {first} -> {later}");
    drop(stalled);
}

#[test]
fn source_falls_back_to_local_or_requires_service() {
    // SAFETY: test-only env change before any thread reads it here.
    unsafe { std::env::set_var("NYSM_RUNTIME_DIR", dir("none")) };
    let cfg = EngineConfig {
        filesystems: false,
        ..Default::default()
    };
    let s = source::Source::open(source::Attach::Auto, "t", cfg.clone(), vec![]).unwrap();
    assert!(!s.is_remote());
    assert!(source::Source::open(source::Attach::Require, "t", cfg, vec![]).is_err());
}

#[test]
fn remote_detects_disconnect() {
    let srv = Server::start("disc");
    let c = RemoteLive::connect(&srv.dir, "t", Subscribe::default()).unwrap();
    assert!(c.wait_newer(0, Duration::from_secs(3)).is_some());
    drop(srv);
    let t = Instant::now();
    while c.is_connected() {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "disconnect not noticed"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn lite_subscription_keeps_totals_only() {
    let srv = Server::start("lite");
    let c = RemoteLive::connect(
        &srv.dir,
        "panel",
        Subscribe {
            lite: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut s = c.wait_newer(0, Duration::from_secs(3)).unwrap();
    // Rates need a second sample.
    s = c.wait_newer(s.seq, Duration::from_secs(3)).unwrap();
    assert!(s.cpu.usage.is_available());
    assert!(s.cpu.per_core.is_empty());
    assert!(s.network.interfaces.value.is_none());
    assert!(
        s.network.total.value.is_some() || s.network.total.status != nysm_core::Status::Available
    );
}
