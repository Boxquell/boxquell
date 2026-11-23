//! Terminates TLS towards the guest for selected hosts, forwards the HTTP
//! requests over a fresh verified TLS connection, and swaps placeholder
//! credentials for the real ones on the way out.

use crate::event::{EventKind, NetEvent};
use crate::policy::host_matches;
use crate::tls::CertAuthority;
use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{HeaderValue, HOST, UPGRADE};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use rustls::server::ResolvesServerCert;
pub use rustls::RootCertStore;
use rustls::{ClientConfig, ServerConfig};
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Mutex};
use tokio_rustls::{LazyConfigAcceptor, TlsConnector};

/// A credential that stays on the host. The guest only ever holds `placeholder`.
#[derive(Clone)]
pub struct Secret {
    pub name: String,
    /// Host patterns (`api.example.com`, `*.example.com`) the secret may be sent to.
    pub hosts: Vec<String>,
    pub placeholder: String,
    pub value: String,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret").field("name", &self.name).field("hosts", &self.hosts).finish_non_exhaustive()
    }
}

impl Secret {
    fn applies_to(&self, host: &str) -> bool {
        !self.placeholder.is_empty() && self.hosts.iter().any(|pattern| host_matches(pattern, host))
    }
}

/// TLS interception settings of one sandbox.
#[derive(Clone)]
pub struct Intercept {
    pub ca: Arc<CertAuthority>,
    pub secrets: Arc<RwLock<Vec<Secret>>>,
    /// Intercept every TLS host to log its requests, not only hosts with a secret.
    pub log_all_requests: bool,
    /// Certificate authorities accepted for the real servers.
    pub roots: Arc<RootCertStore>,
    /// Names of secrets that may not be used for now, e.g. because their limit
    /// for the day is reached. A request that would carry one is answered here.
    pub paused: Arc<RwLock<Vec<String>>>,
}

/// The certificate authorities the host itself trusts.
pub fn native_certs() -> Vec<rustls::pki_types::CertificateDer<'static>> {
    rustls_native_certs::load_native_certs().certs
}

impl Intercept {
    /// Whether some secret is meant for `host`, regardless of request logging.
    pub fn has_secret_for(&self, host: &str) -> bool {
        self.secrets.read().unwrap().iter().any(|secret| secret.applies_to(host))
    }

    pub fn wants(&self, host: &str) -> bool {
        self.log_all_requests || self.secrets.read().unwrap().iter().any(|s| s.applies_to(host))
    }
}

#[derive(Debug)]
struct FixedCert(Arc<rustls::sign::CertifiedKey>);

impl ResolvesServerCert for FixedCert {
    fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

type Body = BoxBody<Bytes, hyper::Error>;

#[derive(Clone)]
enum Sender {
    H1(Arc<Mutex<hyper::client::conn::http1::SendRequest<Incoming>>>),
    H2(hyper::client::conn::http2::SendRequest<Incoming>),
}

struct Session {
    host: String,
    sender: Sender,
    intercept: Intercept,
    events: mpsc::UnboundedSender<NetEvent>,
    /// Only requests that fetch are passed on; see `Action::Read`.
    read_only: bool,
}

/// A `Basic …` header value with the secret's placeholder in its decoded
/// `user:password` replaced by the real value; `None` when it is not there.
fn basic_with(value: &str, secret: &Secret) -> Option<String> {
    use base64::Engine;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let decoded = String::from_utf8(engine.decode(encoded.trim()).ok()?).ok()?;
    decoded.contains(&secret.placeholder).then(|| format!("{scheme} {}", engine.encode(decoded.replace(&secret.placeholder, &secret.value))))
}

/// Follows an answer as it passes and, when it is over, reports the tokens it used.
struct UsageReport {
    scanner: crate::usage::Scanner,
    events: mpsc::UnboundedSender<NetEvent>,
    host: String,
    secret: String,
}

impl Drop for UsageReport {
    fn drop(&mut self) {
        let usage = std::mem::take(&mut self.scanner.usage);
        if !usage.is_empty() {
            let mut event = NetEvent::new(EventKind::Http, true, "tokens used").host(&self.host);
            event.secret = Some(std::mem::take(&mut self.secret));
            event.usage = Some(usage);
            let _ = self.events.send(event);
        }
    }
}

fn status(code: StatusCode) -> Response<Body> {
    let mut response = Response::new(Empty::new().map_err(|never| match never {}).boxed());
    *response.status_mut() = code;
    response
}

fn host_only(authority: &str) -> &str {
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    }
}

