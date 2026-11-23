//! Wires the VM link, the TCP/IP stack, DNS and the connection proxy together.

use crate::event::{EventKind, NetEvent};
use crate::mitm::Intercept;
use crate::policy::{Mode, Policy, Verdict};
use crate::stack::{self, Cmd, Filter, GuestStream, Inbound};
use crate::{dns, proxy};
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

/// Largest frame accepted from the VM (ethernet header + MTU, with slack).
const MAX_FRAME: usize = 65_550;
/// How long a DNS answer keeps an address tied to its name.
const PIN_LIFETIME: Duration = Duration::from_secs(3600);
const UDP_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// The real network as seen from the host. Replaced in tests.
pub trait Upstream: Send + Sync + 'static {
    fn resolve(&self, name: &str) -> impl Future<Output = io::Result<Vec<Ipv4Addr>>> + Send;
    fn dial(&self, addr: SocketAddrV4) -> impl Future<Output = io::Result<TcpStream>> + Send;
}

/// Uses the host's resolver and routing table.
pub struct SystemUpstream;

impl Upstream for SystemUpstream {
    async fn resolve(&self, name: &str) -> io::Result<Vec<Ipv4Addr>> {
        let addrs = tokio::net::lookup_host((name, 0)).await?;
        let mut ips: Vec<Ipv4Addr> = addrs
            .filter_map(|a| match a.ip() {
                std::net::IpAddr::V4(ip) => Some(ip),
                std::net::IpAddr::V6(_) => None,
            })
            .collect();
        ips.dedup();
        Ok(ips)
    }

    async fn dial(&self, addr: SocketAddrV4) -> io::Result<TcpStream> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        Ok(stream)
    }
}

struct PinEntry {
    names: Vec<String>,
    seen: Instant,
}

pub(crate) struct Shared<U> {
    pub upstream: U,
    policy: Arc<RwLock<Policy>>,
    pins: Mutex<HashMap<Ipv4Addr, PinEntry>>,
    pub events: mpsc::UnboundedSender<NetEvent>,
    pub intercept: Option<Intercept>,
}

/// Addresses that lead back to the host, its LAN or other special networks.
/// They are reachable only through a rule naming the exact address.
pub(crate) fn is_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // CGNAT 100.64/10
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking 198.18/15
        || o[0] >= 240)
}

impl<U: Upstream> Shared<U> {
    pub fn policy(&self) -> Policy {
        self.policy.read().unwrap().clone()
    }

    pub fn emit(&self, event: NetEvent) {
        let _ = self.events.send(event);
    }

    fn literal_allowed(&self, ip: Ipv4Addr, port: Option<u16>) -> Option<Verdict> {
        self.policy().check_ip(ip, port).filter(|verdict| verdict.allowed)
    }

    /// Remember that `name` resolved to `ips` and return the addresses the guest may be told.
    pub fn pin(&self, name: &str, ips: Vec<Ipv4Addr>) -> Vec<Ipv4Addr> {
        let ips: Vec<_> = ips.into_iter().filter(|ip| is_public(*ip) || self.literal_allowed(*ip, None).is_some()).collect();
        let mut pins = self.pins.lock().unwrap();
        let now = Instant::now();
        pins.retain(|_, entry| now.duration_since(entry.seen) < PIN_LIFETIME);
        for ip in &ips {
            let entry = pins.entry(*ip).or_insert_with(|| PinEntry { names: Vec::new(), seen: now });
            entry.seen = now;
            if !entry.names.iter().any(|n| n == name) {
                entry.names.push(name.to_string());
            }
        }
        ips
    }

    /// Whether a secret is meant for `ip` itself or for a name that resolved to it.
    /// Such connections are inspected on any port, not only 80 and 443.
    pub(crate) fn expects_secret(&self, ip: Ipv4Addr) -> bool {
        let Some(intercept) = &self.intercept else { return false };
        intercept.has_secret_for(&ip.to_string()) || self.names_for(ip).iter().any(|name| intercept.has_secret_for(name))
    }

    fn names_for(&self, ip: Ipv4Addr) -> Vec<String> {
        let pins = self.pins.lock().unwrap();
        match pins.get(&ip) {
            Some(entry) if entry.seen.elapsed() < PIN_LIFETIME => entry.names.clone(),
            _ => Vec::new(),
        }
    }

    /// Decide whether the guest may talk to `dst`. `claimed` is the name the
    /// client itself presented (TLS SNI or HTTP Host), if it is known already.
    /// Returns the host name the connection is attributed to.
    pub fn verdict_for(&self, dst: SocketAddrV4, claimed: Option<&str>) -> (Option<String>, Verdict) {
        let (ip, port) = (*dst.ip(), dst.port());
        let policy = self.policy();
        // Address rules come first, and they are the only way to private and local networks.
        if let Some(verdict) = policy.check_ip(ip, Some(port)) {
            return (claimed.map(str::to_string), verdict);
        }
        if !is_public(ip) {
            let reason = "private or local address: needs a rule naming the address or its network".to_string();
            return (claimed.map(str::to_string), Verdict { allowed: false, reason });
        }
        let names = self.names_for(ip);
        if policy.mode == Mode::Blacklist {
            // The name the client presents decides. Without one, the connection is
            // stopped only if every name known for this address is blocked.
            if let Some(claimed) = claimed {
                return (Some(claimed.to_string()), policy.check(claimed, Some(port)));
            }
            let verdicts: Vec<(&String, Verdict)> = names.iter().map(|name| (name, policy.check(name, Some(port)))).collect();
            return match verdicts.iter().find(|(_, verdict)| verdict.allowed) {
                Some((name, verdict)) => (Some((*name).clone()), verdict.clone()),
                None => match verdicts.into_iter().next() {
                    Some((name, verdict)) => (Some(name.clone()), verdict),
                    None => (None, policy.unmatched()),
                },
            };
        }
        if let Some(claimed) = claimed {
            if !names.iter().any(|n| n == claimed) {
                let reason = format!("{claimed} was not resolved to this address");
                return (Some(claimed.to_string()), Verdict { allowed: false, reason });
            }
            return (Some(claimed.to_string()), policy.check(claimed, Some(port)));
        }
        let mut denied = None;
        for name in &names {
            let verdict = policy.check(name, Some(port));
            if verdict.allowed {
                return (Some(name.clone()), verdict);
            }
            denied.get_or_insert((name.clone(), verdict));
        }
        match denied {
            Some((name, verdict)) => (Some(name), verdict),
            None => (None, Verdict { allowed: false, reason: "address was not obtained from an allowed DNS name".into() }),
        }
    }
}

