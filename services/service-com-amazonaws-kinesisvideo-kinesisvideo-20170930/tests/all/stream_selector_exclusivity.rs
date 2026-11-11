// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the EXCLUSIVE choice between the two identifiers of one
//! resource.
//!
//! RULE: an operation that models the two alternative identifiers of the resource it
//! addresses — a stream's `StreamName`/`StreamARN`, a signaling channel's
//! `ChannelName`/`ChannelARN`, both pairs optional and documented as a choice ("You must
//! specify either the `StreamName` or the `StreamARN`",
//! `smithy_query(query="doc",shape="DescribeStream")`; "You must specify either the name or
//! the Amazon Resource Name (ARN) of the channel",
//! `smithy_query(query="doc",shape="DescribeSignalingChannel")`) — requires EXACTLY one of
//! them. A request that names neither, or that names both, is refused with HTTP 400
//! `InvalidArgumentException` whose message is the one fixed sentence naming the two members,
//! whatever else the request carried and whether or not the two identifiers happen to agree;
//! a request that names exactly one resolves the resource.
//!
//! The sentence has no variable part: both halves of the pair are member NAMES the model
//! declares, so one operation's refusal cannot differ from another's and no value the caller
//! sent reaches the text.
//!
//! The rows below differ along what the rule generalises over: the resource the pair
//! addresses (a stream, a signaling channel), the operation's kind (a read, a tag write, a
//! configuration write, the data-endpoint report), which of the two identifiers is supplied
//! and how many, and whether two supplied identifiers agree. The recorded cases — one
//! `GetDataEndpoint` and one `DescribeStream` naming both — are two rows of it.
//!
//! Every case drives the real request walk (route -> deserialize -> validate -> handle ->
//! serialize) through [`MultiServiceRouter::dispatch`], so a response here is the same bytes
//! a client on the socket would see.
//!
//! The harness mirrors the one in the sibling suites of this crate (each test target module
//! owns its own, since none of them is importable from another).

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own engine, with request authentication off
/// so a case can send an unsigned-but-scoped request.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// The emulator endpoint every case addresses.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The error code this service refuses a bad input value with.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// POST one restJson1 call to `path`, answering its status and its parsed body.
async fn call(
    router: &MultiServiceRouter,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT_HOST}{path}"))
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and names
        // its region; the signature itself is not checked here.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), with any Smithy namespace stripped.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit('#')
        .next()
        .unwrap_or("")
        .to_string()
}

