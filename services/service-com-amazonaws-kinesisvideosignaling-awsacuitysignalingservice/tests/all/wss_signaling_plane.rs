// SPDX-License-Identifier: CC0-1.0

//! The signaling channel's own WebSocket data plane, driven over a real connection: the
//! RFC 6455 opening handshake it completes (or refuses), the SDP and ICE frames it routes
//! between a channel's peers, and the `SendAlexaOfferToMaster` answer that comes back from
//! the master attached to it.
//!
//! The rule these cases assert, one level above the recorded case (one channel, one master,
//! one viewer, one offer and one answer):
//!
//! > A signaling channel is reached at ONE announced `wss://` address, and an opening
//! > handshake against it is answered FROM THE CHANNEL REGISTRY: a handshake naming a
//! > channel the emulator holds is answered `101 Switching Protocols` and attaches the
//! > connection as that channel's MASTER (no `X-Amz-ClientId`) or as the VIEWER that
//! > parameter names; one naming a channel that does not exist is refused `404`, one naming
//! > no readable channel ARN `400`, and a second master `403`. Over the established sockets
//! > every signaling message is routed by the channel: a viewer's frame reaches the master
//! > under that viewer's `ClientId` as `senderClientId`, and the master's frame reaches the
//! > viewer its `recipientClientId` names. The same master answers an offer delivered by
//! > `SendAlexaOfferToMaster`, whose `Answer` is then the one that master sent.
//!
//! The rows span what the rule generalises over: the message types a channel brokers, the
//! payload sizes, the client-id forms, whether the sender chose a correlation id, and every
//! state the handshake's channel can be in.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use engine::InMemoryEngine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice as signaling;
use signaling::state::session::{ICE_CANDIDATE, SDP_ANSWER, SDP_OFFER};
use smithy_server_runtime::MultiServiceRouter;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

/// How long a case waits for one frame before failing: generous for a loaded test machine,
/// and bounded so a case that will never be answered fails instead of hanging the suite.
const FRAME_DEADLINE: Duration = Duration::from_secs(30);

/// The status a completed handshake carries (RFC 6455 §4.1).
const SWITCHING_PROTOCOLS: u16 = 101;

/// The route `SendAlexaOfferToMaster` is served at.
const SEND_ALEXA_OFFER_TO_MASTER: &str = "/v1/send-alexa-offer-to-master";

/// The socket a client gets from a completed handshake.
type ClientSocket = tokio_tungstenite::WebSocketStream<TcpStream>;

/// The emulator a case drives: the shared engine, and a router carrying this one service.
fn emulator() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(signaling::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Put a channel in the registry the way the Kinesis Video control plane's
/// `CreateSignalingChannel` does, with the retention window its record states.
fn register(engine: &InMemoryEngine, arn: &str, message_ttl_secs: i32) {
    assert_eq!(
        signaling::state::register_channel_of(engine, arn, message_ttl_secs),
        signaling::state::ChannelRegistration::Registered,
        "case {arn:?}: the registry must accept the channel the control plane created"
    );
}

/// The `wss://host:port` every channel of this emulator's signaling data plane is reached
/// at, opening the socket on first use — the value `GetSignalingChannelEndpoint` announces
/// for the `WSS` protocol.
async fn announced(engine: &InMemoryEngine) -> String {
    signaling::state::ensure_wss_endpoint(engine)
        .await
        .expect("the signaling data plane's socket must open")
}

/// The loopback host the core binds a resource listener on by default, and so the only
/// address these cases ever dial.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The port of the announced socket, having checked its host is [`LOOPBACK_HOST`] — so the
/// dial below names that literal and reaches nothing else.
fn loopback_port(case: &str, announced: &str) -> u16 {
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| panic!("case {case:?}: expected a wss:// URL, got {announced:?}"));
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: expected a host:port, got {authority:?}"));
    assert_eq!(
        host, LOOPBACK_HOST,
        "case {case:?}: a case dials only the emulator's own loopback listener, got \
         {authority:?}"
    );
    port.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {authority:?}"))
}

/// Percent-encode one query value the way a signing client writes it into a presigned URL.
fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The query a peer's presigned signaling URL carries: the channel, and the client id when
/// the peer is a viewer.
fn handshake_query(channel_arn: &str, client_id: Option<&str>) -> String {
    let mut query = format!("X-Amz-ChannelARN={}", encoded(channel_arn));
    if let Some(client_id) = client_id {
        query.push_str(&format!("&X-Amz-ClientId={}", encoded(client_id)));
    }
    // The parameters a presigned SigV4 query carries beside them, as a client sends them.
    query.push_str("&X-Amz-Algorithm=AWS4-HMAC-SHA256");
    query
}

