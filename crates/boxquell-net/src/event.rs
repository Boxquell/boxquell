use serde::Serialize;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Dns,
    Tcp,
    Udp,
    /// A request seen inside an intercepted TLS connection.
    Http,
}

/// `Open` and `Close` share an `id`; `Once` events have no counterpart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Once,
    Open,
    Close,
}

/// One network decision or connection lifecycle step of a sandbox.
#[derive(Debug, Clone, Serialize)]
pub struct NetEvent {
    pub id: u64,
    pub ts_ms: u64,
    pub kind: EventKind,
    pub phase: Phase,
    pub host: Option<String>,
    pub ip: Option<Ipv4Addr>,
    pub port: Option<u16>,
    pub allowed: bool,
    /// The rule that matched, or why the attempt was blocked or failed.
    pub reason: String,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub duration_ms: u64,
    pub method: Option<String>,
    /// Request path without the query string, which may carry credentials.
    pub path: Option<String>,
    pub status: Option<u16>,
    /// Name of the secret that was put into this request.
    pub secret: Option<String>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl NetEvent {
    pub fn new(kind: EventKind, allowed: bool, reason: impl Into<String>) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ts_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
            kind,
            phase: Phase::Once,
            host: None,
            ip: None,
            port: None,
            allowed,
            reason: reason.into(),
            bytes_up: 0,
            bytes_down: 0,
            duration_ms: 0,
            method: None,
            path: None,
            status: None,
            secret: None,
        }
    }

    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn dest(mut self, ip: Ipv4Addr, port: u16) -> Self {
        self.ip = Some(ip);
        self.port = Some(port);
        self
    }

    pub fn phase(mut self, phase: Phase) -> Self {
        self.phase = phase;
        self
    }
}
