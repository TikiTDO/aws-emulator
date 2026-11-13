// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Live resource listeners: the async machinery behind `listeners().http()`
//! and `listeners().tcp()`. The pure enforcement decisions live in
//! [`crate::listener_guard`]; this file binds TLS sockets on the operator-chosen
//! interface (loopback by default), applies
//! those decisions on the HTTP path, hands native connections to the agent's
//! handler, and closes the port AND its in-flight connections when the returned
//! [`ResourceListener`] drops.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{watch, Semaphore};

use crate::TlsCertificateAuthority;
use crate::listener_guard::{
    caps, classify_browser_request, is_loopback_host, is_websocket_upgrade,
    parse_listener_port_range, BrowserVerdict, PortRange, RemoteOrigins, SurfaceError,
    LISTENER_PORT_RANGE_ENV,
};

/// Max time a TLS handshake may take before the half-open connection is
/// dropped, so a client that completes TCP but stalls the handshake cannot hold
/// a connection permit open (slowloris).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// ALPN protocols an `http()` listener offers: HTTP/2 and HTTP/1.1.
const HTTP_LISTENER_ALPN: &[&[u8]] = &[b"h2", b"http/1.1"];

/// Resolve the interface the resource listeners bind. This is the SAME
/// operator control the main emulator port uses ([`crate::runtime_profile::network`]
/// — CLI `--bind` before `EMULATOR_BIND_ADDRESS`, defaulting to `127.0.0.1`), so a resource port and
/// the API share one exposure decision instead of the listeners being pinned to
/// loopback. The default keeps every listener host-only; binding a routable or
/// wildcard interface is the operator's explicit declaration (needed in a
/// container, where a client reaches the task's own address, not loopback). The
/// browser-safety checks on the HTTP path (CORS, cross-site origin, WebSocket
/// refusal, TLS-only) apply on every interface regardless.
fn bind_ip_from_env() -> Result<IpAddr, SurfaceError> {
    crate::runtime_profile::network()
        .map(|profile| profile.bind_address())
        .map_err(|e| SurfaceError::Bind(e.to_string()))
}

/// Resolve [`LISTENER_PORT_RANGE_ENV`] into a [`PortRange`], refusing a value
/// that is set but not valid UTF-8 rather than reading it as unset.
fn listener_port_range_from_env() -> Result<PortRange, SurfaceError> {
    match std::env::var(LISTENER_PORT_RANGE_ENV) {
        Ok(v) => parse_listener_port_range(Some(&v)),
        Err(std::env::VarError::NotPresent) => Ok(PortRange::Ephemeral),
        Err(std::env::VarError::NotUnicode(_)) => Err(SurfaceError::InvalidPortRange(format!(
            "{LISTENER_PORT_RANGE_ENV} is set but is not valid UTF-8"
        ))),
    }
}

/// Bind a std TCP listener on `bind_ip` within the configured range: an
/// ephemeral port when unset, otherwise the first free port in `[low, high]`.
/// The OS rejects a port another listener already holds, so concurrent resources
/// walk to distinct ports; a full range is reported rather than silently
/// falling back.
fn bind_std(bind_ip: IpAddr, range: PortRange) -> Result<std::net::TcpListener, SurfaceError> {
    match range {
        PortRange::Ephemeral => std::net::TcpListener::bind((bind_ip, 0))
            .map_err(|e| SurfaceError::Bind(e.to_string())),
        PortRange::Range(low, high) => {
            let mut last = None;
            for port in low..=high {
                match std::net::TcpListener::bind((bind_ip, port)) {
                    Ok(listener) => return Ok(listener),
                    Err(e) => last = Some(e),
                }
            }
            Err(SurfaceError::PortRangeExhausted(format!(
                "no free port in {low}-{high} on {bind_ip}{}",
                last.map(|e| format!(" (last: {e})")).unwrap_or_default()
            )))
        }
    }
}