/// Open an RFC 6455 handshake against the announced socket, as a WebSocket client library
/// does: the established socket, or the status a refusal carries.
async fn handshake(
    case: &str,
    announced: &str,
    channel_arn: &str,
    client_id: Option<&str>,
) -> Result<ClientSocket, u16> {
    handshake_with_query(case, announced, &handshake_query(channel_arn, client_id)).await
}

/// [`handshake`] with the query written out, for a case whose query is the point.
async fn handshake_with_query(
    case: &str,
    announced: &str,
    query: &str,
) -> Result<ClientSocket, u16> {
    let port = loopback_port(case, announced);
    // The emulator's own loopback port, named as a literal: a case dials this address and
    // nothing else.
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the announced socket must accept a connection, got {error}")
        });
    let url = format!("ws://{LOOPBACK_HOST}:{port}/?{query}");
    match tokio_tungstenite::client_async(&url, stream).await {
        Ok((socket, response)) => {
            assert_eq!(
                response.status().as_u16(),
                SWITCHING_PROTOCOLS,
                "case {case:?}: a completed handshake must answer 101"
            );
            Ok(socket)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(error) => panic!("case {case:?}: the handshake failed before any answer: {error}"),
    }
}

/// Send one frame on `socket`.
async fn send(case: &str, socket: &mut ClientSocket, frame: &Value) {
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the frame must be sent, got {error}"));
}

/// The next frame `socket` is delivered, parsed, or `None` when none arrives within
/// [`FRAME_DEADLINE`].
async fn next_frame(case: &str, socket: &mut ClientSocket) -> Option<Value> {
    match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => Some(
            serde_json::from_str(text.as_str())
                .unwrap_or_else(|error| panic!("case {case:?}: frame is not JSON ({error}): {text}")),
        ),
        Ok(Some(Ok(Message::Binary(bytes)))) => Some(
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("case {case:?}: frame is not JSON ({error})")),
        ),
        Ok(Some(Ok(other))) => panic!("case {case:?}: unexpected frame {other:?}"),
        Ok(Some(Err(error))) => panic!("case {case:?}: the socket failed: {error}"),
        Ok(None) => panic!("case {case:?}: the socket closed instead of delivering a frame"),
        Err(_elapsed) => None,
    }
}

/// The string member `name` of `frame`.
fn member(frame: &Value, name: &str) -> Option<String> {
    frame.get(name).and_then(Value::as_str).map(str::to_string)
}

/// Base64, the encoding every signaling payload is carried in.
fn b64(text: &str) -> String {
    aws_smithy_types::base64::encode(text)
}

/// One routed message: what a peer sends, and what the other side must be delivered.
struct Row {
    case: &'static str,
    /// The viewer's own client id.
    client_id: &'static str,
    /// The message type the viewer sends to the master.
    to_master: &'static str,
    /// The payload it carries.
    offer: String,
    /// The correlation id the viewer chose, when it chose one.
    correlation_id: Option<&'static str>,
    /// The message type the master sends back to that viewer.
    to_viewer: &'static str,
    /// The payload the master sends back.
    answer: String,
}

