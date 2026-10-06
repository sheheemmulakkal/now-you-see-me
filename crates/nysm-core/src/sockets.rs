//! Socket ownership model for `ports` lookups (on demand, never sampled
//! in the background).

use std::net::IpAddr;

use serde::Serialize;

use crate::Status;
use crate::raw::ProcessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

/// TCP states as named by the kernel; UDP sockets use `Listen` when bound
/// without a peer and `Established` when connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketState {
    Established,
    SynSent,
    SynRecv,
    FinWait1,
    FinWait2,
    TimeWait,
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    NewSynRecv,
    Unknown,
}

impl SocketState {
    pub fn from_tcp(code: u8) -> Self {
        use SocketState::*;
        match code {
            0x01 => Established,
            0x02 => SynSent,
            0x03 => SynRecv,
            0x04 => FinWait1,
            0x05 => FinWait2,
            0x06 => TimeWait,
            0x07 => Close,
            0x08 => CloseWait,
            0x09 => LastAck,
            0x0A => Listen,
            0x0B => Closing,
            0x0C => NewSynRecv,
            _ => Unknown,
        }
    }

    pub fn from_udp(code: u8) -> Self {
        match code {
            0x01 => SocketState::Established,
            0x07 => SocketState::Listen,
            _ => SocketState::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        use SocketState::*;
        match self {
            Established => "established",
            SynSent => "syn-sent",
            SynRecv => "syn-recv",
            FinWait1 => "fin-wait-1",
            FinWait2 => "fin-wait-2",
            TimeWait => "time-wait",
            Close => "close",
            CloseWait => "close-wait",
            LastAck => "last-ack",
            Listen => "listen",
            Closing => "closing",
            NewSynRecv => "new-syn-recv",
            Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SocketOwner {
    #[serde(flatten)]
    pub id: ProcessId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SocketEntry {
    pub protocol: Protocol,
    pub local_addr: IpAddr,
    pub local_port: u16,
    pub remote_addr: IpAddr,
    pub remote_port: u16,
    pub state: SocketState,
    pub uid: u32,
    pub inode: u64,
    /// Processes holding the socket (several after fork/SO_REUSEPORT).
    pub owners: Vec<SocketOwner>,
    /// Why `owners` may be empty or incomplete.
    pub owner_status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_reason: Option<String>,
}

impl SocketEntry {
    pub fn is_listening(&self) -> bool {
        self.state == SocketState::Listen
    }
}

/// Filter used by `nysm ports`.
#[derive(Debug, Clone, Default)]
pub struct SocketFilter {
    pub port: Option<u16>,
    pub protocol: Option<Protocol>,
    pub listening_only: bool,
}

impl SocketFilter {
    pub fn matches(&self, s: &SocketEntry) -> bool {
        self.protocol.is_none_or(|p| p == s.protocol)
            && self
                .port
                .is_none_or(|p| s.local_port == p || s.remote_port == p)
            && (!self.listening_only || s.is_listening())
    }
}
