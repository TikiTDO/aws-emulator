// SPDX-License-Identifier: CC0-1.0

//! The SOCKET a signaling channel's peers meet on: the RFC 6455 opening handshake this
//! data plane completes, and the SDP/ICE frames it then routes between the channel's
//! master and its viewers.
//!
//! A signaling channel cannot be served through the emulator's front door: that door
//! serves ONE buffered request per call, and the seams a service is given take an
//! already-buffered request and return a buffered response, so nothing there can switch a
//! connection to another protocol and carry frames over it. What the channel's data plane
//! needs is a socket, which the core lends a service for a resource with a native protocol
//! ([`crate::state::wss_endpoint`] opens it and owns its lifetime).
//!
//! # What this module does with a connection, in order
//!
//! 1. ENCRYPTION, negotiated in band: the connection's first byte says whether the client
//!    dialled `wss://` or `ws://`, and the TLS half is the core's own server
//!    ([`::service_common::in_band_tls`]), so this crate never touches a key or a
//!    certificate.
//! 2. The HANDSHAKE, decided from the channel REGISTRY and from what the request names:
//!    the SDKs put the channel in the `X-Amz-ChannelARN` query parameter and a viewer's
//!    own id in `X-Amz-ClientId`, and a connection naming no client id is the channel's
//!    MASTER ([`Role`]). A handshake for a channel this emulator does not hold is refused
//!    404, one naming no readable channel ARN 400, and one a channel cannot take another
//!    of (a second master, a viewer past the per-channel cap) 403. The implementation
//!    derives `Sec-WebSocket-Accept` and writes the `101` or the refusal itself; nothing
//!    about the protocol is written here.
//! 3. The FRAMES, each routed to the peer the channel's own rule names
//!    ([`routed`]): a viewer's frame goes to the master under the viewer's `ClientId`, and
//!    the master's goes to the viewer its `recipientClientId` names — or, when that
//!    recipient holds no socket, to whoever is waiting for that answer, which is how a
//!    `SendAlexaOfferToMaster` caller is answered by the live master.
//! 4. The CLOSE, which detaches the peer: from then on the channel has no master, or one
//!    fewer viewer.
//!
//! Every decision is read from this service's own store under one lock, and a frame is
//! handed to a peer's bounded queue without ever awaiting ([`crate::state::session`]), so
//! one peer that has stopped reading cannot stall another's frame or the store.
//!
//! # What this socket does NOT decide
//!
//! The handshake makes no AUTHORIZATION decision, while the two modelled routes do
//! ([`crate::state::authorization`]). Two reasons, and both are properties of this
//! emulator rather than of the channel: a request's signature is verified by the emulator
//! itself and never by a service, and what arrives here is not a modelled operation of any
//! service — it is a connection to a loopback listener this emulator opened, carrying the
//! query parameters a presigned signaling URL puts there. So the channel the handshake
//! names is resolved from the REGISTRY, and the identity a frame's sender claims is the
//! role its own connection holds, never a value inside the frame.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use ::emulator_server::{ListenerConn, NativeHandler, RemoteOrigins, TlsUpgrader};
use ::futures_util::{SinkExt as _, StreamExt as _};
use ::tokio::io::{AsyncRead, AsyncWrite};
use ::tokio::sync::mpsc::Receiver;
use ::tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use ::tokio_tungstenite::tungstenite::Message;

use crate::state::session::{Attachment, PeerConnection, ICE_CANDIDATE, SDP_ANSWER, SDP_OFFER};
use crate::state::ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState as ServiceStore;

/// The query parameter a handshake names its channel in: what the Kinesis Video Streams
/// WebRTC SDKs put the `ChannelARN` under when they presign the signaling URL.
const CHANNEL_ARN_PARAMETER: &str = "X-Amz-ChannelARN";

/// The query parameter a VIEWER names itself in; a handshake carrying none is the
/// channel's master.
const CLIENT_ID_PARAMETER: &str = "X-Amz-ClientId";

