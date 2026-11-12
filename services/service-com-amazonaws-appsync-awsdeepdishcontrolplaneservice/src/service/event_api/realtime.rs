// SPDX-License-Identifier: CC0-1.0

//! The Event API's REAL-TIME endpoint — `Api$dns`'s `REALTIME` record — and the WebSocket
//! conversation it carries.
//!
//! What it serves, over RFC 6455 on the API's own socket:
//!
//! > An opening handshake on [`REALTIME_PATH`] offering the [`SUBPROTOCOL`] subprotocol is
//! > completed when the material the handshake carries authorizes a connection to the API,
//! > and refused otherwise. Over the established socket:
//! > `connection_init` is answered `connection_ack`; `subscribe` on a channel of one of the
//! > API's channel namespaces runs that namespace's `onSubscribe` handler and is answered
//! > `subscribe_success` (or `subscribe_error`); `unsubscribe` is answered
//! > `unsubscribe_success`; a `publish` takes the same publish path the HTTP endpoint takes
//! > ([`super::publish`]) and is answered `publish_success`; and every event published to a
//! > channel a subscription of this connection covers arrives on it as a `data` frame.
//!
//! The handshake is read and written by the maintained RFC 6455 implementation
//! (`tokio-tungstenite`): nothing about the protocol itself is hand-written here. What this
//! module decides is which handshakes are completed and what each frame is answered with,
//! both from this service's own records.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::{Arc, Mutex};

use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::futures_util::{SinkExt, StreamExt};
use ::serde_json::{json, Value as Json};
use ::tokio::io::{AsyncRead, AsyncWrite};
use ::tokio::sync::mpsc::{channel, Receiver, Sender};
use ::tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use ::tokio_tungstenite::tungstenite::Message;

use super::{handlers, PlaneContext, PresentedAuth};
use crate::state::event_data_plane as plane;

/// The path the real-time endpoint serves its WebSocket at.
pub const REALTIME_PATH: &str = "/event/realtime";

/// The subprotocol an Event API's real-time client offers and this endpoint selects.
pub const SUBPROTOCOL: &str = "aws-appsync-event-ws";

/// The largest MESSAGE this endpoint reads from a client, in bytes, and the largest FRAME
/// one may arrive in.
///
/// The message bound is the plane's own ([`super::MAX_MESSAGE_BYTES`]), shared with the
/// publish route so an event's bytes are bounded the same way whichever endpoint it
/// arrived on. Stated rather than left to the WebSocket implementation's own default, so
/// the bound a client's bytes are buffered under is citable and survives a dependency
/// bump: it is what stands between one local client and the host's memory.
const MAX_MESSAGE_BYTES: usize = super::MAX_MESSAGE_BYTES;
const MAX_FRAME_BYTES: usize = 32 * 1024;

/// Serves one Event API's real-time endpoint: the socket the API's `REALTIME` DNS record
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
    let Some(conn) = super::negotiated(conn, upgrader).await else {
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
                Err(refusal) => Err(refusal.rendered()),
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
        .max_write_buffer_size(plane::CONNECTION_FRAME_DEPTH * MAX_MESSAGE_BYTES)
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
        let body = super::publish::errors_document(self.error_type, &self.message).to_string();
        let mut response = ErrorResponse::new(Some(body));
        *response.status_mut() = ::http::StatusCode::from_u16(self.status)
            .unwrap_or(::http::StatusCode::INTERNAL_SERVER_ERROR);
        response.headers_mut().insert(
            ::http::header::CONTENT_TYPE,
            ::http::HeaderValue::from_static("application/json"),
        );
        response
    }
}