impl Session {
    /// Replace placeholders in the request headers. Returns the secret used.
    fn inject(&self, request: &mut Request<Incoming>) -> Option<String> {
        let secrets = self.intercept.secrets.read().unwrap();
        let mut used = None;
        for secret in secrets.iter().filter(|s| s.applies_to(&self.host)) {
            for value in request.headers_mut().values_mut() {
                let Ok(text) = value.to_str() else { continue };
                // In `Basic` sign-in the placeholder is inside an encoded `user:password`.
                let swapped = if text.contains(&secret.placeholder) {
                    Some(text.replace(&secret.placeholder, &secret.value))
                } else {
                    basic_with(text, secret)
                };
                let Some(swapped) = swapped else { continue };
                if let Ok(mut replaced) = HeaderValue::from_str(&swapped) {
                    replaced.set_sensitive(true);
                    *value = replaced;
                    used = Some(secret.name.clone());
                }
            }
        }
        used
    }

    async fn forward(self: Arc<Self>, mut request: Request<Incoming>) -> Result<Response<Body>, hyper::Error> {
        let mut event = NetEvent::new(EventKind::Http, true, "").host(&self.host);
        event.method = Some(request.method().to_string());
        event.path = Some(request.uri().path().to_string());

        // The request must be for the host the TLS session was opened to; otherwise
        // an allowed session could carry a secret or a request to another site.
        let target = request.uri().host().map(str::to_string).or_else(|| {
            let value = request.headers().get(HOST)?.to_str().ok()?;
            Some(host_only(value).to_string())
        });
        if target.as_deref().map(str::to_ascii_lowercase).as_deref() != Some(self.host.as_str()) {
            event.allowed = false;
            event.reason = format!("request host {target:?} differs from the TLS name");
            event.status = Some(StatusCode::MISDIRECTED_REQUEST.as_u16());
            let _ = self.events.send(event);
            return Ok(status(StatusCode::MISDIRECTED_REQUEST));
        }

        // A destination opened for reading only: nothing that sends or changes
        // data, and no upgrade to a connection that carries data both ways.
        let fetches = matches!(*request.method(), hyper::Method::GET | hyper::Method::HEAD | hyper::Method::OPTIONS);
        if self.read_only && (!fetches || request.headers().contains_key(UPGRADE)) {
            event.allowed = false;
            event.reason = format!("read-only rule: {} is not let through", if fetches { "an upgrade" } else { request.method().as_str() });
            event.status = Some(StatusCode::FORBIDDEN.as_u16());
            let _ = self.events.send(event);
            return Ok(status(StatusCode::FORBIDDEN));
        }

        event.secret = self.inject(&mut request);
        if let Some(secret) = event.secret.as_ref().filter(|secret| self.intercept.paused.read().unwrap().contains(secret)) {
            event.allowed = false;
            event.reason = format!("the limit set for the key {secret} is reached");
            event.status = Some(StatusCode::TOO_MANY_REQUESTS.as_u16());
            let message = format!(
                "Boxquell: the limit set for the key \"{secret}\" is reached. Raise it or wait for the next day; see Usage in the Boxquell app.\n"
            );
            let _ = self.events.send(event);
            let mut response = Response::new(http_body_util::Full::new(Bytes::from(message)).map_err(|never| match never {}).boxed());
            *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
            return Ok(response);
        }
        // A request that carries a key goes to a model API or the like: its
        // answer is read for the tokens it used, which needs it uncompressed.
        let metered = event.secret.clone();
        if metered.is_some() {
            request.headers_mut().remove(hyper::header::ACCEPT_ENCODING);
        }
        let guest_upgrade = request.headers().contains_key(UPGRADE).then(|| hyper::upgrade::on(&mut request));
        let sent = match &self.sender {
            Sender::H1(sender) => {
                let mut sender = sender.lock().await;
                match sender.ready().await {
                    Ok(()) => sender.send_request(request).await,
                    Err(err) => Err(err),
                }
            }
            Sender::H2(sender) => sender.clone().send_request(request).await,
        };
        let mut response = match sent {
            Ok(response) => response,
            Err(err) => {
                event.reason = format!("upstream error: {err}");
                event.status = Some(StatusCode::BAD_GATEWAY.as_u16());
                let _ = self.events.send(event);
                return Ok(status(StatusCode::BAD_GATEWAY));
            }
        };
        event.status = Some(response.status().as_u16());
        let _ = self.events.send(event);

        if let (StatusCode::SWITCHING_PROTOCOLS, Some(guest_upgrade)) = (response.status(), guest_upgrade) {
            let server_upgrade = hyper::upgrade::on(&mut response);
            tokio::spawn(async move {
                if let (Ok(guest), Ok(server)) = (guest_upgrade.await, server_upgrade.await) {
                    let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(guest), &mut TokioIo::new(server)).await;
                }
            });
        }
        match metered.filter(|_| response.status().is_success()) {
            Some(secret) => {
                let mut report = UsageReport { scanner: Default::default(), events: self.events.clone(), host: self.host.clone(), secret };
                Ok(response.map(|body| {
                    body.map_frame(move |frame| {
                        if let Some(data) = frame.data_ref() {
                            report.scanner.feed(data);
                        }
                        frame
                    })
                    .boxed()
                }))
            }
            None => Ok(response.map(BodyExt::boxed)),
        }
    }
}