/// Async sibling of [`bind_std`] for the native (`tcp`) path.
async fn bind_tokio(bind_ip: IpAddr, range: PortRange) -> Result<tokio::net::TcpListener, SurfaceError> {
    match range {
        PortRange::Ephemeral => tokio::net::TcpListener::bind((bind_ip, 0))
            .await
            .map_err(|e| SurfaceError::Bind(e.to_string())),
        PortRange::Range(low, high) => {
            let mut last = None;
            for port in low..=high {
                match tokio::net::TcpListener::bind((bind_ip, port)).await {
                    Ok(listener) => return Ok(listener),
                    Err(e) => last = Some(e),
                }
            }
            Err(SurfaceError::PortRangeExhausted(format!(
                "no free port in {low}-{high} on {bind_ip}{}",
                last.map(|e| format!(" (last: {e})")).unwrap_or_default()
            )))
        }
    }
}

/// Whether an HTTP request's `Host` is allowed given the bound interface. On the
/// default loopback bind only a loopback Host passes — the DNS-rebinding guard a
/// browser needs. When the operator has bound a routable interface, a Host that
/// is that address passes; a wildcard bind (`0.0.0.0`/`::`) means the operator
/// exposed the port deliberately, so any Host passes.
fn host_allowed(host_only: &str, bind_ip: IpAddr) -> bool {
    is_loopback_host(host_only)
        || bind_ip.is_unspecified()
        || host_only.parse::<IpAddr>().map(|h| h == bind_ip).unwrap_or(false)
}

/// A connection handed to a [`NativeHandler`]: an already-TLS-terminated byte
/// stream. The handler speaks its own protocol (Postgres, Redis, …) on top.
pub trait ListenerStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ListenerStream for T {}

/// Boxed connection type passed to [`NativeHandler::serve`].
pub type ListenerConn = Box<dyn ListenerStream>;

/// Agent-written handler for a native-protocol (`tcp`) listener. One call per
/// accepted, TLS-terminated connection. The handler owns the protocol and its
/// authentication; the core owns only the socket, TLS, and the connection's
/// lifetime (idle timeout, caps, teardown on Delete).
///
/// Note: `serve` returns an owned `'static` future, so capture any shared state
/// as an owned clone (e.g. `let s = self.store.clone();`) before `async move`
/// rather than borrowing `&self` across the future.
pub trait NativeHandler: Send + Sync + 'static {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Agent-written handler for an HTTP (`http`) listener. The core runs the
/// browser checks and owns CORS before this is called. The core also answers
/// every `OPTIONS` request itself (CORS preflight), so a handler does not serve
/// its own `OPTIONS` route. Capture shared state as an owned clone (see
/// [`NativeHandler`]).
pub trait HttpHandler: Send + Sync + 'static {
    fn handle(
        &self,
        req: http::Request<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = http::Response<Vec<u8>>> + Send>>;
}

/// Operator-resolved configuration for the HTTP browser check. Built once at
/// boot; never read from the environment in request-path code.
#[derive(Debug, Clone)]
pub struct HttpPolicy {
    /// Which cross-origin requests are allowed (default: same-origin only).
    pub remote_origins: RemoteOrigins,
    /// False for the control-plane / `/__admin` host: a configured remote
    /// origin can never reach it.
    pub allow_remote: bool,
}

impl Default for HttpPolicy {
    fn default() -> Self {
        Self { remote_origins: RemoteOrigins::SameOriginOnly, allow_remote: true }
    }
}

/// The agent-facing listener handle, reached via `engine.listeners()`. Cheap
/// to construct: every listener presents the process's one
/// [`TlsCertificateAuthority`], made on first use, so a client that trusts the
/// emulator's `ca.pem` reaches every listener too.
pub struct Listeners {
    _private: (),
}

impl Default for Listeners {
    fn default() -> Self {
        Self::new()
    }
}

impl Listeners {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// The rustls config every listener starts from: the process certificate, no ALPN.
    /// The certificate names loopback and the operator-chosen bind address, so a
    /// client that verifies the hostname matches the address [`ResourceListener::url`]
    /// reports.
    fn rustls_server_config(&self) -> Result<rustls::ServerConfig, SurfaceError> {
        // Refuse a malformed selected bind before creating the process CA.
        bind_ip_from_env()?;
        let ca = TlsCertificateAuthority::covering(None)
            .map_err(|e| SurfaceError::Tls(e.to_string()))?;
        ca.server_config().map_err(|e| SurfaceError::Tls(e.to_string()))
    }

