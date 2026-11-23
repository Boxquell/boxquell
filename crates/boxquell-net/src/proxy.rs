//! Carries one accepted guest connection to its real destination.

use crate::event::{EventKind, NetEvent, Phase};
use crate::gateway::{is_public, Shared, Upstream};
use crate::mitm;
use crate::sniff::{http_host, tls_sni, Sniff};
use crate::stack::{GuestStream, GATEWAY_IP};
use bytes::Bytes;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

const SNIFF_LIMIT: usize = 16 * 1024 + 5;
const SNIFF_TIMEOUT: Duration = Duration::from_secs(5);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Read from the guest until `parse` has seen enough. The bytes stay in `head`.
/// First byte of a TLS record that carries a handshake message.
const TLS_HANDSHAKE: u8 = 0x16;

fn tls_or_http(head: &[u8]) -> Sniff {
    match head.first() {
        Some(&TLS_HANDSHAKE) => tls_sni(head),
        Some(_) => http_host(head),
        None => Sniff::Incomplete,
    }
}

async fn sniff(guest: &mut GuestStream, head: &mut Vec<u8>, parse: fn(&[u8]) -> Sniff) -> Option<String> {
    let read = async {
        let mut buf = [0u8; 4096];
        loop {
            match parse(head) {
                Sniff::Name(name) => return name,
                Sniff::Unknown => return None,
                Sniff::Incomplete if head.len() >= SNIFF_LIMIT => return None,
                Sniff::Incomplete => {}
            }
            match guest.read(&mut buf).await {
                Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
                _ => return None,
            }
        }
    };
    tokio::time::timeout(SNIFF_TIMEOUT, read).await.ok().flatten()
}

/// The guest stream with the already sniffed bytes put back in front, counting traffic.
struct Replay {
    head: Bytes,
    guest: GuestStream,
    up: Arc<AtomicU64>,
    down: Arc<AtomicU64>,
}

impl AsyncRead for Replay {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        if self.head.is_empty() {
            ready!(Pin::new(&mut self.guest).poll_read(cx, buf))?;
        } else {
            let n = buf.remaining().min(self.head.len());
            buf.put_slice(&self.head.split_to(n));
        }
        self.up.fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Replay {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let n = ready!(Pin::new(&mut self.guest).poll_write(cx, buf))?;
        self.down.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Ok(n))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.guest).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.guest).poll_shutdown(cx)
    }
}

pub(crate) async fn handle<U: Upstream>(shared: Arc<Shared<U>>, dst: SocketAddrV4, mut guest: GuestStream) {
    let started = Instant::now();
    let mut head = Vec::new();
    let claimed = match dst.port() {
        443 => sniff(&mut guest, &mut head, tls_sni).await,
        80 => sniff(&mut guest, &mut head, http_host).await,
        // Gateways such as LiteLLM listen on other ports.
        _ if shared.expects_secret(*dst.ip()) => sniff(&mut guest, &mut head, tls_or_http).await,
        _ => None,
    };

    // The name the client presents must be one that resolved to this address,
    // otherwise an allowed address could be used to reach a different site.
    let (host, verdict) = shared.verdict_for(dst, claimed.as_deref());
    let event = |allowed: bool, reason: String| {
        let event = NetEvent::new(EventKind::Tcp, allowed, reason).dest(*dst.ip(), dst.port());
        match &host {
            Some(host) => event.host(host),
            None => event,
        }
    };
    if !verdict.allowed {
        shared.emit(event(false, verdict.reason));
        return;
    }

    let open = event(true, verdict.reason.clone()).phase(Phase::Open);
    let id = open.id;
    shared.emit(open);

    // TLS is terminated here only for hosts whose requests have to be seen or changed.
    let tls = head.first() == Some(&TLS_HANDSHAKE);
    let intercept = match (&shared.intercept, &host) {
        (Some(intercept), Some(host)) if claimed.is_some() && tls && intercept.wants(host) => Some((intercept.clone(), host.clone())),
        // Without TLS a secret would cross the network readable, so it is put in
        // only on the way to private addresses: this computer or its local network.
        (Some(intercept), Some(host)) if claimed.is_some() && !tls && !is_public(*dst.ip()) && intercept.has_secret_for(host) => {
            Some((intercept.clone(), host.clone()))
        }
        _ => None,
    };
    // The gateway address stands for this computer, like `localhost` on the host.
    let target = if *dst.ip() == GATEWAY_IP { SocketAddrV4::new(Ipv4Addr::LOCALHOST, dst.port()) } else { dst };
    let (mut reason, mut bytes) = (verdict.reason, (0, 0));
    match tokio::time::timeout(DIAL_TIMEOUT, shared.upstream.dial(target)).await {
        Ok(Ok(upstream)) if intercept.is_some() => {
            let (intercept, host) = intercept.expect("checked by the match guard");
            let (up, down) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
            let guest = Replay { head: Bytes::from(std::mem::take(&mut head)), guest, up: up.clone(), down: down.clone() };
            let served = if tls {
                mitm::serve(guest, upstream, host, intercept, shared.events.clone()).await
            } else {
                mitm::serve_plain(guest, upstream, host, intercept, shared.events.clone()).await
            };
            if let Err(err) = served {
                reason = format!("intercepted connection ended: {err:#}");
            }
            bytes = (up.load(Ordering::Relaxed), down.load(Ordering::Relaxed));
        }
        Ok(Ok(mut upstream)) => {
            let relay = async {
                upstream.write_all(&head).await?;
                tokio::io::copy_bidirectional(&mut guest, &mut upstream).await
            };
            match relay.await {
                Ok((up, down)) => bytes = (up + head.len() as u64, down),
                Err(err) => reason = format!("connection error: {err}"),
            }
        }
        Ok(Err(err)) => reason = format!("connect failed: {err}"),
        Err(_) => reason = "connect timed out".into(),
    }
    let mut close = event(true, reason).phase(Phase::Close);
    close.id = id;
    (close.bytes_up, close.bytes_down) = bytes;
    close.duration_ms = started.elapsed().as_millis() as u64;
    shared.emit(close);
}
