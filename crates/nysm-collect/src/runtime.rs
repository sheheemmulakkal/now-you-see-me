//! Opt-in container name resolution through the container runtime API
//! (Docker or Podman, read-only `GET /containers/json`).
//!
//! Access to these sockets is effectively root-equivalent on most systems,
//! so this is never used unless the user asks for it (`--names`), it sends
//! a single read-only request, and failures are reported, not escalated.

use std::collections::HashMap;
#[cfg(unix)]
use std::io::{Read, Write};
use std::time::Duration;

use serde::Serialize;

use crate::error::{CResult, CollectError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub state: String,
}

#[cfg(unix)]
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

/// "infra-api-1 (1ffec9e0869c)": the runtime's name with the short id.
pub fn label(name: &str, id: &str) -> String {
    let short: String = id.chars().take(12).collect();
    format!("{name} ({short})")
}

/// Container id from a cgroup path such as
/// `/system.slice/docker-<id>.scope`, for looking up runtime names.
pub fn container_id(cgroup_path: &str) -> Option<&str> {
    let leaf = cgroup_path.rsplit('/').next()?;
    let leaf = leaf.strip_suffix(".scope")?;
    ["docker-", "libpod-", "cri-containerd-", "crio-"]
        .iter()
        .find_map(|p| leaf.strip_prefix(p))
}

/// Parse a `/containers/json` body into id → info (ids are full 64-hex).
pub fn parse_containers(body: &str) -> CResult<HashMap<String, ContainerInfo>> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| CollectError::Failed(format!("runtime API: {e}")))?;
    let arr = v
        .as_array()
        .ok_or_else(|| CollectError::Failed("runtime API: expected a list".into()))?;
    Ok(arr
        .iter()
        .filter_map(|c| {
            let id = c.get("Id")?.as_str()?.to_string();
            let name = c
                .get("Names")
                .and_then(|n| n.as_array())
                .and_then(|n| n.first())
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .trim_start_matches('/')
                .to_string();
            let s = |k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            Some((
                id,
                ContainerInfo {
                    name,
                    image: s("Image"),
                    state: s("State"),
                },
            ))
        })
        .collect())
}

/// Split an HTTP/1.0 response into (status code, body).
pub fn parse_http(resp: &[u8]) -> CResult<(u16, String)> {
    let text = String::from_utf8_lossy(resp);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| CollectError::Failed("runtime API: malformed HTTP response".into()))?;
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| CollectError::Failed("runtime API: no status code".into()))?;
    Ok((code, body.to_string()))
}

#[cfg(unix)]
fn query(socket: &std::path::Path, timeout: Duration) -> CResult<HashMap<String, ContainerInfo>> {
    use std::os::unix::net::UnixStream;
    let mut s = UnixStream::connect(socket)
        .map_err(|e| CollectError::from_io(&socket.display().to_string(), &e))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    // HTTP/1.0: the server closes the connection; no chunked encoding.
    s.write_all(b"GET /containers/json HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .map_err(|e| CollectError::Failed(format!("runtime API: {e}")))?;
    let mut buf = Vec::new();
    s.take(MAX_RESPONSE)
        .read_to_end(&mut buf)
        .map_err(|e| CollectError::Failed(format!("runtime API: {e}")))?;
    let (code, body) = parse_http(&buf)?;
    if code != 200 {
        return Err(CollectError::Failed(format!(
            "runtime API returned HTTP {code}"
        )));
    }
    parse_containers(&body)
}

/// Try Docker, then rootless Podman. Returns the first that answers.
#[cfg(unix)]
pub fn container_names(timeout: Duration) -> CResult<HashMap<String, ContainerInfo>> {
    let mut candidates = vec![std::path::PathBuf::from("/var/run/docker.sock")];
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(std::path::PathBuf::from(rt).join("podman/podman.sock"));
    }
    candidates.push(std::path::PathBuf::from("/run/podman/podman.sock"));
    let mut last = CollectError::Unsupported("no container runtime socket found".into());
    for c in candidates.iter().filter(|c| c.exists()) {
        match query(c, timeout) {
            Ok(m) => return Ok(m),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(not(unix))]
pub fn container_names(_: Duration) -> CResult<HashMap<String, ContainerInfo>> {
    Err(CollectError::Unsupported(
        "not implemented on this platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_containers() {
        let resp = b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"Id\":\"abc123\",\"Names\":[\"/infra-nginx-1\"],\"Image\":\"nginx:1.27-alpine\",\"State\":\"running\"},{\"Id\":\"x\"}]";
        let (code, body) = parse_http(resp).unwrap();
        assert_eq!(code, 200);
        let m = parse_containers(&body).unwrap();
        assert_eq!(m["abc123"].name, "infra-nginx-1");
        assert_eq!(m["abc123"].image, "nginx:1.27-alpine");
        assert_eq!(m["x"].name, "");
        assert!(parse_http(b"garbage").is_err());
        assert!(parse_containers("{}").is_err());
        assert_eq!(
            parse_http(b"HTTP/1.0 403 Forbidden\r\n\r\n").unwrap().0,
            403
        );
    }
}
