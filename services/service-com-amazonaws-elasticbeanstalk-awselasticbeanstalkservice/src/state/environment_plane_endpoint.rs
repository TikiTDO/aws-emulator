// SPDX-License-Identifier: CC0-1.0

//! The SOCKET an environment's address is served on: the port a client dials to
//! reach the application an environment runs.
//!
//! `EnvironmentDescription$CNAME` is "the URL to the CNAME for this environment",
//! and a client takes that address and sends its traffic to it — in the CLEAR,
//! because an Elastic Beanstalk environment answers HTTP on port 80 and nothing
//! in the published address says otherwise. The one endpoint this emulator serves
//! its API on speaks TLS, so an address on it is not an address an `http://`
//! client can dial: a client that followed the published address would be handed
//! a TLS record where it expected a status line.
//!
//! So the environment plane gets a socket of the EMULATOR's own, opened through
//! the core (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the
//! bind interface and the port range), and every environment's published address
//! is that socket's `host:port` with the environment's Region and CNAME prefix in
//! the PATH ([`super::environment_plane_path`]). A client dials exactly what it
//! was told.
//!
//! ONE socket serves every environment of an engine: WHICH environment a request
//! is for is in the path, so a second port would answer the same requests at
//! another number. It is opened when the first address is needed and held on the
//! store for the life of the engine; dropping it closes the port.
//!
//! The socket is the ADDRESS, not the data plane: everything a request is
//! answered BY — which environment carries the address, what its deployed
//! application answers — stays in [`crate::service::environment_plane`], which
//! serves the same request however it was addressed. What the socket owns is the
//! TRANSPORT: it speaks HTTP/1.1 on the connection the core handed it and hands
//! each request to this service's own front door, over the in-process callback.

use super::*;

use ::std::future::Future;
use ::std::pin::Pin;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::smithy_server_runtime::EmbeddedEmulator;
use aws_smithy_runtime_api::client::http::HttpConnector as _;
use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_types::body::SdkBody;

/// The host the forwarded leg addresses this service's own front door at, so the
/// request it hands over is routed to this service exactly as a client's own API
/// call to it is.
const SERVICE_ENDPOINT: &str = "elasticbeanstalk.amazonaws.com";

/// How long one request on this socket may take before the connection answers
/// the status a proxy answers for an upstream that did not finish.
///
/// An EMULATOR bound: the request is served in this process, so this only bounds
/// a request whose serving does not return.
const REQUEST_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// What this socket answers a request its own machinery could not serve: the
/// status a proxy answers when it produced the answer itself instead of the
/// application behind the address.
const BAD_GATEWAY: u16 = 502;

/// And the status it answers a request that did not finish inside
/// [`REQUEST_DEADLINE`].
const GATEWAY_TIMEOUT: u16 = 504;

/// What it answers a request that addresses no environment plane: this socket
/// serves environment traffic and nothing else.
const NOT_SERVED_STATUS: u16 = 404;

/// The headers of the client's own request that are NOT forwarded.
///
/// A connection's own framing belongs to the connection the client opened, not to
/// the request the front door is handed. And a CREDENTIAL a client presented on
/// this socket is never replayed: this port serves one plane, which authenticates
/// nobody, so a SigV4 `Authorization` header or any other signing header it
/// carries is dropped rather than forwarded into a request the front door would
/// then route by that credential's own scope.
const HEADERS_NOT_FORWARDED: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "authorization",
    "cookie",
];

/// The prefix of every signing header a request may carry, which is dropped with
/// the credential itself for the same reason.
const SIGNING_HEADER_PREFIX: &str = "x-amz-";

/// The socket every environment's address is served on, and the handle that keeps
/// it open.
///
/// Dropping it closes the port, so the record is held for as long as the engine
/// holds environments.
pub struct EnvironmentPlaneEndpoint {
    /// The `host:port` a client dials, which is what every environment's
    /// published address starts with.
    authority: String,
    /// The listener itself. Never read: it is held because dropping it closes
    /// the port.
    _listener: ::emulator_server::ResourceListener,
}

impl ::std::fmt::Debug for EnvironmentPlaneEndpoint {
    /// Everything about this record that is not a live socket: the address it
    /// answers on. `ResourceListener` is not `Debug`.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("EnvironmentPlaneEndpoint")
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl EnvironmentPlaneEndpoint {
    /// The `host:port` a client reaches an environment's address at.
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// The `host:port` every environment address of this engine starts with, opening
/// the socket that answers there if this is the first address needed.
///
/// Answers the address the emulator's own API endpoint is reached at when this
/// engine vends no listener at all (an emulator driven in process by a host that
/// hands requests no IO driver): the path addressing is the same either way, so
/// such a client still reaches the plane on the endpoint it is already talking
/// to.
pub async fn environment_plane_authority<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> String {
    if let Some(state) = service_state(engine) {
        if let Some(open) = state
            .lock()
            .environment_plane
            .as_ref()
            .map(|endpoint| endpoint.authority().to_string())
        {
            return open;
        }
    }
    let endpoint = open_environment_plane(engine, ctx.callback().clone()).await;
    let Some(endpoint) = endpoint else {
        return ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx);
    };
    let Some(state) = service_state(engine) else {
        return endpoint.authority().to_string();
    };
    let mut store = state.lock();
    // Another request may have opened one while this socket was being bound: the
    // first one open is the one every address names, and the other is dropped
    // with its port.
    let held = store
        .environment_plane
        .get_or_insert(endpoint)
        .authority()
        .to_string();
    held
}

/// Open the socket the environment plane is served on, or `None` when this engine
/// vends no listeners.
///
/// `callback` is the handle into the emulator this request is running within
/// (`ctx.callback()`), which is how a request the socket accepts reaches the
/// plane: the socket is not a second emulator, it is a second ADDRESS of this
/// one.
async fn open_environment_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
) -> Option<EnvironmentPlaneEndpoint> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let handler = EnvironmentPlaneSocket { callback };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all
    // — one built with no IO driver, which is what a host that drives this
    // emulator in process may hand a request — leaves the create without a socket
    // instead of taking the request down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    Some(EnvironmentPlaneEndpoint {
        authority: ::service_common::data_plane_addressing::listener_authority(&listener),
        _listener: listener,
    })
}

