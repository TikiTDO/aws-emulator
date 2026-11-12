// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's REAL-TIME endpoint — `GraphqlApi$uris`'s `REALTIME` entry — and the
//! WebSocket conversation it carries.
//!
//! What it serves, over RFC 6455 on the API's own socket:
//!
//! > An opening handshake on [`REALTIME_PATH`] offering the [`SUBPROTOCOL`] subprotocol is
//! > completed, and one addressed to anything else refused. Over the established socket:
//! > `connection_init` is answered `connection_ack` and the keep-alives
//! > ([`KEEP_ALIVE_INTERVAL`]) that follow it when the material the connection carries
//! > authorizes a connection to the API, and `connection_error` otherwise, which ends the
//! > connection; a `start` naming a subscription of the API's schema is recorded and
//! > answered `start_ack` (or `error`); a `stop` ends one and is answered `complete`; and an
//! > event published to a subscription of this connection arrives on it as a `data` frame.
//!
//! WHEN the material is judged is part of the contract: the opening handshake is completed
//! first and an unauthorized connection is refused at its `connection_init`, rather than
//! the handshake itself being refused — which is what a real-time client of this API
//! observes, and what distinguishes this endpoint from an Event API's, where the material
//! travels on the handshake itself.
//!
//! The handshake is read and written by the maintained RFC 6455 implementation
//! (`tokio-tungstenite`): nothing about the protocol itself is hand-written here. What this
//! module decides is which handshakes are completed and what each frame is answered with,
//! both from this service's own records — the API, its authentication configuration, its
//! API keys and the subscription fields its schema declares.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::{Arc, Mutex};
use ::std::time::Duration;

use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::futures_util::{SinkExt, StreamExt};
use ::serde_json::{json, Value as Json};
use ::tokio::io::{AsyncRead, AsyncWrite};
use ::tokio::sync::mpsc::{channel, Receiver, Sender};
use ::tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use ::tokio_tungstenite::tungstenite::Message;

use super::PlaneContext;
use crate::service::event_api::{self, PresentedAuth};
use crate::state::event_data_plane::{SubscribeRefusal, CONNECTION_FRAME_DEPTH};

/// The path the real-time endpoint serves its WebSocket at — the path `uris`'s `REALTIME`
/// entry carries, which is the same `/graphql` the API's HTTPS endpoint is addressed at:
/// the two are distinct endpoints by HOST, not by path.
pub const REALTIME_PATH: &str = "/graphql";

/// The subprotocol a GraphQL API's real-time client offers and this endpoint selects.
pub const SUBPROTOCOL: &str = "graphql-ws";

/// The query parameter a handshake carries its base64url-encoded authorization object in,
/// which is the only place a WebSocket client can put a request's own fields.
const AUTH_QUERY_PARAMETER: &str = "header";

/// How often this endpoint writes a keep-alive frame to an acknowledged connection.
///
/// A real-time client watches for them: the `connectionTimeoutMs` the `connection_ack`
/// carries is how long it waits for one before treating the connection as dead, so the
/// cadence is well inside that span.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// The largest MESSAGE this endpoint reads from a client, in bytes, and the largest FRAME
/// one may arrive in — the plane's own bounds, shared with the Event API's real-time
/// endpoint so a client's bytes are buffered the same way whichever endpoint they arrive
/// on.
const MAX_MESSAGE_BYTES: usize = event_api::MAX_MESSAGE_BYTES;
const MAX_FRAME_BYTES: usize = 32 * 1024;

/// Serves one GraphQL API's real-time endpoint: the socket `uris`'s `REALTIME` entry
/// publishes.
pub struct RealtimeSocket {
    /// What every connection on this socket is answered from.
    context: PlaneContext,
    /// The core's TLS server, for a client that dialled `wss://`.
    upgrader: Option<TlsUpgrader>,
}

impl RealtimeSocket {
    /// The endpoint of one API.
    pub fn new(context: PlaneContext, upgrader: Option<TlsUpgrader>) -> Self {
        Self { context, upgrader }
    }
}

impl NativeHandler for RealtimeSocket {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let context = self.context.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            serve_connection(context, upgrader, conn).await;
        })
    }
}