/// Whether one opening handshake is completed, and with what authorization material.
///
/// In order, so a handshake is judged by the transport before anything of the API is read:
///
/// * the `Host` the handshake addressed has to be one these sockets serve
///   ([`super::host_allowed`]) — the DNS-rebinding guard that makes the loopback bind mean
///   something to a browser, which the core runs for its own HTTP listeners and a plane on
///   a `tcp_plaintext` socket owns itself. A handshake naming no host at all is refused
///   rather than served: this fails CLOSED, as the core's own listener does;
/// * a handshake a BROWSER made from ANOTHER SITE is refused ([`super::cross_site_browser`]).
///   An RFC 6455 handshake is one of the few requests a page can make to a port on the
///   developer's machine with no CORS preflight to stop it, and a page CAN set the
///   subprotocol field this protocol carries its authorization object in, so the refusal
///   has to be here;
/// * the path has to be the one this endpoint serves, and the client has to offer this
///   endpoint's subprotocol;
/// * the material its auth token carries has to authorize a connection to this API
///   ([`super::authorized`]).
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
            message: super::HOST_REQUIRED.to_string(),
        });
    };
    if !super::host_allowed(host) {
        return Err(Refusal {
            status: 421,
            error_type: BAD_REQUEST_ERROR_TYPE,
            message: super::HOST_NOT_ALLOWED.to_string(),
        });
    }
    if let Some(reason) = super::cross_site_browser(field("origin"), field("sec-fetch-site")) {
        ::tracing::info!(
            api_id = %context.api_id,
            %reason,
            "refusing a real-time handshake a browser made cross-site"
        );
        return Err(Refusal {
            status: 403,
            error_type: super::UNAUTHORIZED_ERROR_TYPE,
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
    let auth = PresentedAuth::from_subprotocols(offered);
    let store = context.state.lock();
    let authorized = super::authorized(
        &store,
        &context.api_id,
        super::PlaneCall::Connect,
        None,
        &auth,
    );
    drop(store);
    if !authorized {
        return Err(Refusal {
            status: 401,
            error_type: super::UNAUTHORIZED_ERROR_TYPE,
            message: super::UNAUTHORIZED_MESSAGE.to_string(),
        });
    }
    Ok(auth)
}

/// Carry one established connection: register it, answer every frame the client sends, and
/// write every event delivered to it, until either side closes.
async fn carry_frames<S>(
    context: PlaneContext,
    auth: PresentedAuth,
    socket: ::tokio_tungstenite::WebSocketStream<S>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (frames, mut delivered): (Sender<String>, Receiver<String>) =
        channel(plane::CONNECTION_FRAME_DEPTH);
    let connection_id = {
        let mut store = context.state.lock();
        let connection_id = store.event_data_plane.mint_connection_id();
        if !store
            .event_data_plane
            .open_connection(&context.api_id, connection_id.clone(), frames)
        {
            return;
        }
        connection_id
    };
    ::tracing::info!(
        api_id = %context.api_id,
        %connection_id,
        "a real-time connection is open"
    );
    // The API now holds a connection, so its two gauges have a quantity to report: this
    // is the write that owes the cadence they are observed on, for as long as the API is
    // connected to ([`super::gauges`]). Claimed once however many connections are opened.
    super::gauges::drive_gauge_sampling(&context);
    let (mut outgoing, mut incoming) = socket.split();
    // A connection is CONNECTED once the client's `connection_init` has been acked, which
    // is what `ConnectSuccess` counts and what a `DisconnectSuccess` is then owed for.
    let mut acked = false;
    loop {
        ::tokio::select! {
            event = delivered.recv() => {
                let Some(frame) = event else { break };
                if outgoing.send(Message::Text(frame.into())).await.is_err() {
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
                let (acked_now, answers) = contained_message(
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
                // whole connection ends rather than waiting for the next read to fail.
                if !written {
                    break;
                }
            }
        }
    }
    // The connection has ended: its record goes, and with it every subscription it held,
    // which is what makes the subscription count fall back.
    let subscriptions = context
        .state
        .lock()
        .event_data_plane
        .close_connection(&context.api_id, &connection_id);
    if acked {
        context
            .metrics
            .disconnect_success(context.connection_counts());
    }
    ::tracing::info!(
        api_id = %context.api_id,
        %connection_id,
        %subscriptions,
        "a real-time connection is closed"
    );
}

/// Answer one message under the core's own PANIC CONTAINMENT
/// (`emulator_server::contain_panics`), and answer a contained panic with one error frame.
///
/// A panic in a handler costs the caller its answer; a panic that unwinds the task carrying
/// the CONNECTION costs it the connection and every subscription on it, and leaves the
/// client reading a dead socket. The front door layers that containment over its router; a
/// plane served on a core-owned socket has no such layer, so each message's answer is
/// driven through the same function the core exports for exactly this case. The panic is
/// logged there with the message's facts and is not swallowed: it stays a visible emulator
/// defect.
///
/// What the contained future carries back is this endpoint's own answer — the frames to
/// write and whether the connection is now acked — as a document, because the containment
/// is stated in terms of an HTTP response. A contained panic arrives instead as the core's
/// `500`, which is written back as the service's own `InternalFailureException` frame, with
/// the connection and its subscriptions untouched.
async fn contained_message(
    context: PlaneContext,
    auth: PresentedAuth,
    connection_id: String,
    body: String,
    acked: bool,
) -> (bool, Vec<String>) {
    let facts = message_facts(&body);
    let guarded = ::emulator_server::contain_panics(facts, async move {
        let mut acked_now = acked;
        let frames =
            served_message(&context, &auth, &connection_id, &body, &mut acked_now).await;
        let document = json!({ACKED_MEMBER: acked_now, FRAMES_MEMBER: frames});
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

/// What the contained answer says: the frames to write and whether the connection is now
/// acked.
///
/// A `500` is a contained panic — the only status this module's own answer carries is the
/// `200` of the document it built — so it becomes one error frame and leaves the ack as it
/// was. A document this module cannot read back is impossible (it wrote it) and is treated
/// the same way, which fails CLOSED rather than inventing an answer.
fn read_contained(
    guarded: &::http::Response<::bytes::Bytes>,
    acked: bool,
) -> (bool, Vec<String>) {
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
            return (acked_now, frames);
        }
    }
    (acked, vec![internal_failure_frame()])
}

/// The frame a CONTAINED PANIC is answered with: the service's own server error, whose
/// documentation words it "An internal AppSync error occurred. Try your request again."
fn internal_failure_frame() -> String {
    json!({
        "type": ERROR,
        "errors": [{
            "errorType": super::publish::INTERNAL_FAILURE_ERROR_TYPE,
            "message": super::publish::INTERNAL_FAILURE_MESSAGE,
        }],
    })
    .to_string()
}

/// The members the contained answer carries, and the member a message names its type in.
const ACKED_MEMBER: &str = "acked";
const FRAMES_MEMBER: &str = "frames";
const MESSAGE_TYPE_MEMBER: &str = "type";

/// The media type the contained answer is written in, named in the facts a panic is logged
/// with.
const JSON_MEDIA_TYPE: &str = "application/json";

/// What one message is answered with: the frames to write back, in order.
async fn served_message(
    context: &PlaneContext,
    connection_auth: &PresentedAuth,
    connection_id: &str,
    body: &str,
    acked: &mut bool,
) -> Vec<String> {
    let Ok(message) = ::serde_json::from_str::<Json>(body) else {
        return vec![protocol_error(MESSAGE_IS_A_DOCUMENT)];
    };
    let kind = message.get("type").and_then(Json::as_str).unwrap_or_default();
    let id = message
        .get("id")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    match kind {
        CONNECTION_INIT => {
            // Idempotent: a second `connection_init` on one connection is acked again and
            // counts no second connection.
            if !*acked {
                *acked = true;
                context.metrics.connect_success(context.connection_counts());
            }
            vec![json!({
                "type": CONNECTION_ACK,
                // How long this connection may stay idle, which is the timeout the core
                // enforces on the socket itself: the value a client paces its keep-alives
                // by is the one that actually severs it.
                "connectionTimeoutMs": listener_caps::IDLE_TIMEOUT_SECS * 1_000,
            })
            .to_string()]
        }
        SUBSCRIBE => vec![subscribed(context, connection_auth, connection_id, &id, &message).await],
        UNSUBSCRIBE => vec![unsubscribed(context, connection_id, &id)],
        PUBLISH => vec![published(context, connection_auth, &id, &message).await],
        other => {
            ::tracing::debug!(
                api_id = %context.api_id,
                %connection_id,
                %other,
                "a real-time message named a type this endpoint does not serve"
            );
            vec![protocol_error(format!(
                "This endpoint serves the {CONNECTION_INIT}, {SUBSCRIBE}, {UNSUBSCRIBE} and \
                 {PUBLISH} message types."
            ))]
        }
    }
}

/// Answer one `subscribe`: the namespace has to exist, the material has to authorize a
/// subscribe, the namespace's `onSubscribe` handler has to return, and the subscription is
/// then recorded under the id the client chose.
async fn subscribed(
    context: &PlaneContext,
    connection_auth: &PresentedAuth,
    connection_id: &str,
    subscription_id: &str,
    message: &Json,
) -> String {
    if subscription_id.is_empty() {
        return subscribe_error(subscription_id, BAD_REQUEST_ERROR_TYPE, MESSAGE_NAMES_AN_ID);
    }
    let Some(channel) = message.get("channel").and_then(Json::as_str) else {
        return subscribe_error(
            subscription_id,
            BAD_REQUEST_ERROR_TYPE,
            SUBSCRIBE_NAMES_A_CHANNEL,
        );
    };
    let path = plane::normalised_channel(channel);
    let Some(namespace_name) = plane::channel_namespace_name(channel).map(str::to_string) else {
        return subscribe_error(
            subscription_id,
            BAD_REQUEST_ERROR_TYPE,
            SUBSCRIBE_NAMES_A_CHANNEL,
        );
    };
    // A subscribe is authorized by the material IT carries, falling back to the material
    // the connection was opened with, because that is the material the client presented
    // for this call.
    let auth = message
        .get("authorization")
        .map(PresentedAuth::from_object)
        .filter(PresentedAuth::is_present)
        .unwrap_or_else(|| connection_auth.clone());
    let code_handlers = {
        let store = context.state.lock();
        if !super::authorized(
            &store,
            &context.api_id,
            super::PlaneCall::Subscribe,
            Some(&namespace_name),
            &auth,
        ) {
            return subscribe_error(
                subscription_id,
                super::UNAUTHORIZED_ERROR_TYPE,
                super::UNAUTHORIZED_MESSAGE,
            );
        }
        let Some(namespace) = store
            .event_apis
            .get(&context.api_id)
            .and_then(|api| api.channel_namespaces.get(&namespace_name))
        else {
            return subscribe_error(
                subscription_id,
                NOT_FOUND_ERROR_TYPE,
                crate::state::CHANNEL_NAMESPACE_NOT_FOUND_MESSAGE,
            );
        };
        namespace.code_handlers.clone()
    };
    if let Some(code) = code_handlers {
        let document = subscribe_context(&path, &namespace_name);
        if let handlers::HandlerRun::Failed(reason) =
            handlers::evaluate(code, handlers::ON_SUBSCRIBE, document).await
        {
            ::tracing::info!(
                api_id = %context.api_id,
                namespace = %namespace_name,
                %reason,
                "a channel namespace's onSubscribe handler refused a subscribe"
            );
            return subscribe_error(subscription_id, super::UNAUTHORIZED_ERROR_TYPE, &reason);
        }
    }
    let recorded = context.state.lock().event_data_plane.subscribe(
        &context.api_id,
        connection_id,
        subscription_id,
        &path,
    );
    match recorded {
        Ok(()) => {
            context
                .metrics
                .subscribe_success(&namespace_name, context.connection_counts());
            json!({"type": SUBSCRIBE_SUCCESS, "id": subscription_id}).to_string()
        }
        Err(refusal) => subscribe_error(
            subscription_id,
            BAD_REQUEST_ERROR_TYPE,
            &subscribe_refusal_message(refusal),
        ),
    }
}

/// Answer one `unsubscribe`: the connection has to hold a subscription under that id, and
/// forgetting it is what stops events reaching this socket.
fn unsubscribed(context: &PlaneContext, connection_id: &str, subscription_id: &str) -> String {
    let channel = context.state.lock().event_data_plane.unsubscribe(
        &context.api_id,
        connection_id,
        subscription_id,
    );
    let Some(channel) = channel else {
        return json!({
            "type": UNSUBSCRIBE_ERROR,
            "id": subscription_id,
            "errors": [{"errorType": NOT_FOUND_ERROR_TYPE, "message": NO_SUCH_SUBSCRIPTION}],
        })
        .to_string();
    };
    if let Some(namespace) = plane::channel_namespace_name(&channel) {
        context
            .metrics
            .unsubscribe_success(namespace, context.connection_counts());
    }
    json!({"type": UNSUBSCRIBE_SUCCESS, "id": subscription_id}).to_string()
}

/// Answer one `publish` made over the socket: the same publish path the HTTP endpoint
/// takes, so one event is published one way however a client sent it.
async fn published(
    context: &PlaneContext,
    connection_auth: &PresentedAuth,
    publish_id: &str,
    message: &Json,
) -> String {
    let Some(channel) = message.get("channel").and_then(Json::as_str) else {
        return publish_error(publish_id, BAD_REQUEST_ERROR_TYPE, PUBLISH_NAMES_A_CHANNEL);
    };
    let events = message
        .get("events")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let auth = message
        .get("authorization")
        .map(PresentedAuth::from_object)
        .filter(PresentedAuth::is_present)
        .unwrap_or_else(|| connection_auth.clone());
    match super::publish::publish(context, &auth, channel, &events).await {
        Ok(outcome) => json!({
            "type": PUBLISH_SUCCESS,
            "id": publish_id,
            "successful": outcome.successful,
            "failed": outcome.failed,
        })
        .to_string(),
        Err(refusal) => {
            publish_error(publish_id, refusal.error_type(), &refusal.message())
        }
    }
}

/// The `ctx` a namespace's `onSubscribe` handler is called with: the channel the subscribe
/// addressed, its segments, the namespace it belongs to and the operation being handled.
fn subscribe_context(path: &str, namespace: &str) -> Json {
    json!({
        "info": {
            "channel": {"path": path, "segments": plane::channel_segments(path)},
            "channelNamespace": {"name": namespace},
            "operation": "SUBSCRIBE",
        },
        "stash": {},
    })
}

/// What a subscribe this plane cannot record is stated with.
fn subscribe_refusal_message(refusal: plane::SubscribeRefusal) -> String {
    match refusal {
        plane::SubscribeRefusal::NoConnection => NO_CONNECTION.to_string(),
        plane::SubscribeRefusal::DuplicateId => SUBSCRIPTION_ID_IN_USE.to_string(),
        plane::SubscribeRefusal::TooMany => format!(
            "A connection holds at most {} subscriptions.",
            plane::SUBSCRIPTIONS_PER_CONNECTION
        ),
    }
}

/// A `subscribe_error` frame: the subscription the client asked for, and why it was not
/// made.
fn subscribe_error(subscription_id: &str, error_type: &str, message: &str) -> String {
    json!({
        "type": SUBSCRIBE_ERROR,
        "id": subscription_id,
        "errors": [{"errorType": error_type, "message": message}],
    })
    .to_string()
}

/// A `publish_error` frame: the publish the client asked for, and why it was not served.
fn publish_error(publish_id: &str, error_type: &str, message: &str) -> String {
    json!({
        "type": PUBLISH_ERROR,
        "id": publish_id,
        "errors": [{"errorType": error_type, "message": message}],
    })
    .to_string()
}

/// An error frame about the CONVERSATION rather than about one subscription or publish: a
/// message this endpoint cannot read at all.
fn protocol_error(message: impl Into<String>) -> String {
    json!({
        "type": ERROR,
        "errors": [{"errorType": BAD_REQUEST_ERROR_TYPE, "message": message.into()}],
    })
    .to_string()
}

/// The message types this endpoint reads from a client.
const CONNECTION_INIT: &str = "connection_init";
const SUBSCRIBE: &str = "subscribe";
const UNSUBSCRIBE: &str = "unsubscribe";
const PUBLISH: &str = "publish";

/// The message types it writes back.
const CONNECTION_ACK: &str = "connection_ack";
const SUBSCRIBE_SUCCESS: &str = "subscribe_success";
const SUBSCRIBE_ERROR: &str = "subscribe_error";
const UNSUBSCRIBE_SUCCESS: &str = "unsubscribe_success";
const UNSUBSCRIBE_ERROR: &str = "unsubscribe_error";
const PUBLISH_SUCCESS: &str = "publish_success";
const PUBLISH_ERROR: &str = "publish_error";
const ERROR: &str = "error";

/// The `errorType` values this endpoint's refusals carry, as AppSync's own errors name
/// them.
const BAD_REQUEST_ERROR_TYPE: &str = "BadRequestException";
const NOT_FOUND_ERROR_TYPE: &str = "NotFoundException";

/// The sentences this endpoint's own refusals state.
const NO_SUCH_ROUTE: &str = "This endpoint serves a WebSocket at /event/realtime.";
const MESSAGE_IS_A_DOCUMENT: &str = "A message must be a JSON document.";
const MESSAGE_NAMES_AN_ID: &str = "A message must name the id it is made under.";
const SUBSCRIBE_NAMES_A_CHANNEL: &str =
    "A subscribe must name a channel of a channel namespace of this API.";
const PUBLISH_NAMES_A_CHANNEL: &str =
    "A publish must name a channel of a channel namespace of this API.";
const NO_SUCH_SUBSCRIPTION: &str = "This connection holds no subscription under that id.";
const SUBSCRIPTION_ID_IN_USE: &str = "This connection already holds a subscription under that id.";
const NO_CONNECTION: &str = "This connection is no longer open.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contained_answer_is_read_back_and_a_contained_panic_is_one_error_frame() {
        // The rows span what the reading generalises over: an answer this endpoint built
        // (with and without a change to the ack), the core's `500` for a contained panic,
        // and a body that is not the document this module writes.
        let healthy = ::http::Response::new(::bytes::Bytes::from(
            json!({ACKED_MEMBER: true, FRAMES_MEMBER: ["{\"type\":\"connection_ack\"}"]})
                .to_string(),
        ));
        assert_eq!(
            read_contained(&healthy, false),
            (true, vec!["{\"type\":\"connection_ack\"}".to_string()]),
            "case \"an acked answer\": expected the frames and the new ack, got {:?}",
            read_contained(&healthy, false)
        );

        let unchanged = ::http::Response::new(::bytes::Bytes::from(
            json!({ACKED_MEMBER: false, FRAMES_MEMBER: []}).to_string(),
        ));
        assert_eq!(
            read_contained(&unchanged, false),
            (false, Vec::new()),
            "case \"an answer with no frames\": expected nothing written and the ack kept"
        );

        for (case, status, body) in [
            (
                "a contained panic",
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                ::serde_json::json!({"message": "InternalFailure"}).to_string(),
            ),
            (
                "a body this module did not write",
                ::http::StatusCode::OK,
                "not a document".to_string(),
            ),
        ] {
            let mut guarded = ::http::Response::new(::bytes::Bytes::from(body));
            *guarded.status_mut() = status;
            let (acked, frames) = read_contained(&guarded, true);
            assert!(acked, "case {case:?}: a failure must leave the ack as it was");
            assert_eq!(
                frames.len(),
                1,
                "case {case:?}: expected exactly one error frame, got {frames:?}"
            );
            let frame: Json = ::serde_json::from_str(&frames[0])
                .unwrap_or_else(|error| panic!("case {case:?}: the frame must be a document: {error}"));
            assert_eq!(
                (
                    frame.get("type").and_then(Json::as_str),
                    frame.pointer("/errors/0/errorType").and_then(Json::as_str),
                ),
                (Some(ERROR), Some(super::super::publish::INTERNAL_FAILURE_ERROR_TYPE)),
                "case {case:?}: expected the service's own internal-failure frame, got {frame}"
            );
        }
    }

    #[test]
    fn the_facts_a_message_is_contained_under_name_its_type_and_nothing_of_its_payload() {
        // The rows span what the reading generalises over: each message type this endpoint
        // serves, a type it does not, and a body that is not a document at all.
        let cases: &[(&str, Option<&str>)] = &[
            (r#"{"type":"connection_init"}"#, Some(CONNECTION_INIT)),
            (
                r#"{"type":"subscribe","id":"s","channel":"/ns/room","secret":"nope"}"#,
                Some(SUBSCRIBE),
            ),
            (r#"{"type":"unsubscribe","id":"s"}"#, Some(UNSUBSCRIBE)),
            (r#"{"type":"publish","events":["{}"]}"#, Some(PUBLISH)),
            (r#"{"type":"whatever"}"#, Some("whatever")),
            ("not a document", None),
        ];
        for (body, kind) in cases {
            let facts = message_facts(body);
            assert_eq!(
                (facts.path.as_str(), facts.target.as_deref()),
                (REALTIME_PATH, *kind),
                "case {body:?}: expected the path and the message type, got {facts:?}"
            );
            assert!(
                !::serde_json::to_string(&facts.target).unwrap_or_default().contains("nope"),
                "case {body:?}: nothing of the message's payload may travel into the facts"
            );
        }
    }
}
