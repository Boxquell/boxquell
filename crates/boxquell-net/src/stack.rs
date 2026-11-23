//! Userspace TCP/IP endpoint for the guest NIC, built on smoltcp.
//!
//! The interface answers for every IPv4 address, so a guest connection to any
//! destination terminates here. A listening socket is created per SYN, but only
//! after the [`Filter`] agreed; refused destinations get a RST from smoltcp.

use bytes::Bytes;
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{
    EthernetAddress, EthernetFrame, EthernetProtocol, HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, IpProtocol, Ipv4Packet,
    TcpPacket, UdpPacket,
};
use std::collections::VecDeque;
use std::hash::BuildHasher;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::error::{TryRecvError, TrySendError};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_util::sync::PollSender;

pub const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
pub const DNS_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 3);
pub const GUEST_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
const GATEWAY_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xcb, 0xff, 0x01];

const TCP_BUFFER: usize = 256 * 1024;
const CHUNK: usize = 16 * 1024;
const CHANNEL_CHUNKS: usize = 32;
const MAX_CONNS: usize = 2048;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Decides, synchronously and before the handshake completes, what the guest may reach.
pub trait Filter: Send + 'static {
    fn accept_tcp(&mut self, dst: SocketAddrV4) -> bool;
    /// A UDP datagram that is not DNS was refused.
    fn udp_blocked(&mut self, dst: SocketAddrV4);
}

/// Where a DNS answer has to be sent back to.
#[derive(Debug, Clone, Copy)]
pub struct DnsPeer {
    endpoint: IpEndpoint,
    local: Option<IpAddress>,
}

pub enum Inbound {
    /// The guest completed a handshake to `dst`.
    Tcp {
        dst: SocketAddrV4,
        stream: GuestStream,
    },
    Dns {
        query: Vec<u8>,
        peer: DnsPeer,
    },
}

pub enum Cmd {
    DnsReply {
        payload: Vec<u8>,
        peer: DnsPeer,
    },
    /// Open a connection from the gateway to a port inside the guest.
    Connect {
        port: u16,
        reply: oneshot::Sender<GuestStream>,
    },
}

/// Byte stream to a TCP connection of the guest. Dropping it closes the connection.
pub struct GuestStream {
    rx: mpsc::Receiver<Bytes>,
    leftover: Bytes,
    tx: PollSender<Bytes>,
    wake: Arc<Notify>,
}

impl AsyncRead for GuestStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.leftover.is_empty() {
            match ready!(self.rx.poll_recv(cx)) {
                Some(chunk) => {
                    self.leftover = chunk;
                    // Channel capacity was freed, the stack may deliver more.
                    self.wake.notify_one();
                }
                None => return Poll::Ready(Ok(())),
            }
        }
        let n = buf.remaining().min(self.leftover.len());
        buf.put_slice(&self.leftover.split_to(n));
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for GuestStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if ready!(self.tx.poll_reserve(cx)).is_err() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let n = buf.len().min(CHUNK);
        if self.tx.send_item(Bytes::copy_from_slice(&buf[..n])).is_err() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        self.wake.notify_one();
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx.close();
        self.wake.notify_one();
        Poll::Ready(Ok(()))
    }
}

impl Drop for GuestStream {
    fn drop(&mut self) {
        self.wake.notify_one();
    }
}

#[derive(Default)]
struct QueueDevice {
    rx: VecDeque<Vec<u8>>,
    tx: Vec<Vec<u8>>,
}

struct RxTok(Vec<u8>);
struct TxTok<'a>(&'a mut Vec<Vec<u8>>);

impl RxToken for RxTok {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for TxTok<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0; len];
        let r = f(&mut frame);
        self.0.push(frame);
        r
    }
}

impl Device for QueueDevice {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok<'a>;

    fn receive(&mut self, _: SmolInstant) -> Option<(RxTok, TxTok<'_>)> {
        let frame = self.rx.pop_front()?;
        Some((RxTok(frame), TxTok(&mut self.tx)))
    }