/// Serve one accepted connection: negotiate encryption, complete (or refuse) the opening
/// handshake, then carry frames until either side closes.
// The handshake callback's `Err` is the REFUSAL RESPONSE the WebSocket implementation
// writes (`http::Response<Option<String>>`), which is that dependency's own signature and
// is large by the `http` crate's measure. Boxing it is not available: the callback's type
// is the one `accept_hdr_async_with_config` takes.
#[allow(clippy::result_large_err)]
async fn serve_connection(
    context: PlaneContext,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    let Some(conn) = event_api::negotiated(conn, upgrader).await else {
        return;
    };
    // Decided inside the handshake callback, which is where the request's path and fields
    // are, and read back here once the handshake has been written.
    let admitted: Arc<Mutex<Option<PresentedAuth>>> = Arc::new(Mutex::new(None));
    let decide = {
        let context = context.clone();
        let admitted = Arc::clone(&admitted);
        move |request: &HandshakeRequest,
              mut response: HandshakeResponse|
              -> Result<HandshakeResponse, ErrorResponse> {
            // Every handshake this endpoint judges is one attempt to connect, whatever
            // becomes of it: the attempts are reported beside what became of them.
            context.metrics.connect_requested();
            match admission(&context, request) {
                Ok(auth) => {
                    *admitted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(auth);
                    // The subprotocol this endpoint selected, which RFC 6455 requires the
                    // answer to name when the client offered one.
                    response.headers_mut().insert(
                        ::http::header::SEC_WEBSOCKET_PROTOCOL,
                        ::http::HeaderValue::from_static(SUBPROTOCOL),
                    );
                    Ok(response)
                }
                Err(refusal) => {
                    // A connect this endpoint turned away on the client's own account is
                    // what `ConnectClientError` counts: the only report of a refused
                    // handshake a client of the API can read.
                    context.metrics.connect_client_error();
                    Err(refusal.rendered())
                }
            }
        }
    };
    let socket = match ::tokio_tungstenite::accept_hdr_async_with_config(
        conn,
        decide,
        Some(socket_limits()),
    )
    .await
    {
        Ok(socket) => socket,
        Err(error) => {
            // Either the refusal above (already written to the client by the handshake) or
            // a client that did not complete the handshake at all.
            ::tracing::debug!(
                api_id = %context.api_id,
                %error,
                "a real-time opening handshake was not completed"
            );
            return;
        }
    };
    let Some(auth) = admitted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    else {
        return;
    };
    carry_frames(context, auth, socket).await;
}

/// The explicit size bounds every socket of this endpoint is read and written under.
fn socket_limits() -> ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(CONNECTION_FRAME_DEPTH * MAX_MESSAGE_BYTES)
}

/// How a handshake this endpoint does not complete is answered: an HTTP status and the
/// `{"errors": …}` document AppSync answers a refused call with.
#[derive(Debug, Clone)]
struct Refusal {
    status: u16,
    error_type: &'static str,
    message: String,
}

impl Refusal {
    /// This refusal as the response the handshake writes.
    fn rendered(&self) -> ErrorResponse {
        let body =
            event_api::publish::errors_document(self.error_type, &self.message).to_string();
        let mut response = ErrorResponse::new(Some(body));
        *response.status_mut() = ::http::StatusCode::from_u16(self.status)
            .unwrap_or(::http::StatusCode::INTERNAL_SERVER_ERROR);
        response.headers_mut().insert(
            ::http::header::CONTENT_TYPE,
            ::http::HeaderValue::from_static(JSON_MEDIA_TYPE),
        );
        response
    }
}