    /// Open a native-protocol (TCP) listener, TLS-only, on the configured
    /// interface (`EMULATOR_BIND_ADDRESS`, loopback by default) and a port from
    /// `EMULATOR_LISTENER_PORT_RANGE` (OS-assigned when unset). The core TLS-terminates each connection and hands it to
    /// `handler`. Connections idle beyond [`caps::IDLE_TIMEOUT_SECS`] are
    /// dropped. Dropping the returned listener closes the port AND severs every
    /// in-flight connection.
    pub async fn tcp<H: NativeHandler>(
        &self,
        handler: H,
    ) -> Result<ResourceListener, SurfaceError> {
        self.tcp_with_idle(handler, Duration::from_secs(caps::IDLE_TIMEOUT_SECS)).await
    }

    /// `tcp` with an explicit idle timeout (used by tests to exercise the idle
    /// path without waiting the production default).
    pub async fn tcp_with_idle<H: NativeHandler>(
        &self,
        handler: H,
        idle: Duration,
    ) -> Result<ResourceListener, SurfaceError> {
        let server_config = Arc::new(self.rustls_server_config()?);
        let acceptor = tokio_rustls::TlsAcceptor::from(server_config);
        serve_native(handler, idle, Some(acceptor)).await
    }

    /// Open a native-protocol (TCP) listener that hands the RAW, unencrypted
    /// connection to `handler`: no TLS on byte 0. For a wire protocol whose
    /// encryption is negotiated in-band (PostgreSQL `SSLRequest`, MySQL's
    /// server-greeting-first handshake, SMTP `STARTTLS`), the protocol itself
    /// must see the client's first bytes. Same interface, port range, caps, idle
    /// timeout and teardown as [`Self::tcp`]. The handler owns the in-band
    /// refusal of plaintext (e.g. `rds.force_ssl`); the core does not encrypt.
    pub async fn tcp_plaintext<H: NativeHandler>(
        &self,
        handler: H,
    ) -> Result<ResourceListener, SurfaceError> {
        serve_native(handler, Duration::from_secs(caps::IDLE_TIMEOUT_SECS), None).await
    }
}

impl Listeners {
    /// Lend a plaintext handler ([`Self::tcp_plaintext`]) the core's TLS, for
    /// a protocol that negotiates encryption in-band (PostgreSQL answers its
    /// `SSLRequest` with `S`, MySQL sets `CLIENT_SSL`, SMTP says `STARTTLS`).
    /// The certificate, key and rustls configuration stay in the core: the
    /// handler can only run the server side of a handshake on its connection.
    pub fn tls_upgrader(&self) -> Result<TlsUpgrader, SurfaceError> {
        Ok(TlsUpgrader { config: Arc::new(self.rustls_server_config()?) })
    }
}

/// The core's TLS server, lent to a plaintext native handler. See
/// [`Listeners::tls_upgrader`].
#[derive(Clone)]
pub struct TlsUpgrader {
    config: Arc<rustls::ServerConfig>,
}

impl TlsUpgrader {
    /// Run the server side of a TLS handshake on `conn` (bounded by the same
    /// handshake deadline as [`Listeners::tcp`]) and return the decrypted
    /// stream with the ALPN protocol the client and server agreed on, if any.
    /// With `alpn = Some(id)` the server offers only `id`: a client offering
    /// other protocols is refused with `no_application_protocol`, a client
    /// offering none proceeds without ALPN (the handler decides whether that is
    /// acceptable, e.g. PostgreSQL direct TLS requires it).
    pub async fn accept(
        &self,
        conn: ListenerConn,
        alpn: Option<&[u8]>,
    ) -> std::io::Result<(ListenerConn, Option<Vec<u8>>)> {
        let mut config = (*self.config).clone();
        if let Some(id) = alpn {
            config.alpn_protocols = vec![id.to_vec()];
        }
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(conn))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "TLS handshake timed out"))??;
        let negotiated = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        Ok((Box::new(tls), negotiated))
    }
}

