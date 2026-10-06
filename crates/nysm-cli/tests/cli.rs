//! End-to-end tests of the `nysm` binary against the real host.

use std::process::{Command, Output};

fn nysm(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nysm"))
        .args(args)
        .env_remove("NO_COLOR")
        .output()
        .expect("run nysm")
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("bad JSON ({e}): {}", String::from_utf8_lossy(&o.stdout)))
}

#[test]
fn usage_errors_exit_2() {
    assert_eq!(
        nysm(&["summary", "--warmup", "bogus"]).status.code(),
        Some(2)
    );
    assert_eq!(
        nysm(&["summary", "--warmup", "10ms"]).status.code(),
        Some(2)
    );
    assert_eq!(nysm(&["no-such-command"]).status.code(), Some(2));
}

#[test]
fn help_has_examples_and_exit_codes() {
    let o = nysm(&["--help"]);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.contains("Examples:") && s.contains("Exit codes"));
}

#[test]
fn redirected_output_has_no_ansi_escapes() {
    let o = nysm(&["summary", "--warmup", "200ms", "--top", "3"]);
    assert!(o.status.success() || o.status.code() == Some(3));
    assert!(
        !o.stdout.contains(&0x1b),
        "ANSI escape in redirected output"
    );
    let forced = nysm(&[
        "--color", "always", "summary", "--warmup", "200ms", "--top", "0",
    ]);
    assert!(forced.stdout.contains(&0x1b));
    let no_color = Command::new(env!("CARGO_BIN_EXE_nysm"))
        .args([
            "--color", "auto", "summary", "--warmup", "200ms", "--top", "0",
        ])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(!no_color.stdout.contains(&0x1b));
}