/// Whether one opening handshake is completed, and with what authorization material.
///
/// In the same order, and for the same reasons, the Event API's real-time endpoint judges
/// one in ([`crate::service::event_api::realtime`] states each): the `Host` has to be one
/// these sockets serve ([`event_api::host_allowed`]), a handshake a BROWSER made from
/// another site is refused ([`event_api::cross_site_browser`]), the path has to be the one
/// this endpoint serves, and the client has to offer this endpoint's subprotocol.
///
/// The AUTHORIZATION is not judged here: this endpoint completes the handshake and refuses
/// an unauthorized connection at its `connection_init` ([`served_message`]), so what this
/// answers is the material the handshake PRESENTED, whatever it authorizes. It travels in
/// the handshake's [`AUTH_QUERY_PARAMETER`] — base64url of the object a request would carry
/// those fields in — which is where a GraphQL API's real-time client puts it, with the
/// subprotocol token the Event API's client uses accepted as well, so one reading serves
/// both forms.
fn admission(context: &PlaneContext, request: &HandshakeRequest) -> Result<PresentedAuth, Refusal> {
    let field = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let Some(host) = field("host").or_else(|| request.uri().host()) else {
        return Err(Refusal {
            status: 421,
            error_type: BAD_REQUEST_ERROR_TYPE,
            message: event_api::HOST_REQUIRED.to_string(),
        });
    };
    if !event_api::host_allowed(host) {
        return Err(Refusal {
            status: 421,
            error_type: BAD_REQUEST_ERROR_TYPE,
            message: event_api::HOST_NOT_ALLOWED.to_string(),
        });
    }
    if let Some(reason) = event_api::cross_site_browser(field("origin"), field("sec-fetch-site")) {
        ::tracing::info!(
            api_id = %context.api_id,
            %reason,
            "refusing a real-time handshake a browser made cross-site"
        );
        return Err(Refusal {
            status: 403,
            error_type: event_api::UNAUTHORIZED_ERROR_TYPE,
            message: reason,
        });
    }
    if request.uri().path() != REALTIME_PATH {
        return Err(Refusal {
            status: 404,
            error_type: NOT_FOUND_ERROR_TYPE,
            message: NO_SUCH_ROUTE.to_string(),
        });
    }
    let offered = request
        .headers()
        .get(::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !offered
        .split(',')
        .map(str::trim)
        .any(|token| token == SUBPROTOCOL)
    {
        return Err(Refusal {
            status: 400,
            error_type: BAD_REQUEST_ERROR_TYPE,
            message: format!("A real-time client offers the {SUBPROTOCOL} subprotocol."),
        });
    }
    let auth = presented_auth(request.uri().query(), offered);
    Ok(auth)
}

/// The material one handshake presented: the object its [`AUTH_QUERY_PARAMETER`] carries,
/// and the one its subprotocol list carries where it names no such parameter.
///
/// The query is read by the maintained URL implementation this crate already parses URLs
/// with (`url`), so the escaping a parameter's value carries is resolved by it rather than
/// by anything written here.
fn presented_auth(query: Option<&str>, subprotocols: &str) -> PresentedAuth {
    let encoded = query.and_then(|query| {
        ::url::form_urlencoded::parse(query.as_bytes())
            .find(|(name, _)| name == AUTH_QUERY_PARAMETER)
            .map(|(_, value)| value.to_string())
    });
    match encoded {
        Some(encoded) => PresentedAuth::from_base64url_object(&encoded),
        None => PresentedAuth::from_subprotocols(subprotocols),
    }
}

/// Carry one established connection: register it, answer every frame the client sends,
/// write every event delivered to it and the keep-alives it is owed, until either side
/// closes.
async fn carry_frames<S>(
    context: PlaneContext,
    auth: PresentedAuth,
    socket: ::tokio_tungstenite::WebSocketStream<S>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (frames, mut delivered): (Sender<String>, Receiver<String>) =
        channel(CONNECTION_FRAME_DEPTH);
    let connection_id = {
        let mut store = context.state.lock();
        let connection_id = store.graphql_data_plane.mint_connection_id();
        if !store
            .graphql_data_plane
            .open_connection(&context.api_id, connection_id.clone(), frames)
        {
            return;
        }
        connection_id
    };
    ::tracing::info!(
        api_id = %context.api_id,
        %connection_id,
        "a GraphQL API real-time connection is open"
    );
    let (mut outgoing, mut incoming) = socket.split();
    // A connection is CONNECTED once the client's `connection_init` has been acked, which
    // is what `ConnectSuccess` counts and what a `DisconnectSuccess` is then owed for.
    let mut acked = false;
    let mut keep_alive = ::tokio::time::interval(KEEP_ALIVE_INTERVAL);
    // The first tick of an interval is immediate; a keep-alive is owed after a span of it,
    // not at once.
    keep_alive.tick().await;
    loop {
        ::tokio::select! {
            event = delivered.recv() => {
                let Some(frame) = event else { break };
                if outgoing.send(Message::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            _ = keep_alive.tick(), if acked => {
                if outgoing
                    .send(Message::Text(keep_alive_frame().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            frame = incoming.next() => {
                let Some(Ok(frame)) = frame else { break };
                let body = match frame {
                    Message::Text(text) => text.as_str().to_string(),
                    Message::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Message::Close(_) => break,
                    // Ping, Pong and Frame are the protocol's own: the implementation
                    // answers a ping itself and nothing of this endpoint serves them.
                    _ => continue,
                };
                let (acked_now, closed, answers) = contained_message(
                    context.clone(),
                    auth.clone(),
                    connection_id.clone(),
                    body,
                    acked,
                )
                .await;
                acked = acked_now;
                let mut written = true;
                for answer in answers {
                    if outgoing.send(Message::Text(answer.into())).await.is_err() {
                        written = false;
                        break;
                    }
                }
                // A socket this endpoint can no longer write to carries nothing more: the
                // whole connection ends rather than waiting for the next read to fail. So
                // does one this endpoint ENDED — the refusal of an unauthorized
                // `connection_init` — once that refusal has been written.
                if !written || closed {
                    break;
                }
            }
        }
    }
    // The connection has ended: its record goes, and with it every subscription it held.
    let subscriptions = context
        .state
        .lock()
        .graphql_data_plane
        .close_connection(&context.api_id, &connection_id);
    if acked {
        context.metrics.disconnect_success();
    }
    ::tracing::info!(
        api_id = %context.api_id,
        %connection_id,
        %subscriptions,
        "a GraphQL API real-time connection is closed"
    );
}

/// Answer one message under the core's own PANIC CONTAINMENT
/// (`emulator_server::contain_panics`), and answer a contained panic with one error frame.
///
/// For the reason the Event API's real-time endpoint does it
/// ([`crate::service::event_api::realtime`] states it in full): a panic that unwinds the
/// task carrying the CONNECTION costs it the connection and every subscription on it, and a
/// plane served on a core-owned socket has no front-door layer to contain it. What the
/// contained future carries back is this endpoint's own answer — the frames to write,
/// whether the connection is now acked and whether this endpoint ended it — as a document,
/// because the containment is stated in terms of an HTTP response.
async fn contained_message(
    context: PlaneContext,
    auth: PresentedAuth,
    connection_id: String,
    body: String,
    acked: bool,
) -> (bool, bool, Vec<String>) {
    let facts = message_facts(&body);
    let guarded = ::emulator_server::contain_panics(facts, async move {
        let mut acked_now = acked;
        let mut closed = false;
        let frames = served_message(
            &context,
            &auth,
            &connection_id,
            &body,
            &mut acked_now,
            &mut closed,
        );
        let document =
            json!({ACKED_MEMBER: acked_now, CLOSED_MEMBER: closed, FRAMES_MEMBER: frames});
        ::http::Response::new(::bytes::Bytes::from(document.to_string()))
    })
    .await;
    read_contained(&guarded, acked)
}

/// The facts one message is contained under: the path this endpoint serves and the message
/// TYPE, which is what names the handler a contained panic happened in. Nothing of the
/// message's own payload travels into them.
fn message_facts(body: &str) -> ::emulator_server::RequestFacts {
    let kind = ::serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|message| {
            message
                .get(MESSAGE_TYPE_MEMBER)
                .and_then(Json::as_str)
                .map(str::to_string)
        });
    ::emulator_server::RequestFacts {
        method: ::http::Method::GET.to_string(),
        path: REALTIME_PATH.to_string(),
        content_type: Some(JSON_MEDIA_TYPE.to_string()),
        target: kind,
        signing_name: None,
        no_error_wrapping: false,
    }
}

/// What the contained answer says: the frames to write, whether the connection is now
/// acked, and whether this endpoint ended it.
///
/// A `500` is a contained panic — the only status this module's own answer carries is the
/// `200` of the document it built — so it becomes one error frame and leaves the ack as it
/// was. A document this module cannot read back is impossible (it wrote it) and is treated
/// the same way, which fails CLOSED rather than inventing an answer.
fn read_contained(
    guarded: &::http::Response<::bytes::Bytes>,
    acked: bool,
) -> (bool, bool, Vec<String>) {
    if guarded.status() == ::http::StatusCode::OK {
        if let Ok(document) = ::serde_json::from_slice::<Json>(guarded.body()) {
            let frames: Vec<String> = document
                .get(FRAMES_MEMBER)
                .and_then(Json::as_array)
                .map(|frames| {
                    frames
                        .iter()
                        .filter_map(|frame| frame.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let acked_now = document
                .get(ACKED_MEMBER)
                .and_then(Json::as_bool)
                .unwrap_or(acked);
            let closed = document
                .get(CLOSED_MEMBER)
                .and_then(Json::as_bool)
                .unwrap_or(false);
            return (acked_now, closed, frames);
        }
    }
    (
        acked,
        false,
        vec![connection_error_frame(
            event_api::publish::INTERNAL_FAILURE_ERROR_TYPE,
            event_api::publish::INTERNAL_FAILURE_MESSAGE,
        )],
    )
}

/// What one message is answered with: the frames to write back, in order.
///
/// `acked` is whether the connection has been acknowledged, and `closed` is set when this
/// endpoint ENDS the connection — which the refusal of an unauthorized `connection_init`
/// does, once that refusal has been written.
fn served_message(
    context: &PlaneContext,
    connection_auth: &PresentedAuth,
    connection_id: &str,
    body: &str,
    acked: &mut bool,
    closed: &mut bool,
) -> Vec<String> {
    let Ok(message) = ::serde_json::from_str::<Json>(body) else {
        return vec![connection_error_frame(
            BAD_REQUEST_ERROR_TYPE,
            MESSAGE_IS_A_DOCUMENT,
        )];
    };
    let kind = message
        .get(MESSAGE_TYPE_MEMBER)
        .and_then(Json::as_str)
        .unwrap_or_default();
    let id = message
        .get("id")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    match kind {
        CONNECTION_INIT => {
            // The material the connection carries is judged HERE rather than on the
            // opening handshake: an unauthorized connection is one this endpoint
            // established and then refused, which is what its client observes.
            let authorized = {
                let store = context.state.lock();
                super::authorized(&store, &context.api_id, connection_auth)
            };
            if !authorized {
                // A connect this endpoint turned away on the client's own account is what
                // `ConnectClientError` counts, and the connection ends with the refusal:
                // nothing more is served over it.
                context.metrics.connect_client_error();
                *closed = true;
                return vec![connection_error_frame(
                    event_api::UNAUTHORIZED_ERROR_TYPE,
                    event_api::UNAUTHORIZED_MESSAGE,
                )];
            }
            // Idempotent: a second `connection_init` on one connection is acked again and
            // counts no second connection.
            if !*acked {
                *acked = true;
                context.metrics.connect_success();
            }
            vec![json!({
                MESSAGE_TYPE_MEMBER: CONNECTION_ACK,
                "payload": {
                    // How long this connection may stay idle, which is the timeout the
                    // core enforces on the socket itself: the value a client paces its
                    // keep-alives by is the one that actually severs it.
                    "connectionTimeoutMs": listener_caps::IDLE_TIMEOUT_SECS * 1_000,
                },
            })
            .to_string()]
        }
        START => vec![started(context, connection_auth, connection_id, &id, &message)],
        STOP => vec![stopped(context, connection_id, &id)],
        other => {
            ::tracing::debug!(
                api_id = %context.api_id,
                %connection_id,
                %other,
                "a real-time message named a type this endpoint does not serve"
            );
            vec![connection_error_frame(
                BAD_REQUEST_ERROR_TYPE,
                &format!(
                    "This endpoint serves the {CONNECTION_INIT}, {START} and {STOP} message \
                     types."
                ),
            )]
        }
    }
}

/// Answer one `start`: the message has to name an id, carry a subscription document, be
/// authorized by the material it presents, and name a subscription field of the API's
/// schema; the subscription is then recorded under the id the client chose.
fn started(
    context: &PlaneContext,
    connection_auth: &PresentedAuth,
    connection_id: &str,
    subscription_id: &str,
    message: &Json,
) -> String {
    if subscription_id.is_empty() {
        return error_frame(subscription_id, BAD_REQUEST_ERROR_TYPE, MESSAGE_NAMES_AN_ID);
    }
    let Some(field) = subscription_field(message) else {
        return error_frame(
            subscription_id,
            BAD_REQUEST_ERROR_TYPE,
            START_CARRIES_A_SUBSCRIPTION,
        );
    };
    // A start is authorized by the material IT carries, falling back to the material the
    // connection was opened with, because that is the material the client presented for
    // this call.
    let auth = message
        .pointer("/payload/extensions/authorization")
        .map(PresentedAuth::from_object)
        .filter(PresentedAuth::is_present)
        .unwrap_or_else(|| connection_auth.clone());
    let recorded = {
        let mut store = context.state.lock();
        if !super::authorized(&store, &context.api_id, &auth) {
            return error_frame(
                subscription_id,
                event_api::UNAUTHORIZED_ERROR_TYPE,
                event_api::UNAUTHORIZED_MESSAGE,
            );
        }
        // The field has to be one the API's SCHEMA declares as a subscription: a
        // subscription on a field the schema does not declare is a subscription nothing
        // can ever publish to.
        let declared = store
            .graphql_apis
            .get(&context.api_id)
            .map(|api| api.subscription_fields())
            .unwrap_or_default();
        if !declared.contains(&field) {
            return error_frame(
                subscription_id,
                NOT_FOUND_ERROR_TYPE,
                &no_such_subscription_field(&field),
            );
        }
        store.graphql_data_plane.start_subscription(
            &context.api_id,
            connection_id,
            subscription_id,
            &field,
        )
    };
    match recorded {
        Ok(()) => json!({"id": subscription_id, MESSAGE_TYPE_MEMBER: START_ACK}).to_string(),
        Err(refusal) => error_frame(
            subscription_id,
            BAD_REQUEST_ERROR_TYPE,
            &start_refusal_message(refusal),
        ),
    }
}

/// Answer one `stop`: the connection has to hold a subscription under that id, and
/// forgetting it is what stops events reaching this socket.
fn stopped(context: &PlaneContext, connection_id: &str, subscription_id: &str) -> String {
    let stopped = context.state.lock().graphql_data_plane.stop_subscription(
        &context.api_id,
        connection_id,
        subscription_id,
    );
    if stopped.is_none() {
        return error_frame(
            subscription_id,
            NOT_FOUND_ERROR_TYPE,
            NO_SUCH_SUBSCRIPTION,
        );
    }
    json!({"id": subscription_id, MESSAGE_TYPE_MEMBER: COMPLETE}).to_string()
}

/// The subscription FIELD a `start` asks for, or `None` for a message that carries no
/// subscription document at all.
///
/// A `start` carries its GraphQL request in `payload.data`, as the document a client would
/// POST to the API's HTTPS endpoint: a JSON object with a `query`. That query is read by
/// the maintained GraphQL parser this crate already reads a schema with (`apollo-parser`),
/// never by anything hand-written, and what makes it a subscription is the operation
/// GraphQL itself defines — a `subscription` operation, whose selected field is the field
/// subscribed to ("Subscription operations must have exactly one root field").
fn subscription_field(message: &Json) -> Option<String> {
    let data = message.pointer("/payload/data").and_then(Json::as_str)?;
    let request: Json = ::serde_json::from_str(data).ok()?;
    let query = request.get("query").and_then(Json::as_str)?;
    let parsed = ::apollo_parser::Parser::new(query)
        .recursion_limit(crate::state::DEFINITION_RECURSION_LIMIT)
        .token_limit(crate::state::DEFINITION_TOKEN_LIMIT)
        .parse();
    if parsed.errors().next().is_some() {
        return None;
    }
    use ::apollo_parser::cst;
    parsed.document().definitions().find_map(|definition| {
        let cst::Definition::OperationDefinition(operation) = definition else {
            return None;
        };
        operation.operation_type()?.subscription_token()?;
        operation
            .selection_set()?
            .selections()
            .find_map(|selection| match selection {
                cst::Selection::Field(selected) => {
                    Some(selected.name()?.text().to_string())
                }
                _ => None,
            })
    })
}

/// What a start this plane cannot record is stated with.
fn start_refusal_message(refusal: SubscribeRefusal) -> String {
    match refusal {
        SubscribeRefusal::NoConnection => NO_CONNECTION.to_string(),
        SubscribeRefusal::DuplicateId => SUBSCRIPTION_ID_IN_USE.to_string(),
        SubscribeRefusal::TooMany => format!(
            "A connection holds at most {} subscriptions.",
            crate::state::event_data_plane::SUBSCRIPTIONS_PER_CONNECTION
        ),
    }
}

/// How a subscription on a field the API's schema does not declare is stated.
fn no_such_subscription_field(field: &str) -> String {
    format!("This API's schema declares no subscription field named {field}.")
}

/// An `error` frame about ONE subscription: the subscription the client asked for, and why
/// it was not started.
fn error_frame(subscription_id: &str, error_type: &str, message: &str) -> String {
    json!({
        "id": subscription_id,
        MESSAGE_TYPE_MEMBER: ERROR,
        "payload": {"errors": [error_object(error_type, message)]},
    })
    .to_string()
}

/// An error frame about the CONNECTION rather than about one subscription: a message this
/// endpoint cannot read at all, and the refusal of an unauthorized `connection_init`.
fn connection_error_frame(error_type: &str, message: &str) -> String {
    json!({
        MESSAGE_TYPE_MEMBER: CONNECTION_ERROR,
        "payload": {"errors": [error_object(error_type, message)]},
    })
    .to_string()
}

/// One error of a frame this endpoint writes: what went wrong, the STATUS that error
/// carries, and the sentence it is stated in.
///
/// The status is the error's own modelled `httpError` ([`error_status`]): a frame of this
/// protocol carries no HTTP response of its own, so the status is reported in the error
/// object — which is what a real-time client of this API reads it from.
fn error_object(error_type: &str, message: &str) -> Json {
    json!({
        "errorType": error_type,
        "errorCode": error_status(error_type),
        "message": message,
    })
}

/// The HTTP status the error `error_type` carries, as this service's model declares it:
/// `BadRequestException` 400, `UnauthorizedException` 401, `NotFoundException` 404
/// (`smithy_query(query="shape",shape="BadRequestException,UnauthorizedException,\
/// NotFoundException")`). Anything else is a failure of this emulator's own, which is the
/// 500 `InternalFailureException` carries.
fn error_status(error_type: &str) -> u16 {
    match error_type {
        BAD_REQUEST_ERROR_TYPE => 400,
        event_api::UNAUTHORIZED_ERROR_TYPE => 401,
        NOT_FOUND_ERROR_TYPE => 404,
        _ => 500,
    }
}

/// The keep-alive frame an acknowledged connection is written every
/// [`KEEP_ALIVE_INTERVAL`].
fn keep_alive_frame() -> String {
    json!({MESSAGE_TYPE_MEMBER: KEEP_ALIVE}).to_string()
}

/// The member a message names its type in, and the members the contained answer carries.
const MESSAGE_TYPE_MEMBER: &str = "type";
const ACKED_MEMBER: &str = "acked";
const CLOSED_MEMBER: &str = "closed";
const FRAMES_MEMBER: &str = "frames";

/// The media type the contained answer and a refused handshake are written in.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The message types this endpoint reads from a client.
const CONNECTION_INIT: &str = "connection_init";
const START: &str = "start";
const STOP: &str = "stop";

/// The message types it writes back.
const CONNECTION_ACK: &str = "connection_ack";
const CONNECTION_ERROR: &str = "connection_error";
const START_ACK: &str = "start_ack";
const COMPLETE: &str = "complete";
const KEEP_ALIVE: &str = "ka";
const ERROR: &str = "error";

/// The `errorType` values this endpoint's refusals carry, as AppSync's own errors name
/// them.
const BAD_REQUEST_ERROR_TYPE: &str = "BadRequestException";
const NOT_FOUND_ERROR_TYPE: &str = "NotFoundException";

/// The sentences this endpoint's own refusals state.
const NO_SUCH_ROUTE: &str = "This endpoint serves a WebSocket at /graphql.";
const MESSAGE_IS_A_DOCUMENT: &str = "A message must be a JSON document.";
const MESSAGE_NAMES_AN_ID: &str = "A message must name the id it is made under.";
const START_CARRIES_A_SUBSCRIPTION: &str =
    "A start must carry a GraphQL subscription document in payload.data.";
const NO_SUCH_SUBSCRIPTION: &str = "This connection holds no subscription under that id.";
const SUBSCRIPTION_ID_IN_USE: &str = "This connection already holds a subscription under that id.";
const NO_CONNECTION: &str = "This connection is no longer open.";
