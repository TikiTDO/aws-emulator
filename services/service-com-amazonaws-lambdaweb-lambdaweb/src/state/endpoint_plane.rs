// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a web function endpoint is published on, and what this emulator answers
//! there.
//!
//! `FunctionEndpointSummary$domainName` and `GetWebFunctionEndpointResponse$domainName` are
//! "The domain name assigned to the endpoint", and that domain is the whole DATA PLANE of a
//! web function endpoint: a client reads it off the endpoint and issues its HTTP request to
//! it. A published address is therefore an address A CLIENT USES, so this emulator
//! publishes one it answers on ITSELF — a listener the core owns
//! (`engine.listeners().http(..)`, which owns the socket, the bind interface and the TLS) —
//! rather than a name under a zone this emulator does not serve, which resolves to nothing
//! on the machine it runs on and would send a client off the emulator entirely.
//!
//! ONE listener per endpoint, so the ADDRESS identifies the endpoint: a client that was
//! handed `domainName` dials it with the endpoint's own path and nothing else, so the
//! endpoint cannot be carried in a path segment. The handle lives in
//! [`super::ComAmazonawsLambdawebLambdawebState::endpoint_planes`] under the endpoint's
//! ARN, beside the records that say which endpoints exist: dropping it closes the port, so
//! a deleted endpoint stops answering ([`super::ComAmazonawsLambdawebLambdawebState::
//! prune_endpoint_planes`]). How many addresses one store can hold open is bounded by the
//! resources a caller may create: the one create path that opens an address creates at most
//! one endpoint per web function, and the store holds at most
//! [`super::MAX_WEB_FUNCTIONS`] of those.
//!
//! The socket, the bind interface and the TLS are the CORE's: the listener is opened
//! through `engine.listeners()`, so this plane binds the interface the operator chose
//! (loopback unless they asked otherwise) and never names an address of its own.
//!
//! What the address answers, in this order:
//!
//! * a request for an endpoint this store no longer holds, or one that is not yet (or no
//!   longer) `Active` — nothing is published there, so `404`;
//! * on an `IamAuth` endpoint ("Lambda uses IAM to authorize requests, which must be signed
//!   with Signature Version 4 (SigV4)"), a request presenting no SigV4 credential at all —
//!   `403` with Lambda's own `Forbidden` document, which is what an unsigned request to an
//!   IAM-authorized Lambda endpoint earns. What is checked is that the request PRESENTS a
//!   SigV4 credential (in the `Authorization` header or as the query parameters of a
//!   presigned URL); verifying a signature is the emulator's own concern and no service
//!   does it, so nothing here claims to. An `ApplicationManaged` endpoint ("Your
//!   application handles authorization") refuses nothing here;
//! * an authorized request — the web function's OWN handler response belongs here, and this
//!   emulator runs no web function code (the revision's deployment package is JavaScript
//!   for a Node.js runtime, and no execution engine is mounted in this crate), so the
//!   honest answer is `501`. Nothing is synthesised in the handler's name.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use ::emulator_server::{ListenerConn, ResourceListener, TlsUpgrader};
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::{ComAmazonawsLambdawebLambdawebState as ServiceState, EndpointState};

/// How many endpoint request BODIES this service reads at once, across every published
/// address: the core's own HTTP in-flight cap, one permit per request, taken before a byte
/// of a body is read and released when the answer is written.
///
/// What one in-flight request may buffer is the core's per-connection ceiling, so what the
/// bodies in flight hold at once is this count times that ceiling — the product the core's
/// own HTTP listeners bound — rather than the number of endpoints times it. ONE semaphore
/// for the whole service, because a caller chooses how many endpoints exist and each one
/// would otherwise raise the peak again. A request that finds no permit is shed, which is
/// what the core's own HTTP listener answers such a request.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| {
        Arc::new(Semaphore::new(
            ::emulator_server::listener_caps::MAX_HTTP_INFLIGHT,
        ))
    }))
}

/// What the address answers when the endpoint it was opened for is gone, or is not serving
/// traffic yet. One sentence carrying nothing from the request, so a caller cannot probe
/// with it.
const NO_ENDPOINT_AT_THIS_ADDRESS: &str = "No web function endpoint is published at this \
                                           address.\n";

/// What an `IamAuth` endpoint answers a request that presents no SigV4 credential: the
/// document Lambda answers an unauthorized request to an IAM-authorized endpoint with.
const FORBIDDEN_DOCUMENT: &str = "{\"Message\":\"Forbidden\"}";