/// The whole offer/answer roundtrip over the channel's own sockets, for every message the
/// channel brokers.
#[tokio::test]
async fn a_channels_sockets_route_signaling_messages_between_its_peers() {
    const ARN: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/wss-roundtrip/1700000000";

    let rows = vec![
        Row {
            case: "the SDP offer/answer roundtrip of the issue report",
            client_id: "viewer-1",
            to_master: SDP_OFFER,
            offer: b64("v=0\r\no=- 1 1 IN IP4 203.0.113.1\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"),
            correlation_id: Some("corr-1"),
            to_viewer: SDP_ANSWER,
            answer: b64("v=0\r\no=- 2 2 IN IP4 198.51.100.7\r\na=recvonly\r\n"),
        },
        Row {
            case: "ICE candidates both ways, with no correlation id",
            client_id: "viewer.2_b-C",
            to_master: ICE_CANDIDATE,
            offer: b64("candidate:1 1 udp 2130706431 192.0.2.4 51820 typ host"),
            correlation_id: None,
            to_viewer: ICE_CANDIDATE,
            answer: b64("candidate:2 1 udp 2130706431 198.51.100.9 51821 typ host"),
        },
        Row {
            case: "a one-character client id and a long payload",
            client_id: "a",
            to_master: SDP_OFFER,
            offer: b64(&"a=candidate:1 1 udp 2130706431 192.0.2.4 51820 typ host\r\n".repeat(60)),
            correlation_id: Some("c"),
            to_viewer: SDP_ANSWER,
            answer: b64(&"a=recvonly\r\n".repeat(80)),
        },
    ];

    let (engine, _router) = emulator();
    register(&engine, ARN, 5);
    let announced = announced(&engine).await;

    let mut master = handshake("the channel's master", &announced, ARN, None)
        .await
        .unwrap_or_else(|status| panic!("the master's handshake must be completed, got {status}"));

    for row in &rows {
        let case = row.case;
        let mut viewer = handshake(case, &announced, ARN, Some(row.client_id))
            .await
            .unwrap_or_else(|status| {
                panic!("case {case:?}: the viewer's handshake must be completed, got {status}")
            });

        let mut sent = json!({
            "action": row.to_master,
            "messagePayload": row.offer,
        });
        if let Some(correlation_id) = row.correlation_id {
            sent["correlationId"] = json!(correlation_id);
        }
        send(case, &mut viewer, &sent).await;

        let delivered = next_frame(case, &mut master)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the master must be delivered the message"));
        assert_eq!(
            member(&delivered, "messageType").as_deref(),
            Some(row.to_master),
            "case {case:?}: expected the message type as sent, got {delivered}"
        );
        assert_eq!(
            member(&delivered, "senderClientId").as_deref(),
            Some(row.client_id),
            "case {case:?}: expected the viewer's own client id, got {delivered}"
        );
        assert_eq!(
            member(&delivered, "messagePayload").as_deref(),
            Some(row.offer.as_str()),
            "case {case:?}: expected the payload as sent, got {delivered}"
        );
        assert_eq!(
            member(&delivered, "correlationId").as_deref(),
            row.correlation_id,
            "case {case:?}: expected the sender's correlation id, got {delivered}"
        );

        send(
            case,
            &mut master,
            &json!({
                "action": row.to_viewer,
                "recipientClientId": row.client_id,
                "messagePayload": row.answer,
            }),
        )
        .await;
        let answered = next_frame(case, &mut viewer)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the viewer must be delivered the answer"));
        assert_eq!(
            member(&answered, "messageType").as_deref(),
            Some(row.to_viewer),
            "case {case:?}: expected the answer's message type, got {answered}"
        );
        assert_eq!(
            member(&answered, "messagePayload").as_deref(),
            Some(row.answer.as_str()),
            "case {case:?}: expected the master's payload, got {answered}"
        );
        assert_ne!(
            member(&answered, "messagePayload").as_deref(),
            Some(row.offer.as_str()),
            "case {case:?}: the answer must not be the offer echoed back"
        );
    }
}

/// The handshake's own answer, for every state the channel it names can be in.
#[tokio::test]
async fn the_announced_socket_answers_a_handshake_from_the_channel_registry() {
    const REGISTERED: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/handshakes/1700000000";

    let (engine, _router) = emulator();
    register(&engine, REGISTERED, 5);
    let announced = announced(&engine).await;

    // The rows span the channel a handshake can name and the role it connects in.
    let rows: Vec<(&str, String, Result<(), u16>)> = vec![
        (
            "a registered channel, as its master",
            handshake_query(REGISTERED, None),
            Ok(()),
        ),
        (
            "a registered channel, as a viewer",
            handshake_query(REGISTERED, Some("viewer-1")),
            Ok(()),
        ),
        (
            "a well-formed channel ARN nobody created",
            handshake_query(
                "arn:aws:kinesisvideo:us-east-1:123456789012:channel/never-created/1700000009",
                None,
            ),
            Err(404),
        ),
        (
            "a channel ARN in another account",
            handshake_query(
                "arn:aws:kinesisvideo:us-east-1:210987654321:channel/handshakes/1700000000",
                Some("viewer-1"),
            ),
            Err(404),
        ),
        (
            "an ARN that is not a signaling channel's",
            handshake_query(
                "arn:aws:kinesisvideo:us-east-1:123456789012:stream/demo-stream/1700000000",
                None,
            ),
            Err(400),
        ),
        (
            "not an ARN at all",
            handshake_query("not-an-arn", None),
            Err(400),
        ),
        (
            "no channel named at all",
            "X-Amz-Algorithm=AWS4-HMAC-SHA256".to_string(),
            Err(400),
        ),
    ];

    for (case, query, expected) in &rows {
        let answered = handshake_with_query(case, &announced, query).await;
        match (answered, expected) {
            (Ok(socket), Ok(())) => drop(socket),
            (Err(status), Err(expected_status)) => assert_eq!(
                status, *expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status}"
            ),
            (Ok(_), Err(expected_status)) => {
                panic!("case {case:?}: expected HTTP {expected_status}, got a completed handshake")
            }
            (Err(status), Ok(())) => {
                panic!("case {case:?}: expected a completed handshake, got HTTP {status}")
            }
        }
    }

    // A channel takes ONE master: while the first holds the role the second is refused,
    // and once the first closes the role is free again.
    let first = handshake("the first master", &announced, REGISTERED, None)
        .await
        .expect("the first master's handshake must be completed");
    let second = handshake("a second master", &announced, REGISTERED, None).await;
    assert_eq!(
        second.err(),
        Some(403),
        "case \"a second master\": expected HTTP 403 while a master holds the role"
    );
    drop(first);
    // The channel notices the closed socket when it next reads its own session, which is
    // what the next handshake does; look again until it does, rather than waiting a fixed
    // time for it.
    let mut replaced = None;
    for _ in 0..(FRAME_DEADLINE.as_millis() / 50) {
        match handshake("a master after the first closed", &announced, REGISTERED, None).await {
            Ok(socket) => {
                replaced = Some(socket);
                break;
            }
            Err(403) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(status) => panic!("case \"a master after the first closed\": got HTTP {status}"),
        }
    }
    assert!(
        replaced.is_some(),
        "case \"a master after the first closed\": the role must be free once its socket is gone"
    );
}