/// The accept loop shared by [`Listeners::tcp`] (`tls = Some`) and
/// [`Listeners::tcp_plaintext`] (`tls = None`).
async fn serve_native<H: NativeHandler>(
    handler: H,
    idle: Duration,
    tls: Option<tokio_rustls::TlsAcceptor>,
) -> Result<ResourceListener, SurfaceError> {
    {
        let acceptor = tls;
        let listener = bind_tokio(bind_ip_from_env()?, listener_port_range_from_env()?).await?;
        let addr = listener.local_addr().map_err(|e| SurfaceError::Bind(e.to_string()))?;

        let handler = Arc::new(handler);
        let sem = Arc::new(Semaphore::new(caps::MAX_CONNECTIONS));
        // A watch channel is the listener-wide cancel signal: sending (or
        // dropping the sender on Drop) severs every in-flight connection. Each
        // connection also has its own idle timeout.
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let loop_rx = cancel_rx.clone();

        let task = tokio::spawn(async move {
            loop {
                // Acquire a connection permit BEFORE accepting, so a full
                // connection table backpressures the accept loop (the next
                // client waits in the kernel backlog) instead of us accepting
                // then dropping at the cap and spinning at 100% CPU.
                let permit = tokio::select! {
                    _ = cancelled(loop_rx.clone()) => break,
                    p = Arc::clone(&sem).acquire_owned() => match p {
                        Ok(p) => p,
                        Err(_) => break, // semaphore closed
                    },
                };
                let accepted = tokio::select! {
                    _ = cancelled(loop_rx.clone()) => break,
                    r = listener.accept() => r,
                };
                let (stream, _peer) = match accepted {
                    Ok(x) => x,
                    // Transient accept error (e.g. EMFILE): back off so we don't
                    // busy-spin at 100% CPU on a persistent failure.
                    Err(_) => {
                        drop(permit);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let handler = Arc::clone(&handler);
                let conn_rx = cancel_rx.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let work = async {
                        // TLS-only, with a handshake deadline so a stalled
                        // half-open connection can't hold the permit (slowloris):
                        // a plaintext/HTTP preface also fails the handshake here
                        // and never reaches the handler.
                        let conn: ListenerConn = match acceptor {
                            Some(acceptor) => match tokio::time::timeout(
                                HANDSHAKE_TIMEOUT,
                                acceptor.accept(stream),
                            )
                            .await
                            {
                                Ok(Ok(tls)) => Box::new(tls),
                                _ => return,
                            },
                            // Plaintext handover: the handler reads byte 0.
                            None => Box::new(stream),
                        };
                        let last = Arc::new(Mutex::new(Instant::now()));
                        let guarded = ActivityStream { inner: conn, last: Arc::clone(&last) };
                        tokio::select! {
                            _ = handler.serve(Box::new(guarded)) => {}
                            _ = idle_elapsed(last, idle) => {}
                        }
                    };
                    // Listener Drop (parent cancel) severs the connection mid-flight.
                    tokio::select! {
                        _ = cancelled(conn_rx) => {}
                        _ = work => {}
                    }
                });
            }
        });

        Ok(ResourceListener {
            addr,
            shutdown: Shutdown::Tcp { accept: task.abort_handle(), cancel: cancel_tx },
        })
    }
}

impl Listeners {
    /// Open an HTTP listener, TLS, on the configured interface
    /// (`EMULATOR_BIND_ADDRESS`, loopback by default) and a port from
    /// `EMULATOR_LISTENER_PORT_RANGE` (OS-assigned when unset). The core
    /// runs the browser check (Origin / `Sec-Fetch-Site`), refuses WebSocket
    /// upgrades with 501, and owns all CORS headers before calling `handler`.
    pub async fn http<H: HttpHandler>(
        &self,
        handler: H,
        policy: HttpPolicy,
    ) -> Result<ResourceListener, SurfaceError> {
        let mut server_config = self.rustls_server_config()?;
        server_config.alpn_protocols = HTTP_LISTENER_ALPN.iter().map(|p| p.to_vec()).collect();
        let tls_config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(server_config));