/// The member a peer states the message's type in when it SENDS one.
const ACTION_MEMBER: &str = "action";
/// The member the channel states it in when it DELIVERS one.
const MESSAGE_TYPE_MEMBER: &str = "messageType";
/// Whom the master addresses a message to.
const RECIPIENT_CLIENT_ID_MEMBER: &str = "recipientClientId";
/// Whom a delivered message came from, when the sender has a client id of its own.
const SENDER_CLIENT_ID_MEMBER: &str = "senderClientId";
/// The base64-encoded SDP or ICE content the message carries.
const MESSAGE_PAYLOAD_MEMBER: &str = "messagePayload";
/// The sender's own correlation id, carried through to the recipient when it sent one.
const CORRELATION_ID_MEMBER: &str = "correlationId";

/// The largest MESSAGE this data plane reads from a peer, in bytes.
///
/// Stated here rather than left to the WebSocket implementation's default, so the bound a
/// caller's bytes are buffered under is citable: a signaling message carries one
/// base64-encoded SDP or ICE payload, which the models bound at 10000 characters
/// (`MessagePayload`, len 1..10000), and this leaves room for the JSON frame around it.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;

/// The largest FRAME this data plane reads, in bytes: one message is one frame here, so
/// the frame bound is the message bound.
const MAX_FRAME_BYTES: usize = MAX_MESSAGE_BYTES;

/// How many bytes may wait on the WRITE side of one socket before the implementation stops
/// buffering: the frames one peer's queue already holds
/// ([`crate::state::session::PEER_FRAME_DEPTH`]) times the largest message, so a peer that
/// stops reading cannot pin more than the queue it is already bounded by.
const MAX_WRITE_BUFFER_BYTES: usize =
    crate::state::session::PEER_FRAME_DEPTH * MAX_MESSAGE_BYTES;

/// The explicit size bounds every socket of this data plane is read and written under; see
/// the three constants above.
fn socket_limits() -> ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    ::tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_WRITE_BUFFER_BYTES)
}

/// Which side of a channel a connection is: the `Role` the control plane's
/// `GetSignalingChannelEndpoint` resolves an endpoint for (`ChannelRole`: `MASTER` |
/// `VIEWER`), read here from whether the handshake named a `ClientId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The channel's single master peer.
    Master,
    /// A viewer, under the `ClientId` its handshake named.
    Viewer(String),
}

impl Role {
    /// The client id this role is addressed by, or `None` for the master.
    fn client_id(&self) -> Option<&str> {
        match self {
            Self::Master => None,
            Self::Viewer(client_id) => Some(client_id),
        }
    }
}

/// The handler the core hands each connection accepted on the signaling socket.
///
/// It carries the shared store — so every connection reads the same channel registry the
/// control plane writes — and the core's TLS server, for a client that dialled `wss://`.
pub struct ChannelSocket {
    state: ::engine::ServiceState<ServiceStore>,
    upgrader: Option<TlsUpgrader>,
}

impl ChannelSocket {
    /// The socket handler over `state`, with the core's TLS server for the `wss://` half
    /// of the announced address.
    #[must_use]
    pub fn new(
        state: ::engine::ServiceState<ServiceStore>,
        upgrader: Option<TlsUpgrader>,
    ) -> Self {
        Self { state, upgrader }
    }
}

impl NativeHandler for ChannelSocket {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let state = self.state.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            serve_connection(state, upgrader, conn).await;
        })
    }
}

/// How a handshake this endpoint does not complete is answered: an HTTP status and the
/// sentence the client reads with it.
#[derive(Debug, Clone)]
struct Refusal {
    /// The HTTP status the handshake is refused with.
    status: u16,
    /// The sentence the client reads with it.
    message: &'static str,
}

impl Refusal {
    /// The refusal as the RFC 6455 implementation writes it. A status the `http` crate
    /// cannot build is answered as the internal failure it is.
    fn rendered(&self) -> ErrorResponse {
        ::http::Response::builder()
            .status(self.status)
            .header("content-type", "application/json")
            .body(Some(
                ::serde_json::json!({ "message": self.message }).to_string(),
            ))
            .unwrap_or_else(|_| {
                ::http::Response::builder()
                    .status(500)
                    .body(None)
                    .expect("500 with no body builds")
            })
    }
}

/// What a handshake naming no readable channel ARN is answered with.
const NO_CHANNEL_ARN: &str = "Invalid Channel ARN !";