/// What the address answers an AUTHORIZED request: the handler's own response belongs
/// there, and no handler ran — see this module's header.
const NO_WEB_FUNCTION_HANDLER: &str = "This web function endpoint is not implemented: the \
                                       emulator runs no web function code, so the handler's \
                                       own response cannot be served.\n";

/// What it answers a request that arrived while this service was already reading as many
/// endpoint bodies as it reads at once.
const SERVICE_UNAVAILABLE: u16 = 503;

/// The body of an answer for a request shed at the in-flight cap.
const SERVER_BUSY: &str = "This address is serving as many requests as it can. Try again.\n";

/// The query parameter a presigned SigV4 request carries its credential scope in, which is
/// the other place a request may present one.
const PRESIGNED_CREDENTIAL: &str = "X-Amz-Credential";

/// One endpoint's data-plane listener: the address it answers on, and the handle that keeps
/// it open.
pub struct EndpointPlane {
    /// The `host:port` a client dials, which is what the endpoint reports as its
    /// `domainName`.
    authority: String,
    /// The listener itself. Never read: it is held because dropping it closes the port.
    _listener: ResourceListener,
}

impl EndpointPlane {
    /// The `host:port` a client reaches this endpoint at — its `domainName`.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// Written by hand rather than derived: an [`EndpointPlane`] holds a core
/// `ResourceListener`, which is a live socket and carries no `Debug`. What is diagnosable
/// is the address it announced.
impl std::fmt::Debug for EndpointPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EndpointPlane")
            .field("authority", &self.authority)
            .finish()
    }
}

/// Open the listener the endpoint `endpoint_name` of `function_name` is served on.
///
/// `None` when this engine vends no listener, which leaves the caller to answer with a
/// modelled server error rather than publish an address nothing serves.
pub async fn open_endpoint_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ServiceState>,
    function_name: &str,
    endpoint_name: &str,
) -> Option<EndpointPlane> {
    let listeners = super::shared_engine(engine)?.listeners();
    let handler = EndpointFrontDoor {
        state: state.clone(),
        function_name: function_name.to_string(),
        endpoint_name: endpoint_name.to_string(),
        // The core's own TLS server, lent to this plaintext listener so the `https://`
        // half of the address is terminated by the core rather than here. A listener this
        // emulator could prepare none for serves the `http://` half alone.
        upgrader: listeners.tls_upgrader().ok(),
    };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one
    // built with no IO driver, which is what a host driving this emulator in process may
    // hand a request — leaves the create without an address instead of taking the request
    // down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    Some(EndpointPlane {
        // The core's OWN reading of the address a client dials, which names the loopback
        // interface for a wildcard bind — where `host()` would report `0.0.0.0`, an
        // address no client dials.
        authority: listener.authority(),
        _listener: listener,
    })
}

/// Serves ONE endpoint's published address: it answers every request it accepts from this
/// store, over the scheme the client opened with.
struct EndpointFrontDoor {
    state: ::engine::ServiceState<ServiceState>,
    function_name: String,
    endpoint_name: String,
    upgrader: Option<TlsUpgrader>,
}

impl ::emulator_server::NativeHandler for EndpointFrontDoor {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(serve_connection(
            self.state.clone(),
            self.function_name.clone(),
            self.endpoint_name.clone(),
            self.upgrader.clone(),
            conn,
        ))
    }
}

/// Speak HTTP/1.1 on one accepted connection until the client closes it, answering each
/// request from this store.
///
/// The encryption is negotiated in band first ([`::service_common::in_band_tls`]), so the
/// ONE address an endpoint publishes serves both the `https://` a client of AWS dials and
/// the `http://` a client that does not verify a self-signed certificate dials. HTTP/1.1
/// only, as the emulator's own front door offers: every request is fully buffered, so
/// nothing relies on HTTP/2 streaming. The header-read deadline and the body ceiling are
/// the core's, so a client that dribbles a request line or sends more than one connection
/// may carry cannot hold the connection permit the core issued for it.
async fn serve_connection(
    state: ::engine::ServiceState<ServiceState>,
    function_name: String,
    endpoint_name: String,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    let Some(conn) = ::service_common::in_band_tls::negotiated(conn, upgrader).await else {
        return;
    };
    let io = ::hyper_util::rt::TokioIo::new(conn);
    let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
        let state = state.clone();
        let function_name = function_name.clone();
        let endpoint_name = endpoint_name.clone();
        async move {
            // A panic anywhere behind this address is CONTAINED here and answered as the
            // protocol-shaped internal failure, exactly as the endpoint a control call
            // arrives on answers the same request: one client cannot take the connection,
            // the listener or the process down with it.
            let facts = ::emulator_server::RequestFacts::from_request(&request);
            Ok::<_, ::std::convert::Infallible>(
                ::emulator_server::contain_panics(
                    facts,
                    answer(state, function_name, endpoint_name, request),
                )
                .await,
            )
        }
    });
    if let Err(error) = ::hyper::server::conn::http1::Builder::new()
        .timer(::hyper_util::rt::TokioTimer::new())
        .header_read_timeout(::std::time::Duration::from_secs(
            ::emulator_server::listener_caps::HTTP_REQUEST_DEADLINE_SECS,
        ))
        .serve_connection(io, service)
        .await
    {
        // A connection the client dropped, or a request this emulator could not frame,
        // ends here and takes nothing else with it: the next connection is served
        // normally.
        ::tracing::debug!(%error, "a connection to a web function endpoint ended before its response");
    }
}