        let bind_ip = bind_ip_from_env()?;
        let std_listener = bind_std(bind_ip, listener_port_range_from_env()?)?;
        std_listener.set_nonblocking(true).map_err(|e| SurfaceError::Bind(e.to_string()))?;
        let addr = std_listener.local_addr().map_err(|e| SurfaceError::Bind(e.to_string()))?;

        let handler = Arc::new(handler);
        let policy = Arc::new(policy);
        // Bound concurrent in-flight HTTP requests. Uses a dedicated, small cap
        // (not the TCP connection cap) because each in-flight request may buffer
        // a body up to MAX_BYTES_PER_CONN, so peak body memory is
        // MAX_HTTP_INFLIGHT * MAX_BYTES_PER_CONN. This caps in-flight requests,
        // not idle keep-alive connections (which the HTTP server owns).
        let limit = Arc::new(Semaphore::new(caps::MAX_HTTP_INFLIGHT));
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let handler = Arc::clone(&handler);
            let policy = Arc::clone(&policy);
            let limit = Arc::clone(&limit);
            async move { serve_http(req, handler, policy, limit, bind_ip).await }
        });

        let handle = axum_server::Handle::new();
        let serve_handle = handle.clone();
        tokio::spawn(async move {
            let _ = axum_server::from_tcp_rustls(std_listener, tls_config)
                .handle(serve_handle)
                .serve(app.into_make_service())
                .await;
        });

        Ok(ResourceListener { addr, shutdown: Shutdown::Http(handle) })
    }
}

/// Resolve when the listener-wide cancel is signalled (a `true` is sent) or the
/// sender is dropped (ResourceListener Drop). Used to sever in-flight TCP
/// connections and stop the accept loop.
async fn cancelled(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return; // sender dropped
        }
    }
}

/// Resolve once the connection has been idle (no read/write progress) for
/// `idle`. Polls once a second, which is ample for the emulator. Uses a
/// monotonic clock, so an NTP step cannot skew the timeout.
async fn idle_elapsed(last: Arc<Mutex<Instant>>, idle: Duration) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if last.lock().elapsed() >= idle {
            return;
        }
    }
}

/// Wraps a connection stream and records the time of the last byte read or
/// written, so [`idle_elapsed`] can drop a silent connection without killing a
/// healthy, actively-used one (a long-lived DB session keeps resetting it).
struct ActivityStream<S> {
    inner: S,
    last: Arc<Mutex<Instant>>,
}