/// Serves the environment plane's socket: it speaks HTTP/1.1 on every connection
/// the core hands it and gives each request to this service's own front door.
///
/// Cheap to clone — its one field is a handle — which is what the core's
/// `'static` per-connection handler contract asks for.
#[derive(Clone)]
struct EnvironmentPlaneSocket {
    /// The handle into the emulator this socket belongs to.
    callback: EmbeddedEmulator,
}

impl NativeHandler for EnvironmentPlaneSocket {
    /// One call per accepted connection.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let socket = self.clone();
        Box::pin(async move { socket.serve_connection(conn).await })
    }
}

impl EnvironmentPlaneSocket {
    /// Speak HTTP/1.1 on one accepted connection until the client closes it.
    async fn serve_connection(&self, conn: ListenerConn) {
        let io = ::hyper_util::rt::TokioIo::new(conn);
        let callback = self.callback.clone();
        let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
            let callback = callback.clone();
            async move {
                // A panic anywhere behind this address is CONTAINED here and
                // answered as the protocol-shaped internal failure, exactly as
                // the endpoint a control call arrives on answers it: one client
                // cannot take the connection, the listener or the process down
                // with it.
                let facts = ::emulator_server::RequestFacts::from_request(&request);
                Ok::<_, ::std::convert::Infallible>(
                    ::emulator_server::contain_panics(facts, answer(callback, request)).await,
                )
            }
        });
        if let Err(error) = ::hyper::server::conn::http1::Builder::new()
            .timer(::hyper_util::rt::TokioTimer::new())
            .header_read_timeout(REQUEST_DEADLINE)
            .serve_connection(io, service)
            .await
        {
            // A connection the client dropped, or a request this emulator could
            // not frame, ends here and takes nothing else with it.
            ::tracing::debug!(
                %error,
                "a connection to an environment's own address ended before its response"
            );
        }
    }
}

/// Answer one request this socket accepted, from this service's own front door.
async fn answer(
    callback: EmbeddedEmulator,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let ceiling = usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX);
    let limited = Limited::new(body, ceiling);
    let served = ::tokio::time::timeout(REQUEST_DEADLINE, async move {
        let Ok(collected) = limited.collect().await else {
            return rendered(BAD_GATEWAY);
        };
        let Some(forwarded) = forwarded(parts, collected.to_bytes()) else {
            // A request this socket does not serve: one that addresses no
            // environment plane at all, or one that does not render as a request.
            return rendered(NOT_SERVED_STATUS);
        };
        match callback.http_connector().call(forwarded).await {
            Ok(response) => {
                let status = response.status().as_u16();
                let mut answer = ::http::Response::builder().status(status);
                for (name, value) in response.headers().iter() {
                    answer = answer.header(name, value);
                }
                let body = Bytes::from(
                    response
                        .into_body()
                        .bytes()
                        .map(<[u8]>::to_vec)
                        .unwrap_or_default(),
                );
                answer
                    .body(Full::new(body.clone()))
                    .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
            }
            // The plane is this same process, so this is no answer about an
            // environment: it is reported the way the plane reports an answer it
            // could not produce.
            Err(_) => rendered(BAD_GATEWAY),
        }
    })
    .await;
    served.unwrap_or_else(|_| rendered(GATEWAY_TIMEOUT))
}

/// The request this service's front door is handed for one client request: the
/// same method, path, query and body, addressed to this service's own endpoint —
/// so the plane claims it exactly as it claims a request a client sent to the
/// emulator's API endpoint under the same path.
///
/// Only a request addressing the PLANE is handed over
/// ([`crate::service::environment_plane::addresses_environment_plane`]): this
/// socket serves environment traffic and nothing else, so a path that is not an
/// environment's own is answered here rather than resolved as some other route.
/// The client's credential and signing headers do not travel with it (see
/// [`HEADERS_NOT_FORWARDED`]).
///
/// `None` when the client's own request does not address this plane, or does not
/// render as a request at all.
fn forwarded(parts: ::http::request::Parts, body: Bytes) -> Option<RtRequest> {
    let path = parts.uri.path();
    if !crate::service::environment_plane::addresses_environment_plane(path) {
        return None;
    }
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let uri = format!("https://{SERVICE_ENDPOINT}{path}{query}");
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri)
        .header(::http::header::HOST, SERVICE_ENDPOINT);
    for (name, value) in parts.headers.iter() {
        let name_text = name.as_str();
        if HEADERS_NOT_FORWARDED.contains(&name_text)
            || name_text.starts_with(SIGNING_HEADER_PREFIX)
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    let built = builder.body(SdkBody::from(body)).ok()?;
    RtRequest::try_from(built).ok()
}

/// One answer this socket renders itself, for a request it could not hand over at
/// all.
fn rendered(status: u16) -> ::http::Response<Full<Bytes>> {
    ::http::Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(Bytes::new())))
}
