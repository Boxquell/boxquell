//! Drives the gateway with a second smoltcp stack playing the guest, so the
//! policy, DNS and proxy paths are covered without booting a VM.

use boxquell_net::{GatewayHandle, Mode, NetEvent, Phase, Policy, Upstream, DNS_IP, GATEWAY_IP, GUEST_IP};
use hickory_proto::op::{Message, Query, ResponseCode};
use hickory_proto::rr::{Name, RData, RecordType};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const SITE_IP: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);

#[derive(Default)]
struct Queue {
    rx: VecDeque<Vec<u8>>,
    tx: Vec<Vec<u8>>,
}
struct Rx(Vec<u8>);
struct Tx<'a>(&'a mut Vec<Vec<u8>>);
impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0; len];
        let r = f(&mut frame);
        self.0.push(frame);
        r
    }
}
impl Device for Queue {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        Some((Rx(self.rx.pop_front()?), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

/// Resolves a fixed table and sends every connection to one local server.
struct FakeInternet {
    names: HashMap<&'static str, Ipv4Addr>,
    server: SocketAddr,
}

impl Upstream for FakeInternet {
    async fn resolve(&self, name: &str) -> io::Result<Vec<Ipv4Addr>> {
        self.names.get(name).map(|ip| vec![*ip]).ok_or_else(|| io::ErrorKind::NotFound.into())
    }
    async fn dial(&self, _: SocketAddrV4) -> io::Result<TcpStream> {
        TcpStream::connect(self.server).await
    }
}

struct Guest {
    iface: Interface,
    dev: Queue,
    sockets: SocketSet<'static>,
    frames: mpsc::Receiver<Vec<u8>>,
    link: WriteHalf<DuplexStream>,
    port: u16,
}

struct World {
    guest: Guest,
    gateway: GatewayHandle,
    events: mpsc::UnboundedReceiver<NetEvent>,
}

async fn world(rules: &[&str]) -> World {
    world_in(Mode::Whitelist, rules).await
}

async fn world_in(mode: Mode, rules: &[&str]) -> World {
    // Replies with "pong:" followed by the first chunk it receives.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let n = conn.read(&mut buf).await.unwrap_or(0);
                let _ = conn.write_all(&[b"pong:", &buf[..n]].concat()).await;
            });
        }
    });
    let names = HashMap::from([("site.test", SITE_IP), ("lan.test", Ipv4Addr::new(192, 168, 1, 1))]);
    let policy = Policy { mode, rules: rules.iter().map(|r| r.parse().unwrap()).collect() };
    let (events_tx, events) = mpsc::unbounded_channel();
    let (host_end, guest_end) = tokio::io::duplex(1 << 20);
    let gateway = boxquell_net::spawn(host_end, FakeInternet { names, server }, Arc::new(RwLock::new(policy)), None, events_tx);

    let (mut link_rx, link) = tokio::io::split(guest_end);
    let (frames_tx, frames) = mpsc::channel(1024);
    tokio::spawn(async move {
        while let Ok(len) = link_rx.read_u32().await {
            let mut frame = vec![0; len as usize];
            if link_rx.read_exact(&mut frame).await.is_err() || frames_tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    let mut dev = Queue::default();
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress([0x52, 0x54, 0, 0xcb, 0, 1])));
    let mut iface = Interface::new(config, &mut dev, Instant::now());
    iface.update_ip_addrs(|a| a.push(IpCidr::new(IpAddress::Ipv4(GUEST_IP), 24)).unwrap());
    iface.routes_mut().add_default_ipv4_route(GATEWAY_IP).unwrap();
    let guest = Guest { iface, dev, sockets: SocketSet::new(vec![]), frames, link, port: 40000 };
    World { guest, gateway, events }
}

impl Guest {
    /// Run the guest stack until `done` returns a value.
    async fn until<T>(&mut self, mut done: impl FnMut(&mut SocketSet<'static>) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            self.iface.poll(Instant::now(), &mut self.dev, &mut self.sockets);
            for frame in self.dev.tx.drain(..) {
                self.link.write_u32(frame.len() as u32).await.unwrap();
                self.link.write_all(&frame).await.unwrap();
            }
            if let Some(value) = done(&mut self.sockets) {
                return value;
            }
            assert!(tokio::time::Instant::now() < deadline, "guest operation timed out");
            if let Ok(Some(frame)) = tokio::time::timeout(Duration::from_millis(5), self.frames.recv()).await {
                self.dev.rx.push_back(frame);
            }
        }
    }