impl<S: AsyncRead + Unpin> AsyncRead for ActivityStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            if buf.filled().len() > before {
                *self.last.lock() = Instant::now();
            }
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ActivityStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let r = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &r {
            if *n > 0 {
                *self.last.lock() = Instant::now();
            }
        }
        r
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Core HTTP request path: concurrency cap → Host check → WebSocket refusal →
/// browser check → preflight → handler → CORS ownership. Shared by every
/// `http()` listener.
async fn serve_http(
    req: axum::extract::Request,
    handler: Arc<dyn HttpHandler>,
    policy: Arc<HttpPolicy>,
    limit: Arc<Semaphore>,
    bind_ip: IpAddr,
) -> axum::response::Response {
    // Concurrency cap: shed load rather than buffer unbounded bodies.
    let _permit = match limit.try_acquire() {
        Ok(p) => p,
        Err(_) => return simple(503, "server busy"),
    };
    let (parts, body) = req.into_parts();
    let h = &parts.headers;
    let header = |n: &str| h.get(n).and_then(|v| v.to_str().ok());

    // 0. Host allowlist. On the default loopback bind this requires a loopback
    // Host — defense-in-depth against DNS rebinding (a rebound name pointing at
    // 127.0.0.1 would be same-origin to the browser otherwise). When the
    // operator has bound a routable or wildcard interface, the matching Host (or
    // any Host, for a wildcard) is accepted — see `host_allowed`.
    let host = match header("host").or_else(|| parts.uri.host()) {
        Some(h) => h,
        // Fail closed: with no authority we cannot run the DNS-rebinding guard,
        // and HTTP/1.1 requires a Host, so a missing one is refused rather than
        // waved through to the handler.
        None => return simple(421, "host required"),
    };
    {
        // Strip the port without mangling an IPv6 literal: "[::1]" / "[::1]:5432"
        // -> "::1"; "127.0.0.1:5432" / "localhost:9297" -> host; bare "::1" as-is.
        let host_only = if let Some(rest) = host.strip_prefix('[') {
            rest.split_once(']').map(|(h, _)| h).unwrap_or(rest)
        } else if host.matches(':').count() == 1 {
            host.split_once(':').map(|(h, _)| h).unwrap_or(host)
        } else {
            host
        };
        if !host_allowed(host_only, bind_ip) {
            return simple(421, "host not allowed");
        }
    }

    // 1. WebSocket upgrades are refused at GA.
    if is_websocket_upgrade(header("connection"), header("upgrade"), header(":protocol")) {
        return simple(501, "websocket listeners are not supported");
    }

    // 2. Browser check, before the handler runs.
    let origin = header("origin").map(|s| s.to_string());
    if let BrowserVerdict::Forbid(reason) = classify_browser_request(
        origin.as_deref(),
        header("sec-fetch-site"),
        &policy.remote_origins,
        policy.allow_remote,
    ) {
        return simple(403, reason);
    }

    // An allowlisted cross-origin request gets its origin reflected back (never
    // `*`, and never with credentials); same-origin needs no ACAO.
    let reflected = reflected_origin(origin.as_deref(), &policy);

    // 3. CORS preflight: answer OPTIONS ourselves (the handler never sees it).
    if parts.method == http::Method::OPTIONS {
        return preflight(reflected.as_deref(), h);
    }

    // 4. Read the body and call the handler, both under one request deadline so
    // a slow-drip body or a slow handler cannot pin an in-flight permit (there
    // are only MAX_HTTP_INFLIGHT of them). Kept well under the tcp idle timeout.
    let deadline = Duration::from_secs(caps::HTTP_REQUEST_DEADLINE_SECS);
    let built = tokio::time::timeout(deadline, async move {
        let bytes = axum::body::to_bytes(body, caps::MAX_BYTES_PER_CONN as usize)
            .await
            .map_err(|_| 413u16)?;
        let mut builder =
            http::Request::builder().method(parts.method.clone()).uri(parts.uri.clone());
        if let Some(hm) = builder.headers_mut() {
            *hm = parts.headers.clone();
        }
        let user_req = builder.body(bytes.to_vec()).map_err(|_| 400u16)?;
        Ok::<_, u16>(handler.handle(user_req).await)
    })
    .await;
    let user_resp = match built {
        Ok(Ok(resp)) => resp,
        Ok(Err(413)) => return simple_cors(413, "request body too large", reflected.as_deref()),
        Ok(Err(_)) => return simple_cors(400, "malformed request", reflected.as_deref()),
        Err(_) => return simple_cors(408, "request timed out", reflected.as_deref()),
    };

    // 5. The core owns CORS: strip anything the handler set, then set the
    // correct headers. Vary: Origin is merged with any Vary the handler kept
    // (we drop only ACA* and re-derive Vary below).
    let (mut rparts, rbody) = user_resp.into_parts();
    let mut extra_vary: Vec<String> = Vec::new();
    if let Some(v) = rparts.headers.get(http::header::VARY).and_then(|v| v.to_str().ok()) {
        for part in v.split(',') {
            let p = part.trim();
            if !p.is_empty() && !p.eq_ignore_ascii_case("origin") {
                extra_vary.push(p.to_string());
            }
        }
    }
    let aca: Vec<http::HeaderName> = rparts
        .headers
        .keys()
        .filter(|n| n.as_str().starts_with("access-control-"))
        .cloned()
        .collect();
    for n in aca {
        rparts.headers.remove(&n);
    }
    rparts.headers.remove(http::header::VARY);
    let vary_value = {
        let mut v = vec!["origin".to_string()];
        v.extend(extra_vary);
        v.join(", ")
    };
    if let Ok(hv) = http::HeaderValue::from_str(&vary_value) {
        rparts.headers.insert(http::header::VARY, hv);
    }
    if let Some(o) = reflected {
        if let Ok(hv) = http::HeaderValue::from_str(&o) {
            rparts.headers.insert(
                http::HeaderName::from_static("access-control-allow-origin"),
                hv,
            );
        }
    }
    axum::response::Response::from_parts(rparts, axum::body::Body::from(rbody))
}

