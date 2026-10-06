//! Optional systemd provider: unit states via `systemctl`, on demand only.
//! Missing systemd (containers, other init systems, non-Linux) is reported
//! as unsupported and never breaks other features.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{CResult, CollectError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitState {
    pub unit: String,
    pub load: String,
    /// active | inactive | failed | activating | deactivating | reloading
    pub active: String,
    /// e.g. running, exited, dead, failed
    pub sub: String,
    #[serde(default)]
    pub description: String,
}

/// Parse `systemctl list-units -o json` output.
pub fn parse_units(json: &str) -> CResult<Vec<UnitState>> {
    serde_json::from_str(json)
        .map_err(|e| CollectError::Failed(format!("unexpected systemctl output: {e}")))
}

/// Run a command with a hard timeout; the child is killed if it overruns.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> CResult<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CollectError::Unsupported(format!("systemctl not available: {e}")))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    let _ = so.read_to_string(&mut out);
                }
                if !status.success() {
                    return Err(CollectError::Unsupported(
                        "systemctl failed (is systemd running?)".into(),
                    ));
                }
                return Ok(out);
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CollectError::Failed(format!(
                    "systemctl did not answer within {}s",
                    timeout.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(CollectError::Failed(e.to_string())),
        }
    }
}

/// States of system services (`--user` for the user manager).
pub fn service_units(user: bool, timeout: Duration) -> CResult<Vec<UnitState>> {
    if !cfg!(target_os = "linux") || !std::path::Path::new("/run/systemd/system").exists() {
        return Err(CollectError::Unsupported(
            "systemd is not the init system here".into(),
        ));
    }
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    cmd.args([
        "list-units",
        "--type=service",
        "--all",
        "--no-pager",
        "--plain",
        "-o",
        "json",
    ]);
    parse_units(&run_with_timeout(&mut cmd, timeout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units_and_rejects_garbage() {
        let j = r#"[{"unit":"cron.service","load":"loaded","active":"active","sub":"running","description":"Regular background program processing daemon"},
                    {"unit":"bad.service","load":"loaded","active":"failed","sub":"failed","description":"x"},
                    {"unit":"x.service","load":"not-found","active":"inactive","sub":"dead"}]"#;
        let u = parse_units(j).unwrap();
        assert_eq!(u.len(), 3);
        assert_eq!(u[1].active, "failed");
        assert_eq!(u[2].description, "");
        assert!(parse_units("not json").is_err());
        assert!(parse_units("[]").unwrap().is_empty());
    }

    #[test]
    fn timeout_kills_a_hung_command() {
        let t = Instant::now();
        let r = run_with_timeout(Command::new("sleep").arg("5"), Duration::from_millis(200));
        assert!(matches!(r, Err(CollectError::Failed(_))));
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