    fn tcp(&mut self) -> SocketHandle {
        let buffer = || tcp::SocketBuffer::new(vec![0; 65536]);
        self.sockets.add(tcp::Socket::new(buffer(), buffer()))
    }

    async fn lookup(&mut self, name: &str) -> Message {
        let buffer = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]);
        let mut socket = udp::Socket::new(buffer(), buffer());
        self.port += 1;
        socket.bind(self.port).unwrap();
        let mut query = Message::query();
        query.queries.push(Query::query(Name::from_str(name).unwrap(), RecordType::A));
        socket.send_slice(&query.to_vec().unwrap(), (IpAddress::Ipv4(DNS_IP), 53)).unwrap();
        let handle = self.sockets.add(socket);
        let reply = self.until(|s| s.get_mut::<udp::Socket>(handle).recv().ok().map(|(data, _)| data.to_vec())).await;
        self.sockets.remove(handle);
        Message::from_vec(&reply).unwrap()
    }

    /// Connect, send `request` and collect the reply. `None` if the connection is refused.
    async fn request(&mut self, ip: Ipv4Addr, port: u16, request: &[u8]) -> Option<Vec<u8>> {
        let handle = self.tcp();
        self.port += 1;
        let local = self.port;
        self.sockets.get_mut::<tcp::Socket>(handle).connect(self.iface.context(), (IpAddress::Ipv4(ip), port), local).unwrap();
        let connected = self
            .until(|s| {
                let socket = s.get_mut::<tcp::Socket>(handle);
                match socket.state() {
                    tcp::State::Closed => Some(false),
                    _ if socket.may_send() => Some(true),
                    _ => None,
                }
            })
            .await;
        if !connected {
            self.sockets.remove(handle);
            return None;
        }
        self.sockets.get_mut::<tcp::Socket>(handle).send_slice(request).unwrap();
        let mut reply = Vec::new();
        self.until(|s| {
            let socket = s.get_mut::<tcp::Socket>(handle);
            while socket.can_recv() {
                socket.recv(|data| (data.len(), reply.extend_from_slice(data))).unwrap();
            }
            (!socket.may_recv()).then_some(())
        })
        .await;
        // Finish the close handshake, as a real guest kernel would.
        self.sockets.get_mut::<tcp::Socket>(handle).close();
        self.until(|s| (s.get::<tcp::Socket>(handle).state() == tcp::State::Closed).then_some(())).await;
        self.sockets.remove(handle);
        Some(reply)
    }
}

fn addresses(reply: &Message) -> Vec<Ipv4Addr> {
    reply
        .answers
        .iter()
        .filter_map(|r| match &r.data {
            RData::A(a) => Some(a.0),
            _ => None,
        })
        .collect()
}

async fn next_event(events: &mut mpsc::UnboundedReceiver<NetEvent>) -> NetEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv()).await.expect("no event").unwrap()
}

#[tokio::test]
async fn allowed_name_resolves_and_connects() {
    let mut w = world(&["allow site.test:80"]).await;
    let reply = w.guest.lookup("site.test.").await;
    assert_eq!(addresses(&reply), [SITE_IP]);
    assert!(next_event(&mut w.events).await.allowed);

    let body = w.guest.request(SITE_IP, 80, b"GET / HTTP/1.1\r\nHost: site.test\r\n\r\n").await.unwrap();
    assert!(body.starts_with(b"pong:GET / HTTP/1.1"));

    let open = next_event(&mut w.events).await;
    assert_eq!((open.phase, open.host.as_deref(), open.port), (Phase::Open, Some("site.test"), Some(80)));
    let close = next_event(&mut w.events).await;
    assert_eq!((close.phase, close.id), (Phase::Close, open.id));
    assert_eq!(close.bytes_up, 35);
    assert_eq!(close.bytes_down, body.len() as u64);
}

#[tokio::test]
async fn unlisted_name_does_not_resolve() {
    let mut w = world(&["allow other.test"]).await;
    let reply = w.guest.lookup("site.test.").await;
    assert_eq!(reply.metadata.response_code, ResponseCode::NXDomain);
    let event = next_event(&mut w.events).await;
    assert!(!event.allowed);
    assert_eq!(event.host.as_deref(), Some("site.test"));
}