/// What a handshake for a channel this emulator does not hold is answered with.
const NO_SUCH_CHANNEL: &str = "The signaling channel does not exist.";

/// What a second master, or a viewer past the channel's cap, is answered with.
const CHANNEL_FULL: &str = "The signaling channel cannot take another peer of that role.";

/// What a handshake a browser made from another site is answered with.
const CROSS_SITE: &str = "This endpoint does not serve cross-site requests.";

/// Serve one accepted connection: negotiate encryption, decide the handshake, complete (or
/// refuse) it, attach the peer, then carry frames until either side closes.
// The handshake callback's `Err` is the REFUSAL RESPONSE the WebSocket implementation writes
// (`http::Response<Option<String>>`), which is that dependency's own signature and is large
// by the `http` crate's measure. Boxing it is not available: the callback's type is the one
// `accept_hdr_async_with_config` takes.
#[allow(clippy::result_large_err)]
async fn serve_connection(
    state: ::engine::ServiceState<ServiceStore>,
    upgrader: Option<TlsUpgrader>,
    conn: ListenerConn,
) {
    let Some(conn) = ::service_common::in_band_tls::negotiated(conn, upgrader).await else {
        return;
    };
    // The decision is a read of this service's own store, so it is made inside the
    // handshake callback the implementation calls with the request it parsed: nothing here
    // has to read or replay the handshake's bytes.
    let decided: Arc<Mutex<Option<(String, Role)>>> = Arc::new(Mutex::new(None));
    let deciding = Arc::clone(&decided);
    let deciding_state = state.clone();
    let decide = move |request: &HandshakeRequest,
                       response: HandshakeResponse|
     -> Result<HandshakeResponse, ErrorResponse> {
        match admission(&deciding_state, request) {
            Ok(admitted) => {
                *deciding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(admitted);
                Ok(response)
            }
            Err(refusal) => {
                ::tracing::debug!(
                    status = refusal.status,
                    message = refusal.message,
                    "refusing a signaling channel handshake"
                );
                Err(refusal.rendered())
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
            ::tracing::debug!(%error, "a signaling WebSocket handshake was not completed");
            return;
        }
    };
    let admitted = decided
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let Some((channel_arn, role)) = admitted else {
        return;
    };

    // The peer EXISTS on the channel from here: its queue is the one every frame addressed
    // to it is handed to, and the task below is what writes that queue to this socket. The
    // attachment it is held under is what its detach below is matched against, so a
    // connection that is replaced while it closes cannot evict its replacement.
    let (peer, queued) = PeerConnection::new();
    let attached: Option<Attachment> = {
        let mut store = state.lock();
        match store.resolve_channel_mut(&channel_arn) {
            Ok(channel) => match &role {
                Role::Master => channel.session.attach_master(peer),
                Role::Viewer(client_id) => channel.session.attach_viewer(client_id, peer),
            },
            // The channel was deleted between the handshake and here.
            Err(_) => None,
        }
    };
    let Some(attachment) = attached else {
        ::tracing::debug!(
            %channel_arn,
            ?role,
            "a signaling peer could not be attached to its channel"
        );
        return;
    };
    ::tracing::info!(%channel_arn, ?role, "a signaling channel peer is connected");

    carry_frames(&state, &channel_arn, &role, socket, queued).await;

    let mut store = state.lock();
    if let Ok(channel) = store.resolve_channel_mut(&channel_arn) {
        match &role {
            Role::Master => channel.session.detach_master(attachment),
            Role::Viewer(client_id) => channel.session.detach_viewer(client_id, attachment),
        }
    }
    ::tracing::info!(%channel_arn, ?role, "a signaling channel peer is disconnected");
}

/// Whether this opening handshake is completed, and as what: the channel it named and the
/// role it connects in.
///
/// The browser check the core applies to every request on one of its own HTTP listeners is
/// applied first, because an RFC 6455 handshake is a request a page can make to a loopback
/// port without a CORS preflight.
fn admission(
    state: &::engine::ServiceState<ServiceStore>,
    request: &HandshakeRequest,
) -> Result<(String, Role), Refusal> {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    if let ::emulator_server::BrowserVerdict::Forbid(reason) =
        ::emulator_server::classify_browser_request(
            header("origin"),
            header("sec-fetch-site"),
            // The channel's socket is this emulator's, reached from the machine it runs
            // on: the same default a core HTTP listener carries.
            &RemoteOrigins::SameOriginOnly,
            false,
        )
    {
        ::tracing::info!(%reason, "refusing a signaling handshake a browser made cross-site");
        return Err(Refusal {
            status: 403,
            message: CROSS_SITE,
        });
    }

    let query = request.uri().query().unwrap_or_default();
    let channel_arn = query_parameter(query, CHANNEL_ARN_PARAMETER).ok_or(Refusal {
        status: 400,
        message: NO_CHANNEL_ARN,
    })?;
    let role = match query_parameter(query, CLIENT_ID_PARAMETER) {
        Some(client_id) => Role::Viewer(client_id),
        None => Role::Master,
    };

    let store = state.lock();
    let channel = store.resolve_channel(&channel_arn).map_err(|error| match error {
        crate::state::ChannelLookupError::MalformedArn => Refusal {
            status: 400,
            message: NO_CHANNEL_ARN,
        },
        crate::state::ChannelLookupError::NotFound { .. } => Refusal {
            status: 404,
            message: NO_SUCH_CHANNEL,
        },
    })?;
    // Whether the channel can take this peer at all is decided here, so a client is
    // refused by the handshake rather than admitted and then dropped.
    let taken = match &role {
        Role::Master => channel.session.has_master(),
        Role::Viewer(client_id) => {
            channel.session.viewer(client_id).is_none()
                && channel.session.viewer_ids().len()
                    >= crate::state::session::MAX_VIEWERS_PER_CHANNEL
        }
    };
    if taken {
        return Err(Refusal {
            status: 403,
            message: CHANNEL_FULL,
        });
    }
    Ok((channel_arn, role))
}

/// The value of the query parameter `name`, percent-decoded by the framework's own query
/// reader (`aws_smithy_schema_server::query::parse_form_urlencoded`, the decoder its query
/// protocols use), or `None` when the query carries no such parameter or an empty one.
///
/// The name is matched case-insensitively, as the signing SDKs spell these parameters in
/// mixed case.
fn query_parameter(query: &str, name: &str) -> Option<String> {
    ::aws_smithy_schema_server::query::parse_form_urlencoded(query.as_bytes())
        .into_iter()
        .find(|(parameter, value)| parameter.eq_ignore_ascii_case(name) && !value.is_empty())
        .map(|(_, value)| value)
}

/// Carry one established connection: write every frame queued for this peer to its socket,
/// route every frame it sends, and end when either side closes.
async fn carry_frames<S>(
    state: &::engine::ServiceState<ServiceStore>,
    channel_arn: &str,
    role: &Role,
    socket: ::tokio_tungstenite::WebSocketStream<S>,
    mut queued: Receiver<String>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut outgoing, mut incoming) = socket.split();
    loop {
        ::tokio::select! {
            queued_frame = queued.recv() => {
                let Some(frame) = queued_frame else { break };
                if outgoing.send(Message::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            sent = incoming.next() => {
                let Some(Ok(frame)) = sent else { break };
                let body = match frame {
                    Message::Text(text) => text.as_str().to_string(),
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => text,
                        // A signaling frame is JSON; bytes that are not text name no
                        // message type and are routed nowhere.
                        Err(_) => continue,
                    },
                    Message::Close(_) => break,
                    // Ping, Pong and Frame are the protocol's own: the implementation
                    // answers a ping itself and no signaling message is carried in one.
                    _ => continue,
                };
                routed(state, channel_arn, role, &body);
            }
        }
    }
}

/// Route one frame the peer `role` sent on `channel_arn`, by the channel's own rule.
///
/// A viewer's frame goes to the channel's MASTER, under that viewer's `ClientId` as
/// `senderClientId` — which is how the master learns who to answer. The master's frame
/// goes to the viewer its `recipientClientId` names; when that recipient holds no socket,
/// the frame's payload is handed to whoever is waiting for an answer addressed to it,
/// which is how the `SendAlexaOfferToMaster` caller gets the master's answer.
///
/// One lock, no awaits: a frame is handed to the recipient's bounded queue and that peer's
/// own task writes it.
fn routed(
    state: &::engine::ServiceState<ServiceStore>,
    channel_arn: &str,
    role: &Role,
    body: &str,
) {
    let Ok(frame) = ::serde_json::from_str::<::serde_json::Value>(body) else {
        ::tracing::debug!(%channel_arn, "a signaling frame that is not JSON was routed nowhere");
        return;
    };
    let member = |name: &str| {
        frame
            .get(name)
            .and_then(::serde_json::Value::as_str)
            .map(str::to_string)
    };
    // A peer states the message's type in `action`; `messageType` is accepted too, which
    // is the member the channel DELIVERS it under, so a client that echoes the delivered
    // form is understood.
    let Some(message_type) = member(ACTION_MEMBER).or_else(|| member(MESSAGE_TYPE_MEMBER)) else {
        ::tracing::debug!(%channel_arn, "a signaling frame naming no action was routed nowhere");
        return;
    };
    if ![SDP_OFFER, SDP_ANSWER, ICE_CANDIDATE].contains(&message_type.as_str()) {
        ::tracing::debug!(
            %channel_arn,
            %message_type,
            "a signaling frame naming an unknown action was routed nowhere"
        );
        return;
    }
    let payload = member(MESSAGE_PAYLOAD_MEMBER).unwrap_or_default();
    let correlation_id = member(CORRELATION_ID_MEMBER);
    let recipient = member(RECIPIENT_CLIENT_ID_MEMBER);

    let mut store = state.lock();
    let Ok(channel) = store.resolve_channel_mut(channel_arn) else {
        return;
    };
    let delivered = match role {
        // From a viewer: to the master, carrying the viewer's own id.
        Role::Viewer(client_id) => {
            let frame = delivered_frame(
                &message_type,
                Some(client_id),
                &payload,
                correlation_id.as_deref(),
            );
            channel
                .session
                .master()
                .is_some_and(|master| master.deliver(frame))
        }
        // From the master: to the viewer it names, and to whoever is waiting for an answer
        // addressed to that recipient. BOTH, never one or the other: a viewer and a
        // `SendAlexaOfferToMaster` caller may carry the same client id, and the master's
        // answer is addressed to that id rather than to one of them.
        Role::Master => {
            let frame = delivered_frame(&message_type, None, &payload, correlation_id.as_deref());
            match recipient.as_deref() {
                Some(recipient) => {
                    let to_viewer = channel
                        .session
                        .viewer(recipient)
                        .is_some_and(|viewer| viewer.deliver(frame));
                    let to_waiter = message_type == SDP_ANSWER
                        && channel.session.deliver_answer(recipient, &payload);
                    to_viewer || to_waiter
                }
                None => false,
            }
        }
    };
    if !delivered {
        ::tracing::debug!(
            %channel_arn,
            %message_type,
            sender = ?role.client_id(),
            "a signaling frame reached no peer of its channel"
        );
    }
}

/// One frame as the channel DELIVERS it: the message's type, who sent it when the sender
/// has an id of its own, the payload as sent, and the correlation id the sender chose when
/// it chose one.
///
/// `pub(crate)` because the Alexa route delivers an offer into the same session over the
/// same frame form: there is one rendering of a delivered frame, not one per sender.
pub(crate) fn delivered_frame(
    message_type: &str,
    sender_client_id: Option<&str>,
    payload: &str,
    correlation_id: Option<&str>,
) -> String {
    let mut frame = ::serde_json::Map::new();
    frame.insert(
        MESSAGE_TYPE_MEMBER.to_string(),
        ::serde_json::Value::String(message_type.to_string()),
    );
    if let Some(sender_client_id) = sender_client_id {
        frame.insert(
            SENDER_CLIENT_ID_MEMBER.to_string(),
            ::serde_json::Value::String(sender_client_id.to_string()),
        );
    }
    frame.insert(
        MESSAGE_PAYLOAD_MEMBER.to_string(),
        ::serde_json::Value::String(payload.to_string()),
    );
    if let Some(correlation_id) = correlation_id {
        frame.insert(
            CORRELATION_ID_MEMBER.to_string(),
            ::serde_json::Value::String(correlation_id.to_string()),
        );
    }
    ::serde_json::Value::Object(frame).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handshake_query_names_the_channel_and_the_role() {
        // The rows span how a presigned signaling URL carries these: the ARN
        // percent-encoded as a signer writes it, a viewer's id beside it, the parameters in
        // either order, a mixed-case spelling, and a query naming neither.
        const ARN: &str = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo/1700000000";
        let encoded = "arn%3Aaws%3Akinesisvideo%3Aus-east-1%3A123456789012%3Achannel%2Fdemo%2F1700000000";
        let cases: Vec<(&str, String, Option<&str>, Option<&str>)> = vec![
            (
                "the master's presigned URL",
                format!("X-Amz-ChannelARN={encoded}&X-Amz-Algorithm=AWS4-HMAC-SHA256"),
                Some(ARN),
                None,
            ),
            (
                "a viewer's presigned URL",
                format!("X-Amz-ChannelARN={encoded}&X-Amz-ClientId=viewer-1"),
                Some(ARN),
                Some("viewer-1"),
            ),
            (
                "the client id before the channel",
                format!("X-Amz-ClientId=viewer.2&X-Amz-ChannelARN={encoded}"),
                Some(ARN),
                Some("viewer.2"),
            ),
            (
                "a lowercase spelling",
                format!("x-amz-channelarn={encoded}&x-amz-clientid=viewer-3"),
                Some(ARN),
                Some("viewer-3"),
            ),
            (
                "a query naming neither",
                "X-Amz-Algorithm=AWS4-HMAC-SHA256".to_string(),
                None,
                None,
            ),
            (
                "an empty channel parameter",
                "X-Amz-ChannelARN=".to_string(),
                None,
                None,
            ),
        ];
        for (case, query, expected_arn, expected_client) in cases {
            let arn = query_parameter(&query, CHANNEL_ARN_PARAMETER);
            assert_eq!(
                arn.as_deref(),
                expected_arn,
                "case {case:?}: expected {expected_arn:?}, got {arn:?}"
            );
            let client_id = query_parameter(&query, CLIENT_ID_PARAMETER);
            assert_eq!(
                client_id.as_deref(),
                expected_client,
                "case {case:?}: expected {expected_client:?}, got {client_id:?}"
            );
        }
    }

    #[test]
    fn a_delivered_frame_states_the_type_the_sender_and_the_payload() {
        // The rows span the two directions and whether the sender chose a correlation id.
        let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            ("a viewer's offer", SDP_OFFER, Some("viewer-1"), Some("corr-1")),
            ("the master's answer", SDP_ANSWER, None, None),
            ("a candidate from a viewer", ICE_CANDIDATE, Some("viewer-2"), None),
        ];
        for (case, message_type, sender, correlation_id) in cases {
            let rendered = delivered_frame(message_type, *sender, "cGF5bG9hZA==", *correlation_id);
            let parsed: ::serde_json::Value = ::serde_json::from_str(&rendered)
                .unwrap_or_else(|error| panic!("case {case:?}: the frame must be JSON: {error}"));
            assert_eq!(
                parsed.get(MESSAGE_TYPE_MEMBER).and_then(::serde_json::Value::as_str),
                Some(*message_type),
                "case {case:?}: expected the message type, got {rendered}"
            );
            assert_eq!(
                parsed
                    .get(SENDER_CLIENT_ID_MEMBER)
                    .and_then(::serde_json::Value::as_str),
                *sender,
                "case {case:?}: expected sender {sender:?}, got {rendered}"
            );
            assert_eq!(
                parsed
                    .get(MESSAGE_PAYLOAD_MEMBER)
                    .and_then(::serde_json::Value::as_str),
                Some("cGF5bG9hZA=="),
                "case {case:?}: expected the payload as sent, got {rendered}"
            );
            assert_eq!(
                parsed
                    .get(CORRELATION_ID_MEMBER)
                    .and_then(::serde_json::Value::as_str),
                *correlation_id,
                "case {case:?}: expected correlation {correlation_id:?}, got {rendered}"
            );
        }
    }
}