/// The origin to reflect in `Access-Control-Allow-Origin`, if any: present only
/// when the request carried an `Origin` that the operator allowlist admits for
/// this host. Same-origin / non-browser requests reflect nothing.
fn reflected_origin(origin: Option<&str>, policy: &HttpPolicy) -> Option<String> {
    let o = origin?;
    if o == "null" {
        return None;
    }
    match &policy.remote_origins {
        RemoteOrigins::Exact(list) if policy.allow_remote && list.iter().any(|x| x == o) => {
            Some(o.to_string())
        }
        _ => None,
    }
}

/// Build a CORS preflight (OPTIONS) response. If the origin is allowlisted we
/// echo the requested method/headers; otherwise a bare 204 with no CORS grant.
fn preflight(reflected: Option<&str>, req_headers: &http::HeaderMap) -> axum::response::Response {
    let mut b = axum::response::Response::builder().status(204).header("vary", "origin");
    if let Some(o) = reflected {
        b = b.header("access-control-allow-origin", o);
        let methods = req_headers
            .get("access-control-request-method")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("GET, POST, PUT, DELETE, PATCH, OPTIONS");
        b = b.header("access-control-allow-methods", methods);
        if let Some(hdrs) =
            req_headers.get("access-control-request-headers").and_then(|v| v.to_str().ok())
        {
            b = b.header("access-control-allow-headers", hdrs);
        }
        b = b.header("access-control-max-age", "600");
    }
    b.body(axum::body::Body::empty()).expect("preflight builds")
}

fn simple(status: u16, msg: &str) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(axum::body::Body::from(msg.to_string()))
        .expect("static response builds")
}

/// Like [`simple`] but carries the CORS headers an allowlisted browser client
/// needs to actually read the response. Used for errors returned AFTER the
/// browser check has passed (413/408/400), where the reflected origin is known;
/// without these an allowlisted SPA would see only an opaque network error
/// instead of the status and body.
fn simple_cors(status: u16, msg: &str, reflected: Option<&str>) -> axum::response::Response {
    let mut b = axum::response::Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .header("vary", "origin");
    if let Some(o) = reflected {
        b = b.header("access-control-allow-origin", o);
    }
    b.body(axum::body::Body::from(msg.to_string()))
        .expect("static response builds")
}

/// Handle to a running resource listener. The listener is the resource: drop
/// it on Delete and the port AND its in-flight connections close. The announced
/// address/port is what the control plane reports as the resource endpoint.
pub struct ResourceListener {
    addr: SocketAddr,
    shutdown: Shutdown,
}

enum Shutdown {
    Http(axum_server::Handle),
    Tcp { accept: tokio::task::AbortHandle, cancel: watch::Sender<bool> },
}

impl ResourceListener {
    /// The address the listener is bound to (e.g. `127.0.0.1`, or `0.0.0.0` on a
    /// wildcard bind). To hand a client an endpoint, use [`url`](Self::url), or
    /// [`authority`](Self::authority) for a field without a scheme.
    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }
    /// The OS-assigned port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    /// The URL a client connects to, such as `https://127.0.0.1:52811`: always
    /// `https`, loopback for a wildcard bind (which has no single address to dial),
    /// and an IPv6 address in brackets. Append the resource's own path to it.
    pub fn url(&self) -> String {
        format!("https://{}", self.authority())
    }
    /// The `host:port` a client dials, such as `127.0.0.1:52811` or `[::1]:52811`:
    /// the host [`url`](Self::url) names, without the scheme. Use it for a field an
    /// AWS API returns without a scheme, such as an OpenSearch domain's `Endpoint`.
    pub fn authority(&self) -> String {
        authority_for(self.addr)
    }
}