#[tokio::test]
async fn address_without_allowed_name_is_refused() {
    let mut w = world(&["allow site.test:80"]).await;
    assert!(w.guest.request(SITE_IP, 80, b"x").await.is_none(), "no DNS lookup happened yet");
    assert!(!next_event(&mut w.events).await.allowed);

    w.guest.lookup("site.test.").await;
    next_event(&mut w.events).await;
    assert!(w.guest.request(SITE_IP, 22, b"x").await.is_none(), "port 22 is not allowed");
    let event = next_event(&mut w.events).await;
    assert_eq!((event.allowed, event.port), (false, Some(22)));
}

#[tokio::test]
async fn http_host_must_match_resolved_name() {
    let mut w = world(&["allow site.test:80"]).await;
    w.guest.lookup("site.test.").await;
    next_event(&mut w.events).await;
    let body = w.guest.request(SITE_IP, 80, b"GET / HTTP/1.1\r\nHost: other.test\r\n\r\n").await.unwrap();
    assert!(body.is_empty(), "connection is dropped before anything reaches the server");
    let event = next_event(&mut w.events).await;
    assert_eq!((event.allowed, event.host.as_deref()), (false, Some("other.test")));
}

#[tokio::test]
async fn private_addresses_need_an_explicit_rule() {
    let mut w = world(&["allow *"]).await;
    assert!(addresses(&w.guest.lookup("lan.test.").await).is_empty());
    assert!(w.guest.request(Ipv4Addr::new(192, 168, 1, 1), 80, b"x").await.is_none());

    let mut w = world(&["allow 192.168.1.1:8080"]).await;
    let body = w.guest.request(Ipv4Addr::new(192, 168, 1, 1), 8080, b"hi").await.unwrap();
    assert_eq!(body, b"pong:hi");
}

#[tokio::test]
async fn blacklist_allows_everything_public_except_denied() {
    let mut w = world_in(Mode::Blacklist, &["deny site.test:22", "deny 192.0.2.0/24"]).await;
    // A public address works without any lookup or rule.
    assert_eq!(w.guest.request(SITE_IP, 80, b"hi").await.unwrap(), b"pong:hi");
    assert!(next_event(&mut w.events).await.allowed);
    assert_eq!(addresses(&w.guest.lookup("site.test.").await), [SITE_IP]);
    next_event(&mut w.events).await;
    assert!(w.guest.request(SITE_IP, 22, b"x").await.is_none(), "the denied port stays closed");
    // A denied network and private addresses stay closed as well.
    assert!(w.guest.request(Ipv4Addr::new(192, 0, 2, 9), 80, b"x").await.is_none());
    assert!(w.guest.request(Ipv4Addr::new(192, 168, 1, 1), 80, b"x").await.is_none());
}

#[tokio::test]
async fn network_rules_open_private_ranges() {
    let mut w = world(&["deny 192.168.1.1:22", "allow 192.168.0.0/16"]).await;
    assert_eq!(w.guest.request(Ipv4Addr::new(192, 168, 1, 1), 8080, b"hi").await.unwrap(), b"pong:hi");
    assert!(w.guest.request(Ipv4Addr::new(192, 168, 1, 1), 22, b"x").await.is_none());
    assert_eq!(addresses(&w.guest.lookup("lan.test.").await), Vec::<Ipv4Addr>::new(), "the name itself is not allowed");
}

#[tokio::test]
async fn host_can_connect_into_guest() {
    let mut w = world(&[]).await;
    let handle = w.guest.tcp();
    w.guest.sockets.get_mut::<tcp::Socket>(handle).listen(22).unwrap();
    let gateway = w.gateway.clone();
    let client = tokio::spawn(async move {
        let mut stream = gateway.connect_guest(22).await.unwrap();
        stream.write_all(b"hello guest").await.unwrap();
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).await.unwrap();
        reply
    });
    let mut received = Vec::new();
    w.guest
        .until(|s| {
            let socket = s.get_mut::<tcp::Socket>(handle);
            while socket.can_recv() {
                socket.recv(|data| (data.len(), received.extend_from_slice(data))).unwrap();
            }
            (received.len() == 11).then_some(())
        })
        .await;
    assert_eq!(received, b"hello guest");
    w.guest.sockets.get_mut::<tcp::Socket>(handle).send_slice(b"ok").unwrap();
    w.guest.until(|s| (s.get::<tcp::Socket>(handle).send_queue() == 0).then_some(())).await;
    assert_eq!(&client.await.unwrap(), b"ok");
}