/// Serve one intercepted connection. `guest` starts at the TLS ClientHello,
/// `server` is a plain TCP connection to the real destination.
/// Like [`serve`], for unencrypted HTTP: used for gateways in the local network,
/// where the secret would otherwise have to live in the sandbox.
pub(crate) async fn serve_plain<G, S>(
    guest: G,
    server: S,
    host: String,
    intercept: Intercept,
    events: mpsc::UnboundedSender<NetEvent>,
    read_only: bool,
) -> Result<()>
where
    G: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(server)).await?;
    tokio::spawn(connection.with_upgrades());
    let session = Arc::new(Session { host, sender: Sender::H1(Arc::new(Mutex::new(sender))), intercept, events, read_only });
    let service = service_fn(move |request| session.clone().forward(request));
    hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(guest), service).with_upgrades().await?;
    Ok(())
}

pub(crate) async fn serve<G, S>(
    guest: G,
    server: S,
    host: String,
    intercept: Intercept,
    events: mpsc::UnboundedSender<NetEvent>,
    read_only: bool,
) -> Result<()>
where
    G: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), guest).await.context("guest TLS hello")?;
    let offered: Vec<Vec<u8>> = start
        .client_hello()
        .alpn()
        .map(|protocols| protocols.filter(|p| *p == b"h2" || *p == b"http/1.1").map(<[u8]>::to_vec).collect())
        .unwrap_or_default();

    // Offer the server what the guest offered, so both legs end up on the same HTTP version.
    let mut client = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(intercept.roots.clone())
        .with_no_client_auth();
    client.alpn_protocols = offered;
    let name = ServerName::try_from(host.clone()).context("invalid TLS server name")?;
    let server = TlsConnector::from(Arc::new(client)).connect(name, server).await.context("TLS to server")?;
    let protocol = server.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
    let h2 = protocol.as_deref() == Some(b"h2");

    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedCert(intercept.ca.leaf(&host)?)));
    config.alpn_protocols = protocol.into_iter().collect();
    let guest = start.into_stream(Arc::new(config)).await.context("TLS to guest")?;

    let (guest, server) = (TokioIo::new(guest), TokioIo::new(server));
    if h2 {
        let (sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), server).await?;
        tokio::spawn(connection);
        let session = Arc::new(Session { host, sender: Sender::H2(sender), intercept, events, read_only });
        let service = service_fn(move |request| session.clone().forward(request));
        hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(guest, service).await?;
    } else {
        let (sender, connection) = hyper::client::conn::http1::handshake(server).await?;
        tokio::spawn(connection.with_upgrades());
        let session = Arc::new(Session { host, sender: Sender::H1(Arc::new(Mutex::new(sender))), intercept, events, read_only });
        let service = service_fn(move |request| session.clone().forward(request));
        hyper::server::conn::http1::Builder::new().serve_connection(guest, service).with_upgrades().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::Full;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::CertificateDer;
    use tokio::net::{TcpListener, TcpStream};

    const REAL: &str = "sk-real-secret";
    const PLACEHOLDER: &str = "boxquell-placeholder";

    fn roots(ca: &CertAuthority) -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from_pem_slice(ca.cert_pem().as_bytes()).unwrap()).unwrap();
        Arc::new(roots)
    }

    /// "The internet": a TLS server for api.test that reports the credential it received.
    async fn real_server(ca: &CertAuthority) -> std::net::SocketAddr {
        let mut config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(FixedCert(ca.leaf("api.test").unwrap())));
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let tls = acceptor.accept(tcp).await.unwrap();
                    let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
                    let service = service_fn(|request: Request<Incoming>| async move {
                        let key = request.headers().get("x-api-key").map_or("none", |v| v.to_str().unwrap());
                        let auth = request.headers().get("authorization").map_or("none", |v| v.to_str().unwrap());
                        let body = format!("{:?} key={key} auth={auth}", request.version());
                        Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(body))))
                    });
                    let io = TokioIo::new(tls);
                    if h2 {
                        let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(io, service).await;
                    } else {
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(io, service).await;
                    }
                });
            }
        });
        addr
    }

    struct Fixture {
        guest_tls: tokio_rustls::client::TlsStream<tokio::io::DuplexStream>,
        events: mpsc::UnboundedReceiver<NetEvent>,
    }

    /// Run `serve` between an in-memory guest connection and the test server.
    async fn fixture(alpn: &[&[u8]]) -> Fixture {
        fixture_with(alpn, false).await
    }

    async fn fixture_with(alpn: &[&[u8]], read_only: bool) -> Fixture {
        let internet_ca = CertAuthority::generate().unwrap();
        let sandbox_ca = Arc::new(CertAuthority::generate().unwrap());
        let server = TcpStream::connect(real_server(&internet_ca).await).await.unwrap();
        let secret = Secret { name: "test".into(), hosts: vec!["api.test".into()], placeholder: PLACEHOLDER.into(), value: REAL.into() };
        let intercept = Intercept {
            ca: sandbox_ca.clone(),
            secrets: Arc::new(RwLock::new(vec![secret])),
            log_all_requests: false,
            roots: roots(&internet_ca),
            paused: Default::default(),
        };
        let (guest_end, proxy_end) = tokio::io::duplex(1 << 16);
        let (events_tx, events) = mpsc::unbounded_channel();
        tokio::spawn(serve(proxy_end, server, "api.test".into(), intercept, events_tx, read_only));

        let mut config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots(&sandbox_ca))
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let name = ServerName::try_from("api.test").unwrap();
        let guest_tls = TlsConnector::from(Arc::new(config)).connect(name, guest_end).await.unwrap();
        Fixture { guest_tls, events }
    }

    fn request(host: &str) -> Request<Empty<Bytes>> {
        Request::builder()
            .uri("/v1/messages?debug=1")
            .header(HOST, host)
            .header("x-api-key", PLACEHOLDER)
            .header("authorization", format!("Bearer {PLACEHOLDER}"))
            .body(Empty::new())
            .unwrap()
    }

    async fn text(response: Response<Incoming>) -> String {
        String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn http1_request_gets_the_real_secret() {
        let mut f = fixture(&[b"http/1.1"]).await;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(f.guest_tls)).await.unwrap();
        tokio::spawn(connection);
        let response = sender.send_request(request("api.test")).await.unwrap();
        assert_eq!(text(response).await, format!("HTTP/1.1 key={REAL} auth=Bearer {REAL}"));

        let event = f.events.recv().await.unwrap();
        assert_eq!(event.kind, EventKind::Http);
        assert_eq!((event.method.as_deref(), event.path.as_deref(), event.status), (Some("GET"), Some("/v1/messages"), Some(200)));
        assert_eq!(event.secret.as_deref(), Some("test"));
        assert!(!serde_json::to_string(&event).unwrap().contains(REAL));
    }

    #[tokio::test]
    async fn http2_request_gets_the_real_secret() {
        let f = fixture(&[b"h2", b"http/1.1"]).await;
        assert_eq!(f.guest_tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
        let (mut sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(f.guest_tls)).await.unwrap();
        tokio::spawn(connection);
        let request = Request::builder()
            .uri("https://api.test/v1/responses")
            .header("authorization", format!("Bearer {PLACEHOLDER}"))
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(text(response).await, format!("HTTP/2.0 key=none auth=Bearer {REAL}"));
    }

    #[tokio::test]
    async fn a_read_only_destination_lets_fetching_through_and_nothing_else() {
        let mut f = fixture_with(&[b"http/1.1"], true).await;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(f.guest_tls)).await.unwrap();
        tokio::spawn(connection);
        let response = sender.send_request(request("api.test")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = text(response).await;
        assert!(f.events.recv().await.unwrap().allowed);

        let post = Request::builder().method("POST").uri("/upload").header(HOST, "api.test").body(Empty::<Bytes>::new()).unwrap();
        let response = sender.send_request(post).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let event = f.events.recv().await.unwrap();
        assert!(!event.allowed && event.reason.contains("POST"));
    }

    #[test]
    fn a_placeholder_in_basic_sign_in_is_replaced() {
        use base64::Engine;
        let engine = base64::engine::general_purpose::STANDARD;
        let secret = Secret { name: "test".into(), hosts: vec![], placeholder: PLACEHOLDER.into(), value: REAL.into() };
        let sent = format!("Basic {}", engine.encode(format!("me@example.com:{PLACEHOLDER}")));
        let swapped = basic_with(&sent, &secret).unwrap();
        assert_eq!(engine.decode(swapped.strip_prefix("Basic ").unwrap()).unwrap(), format!("me@example.com:{REAL}").into_bytes());
        assert_eq!(basic_with(&format!("Basic {}", engine.encode("me:other")), &secret), None);
        assert_eq!(basic_with("Bearer abc", &secret), None);
    }

    #[tokio::test]
    async fn request_for_another_host_is_rejected() {
        let mut f = fixture(&[b"http/1.1"]).await;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(f.guest_tls)).await.unwrap();
        tokio::spawn(connection);
        let response = sender.send_request(request("evil.test")).await.unwrap();
        assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
        let event = f.events.recv().await.unwrap();
        assert!(!event.allowed);
        assert_eq!(event.secret, None);
    }
}