/// The `host:port` a client dials for a listener bound on `addr`: loopback for a wildcard
/// bind, an IPv6 address in brackets. See [`ResourceListener::authority`].
pub(crate) fn authority_for(addr: SocketAddr) -> String {
    SocketAddr::new(crate::dialable_host(addr.ip()), addr.port()).to_string()
}

/// The `https` URL a client dials for a listener bound on `addr`; see [`ResourceListener::url`].
pub(crate) fn url_for(addr: SocketAddr) -> String {
    format!("https://{}", authority_for(addr))
}

impl Drop for ResourceListener {
    fn drop(&mut self) {
        match &self.shutdown {
            // Non-graceful: stops serving and drops in-flight HTTP connections.
            Shutdown::Http(h) => h.shutdown(),
            // Signal cancel (severs in-flight connections) and stop accepting.
            Shutdown::Tcp { accept, cancel } => {
                let _ = cancel.send(true);
                accept.abort();
            }
        }
    }
}

#[cfg(test)]
mod host_allowed_tests {
    use super::*;

    #[test]
    fn host_allowed_tracks_the_bound_interface() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let wildcard: IpAddr = "0.0.0.0".parse().unwrap();
        let routable: IpAddr = "10.0.0.5".parse().unwrap();

        // Default loopback bind: only a loopback Host passes (DNS-rebinding guard).
        assert!(host_allowed("127.0.0.1", loopback));
        assert!(host_allowed("localhost", loopback));
        assert!(host_allowed("::1", loopback));
        assert!(!host_allowed("10.0.0.5", loopback));
        assert!(!host_allowed("evil.example", loopback));

        // Wildcard bind: the operator exposed the port, so any Host passes.
        assert!(host_allowed("evil.example", wildcard));
        assert!(host_allowed("10.0.0.5", wildcard));

        // Routable bind: loopback or that exact address, nothing else.
        assert!(host_allowed("10.0.0.5", routable));
        assert!(host_allowed("127.0.0.1", routable));
        assert!(!host_allowed("10.0.0.6", routable));
        assert!(!host_allowed("evil.example", routable));
    }

    #[test]
    fn url_is_https_dials_loopback_for_a_wildcard_and_brackets_ipv6() {
        let url = |addr: &str| url_for(addr.parse().unwrap());
        assert_eq!(url("127.0.0.1:52811"), "https://127.0.0.1:52811");
        assert_eq!(url("[::1]:52811"), "https://[::1]:52811");
        assert_eq!(url("0.0.0.0:52811"), "https://127.0.0.1:52811");
        assert_eq!(url("[::]:52811"), "https://[::1]:52811");
        assert_eq!(url("10.0.0.5:443"), "https://10.0.0.5:443");
        assert_eq!(url("[fd00::5]:443"), "https://[fd00::5]:443");
    }

    #[test]
    fn authority_is_the_url_without_its_scheme() {
        let authority = |addr: &str| authority_for(addr.parse().unwrap());
        // Loopback as itself; IPv6 bracketed; a wildcard bind dials loopback.
        assert_eq!(authority("127.0.0.1:52811"), "127.0.0.1:52811");
        assert_eq!(authority("[::1]:52811"), "[::1]:52811");
        assert_eq!(authority("[fd00::5]:443"), "[fd00::5]:443");
        assert_eq!(authority("0.0.0.0:52811"), "127.0.0.1:52811");
        assert_eq!(authority("[::]:52811"), "[::1]:52811");
        for addr in ["127.0.0.1:1", "[::]:2", "10.0.0.5:3"] {
            let addr = addr.parse().unwrap();
            assert_eq!(url_for(addr), format!("https://{}", authority_for(addr)));
        }
    }
}