struct PolicyFilter<U> {
    shared: Arc<Shared<U>>,
    udp_logged: HashMap<SocketAddrV4, Instant>,
}

impl<U: Upstream> Filter for PolicyFilter<U> {
    fn accept_tcp(&mut self, dst: SocketAddrV4) -> bool {
        let (host, verdict) = self.shared.verdict_for(dst, None);
        if !verdict.allowed {
            let event = NetEvent::new(EventKind::Tcp, false, verdict.reason).dest(*dst.ip(), dst.port());
            self.shared.emit(match host {
                Some(host) => event.host(host),
                None => event,
            });
        }
        verdict.allowed
    }

    fn udp_blocked(&mut self, dst: SocketAddrV4) {
        let now = Instant::now();
        self.udp_logged.retain(|_, at| now.duration_since(*at) < UDP_LOG_INTERVAL);
        if self.udp_logged.insert(dst, now).is_none() {
            let event = NetEvent::new(EventKind::Udp, false, "only DNS is carried over UDP").dest(*dst.ip(), dst.port());
            self.shared.emit(event);
        }
    }
}

/// Control handle for a running gateway.
#[derive(Clone)]
pub struct GatewayHandle {
    cmds: mpsc::Sender<Cmd>,
}

impl GatewayHandle {
    /// Open a TCP connection to a port inside the guest (used for SSH).
    pub async fn connect_guest(&self, port: u16) -> io::Result<GuestStream> {
        let (reply, stream) = oneshot::channel();
        let gone = || io::Error::new(io::ErrorKind::NotConnected, "sandbox network is down");
        self.cmds.send(Cmd::Connect { port, reply }).await.map_err(|_| gone())?;
        stream.await.map_err(|_| gone())
    }

    /// Resolves when the VM disconnected and the gateway stopped.
    pub async fn closed(&self) {
        self.cmds.closed().await
    }
}

/// Start a gateway for one VM. `link` carries ethernet frames, each prefixed
/// with its length as a big-endian u32 (the passt / QEMU `-netdev stream` format).
/// Without `intercept`, TLS is passed through untouched and no secrets are injected.
pub fn spawn<L, U>(
    link: L,
    upstream: U,
    policy: Arc<RwLock<Policy>>,
    intercept: Option<Intercept>,
    events: mpsc::UnboundedSender<NetEvent>,
) -> GatewayHandle
where
    L: AsyncRead + AsyncWrite + Send + 'static,
    U: Upstream,
{
    let shared = Arc::new(Shared { upstream, policy, pins: Mutex::new(HashMap::new()), events, intercept });
    let (mut link_rx, mut link_tx) = tokio::io::split(link);
    let (frames_in_tx, frames_in) = mpsc::channel::<Vec<u8>>(1024);
    let (frames_out, mut frames_out_rx) = mpsc::channel::<Vec<u8>>(1024);
    let (cmds, cmds_rx) = mpsc::channel(256);
    let (inbound_tx, mut inbound) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        while let Ok(len) = link_rx.read_u32().await {
            if len as usize > MAX_FRAME {
                tracing::warn!(len, "oversized frame from VM, closing link");
                break;
            }
            let mut frame = vec![0; len as usize];
            if link_rx.read_exact(&mut frame).await.is_err() || frames_in_tx.send(frame).await.is_err() {
                break;
            }
        }
    });

    tokio::spawn(async move {
        let mut buf = Vec::with_capacity(2048);
        while let Some(frame) = frames_out_rx.recv().await {
            buf.clear();
            buf.extend_from_slice(&(frame.len() as u32).to_be_bytes());
            buf.extend_from_slice(&frame);
            if link_tx.write_all(&buf).await.is_err() {
                break;
            }
        }
    });

    let filter = PolicyFilter { shared: shared.clone(), udp_logged: HashMap::new() };
    tokio::spawn(stack::run(frames_in, frames_out, cmds_rx, inbound_tx, filter));

    // Replies go through a weak sender so that dropping every GatewayHandle stops the stack.
    let replies = cmds.downgrade();
    tokio::spawn(async move {
        while let Some(event) = inbound.recv().await {
            let shared = shared.clone();
            match event {
                Inbound::Tcp { dst, stream } => {
                    tokio::spawn(proxy::handle(shared, dst, stream));
                }
                Inbound::Dns { query, peer } => {
                    let replies = replies.clone();
                    tokio::spawn(async move {
                        if let (Some(payload), Some(cmds)) = (dns::answer(&shared, &query).await, replies.upgrade()) {
                            let _ = cmds.send(Cmd::DnsReply { payload, peer }).await;
                        }
                    });
                }
            }
        }
    });

    GatewayHandle { cmds }
}