    fn transmit(&mut self, _: SmolInstant) -> Option<TxTok<'_>> {
        Some(TxTok(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

struct Conn {
    handle: SocketHandle,
    /// Guest SYN this listener was created for; `None` for gateway-initiated connections.
    key: Option<(SocketAddrV4, SocketAddrV4)>,
    born: Instant,
    established: bool,
    /// Handed to the gateway once the handshake completed.
    stream: Option<GuestStream>,
    to_app: Option<mpsc::Sender<Bytes>>,
    from_app: mpsc::Receiver<Bytes>,
    pending: Option<Bytes>,
    app_done: bool,
    fin_sent: bool,
}

impl Conn {
    fn new(handle: SocketHandle, key: Option<(SocketAddrV4, SocketAddrV4)>, wake: &Arc<Notify>) -> Self {
        let (to_app, rx) = mpsc::channel(CHANNEL_CHUNKS);
        let (tx, from_app) = mpsc::channel(CHANNEL_CHUNKS);
        let stream = GuestStream { rx, leftover: Bytes::new(), tx: PollSender::new(tx), wake: wake.clone() };
        Self {
            handle,
            key,
            born: Instant::now(),
            established: false,
            stream: Some(stream),
            to_app: Some(to_app),
            from_app,
            pending: None,
            app_done: false,
            fin_sent: false,
        }
    }
}

enum Seen {
    Syn { src: SocketAddrV4, dst: SocketAddrV4 },
    Udp { dst: SocketAddrV4 },
    Icmp { dst: Ipv4Addr },
    Other,
}

fn inspect(frame: &[u8]) -> Option<Seen> {
    let eth = EthernetFrame::new_checked(frame).ok()?;
    if eth.ethertype() != EthernetProtocol::Ipv4 {
        return Some(Seen::Other);
    }
    let ip = Ipv4Packet::new_checked(eth.payload()).ok()?;
    let (src, dst) = (ip.src_addr(), ip.dst_addr());
    Some(match ip.next_header() {
        IpProtocol::Tcp => {
            let tcp = TcpPacket::new_checked(ip.payload()).ok()?;
            if tcp.syn() && !tcp.ack() {
                Seen::Syn { src: SocketAddrV4::new(src, tcp.src_port()), dst: SocketAddrV4::new(dst, tcp.dst_port()) }
            } else {
                Seen::Other
            }
        }
        IpProtocol::Udp => Seen::Udp { dst: SocketAddrV4::new(dst, UdpPacket::new_checked(ip.payload()).ok()?.dst_port()) },
        IpProtocol::Icmp => Seen::Icmp { dst },
        _ => Seen::Other,
    })
}

fn tcp_socket() -> tcp::Socket<'static> {
    let mut socket = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; TCP_BUFFER]), tcp::SocketBuffer::new(vec![0; TCP_BUFFER]));
    socket.set_ack_delay(None);
    socket
}

fn is_own(ip: Ipv4Addr) -> bool {
    ip == GATEWAY_IP || ip == DNS_IP
}

/// Runs until the VM link closes or every command sender is gone.
pub async fn run(
    mut frames_in: mpsc::Receiver<Vec<u8>>,
    frames_out: mpsc::Sender<Vec<u8>>,
    mut cmds: mpsc::Receiver<Cmd>,
    inbound: mpsc::UnboundedSender<Inbound>,
    mut filter: impl Filter,
) {
    let mut dev = QueueDevice::default();
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(GATEWAY_MAC)));
    config.random_seed = std::collections::hash_map::RandomState::new().hash_one(std::process::id());
    let mut iface = Interface::new(config, &mut dev, SmolInstant::now());
    iface.update_ip_addrs(|addrs| {
        for ip in [GATEWAY_IP, DNS_IP] {
            addrs.push(IpCidr::new(IpAddress::Ipv4(ip), 24)).expect("smoltcp address slots");
        }
    });
    iface.routes_mut().add_default_ipv4_route(GATEWAY_IP).expect("smoltcp route slots");
    iface.set_any_ip(true);

    let mut sockets = SocketSet::new(vec![]);
    let mut dns = udp::Socket::new(
        udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 64 * 1024]),
        udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 64 * 1024]),
    );
    dns.bind(53).expect("bind dns socket");
    let dns = sockets.add(dns);

    let wake = Arc::new(Notify::new());
    let mut conns: Vec<Conn> = Vec::new();
    let mut next_port: u16 = 49152;

    loop {
        let delay = iface.poll_delay(SmolInstant::now(), &sockets).map_or(Duration::from_secs(1), |d| Duration::from_micros(d.total_micros()));
        let mut frames = Vec::new();
        tokio::select! {
            frame = frames_in.recv() => match frame {
                Some(frame) => frames.push(frame),
                None => return,
            },
            cmd = cmds.recv() => match cmd {
                Some(Cmd::DnsReply { payload, peer }) => {
                    let mut meta = udp::UdpMetadata::from(peer.endpoint);
                    meta.local_address = peer.local;
                    let _ = sockets.get_mut::<udp::Socket>(dns).send_slice(&payload, meta);
                }
                Some(Cmd::Connect { port, reply }) => {
                    let mut socket = tcp_socket();
                    next_port = if next_port == u16::MAX { 49152 } else { next_port + 1 };
                    let remote = (IpAddress::Ipv4(GUEST_IP), port);
                    let local = (IpAddress::Ipv4(GATEWAY_IP), next_port);
                    if socket.connect(iface.context(), remote, local).is_ok() {
                        let mut conn = Conn::new(sockets.add(socket), None, &wake);
                        let _ = reply.send(conn.stream.take().expect("new conn has a stream"));
                        conns.push(conn);
                    }
                }
                None => return,
            },
            _ = wake.notified() => {}
            _ = tokio::time::sleep(delay) => {}
        }
        while let Ok(frame) = frames_in.try_recv() {
            frames.push(frame);
        }

        for frame in frames {
            match inspect(&frame) {
                None => continue,
                Some(Seen::Icmp { dst }) if !is_own(dst) => continue,
                Some(Seen::Udp { dst }) if dst.port() != 53 => {
                    if !dst.ip().is_broadcast() && !dst.ip().is_multicast() {
                        filter.udp_blocked(dst);
                    }
                }
                // The gateway address stands for the host itself; the filter decides about it like any other.
                Some(Seen::Syn { src, dst }) if *dst.ip() != DNS_IP => {
                    let known = conns.iter().any(|c| c.key == Some((src, dst)));
                    if !known && conns.len() < MAX_CONNS && filter.accept_tcp(dst) {
                        let mut socket = tcp_socket();
                        let endpoint = IpListenEndpoint { addr: Some(IpAddress::Ipv4(*dst.ip())), port: dst.port() };
                        if socket.listen(endpoint).is_ok() {
                            conns.push(Conn::new(sockets.add(socket), Some((src, dst)), &wake));
                        }
                    }
                }
                Some(_) => {}
            }
            dev.rx.push_back(frame);
        }

        loop {
            let mut progress = false;
            for conn in &mut conns {
                progress |= pump_to_guest(conn, sockets.get_mut(conn.handle));
            }
            progress |= iface.poll(SmolInstant::now(), &mut dev, &mut sockets) == PollResult::SocketStateChanged;
            for conn in &mut conns {
                progress |= pump_from_guest(conn, sockets.get_mut(conn.handle), &inbound);
            }
            conns.retain(|conn| {
                let socket = sockets.get::<tcp::Socket>(conn.handle);
                let stale = !conn.established && conn.born.elapsed() > HANDSHAKE_TIMEOUT;
                let keep = socket.state() != tcp::State::Closed && !stale;
                if !keep {
                    sockets.remove(conn.handle);
                }
                keep
            });
            let socket = sockets.get_mut::<udp::Socket>(dns);
            while let Ok((query, meta)) = socket.recv() {
                let peer = DnsPeer { endpoint: meta.endpoint, local: meta.local_address };
                let _ = inbound.send(Inbound::Dns { query: query.to_vec(), peer });
            }
            for frame in dev.tx.drain(..) {
                if frames_out.send(frame).await.is_err() {
                    return;
                }
            }
            if !progress {
                break;
            }
        }
    }
}