/// Answer one request to the published address of the endpoint this listener was opened
/// for.
///
/// The body is read up to the core's per-connection ceiling before the decision is taken,
/// so a request larger than this address accepts is refused rather than buffered.
async fn answer(
    state: ::engine::ServiceState<ServiceState>,
    function_name: String,
    endpoint_name: String,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<::http_body_util::Full<::bytes::Bytes>> {
    let (parts, body) = request.into_parts();
    if let Some(refused) = refused_before_the_endpoint(&parts) {
        return refused;
    }
    let ceiling =
        usize::try_from(::emulator_server::listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX);
    // One permit per request IN FLIGHT, taken BEFORE a byte of the body is read: what the
    // bodies being read hold at once is then the in-flight cap times the ceiling below,
    // rather than the number of published endpoints times it.
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return served(plain(false, SERVICE_UNAVAILABLE, SERVER_BUSY));
    };
    // Held for the whole read and the decision, and released with them.
    let _permit: OwnedSemaphorePermit = permit;
    let Ok(collected) = ::http_body_util::BodyExt::collect(::http_body_util::Limited::new(
        body, ceiling,
    ))
    .await
    else {
        return served(plain(false, PAYLOAD_TOO_LARGE, BODY_TOO_LARGE));
    };
    let buffered = ::http::Request::from_parts(parts, collected.to_bytes().to_vec());
    served(serve_request(
        &state,
        &function_name,
        &endpoint_name,
        &buffered,
    ))
}

/// One answer of [`serve_request`] as the connection writes it.
fn served(
    response: ::http::Response<Vec<u8>>,
) -> ::http::Response<::http_body_util::Full<::bytes::Bytes>> {
    let (parts, body) = response.into_parts();
    ::http::Response::from_parts(parts, ::http_body_util::Full::new(::bytes::Bytes::from(body)))
}

/// The transport-safety refusals the core applies to its OWN HTTP listeners, applied here
/// from the core's own predicates so this address and a core listener decide them alike:
/// the `Host` allowlist (the DNS-rebinding guard a browser needs), the WebSocket refusal
/// and the browser check. `None` lets the request through to the endpoint's own decision.
fn refused_before_the_endpoint(
    parts: &::http::request::Parts,
) -> Option<::http::Response<::http_body_util::Full<::bytes::Bytes>>> {
    let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());

    // HTTP/1.1 requires a `Host`, and without an authority the rebinding guard cannot be
    // run at all, so a request carrying none is refused rather than served: this fails
    // CLOSED, exactly as the core's own HTTP listener does.
    let Some(host) = header("host").or_else(|| parts.uri.host()) else {
        return Some(served(plain(false, MISDIRECTED_REQUEST, HOST_REQUIRED)));
    };
    if !host_allowed(::service_common::data_plane_addressing::host_only(host)) {
        return Some(served(plain(false, MISDIRECTED_REQUEST, HOST_NOT_ALLOWED)));
    }
    if ::emulator_server::is_websocket_upgrade(
        header("connection"),
        header("upgrade"),
        header(":protocol"),
    ) {
        return Some(served(plain(false, 501, NO_WEBSOCKETS)));
    }
    if let ::emulator_server::BrowserVerdict::Forbid(reason) =
        ::emulator_server::classify_browser_request(
            header("origin"),
            header("sec-fetch-site"),
            // This address is the emulator's own, reached from the machine it runs on: the
            // same default a core HTTP listener carries, which admits no remote origin.
            &::emulator_server::RemoteOrigins::SameOriginOnly,
            false,
        )
    {
        let body = ::bytes::Bytes::from(reason.to_string());
        return Some(
            ::http::Response::builder()
                .status(FORBIDDEN)
                .header("content-type", "text/plain; charset=utf-8")
                .body(::http_body_util::Full::new(body.clone()))
                .unwrap_or_else(|_| {
                    let mut response = ::http::Response::new(::http_body_util::Full::new(body));
                    *response.status_mut() = ::http::StatusCode::FORBIDDEN;
                    response
                }),
        );
    }
    None
}

