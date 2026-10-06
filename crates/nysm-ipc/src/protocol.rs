//! Wire protocol, version 1.
//!
//! Each frame is a 4-byte big-endian payload length followed by a UTF-8
//! JSON object with a `"type"` tag. Frames larger than `MAX_FRAME` are a
//! protocol error and close the connection.
//!
//! ```text
//! client → hello {protocol_version, client, subscribe{processes}}
//! server → welcome {protocol_version, producer, interval_ms, history[], alerts[], pinned[]}
//! server → update {seq, snapshot, dropped, history[], alerts[], events[], pinned[]}   (repeated)
//! client → set_processes | pin | unpin | details{request_id,...} | ping
//! server → details {request_id, ...} | pong | error {message}
//! ```

use std::io::{self, Read, Write};

use nysm_core::alerts::{ActiveAlert, AlertEvent};
use nysm_core::history::{HistoryPoint, PinnedProcess};
use nysm_core::raw::ProcessId;
use nysm_core::snapshot::Snapshot;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Upper bound on one frame (a full process table is ~0.2–1 MiB).
pub const MAX_FRAME: u32 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Subscribe {
    /// Include the process table in updates (costs a /proc scan every
    /// process interval while any client wants it).
    #[serde(default)]
    pub processes: bool,
    /// Totals only: omit per-interface, per-device, per-core and
    /// filesystem rows (for lightweight clients such as a panel indicator).
    #[serde(default)]
    pub lite: bool,
    /// Include per-container/service/app (cgroup) tables.
    #[serde(default)]
    pub cgroups: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        protocol_version: u32,
        client: String,
        #[serde(default)]
        subscribe: Subscribe,
    },
    SetProcesses {
        on: bool,
    },
    SetCgroups {
        on: bool,
    },
    Pin {
        id: ProcessId,
        name: String,
    },
    Unpin {
        id: ProcessId,
    },
    Details {
        request_id: u64,
        pid: u32,
        start_ticks: Option<u64>,
    },
    Ping,
}

/// Process details as sent over the wire (statuses instead of errors).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireField<T> {
    pub status: nysm_core::Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl<T> From<nysm_collect::CResult<T>> for WireField<T> {
    fn from(r: nysm_collect::CResult<T>) -> Self {
        match r {
            Ok(v) => WireField {
                status: nysm_core::Status::Available,
                value: Some(v),
                reason: None,
            },
            Err(e) => WireField {
                status: e.status(),
                value: None,
                reason: Some(e.reason()),
            },
        }
    }
}

impl<T> From<WireField<T>> for nysm_collect::CResult<T> {
    fn from(w: WireField<T>) -> Self {
        use nysm_collect::CollectError as E;
        match (w.status, w.value) {
            (nysm_core::Status::Available, Some(v)) => Ok(v),
            (nysm_core::Status::PermissionDenied, _) => {
                Err(E::PermissionDenied(w.reason.unwrap_or_default()))
            }
            (nysm_core::Status::Unsupported, _) => {
                Err(E::Unsupported(w.reason.unwrap_or_default()))
            }
            _ => Err(E::Failed(w.reason.unwrap_or_else(|| "unavailable".into()))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireDetails {
    pub exe: WireField<String>,
    pub cwd: WireField<String>,
    pub cgroup: WireField<String>,
    pub open_fds: WireField<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        protocol_version: u32,
        producer: String,
        interval_ms: u64,
        history: Vec<HistoryPoint>,
        alerts: Vec<ActiveAlert>,
        pinned: Vec<PinnedProcess>,
    },
    Update {
        seq: u64,
        snapshot: Box<Snapshot>,
        /// Samples taken since this client's previous update that it did
        /// not receive as full snapshots (coalesced because it was slow).
        dropped: u64,
        /// History points the client has not seen yet (complete even when
        /// snapshots were coalesced).
        history: Vec<HistoryPoint>,
        alerts: Vec<ActiveAlert>,
        events: Vec<AlertEvent>,
        pinned: Vec<PinnedProcess>,
        /// The process table equals the one in the previous update and was
        /// omitted to save bandwidth; reuse it.
        #[serde(default)]
        processes_unchanged: bool,
        /// Same for the cgroup table.
        #[serde(default)]
        cgroups_unchanged: bool,
    },
    Details {
        request_id: u64,
        result: Result<WireDetails, String>,
    },
    Pong,
    Error {
        message: String,
    },
}

pub fn write_frame<T: Serialize, W: Write + ?Sized>(w: &mut W, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    if body.len() as u64 > MAX_FRAME as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_frame<T: for<'de> Deserialize<'de>, R: Read + ?Sized>(r: &mut R) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds limit"),
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &ClientMsg::Ping).unwrap();
        write_frame(
            &mut buf,
            &ClientMsg::Hello {
                protocol_version: 1,
                client: "t".into(),
                subscribe: Subscribe {
                    processes: true,
                    ..Default::default()
                },
            },
        )
        .unwrap();
        let mut r = Cursor::new(buf);
        assert!(matches!(
            read_frame::<ClientMsg, _>(&mut r).unwrap(),
            ClientMsg::Ping
        ));
        assert!(matches!(
            read_frame::<ClientMsg, _>(&mut r).unwrap(),
            ClientMsg::Hello {
                subscribe: Subscribe {
                    processes: true,
                    ..
                },
                ..
            }
        ));
        assert_eq!(
            read_frame::<ClientMsg, _>(&mut r).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn oversized_and_garbage_frames_are_rejected() {
        let mut r = Cursor::new((MAX_FRAME + 1).to_be_bytes().to_vec());
        assert_eq!(
            read_frame::<ClientMsg, _>(&mut r).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let mut buf = 5u32.to_be_bytes().to_vec();
        buf.extend_from_slice(b"hello");
        assert_eq!(
            read_frame::<ClientMsg, _>(&mut Cursor::new(buf))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        // Truncated body.
        let mut buf = 10u32.to_be_bytes().to_vec();
        buf.extend_from_slice(b"{}");
        assert!(read_frame::<ClientMsg, _>(&mut Cursor::new(buf)).is_err());
    }

    #[test]
    fn unknown_message_types_are_errors_not_panics() {
        let body = br#"{"type":"teleport"}"#;
        let mut buf = (body.len() as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(body);
        assert!(read_frame::<ClientMsg, _>(&mut Cursor::new(buf)).is_err());
    }
}