/// The text of a failure, from the `Message` member every error shape of this model declares
/// — the member an error a HANDLER returned carries its text in, which is what a refusal of
/// the pair's arity is. A refusal that reached the caller under the framework's lowercase
/// member therefore reads as no text at all here.
fn message(body: &serde_json::Value) -> String {
    body.get("Message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create `name` and return its `StreamARN`.
async fn create_stream(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "/createStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(status, 200, "CreateStream {name:?} failed with {body:?}");
    body.get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("CreateStream {name:?} returned no StreamARN: {body:?}"))
        .to_string()
}

/// Create the signaling channel `name` and return its `ChannelARN`.
async fn create_channel(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "/createSignalingChannel",
        serde_json::json!({ "ChannelName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSignalingChannel {name:?} failed with {body:?}"
    );
    body.get("ChannelARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("CreateSignalingChannel {name:?} returned no ChannelARN: {body:?}"))
        .to_string()
}

/// One operation that takes an identifier pair: where it goes, and the members it carries
/// BESIDE the pair (its own `@required` members, so the boundary validator passes the
/// request through to the rule under test).
struct Addressed {
    path: &'static str,
    besides: serde_json::Value,
}

/// The stream-addressed operations this suite drives, spanning reads, tag writes,
/// configuration writes and the data-endpoint report. The three operations that take the
/// pair and are still scaffolded answer 501 before any of this, so they are not here.
fn stream_addressed(version: &str) -> Vec<Addressed> {
    vec![
        Addressed {
            path: "/describeStream",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/getDataEndpoint",
            besides: serde_json::json!({ "APIName": "PUT_MEDIA" }),
        },
        Addressed {
            path: "/listTagsForStream",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/tagStream",
            besides: serde_json::json!({ "Tags": { "owner": "probe" } }),
        },
        Addressed {
            path: "/untagStream",
            besides: serde_json::json!({ "TagKeyList": ["owner"] }),
        },
        Addressed {
            path: "/describeNotificationConfiguration",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/updateNotificationConfiguration",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/describeImageGenerationConfiguration",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/describeMappedResourceConfiguration",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/describeEdgeConfiguration",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/describeStreamStorageConfiguration",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/updateDataRetention",
            besides: serde_json::json!({
                "CurrentVersion": version,
                "Operation": "INCREASE_DATA_RETENTION",
                "DataRetentionChangeInHours": 1
            }),
        },
    ]
}

/// The two signaling-channel readers that take the channel pair.
fn channel_addressed() -> Vec<Addressed> {
    vec![
        Addressed {
            path: "/describeSignalingChannel",
            besides: serde_json::json!({}),
        },
        Addressed {
            path: "/describeMediaStorageConfiguration",
            besides: serde_json::json!({}),
        },
    ]
}

/// `besides` with every member of `identifiers` added, as one request body.
fn request(besides: &serde_json::Value, identifiers: &serde_json::Value) -> serde_json::Value {
    let mut body = besides.clone();
    let object = body.as_object_mut().expect("each row is an object");
    for (member, value) in identifiers.as_object().expect("an object of identifiers") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// The stream version of `name`, which `UpdateDataRetention` has to carry.
async fn stream_version(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "/describeStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream {name:?} failed with {body:?}");
    body.pointer("/StreamInfo/Version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("DescribeStream {name:?} reported no Version: {body:?}"))
        .to_string()
}

/// RULE: a pair that does not name exactly one identifier is refused with 400
/// `InvalidArgumentException` and the pair's one fixed sentence — on every operation that
/// takes the pair, for both ways of missing it, and whether or not two supplied identifiers
/// agree.
#[tokio::test]
async fn a_pair_naming_neither_or_both_identifiers_is_refused() {
    let router = harness();
    let arn = create_stream(&router, "present").await;
    let other_arn = create_stream(&router, "other").await;
    let version = stream_version(&router, "present").await;

    // (case, the identifier members the request carries)
    let streams: Vec<(&str, serde_json::Value)> = vec![
        ("neither identifier", serde_json::json!({})),
        (
            "both, agreeing",
            serde_json::json!({ "StreamName": "present", "StreamARN": arn.clone() }),
        ),
        (
            "both, naming different streams",
            serde_json::json!({ "StreamName": "present", "StreamARN": other_arn.clone() }),
        ),
        (
            "both, the ARN naming no stream at all",
            serde_json::json!({
                "StreamName": "present",
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/ghost/1"
            }),
        ),
        (
            "both, neither naming a stored stream",
            serde_json::json!({
                "StreamName": "no-such-stream",
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/ghost/1"
            }),
        ),
    ];

    for addressed in stream_addressed(&version) {
        for (identifiers, body) in &streams {
            let case = format!("{identifiers} on {}", addressed.path);
            let (status, answer) =
                call(&router, addressed.path, request(&addressed.besides, body)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                INVALID_ARGUMENT,
                "case {case:?}: expected {INVALID_ARGUMENT}, got {answer:?}"
            );
            assert_eq!(
                message(&answer),
                kvs::state::STREAM_IDENTIFIER_MESSAGE,
                "case {case:?}: expected {:?}, got {:?}",
                kvs::state::STREAM_IDENTIFIER_MESSAGE,
                message(&answer)
            );
        }
    }

    // The same rule on the other pair this service models, with that pair's member names.
    let channel_arn = create_channel(&router, "channel").await;
    let channels: Vec<(&str, serde_json::Value)> = vec![
        ("neither identifier", serde_json::json!({})),
        (
            "both, agreeing",
            serde_json::json!({ "ChannelName": "channel", "ChannelARN": channel_arn.clone() }),
        ),
        (
            "both, the ARN naming no channel at all",
            serde_json::json!({
                "ChannelName": "channel",
                "ChannelARN": "arn:aws:kinesisvideo:us-east-1:000000000000:channel/ghost/1"
            }),
        ),
    ];

    for addressed in channel_addressed() {
        for (identifiers, body) in &channels {
            let case = format!("{identifiers} on {}", addressed.path);
            let (status, answer) =
                call(&router, addressed.path, request(&addressed.besides, body)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                INVALID_ARGUMENT,
                "case {case:?}: expected {INVALID_ARGUMENT}, got {answer:?}"
            );
            assert_eq!(
                message(&answer),
                kvs::state::CHANNEL_IDENTIFIER_MESSAGE,
                "case {case:?}: expected {:?}, got {:?}",
                kvs::state::CHANNEL_IDENTIFIER_MESSAGE,
                message(&answer)
            );
        }
    }

    assert_ne!(
        kvs::state::STREAM_IDENTIFIER_MESSAGE,
        kvs::state::CHANNEL_IDENTIFIER_MESSAGE,
        "each pair's refusal names its own two members"
    );
}

/// RULE: exactly one identifier resolves the resource — the refusal above is about the
/// pair's arity and not about the pair being there at all, so each identifier alone still
/// addresses the record, and an identifier that names nothing is the missing resource (404)
/// rather than a bad argument.
#[tokio::test]
async fn exactly_one_identifier_still_resolves_the_resource() {
    let router = harness();
    let arn = create_stream(&router, "present").await;
    let channel_arn = create_channel(&router, "channel").await;

    // (case, path, the members beside the pair, the one identifier, the expected status)
    let rows: Vec<(&str, &str, serde_json::Value, serde_json::Value, u16)> = vec![
        (
            "a stream read by name",
            "/describeStream",
            serde_json::json!({}),
            serde_json::json!({ "StreamName": "present" }),
            200,
        ),
        (
            "a stream read by ARN",
            "/describeStream",
            serde_json::json!({}),
            serde_json::json!({ "StreamARN": arn.clone() }),
            200,
        ),
        (
            "the data endpoint by name",
            "/getDataEndpoint",
            serde_json::json!({ "APIName": "PUT_MEDIA" }),
            serde_json::json!({ "StreamName": "present" }),
            200,
        ),
        (
            "the data endpoint by ARN",
            "/getDataEndpoint",
            serde_json::json!({ "APIName": "GET_MEDIA" }),
            serde_json::json!({ "StreamARN": arn.clone() }),
            200,
        ),
        (
            "a tag write by name",
            "/tagStream",
            serde_json::json!({ "Tags": { "owner": "probe" } }),
            serde_json::json!({ "StreamName": "present" }),
            200,
        ),
        (
            "a tag listing by ARN",
            "/listTagsForStream",
            serde_json::json!({}),
            serde_json::json!({ "StreamARN": arn.clone() }),
            200,
        ),
        (
            "a name that is not in the store",
            "/describeStream",
            serde_json::json!({}),
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
        ),
        (
            "an ARN that is not in the store",
            "/describeStream",
            serde_json::json!({}),
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/ghost/1"
            }),
            404,
        ),
        (
            "a channel read by name",
            "/describeSignalingChannel",
            serde_json::json!({}),
            serde_json::json!({ "ChannelName": "channel" }),
            200,
        ),
        (
            "a channel read by ARN",
            "/describeSignalingChannel",
            serde_json::json!({}),
            serde_json::json!({ "ChannelARN": channel_arn.clone() }),
            200,
        ),
        (
            "a channel name that is not in the store",
            "/describeSignalingChannel",
            serde_json::json!({}),
            serde_json::json!({ "ChannelName": "no-such-channel" }),
            404,
        ),
    ];

    for (case, path, besides, identifier, expected) in &rows {
        let (status, answer) = call(&router, path, request(besides, identifier)).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {answer:?}"
        );
        if *expected != 200 {
            assert_ne!(
                message(&answer),
                kvs::state::STREAM_IDENTIFIER_MESSAGE,
                "case {case:?}: an identifier that names nothing is not the pair's refusal, got \
                 {answer:?}"
            );
        }
    }
}