/// Whether a request's `Host` names the interface this address was bound on, by the rule a
/// core HTTP listener applies: a loopback `Host` on the default loopback bind (which is
/// what guards a browser against DNS rebinding), the operator's own address when they
/// bound a routable one, and any `Host` on a wildcard bind — there the operator exposed the
/// port deliberately and no single address names it.
fn host_allowed(host: &str) -> bool {
    if ::emulator_server::is_loopback_host(host) {
        return true;
    }
    let Ok(bound) = ::emulator_server::bind_address_from_env() else {
        return false;
    };
    bound.is_unspecified()
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<::std::net::IpAddr>()
            .is_ok_and(|named| named == bound)
}

/// What this address answers a request whose `Host` is not the address it serves, or that
/// names none.
const MISDIRECTED_REQUEST: u16 = 421;

/// What it answers a request a browser made cross-site.
const FORBIDDEN: u16 = 403;

/// What it answers a request carrying more than one connection may.
const PAYLOAD_TOO_LARGE: u16 = 413;

/// The body of an answer for a request whose `Host` is not the address it serves.
const HOST_NOT_ALLOWED: &str = "This address does not serve that host.\n";

/// The body of an answer for a request that names no host at all, which no HTTP/1.1
/// request may.
const HOST_REQUIRED: &str = "This address requires the host it is addressed by.\n";

/// The body of an answer for a WebSocket upgrade, which no web function endpoint serves.
const NO_WEBSOCKETS: &str = "This address does not serve WebSocket connections.\n";

/// The body of an answer for a request larger than this address accepts.
const BODY_TOO_LARGE: &str = "This request is larger than this address accepts.\n";

/// One request to a published endpoint, answered from this store — the decision the
/// listener's handler serves, and the one this service's tests drive.
///
/// The endpoint is resolved FIRST and the authorization second, which is the order the real
/// address works in: only an address that publishes an endpoint answers at all, and then
/// the endpoint's `authType` decides. The one write is the clock's own
/// ([`ServiceState::materialise_due_transitions`]), owed at this instant whether or not
/// anybody connected, so reaching an endpoint changes nothing a later caller sees.
pub fn serve_request(
    state: &::engine::ServiceState<ServiceState>,
    function_name: &str,
    endpoint_name: &str,
    req: &::http::Request<Vec<u8>>,
) -> ::http::Response<Vec<u8>> {
    let head = req.method() == ::http::Method::HEAD;
    let presented = presents_sigv4_credential(req);

    let mut store = state.lock();
    store.materialise_due_transitions(SystemTime::now());
    let Some(endpoint) = store
        .functions
        .get(function_name)
        .and_then(|function| function.endpoints.get(endpoint_name))
        .filter(|endpoint| endpoint.state == EndpointState::Active)
    else {
        return plain(head, 404, NO_ENDPOINT_AT_THIS_ADDRESS);
    };
    let auth_type = endpoint.auth_type.clone();
    let endpoint_arn = endpoint.endpoint_arn.clone();
    drop(store);

    if auth_type == super::AUTH_TYPE_IAM && !presented {
        // Every attempt on a published endpoint is attributable: the endpoint addressed and
        // the outcome. Nothing the client sent is logged — not the path, not a header.
        ::tracing::info!(
            audit = true,
            operation = "WebFunctionEndpointRequest",
            outcome = "refused",
            endpoint_arn = %endpoint_arn,
            "web function endpoint request presenting no SigV4 credential"
        );
        return json(head, 403, FORBIDDEN_DOCUMENT);
    }
    plain(head, 501, NO_WEB_FUNCTION_HANDLER)
}