/// `SendAlexaOfferToMaster` is answered BY THE LIVE MASTER: the offer is delivered over
/// that master's own socket, and the `Answer` is the payload the master sent back for that
/// sender.
///
/// The rows span the senders and payloads an Alexa offer carries.
#[tokio::test]
async fn send_alexa_offer_to_master_is_answered_over_the_masters_socket() {
    const ARN: &str =
        "arn:aws:kinesisvideo:us-west-2:123456789012:channel/alexa-live/1700000000";

    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "the sender of the issue report",
            "alexa-1",
            b64("v=0\r\no=- 1 1 IN IP4 203.0.113.1\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n"),
            b64("v=0\r\no=- 2 2 IN IP4 198.51.100.7\r\na=recvonly\r\n"),
        ),
        (
            "a sender whose id carries every admitted character",
            "alexa.sender_2-X9",
            b64("v=0\r\na=sendrecv\r\n"),
            b64("v=0\r\na=sendonly\r\n"),
        ),
    ];

    let (engine, router) = emulator();
    register(&engine, ARN, 5);
    let announced = announced(&engine).await;
    let mut master = handshake("the channel's master", &announced, ARN, None)
        .await
        .unwrap_or_else(|status| panic!("the master's handshake must be completed, got {status}"));

    for (case, sender, offer, answer) in &rows {
        let body = json!({
            "ChannelARN": ARN,
            "SenderClientId": sender,
            "MessagePayload": offer,
        })
        .to_string();
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://kinesisvideo.us-west-2.amazonaws.com{SEND_ALEXA_OFFER_TO_MASTER}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-west-2/kinesisvideo/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(body))
            .expect("the test request is well formed");

        // The call and the master run together: the handler delivers the offer over the
        // master's socket and waits for its answer, and the master here is what answers.
        let served = router.dispatch(request);
        let mastering = async {
            let delivered = next_frame(case, &mut master)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the master must be delivered the offer"));
            assert_eq!(
                member(&delivered, "messageType").as_deref(),
                Some(SDP_OFFER),
                "case {case:?}: expected an SDP offer, got {delivered}"
            );
            assert_eq!(
                member(&delivered, "senderClientId").as_deref(),
                Some(*sender),
                "case {case:?}: expected the Alexa sender's id, got {delivered}"
            );
            assert_eq!(
                member(&delivered, "messagePayload").as_deref(),
                Some(offer.as_str()),
                "case {case:?}: expected the offer as sent, got {delivered}"
            );
            send(
                case,
                &mut master,
                &json!({
                    "action": SDP_ANSWER,
                    "recipientClientId": sender,
                    "messagePayload": answer,
                }),
            )
            .await;
        };
        let (response, ()) = tokio::join!(served, mastering);

        let status = response.status().as_u16();
        let bytes = response.into_body();
        let answered: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: response is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        });
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered}"
        );
        assert_eq!(
            member(&answered, "Answer").as_deref(),
            Some(answer.as_str()),
            "case {case:?}: expected the master's answer, got {answered}"
        );
    }
}