#[cfg(target_os = "linux")]
#[test]
fn summary_json_is_versioned_and_never_fakes_zero() {
    let o = nysm(&["summary", "--json", "--warmup", "300ms", "--top", "2"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v = json(&o);
    assert_eq!(v["schema_version"], 1);
    assert!(v["producer"].as_str().unwrap().starts_with("nysm/"));
    assert_eq!(v["cpu"]["usage"]["status"], "available");
    let pct = v["cpu"]["usage"]["value"]["total_pct"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&pct));
    assert!(v["processes"]["entries"].as_array().unwrap().len() <= 2);
    // Every reading carries a status.
    assert!(v["memory"]["pressure"]["status"].is_string());
}

#[cfg(target_os = "linux")]
#[test]
fn watch_jsonl_count_and_sequence() {
    let o = nysm(&[
        "watch",
        "--interval",
        "150ms",
        "--format",
        "jsonl",
        "--count",
        "3",
    ]);
    assert!(o.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    let seqs: Vec<u64> = lines.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    // The baseline sample is not emitted, so every line has real rates.
    assert!(
        lines
            .iter()
            .all(|l| l["cpu"]["usage"]["status"] == "available")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn processes_and_inspect() {
    let o = nysm(&[
        "processes",
        "--json",
        "--sort",
        "mem",
        "--limit",
        "5",
        "--warmup",
        "200ms",
    ]);
    let v = json(&o);
    let procs = v["processes"].as_array().unwrap();
    assert!(!procs.is_empty() && procs.len() <= 5);
    let rss: Vec<u64> = procs
        .iter()
        .map(|p| p["rss_bytes"].as_u64().unwrap())
        .collect();
    assert!(
        rss.windows(2).all(|w| w[0] >= w[1]),
        "sorted by memory desc"
    );

    let me = std::process::id().to_string();
    let o = nysm(&["inspect", "--pid", &me, "--json", "--warmup", "100ms"]);
    assert_eq!(o.status.code(), Some(0));
    let v = json(&o);
    assert_eq!(v["process"]["pid"].as_u64().unwrap().to_string(), me);
    assert!(v.get("cmdline").is_none(), "cmdline must be opt-in");
    assert_eq!(v["exe"]["status"], "available");

    let o = nysm(&[
        "inspect",
        "--pid",
        &me,
        "--json",
        "--show-args",
        "--warmup",
        "100ms",
    ]);
    assert!(json(&o)["cmdline"]["value"].is_array());

    assert_eq!(
        nysm(&["inspect", "--pid", "4294967", "--warmup", "100ms"])
            .status
            .code(),
        Some(4)
    );
}

#[test]
fn capabilities_json() {
    let v = json(&nysm(&["capabilities", "--json"]));
    let caps = v["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c["id"] == "cpu.usage"));
    assert!(
        caps.iter()
            .all(|c| c["status"].is_string() && c["unit"].is_string())
    );
}

#[test]
fn tui_refuses_non_terminal() {
    let o = nysm(&["tui"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("interactive terminal"));
}

#[test]
fn doctor_runs() {
    let o = nysm(&["doctor"]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("doctor"));
}

#[cfg(target_os = "linux")]
#[test]
fn ports_finds_our_listener_and_owner() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port().to_string();
    let o = nysm(&["ports", "--port", &port, "--json"]);
    assert_eq!(o.status.code(), Some(0));
    let v = json(&o);
    let s = &v["sockets"][0];
    assert_eq!(s["state"], "listen");
    assert_eq!(s["protocol"], "tcp");
    assert_eq!(
        s["owners"][0]["pid"].as_u64().unwrap(),
        std::process::id() as u64
    );
    drop(l);
    // Nothing on the port any more (except possibly TIME_WAIT): listening-only search.
    let o = nysm(&["ports", "--port", &port, "--json"]);
    let v = json(&o);
    assert!(
        v["sockets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] != "listen")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn record_command_then_compare() {
    let dir = std::env::temp_dir().join(format!("nysm-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.nysm");
    let b = dir.join("b.nysm");
    // The command's exit status is propagated.
    let o = nysm(&[
        "record",
        "-o",
        a.to_str().unwrap(),
        "--interval",
        "100ms",
        "--",
        "sh",
        "-c",
        "sleep 0.4; exit 7",
    ]);
    assert_eq!(
        o.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // Never overwrites without --force.
    let o = nysm(&["record", "-o", a.to_str().unwrap(), "--", "true"]);
    assert_eq!(o.status.code(), Some(1));
    let o = nysm(&[
        "record",
        "-o",
        b.to_str().unwrap(),
        "--interval",
        "100ms",
        "--duration",
        "300ms",
    ]);
    assert_eq!(o.status.code(), Some(0));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&a).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let v = json(&nysm(&["compare", a.to_str().unwrap(), "--json"]));
    assert_eq!(v["first"]["command"]["exit_code"], 7);
    assert!(v["first"]["command"]["wall_s"].as_f64().unwrap() >= 0.4);
    assert!(v["first"]["samples"].as_u64().unwrap() >= 2);
    let v = json(&nysm(&[
        "compare",
        a.to_str().unwrap(),
        b.to_str().unwrap(),
        "--json",
    ]));
    assert!(
        v["comparison"]["caveats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().contains("command rows are omitted"))
    );
    // Garbage input is a clean failure, not a panic.
    std::fs::write(dir.join("junk.nysm"), "hello").unwrap();
    assert_eq!(
        nysm(&["compare", dir.join("junk.nysm").to_str().unwrap()])
            .status
            .code(),
        Some(1)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn config_lifecycle_and_invalid_config_fallback() {
    let dir = std::env::temp_dir().join(format!("nysm-cfgcli-{}", std::process::id()));
    let path = dir.join("config.toml");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nysm"))
            .args(args)
            .env("NYSM_CONFIG", &path)
            .output()
            .unwrap()
    };
    assert_eq!(run(&["config", "check"]).status.code(), Some(0));
    assert_eq!(run(&["config", "init"]).status.code(), Some(0));
    assert_eq!(
        run(&["config", "init"]).status.code(),
        Some(1),
        "no silent overwrite"
    );
    assert_eq!(run(&["config", "check"]).status.code(), Some(0));
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[alerts.rules]]\nid = \"always\"\nmetric = \"cpu_pct\"\nthreshold = -1.0\nclear = -2.0\nfor_s = 0.0\n");
    std::fs::write(&path, &text).unwrap();
    let o = run(&["alerts", "--list"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("always"));
    // A rule that is always breached fires on the first rated sample.
    let o = run(&[
        "alerts",
        "--interval",
        "200ms",
        "--count",
        "2",
        "--format",
        "jsonl",
    ]);
    let first = String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .map(str::to_string)
        .unwrap_or_default();
    assert!(
        first.contains("\"event\":\"fired\"") && first.contains("\"rule\":\"always\""),
        "{first}"
    );
    // Invalid config: commands still work on defaults, with a warning; check exits 2.
    std::fs::write(&path, "version = 1\n[sampling]\ninterval = \"forever\"\n").unwrap();
    let o = run(&["summary", "--warmup", "100ms", "--top", "0"]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("ignoring invalid configuration"));
    assert_eq!(run(&["config", "check"]).status.code(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn groups_json_reports_cgroups() {
    let o = nysm(&["groups", "--json", "--warmup", "300ms", "--limit", "5"]);
    if o.status.code() == Some(1) {
        // No cgroup v2 here (e.g. some CI containers): must say so on stderr.
        assert!(String::from_utf8_lossy(&o.stderr).contains("cgroup"));
        return;
    }
    let v = json(&o);
    let groups = v["groups"].as_array().unwrap();
    assert!(groups.len() <= 5);
    for g in groups {
        assert!(g["kind"].is_string() && g["name"].is_string());
        assert!(g["cpu_pct"]["status"].is_string());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn failed_services_never_errors_without_systemd() {
    let o = nysm(&["services", "--failed", "--json"]);
    let v = json(&o);
    assert!(v["failed"].is_array());
    // Either systemd answered (OK) or both managers are reported unavailable (exit 1).
    assert!(o.status.code() == Some(0) || v["errors"].as_array().unwrap().len() == 2);
}

#[cfg(target_os = "linux")]
#[test]
fn inspect_shows_own_listening_socket_and_group() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let me = std::process::id().to_string();
    let v = json(&nysm(&[
        "inspect", "--pid", &me, "--json", "--warmup", "100ms",
    ]));
    let socks = v["sockets"]["value"]
        .as_array()
        .expect("sockets readable for own process");
    assert!(
        socks
            .iter()
            .any(|s| s["state"] == "listen" && s["local_port"] == port),
        "{socks:?}"
    );
    // Every process on a cgroup v2 system belongs to some group.
    if v["cgroup"]["status"] == "available" {
        assert!(v["belongs_to"]["kind"].is_string());
    }
    drop(l);
}