/// Whether `req` PRESENTS a SigV4 credential: the `Authorization` header a signed request
/// carries its credential scope in ([`::emulator_server::sigv4_credential_scope`]), or the
/// `X-Amz-Credential` query parameter a presigned URL carries it in.
///
/// Presenting a credential is not the same as a verified signature, and nothing here claims
/// otherwise: verifying a request's signature is the emulator's own concern and no service
/// does it.
fn presents_sigv4_credential(req: &::http::Request<Vec<u8>>) -> bool {
    if ::emulator_server::sigv4_credential_scope(req.headers()).is_some() {
        return true;
    }
    req.uri().query().is_some_and(|query| {
        query
            .split('&')
            .filter_map(|pair| pair.split('=').next())
            .any(|name| name.eq_ignore_ascii_case(PRESIGNED_CREDENTIAL))
    })
}

/// One plain-text answer of this address's own.
///
/// A published endpoint is plain HTTP rather than a modelled operation, so it answers in
/// its own form instead of this service's restJson1 envelope. Every message is a constant
/// of this module, never built from the request. A `HEAD` carries the metadata of what a
/// `GET` of the same address would answer — its status and its length — without the body,
/// which is what the method means.
fn plain(head: bool, status: u16, message: &'static str) -> ::http::Response<Vec<u8>> {
    text_answer(head, status, "text/plain; charset=utf-8", message)
}

/// One JSON answer of this address's own, for the refusal Lambda words as a document.
fn json(head: bool, status: u16, document: &'static str) -> ::http::Response<Vec<u8>> {
    text_answer(head, status, "application/json", document)
}

/// The response both forms above are built from.
fn text_answer(
    head: bool,
    status: u16,
    content_type: &'static str,
    message: &'static str,
) -> ::http::Response<Vec<u8>> {
    let body = if head {
        Vec::new()
    } else {
        message.as_bytes().to_vec()
    };
    ::http::Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("content-length", message.len().to_string())
        .body(body)
        // Every argument is this module's own constant, so the builder cannot fail; answer
        // a bare 500 rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Vec::new());
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request of `method` to `uri` carrying the headers `headers`.
    fn request(method: &str, uri: &str, headers: &[(&str, &str)]) -> ::http::Request<Vec<u8>> {
        let mut builder = ::http::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Vec::new()).expect("a well-formed request")
    }

    /// One row: the case, the request URI, the headers it carries, and whether it presents
    /// a credential.
    type CredentialRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], bool);

    /// A SigV4 credential travels in the `Authorization` header or in a presigned URL's
    /// query, and nothing else presents one.
    #[test]
    fn a_sigv4_credential_is_recognised_wherever_a_client_presents_it() {
        let signed = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/lambda/\
                      aws4_request, SignedHeaders=host, Signature=deadbeef";
        let rows: &[CredentialRow<'_>] = &[
            ("a signed GET", "/", &[("authorization", signed)], true),
            (
                "a presigned GET",
                "/?X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Flambda%2Faws4_request\
                 &X-Amz-Signature=deadbeef",
                &[],
                true,
            ),
            (
                "a presigned GET whose parameter is lower-cased",
                "/?x-amz-credential=AKIDEXAMPLE",
                &[],
                true,
            ),
            ("an unsigned GET", "/", &[], false),
            ("an unsigned GET with a query", "/items?page=2", &[], false),
            (
                "another authorization scheme",
                "/",
                &[("authorization", "Basic dXNlcjpwYXNz")],
                false,
            ),
        ];
        for (case, uri, headers, expected) in rows {
            let presented = presents_sigv4_credential(&request("GET", uri, headers));
            assert_eq!(
                presented, *expected,
                "case {case:?}: expected presents-a-credential {expected}, got {presented}"
            );
        }
    }

    /// A `HEAD` carries the status and the length a `GET` would answer, and no body.
    #[test]
    fn a_head_answers_the_metadata_of_the_get() {
        for (case, head, expected_len) in [
            ("a GET", false, NO_WEB_FUNCTION_HANDLER.len()),
            ("a HEAD", true, 0),
        ] {
            let answered = plain(head, 501, NO_WEB_FUNCTION_HANDLER);
            assert_eq!(
                answered.status().as_u16(),
                501,
                "case {case:?}: expected 501, got {}",
                answered.status()
            );
            assert_eq!(
                answered
                    .headers()
                    .get("content-length")
                    .and_then(|value| value.to_str().ok()),
                Some(NO_WEB_FUNCTION_HANDLER.len().to_string().as_str()),
                "case {case:?}: expected the length of the GET's body"
            );
            assert_eq!(
                answered.body().len(),
                expected_len,
                "case {case:?}: expected a body of {expected_len} bytes, got {}",
                answered.body().len()
            );
        }
    }
}