/// Move bytes written by the host side into the guest connection.
fn pump_to_guest(conn: &mut Conn, socket: &mut tcp::Socket) -> bool {
    let mut progress = false;
    while socket.can_send() {
        let chunk = match conn.pending.take() {
            Some(chunk) => chunk,
            None => match conn.from_app.try_recv() {
                Ok(chunk) => chunk,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    conn.app_done = true;
                    break;
                }
            },
        };
        let sent = socket.send_slice(&chunk).unwrap_or(0);
        progress |= sent > 0;
        if sent < chunk.len() {
            conn.pending = Some(chunk.slice(sent..));
            break;
        }
    }
    if conn.app_done && conn.pending.is_none() && !conn.fin_sent {
        socket.close();
        conn.fin_sent = true;
        progress = true;
    }
    progress
}

/// Move bytes received from the guest to the host side and track the handshake.
fn pump_from_guest(conn: &mut Conn, socket: &mut tcp::Socket, inbound: &mpsc::UnboundedSender<Inbound>) -> bool {
    let mut progress = false;
    if !conn.established && (socket.may_send() || socket.may_recv()) {
        conn.established = true;
        progress = true;
        if let (Some(stream), Some((_, dst))) = (conn.stream.take(), conn.key) {
            let _ = inbound.send(Inbound::Tcp { dst, stream });
        }
    }
    let Some(to_app) = conn.to_app.clone() else { return progress };
    while socket.can_recv() {
        match to_app.try_reserve() {
            Ok(permit) => {
                let chunk = socket.recv(|data| {
                    let n = data.len().min(CHUNK);
                    (n, Bytes::copy_from_slice(&data[..n]))
                });
                match chunk {
                    Ok(chunk) if !chunk.is_empty() => {
                        permit.send(chunk);
                        progress = true;
                    }
                    _ => break,
                }
            }
            Err(TrySendError::Full(())) => break,
            Err(TrySendError::Closed(())) => {
                // The host side is gone while the guest still sends: reset.
                socket.abort();
                conn.to_app = None;
                return true;
            }
        }
    }
    if conn.established && !socket.may_recv() && !socket.can_recv() {
        // Guest finished sending; dropping the sender is EOF for the host side.
        conn.to_app = None;
        progress = true;
    }
    progress
}
