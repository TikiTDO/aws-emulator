// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for how this service WORDS a request the boundary
//! validator refused before any handler ran.
//!
//! RULE: a constraint refusal is HTTP 400 carrying the validation envelope over one clause
//! per refused member — the member's lowerCamel path and the requirement its value had to
//! satisfy — and the clause states NO value: neither the rejected one, whatever constraint
//! class it broke, nor the `null` a renderer writes for a member the request did not carry at
//! all. The requirement is a property of the model, so the same request always produces the
//! same sentence and no caller-supplied bytes reach it; a `@pattern` requirement quotes the
//! expression without the whole-value anchors the model wraps it in, and an enumerated type's
//! admissible values are listed in the order the front door walks its own table of them.
//!
//! The second rule here is about the ENVELOPE that text travels in, which is a property of
//! the OPERATION the call addressed (this crate's `service::refusal_envelope`): a
//! stream-addressed operation — one whose input names the stream by the
//! `StreamName`/`StreamARN` pair — is fronted by the stream stack, which answers the GENERIC
//! validation refusal (the code a constraint refusal carries on this protocol, that shape
//! named in `x-amzn-errortype`, and the text in that envelope's own `message` member); every
//! other operation answers this service's MODELLED `InvalidArgumentException`, whose text is
//! in the `Message` member every error shape of this model declares.
//!
//! Every case drives the real request walk (route -> deserialize -> validate -> handle
//! -> serialize) through [`MultiServiceRouter::dispatch`], so a response here is the
//! same bytes a client on the socket would see.
//!
//! The harness mirrors the one in the sibling suites of this crate (each test target
//! module owns its own, since none of them is importable from another).

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own engine, with request
/// authentication off so a case can send an unsigned-but-scoped request.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// The emulator endpoint every case addresses.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The error code the MODELLED exception of a refused input value names.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// The error code a constraint refusal carries on this protocol, which the stream stack's
/// refusal keeps.
const GENERIC_VALIDATION: &str = "ValidationException";

/// The body member the MODELLED exception carries its text in.
const MODELLED_MEMBER: &str = "Message";

/// The body member the generic validation envelope carries its text in.
const RENDERED_MEMBER: &str = "message";

/// The order a refusal of `APIName` lists that type's eight admissible values in, as a
/// recorded refusal of this member shows it. It is neither the order the model declares them
/// in nor lexicographic: it is the order the front door's own table of them is walked, which
/// is why the suite below asserts the shared derivation reproduces exactly this.
const STATED_API_NAME_ORDER: &str = "GET_IMAGES, LIST_FRAGMENTS, GET_DASH_STREAMING_SESSION_URL, \
     GET_HLS_STREAMING_SESSION_URL, GET_MEDIA_FOR_FRAGMENT_LIST, PUT_MEDIA, GET_CLIP, GET_MEDIA";

/// POST one restJson1 call to `path`, answering its status, its parsed body and its
/// `x-amzn-errortype` header.
async fn call(
    router: &MultiServiceRouter,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT_HOST}{path}"))
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // names its region; the signature itself is not checked here.
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed, error_type)
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

/// Which envelope a refusal of one operation travels in: the stream stack's generic
/// validation envelope, or the modelled exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Envelope {
    /// The stream stack's: the text in the generic `message` member, the shape named with
    /// this service's locator appended.
    StreamStack,
    /// The modelled exception's: the text in `Message`, the shape named bare.
    Modelled,
}

impl Envelope {
    /// The body member a refusal in this envelope carries its text in.
    fn member(self) -> &'static str {
        match self {
            Self::StreamStack => RENDERED_MEMBER,
            Self::Modelled => MODELLED_MEMBER,
        }
    }

    /// The member a refusal in this envelope must NOT also carry, so one refusal states its
    /// text once.
    fn other_member(self) -> &'static str {
        match self {
            Self::StreamStack => MODELLED_MEMBER,
            Self::Modelled => RENDERED_MEMBER,
        }
    }

    /// What `x-amzn-errortype` and the body's `__type` name in this envelope. Both envelopes
    /// name the shape bare; only WHICH shape it is differs.
    fn code(self) -> &'static str {
        match self {
            Self::StreamStack => GENERIC_VALIDATION,
            Self::Modelled => INVALID_ARGUMENT,
        }
    }
}

/// The refusal's text, read from the member the row's envelope carries it in — so a refusal
/// that reached the caller under the other member reads as no text at all here.
fn message(body: &serde_json::Value, envelope: Envelope) -> String {
    body.get(envelope.member())
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert the status, the code, the envelope and the member of one refused call.
fn assert_envelope(
    case: &str,
    status: u16,
    body: &serde_json::Value,
    error_type: &str,
    envelope: Envelope,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(body),
        envelope.code(),
        "case {case:?}: expected {:?}, got {body:?}",
        envelope.code()
    );
    let expected_named = envelope.code();
    assert_eq!(
        error_type, expected_named,
        "case {case:?}: expected x-amzn-errortype {expected_named:?}, got {error_type:?}"
    );
    assert!(
        body.get(envelope.other_member()).is_none(),
        "case {case:?}: the text must be carried under {:?} alone, got {body:?}",
        envelope.member()
    );
}

/// One refused request: what it is, where it goes, what it sends, which envelope answers it,
/// the clause it must be stated with, and the bytes of it that must NOT appear in that
/// clause.
struct Row {
    case: &'static str,
    path: &'static str,
    request: serde_json::Value,
    envelope: Envelope,
    clause: String,
    rejected: String,
}

/// RULE: every constraint class is refused with `InvalidArgumentException` (400) and the
/// one envelope above — the member path and its requirement, with no rejected value —
/// carried in the envelope of the stack that fronts the operation.
///
/// The rows differ along what the rule generalises over: the constraint class (an
/// enumerated type's value set, `@range`, `@length` on a string, `@length` on a list,
/// `@length` on a map, `@pattern`), the operation carrying it, which stack fronts that
/// operation, and how deep in the input the member sits (a top-level member, and one nested
/// inside a structure member). Every bound and value set quoted below is the model's, from
/// `smithy_query(query="shape", shape="ComparisonOperator,ResourceARN,StreamName,ListStreamsInput,TagKeyList,ResourceTags,APIName")`.
#[tokio::test]
async fn a_constraint_refusal_states_the_requirement_and_no_rejected_value() {
    let router = harness();

    // Past `StreamName`'s `@length` maximum of 256, while still inside its `@pattern`,
    // so the length is the one constraint the request breaks.
    let over_long_name = "n".repeat(300);
    // Past `ResourceTags`' `@length` maximum of 50 entries.
    let over_long_tags: serde_json::Map<String, serde_json::Value> = (0..51)
        .map(|index| (format!("key-{index}"), serde_json::json!("value")))
        .collect();

    let rows: Vec<Row> = vec![
        Row {
            case: "an operator outside ComparisonOperator's value set, nested in a structure",
            path: "/listStreams",
            request: serde_json::json!({
                "StreamNameCondition": {
                    "ComparisonOperator": "EQUALS",
                    "ComparisonValue": "probe"
                }
            }),
            envelope: Envelope::Modelled,
            clause: "Value at 'streamNameCondition.comparisonOperator' failed to satisfy \
                     constraint: Member must satisfy enum value set: [BEGINS_WITH]"
                .to_string(),
            rejected: "EQUALS".to_string(),
        },
        Row {
            case: "the same value set on the signaling-channel listing",
            path: "/listSignalingChannels",
            request: serde_json::json!({
                "ChannelNameCondition": {
                    "ComparisonOperator": "CONTAINS",
                    "ComparisonValue": "probe"
                }
            }),
            envelope: Envelope::Modelled,
            clause: "Value at 'channelNameCondition.comparisonOperator' failed to satisfy \
                     constraint: Member must satisfy enum value set: [BEGINS_WITH]"
                .to_string(),
            rejected: "CONTAINS".to_string(),
        },
        Row {
            case: "an APIName outside its value set",
            path: "/getDataEndpoint",
            request: serde_json::json!({ "StreamName": "probe", "APIName": "GET_EVERYTHING" }),
            envelope: Envelope::StreamStack,
            clause: "at 'aPIName' failed to satisfy constraint: Member must satisfy enum value \
                     set: ["
                .to_string(),
            rejected: "GET_EVERYTHING".to_string(),
        },
        Row {
            case: "a MaxResults below the modeled range minimum of 1",
            path: "/listStreams",
            request: serde_json::json!({ "MaxResults": 0 }),
            envelope: Envelope::Modelled,
            clause: "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                     greater than or equal to 1"
                .to_string(),
            rejected: "0".to_string(),
        },
        Row {
            case: "a MaxResults above the modeled range maximum of 10000",
            path: "/listSignalingChannels",
            request: serde_json::json!({ "MaxResults": 10_001 }),
            envelope: Envelope::Modelled,
            clause: "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                     less than or equal to 10000"
                .to_string(),
            rejected: "10001".to_string(),
        },
        Row {
            case: "a StreamName past its length maximum of 256",
            path: "/describeStream",
            request: serde_json::json!({ "StreamName": over_long_name.clone() }),
            envelope: Envelope::StreamStack,
            clause: "Value at 'streamName' failed to satisfy constraint: Member must have length \
                     less than or equal to 256"
                .to_string(),
            rejected: over_long_name.clone(),
        },
        Row {
            case: "a StreamARN that is not an ARN at all",
            path: "/listTagsForStream",
            request: serde_json::json!({ "StreamARN": "not-an-arn" }),
            envelope: Envelope::StreamStack,
            clause: "Value at 'streamARN' failed to satisfy constraint: Member must satisfy \
                     regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:\
                     [a-z]+/[a-zA-Z0-9_.-]+/[0-9]+"
                .to_string(),
            rejected: "not-an-arn".to_string(),
        },
        Row {
            case: "an empty TagKeyList, below the list's length minimum of 1",
            path: "/UntagResource",
            request: serde_json::json!({
                "ResourceARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1700000000000",
                "TagKeyList": []
            }),
            envelope: Envelope::Modelled,
            clause: "Value at 'tagKeyList' failed to satisfy constraint: Member must have length \
                     greater than or equal to 1"
                .to_string(),
            rejected: "0 items".to_string(),
        },
        Row {
            case: "a tag list past the list's length maximum of 50 entries",
            path: "/TagResource",
            request: serde_json::json!({
                "ResourceARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1700000000000",
                "Tags": (0..51)
                    .map(|index| serde_json::json!({
                        "Key": format!("key-{index}"),
                        "Value": "value"
                    }))
                    .collect::<Vec<serde_json::Value>>()
            }),
            envelope: Envelope::Modelled,
            clause: "Value at 'tags' failed to satisfy constraint: Member must have length less \
                     than or equal to 50"
                .to_string(),
            rejected: "51 items".to_string(),
        },
        Row {
            case: "a tag map past the map's length maximum of 50 entries",
            path: "/tagStream",
            request: serde_json::json!({
                "StreamName": "probe",
                "Tags": serde_json::Value::Object(over_long_tags.clone())
            }),
            envelope: Envelope::StreamStack,
            clause: "Value at 'tags' failed to satisfy constraint: Member must have length less \
                     than or equal to 50"
                .to_string(),
            rejected: "51 entries".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body, error_type) = call(&router, row.path, row.request.clone()).await;
        assert_envelope(case, status, &body, &error_type, row.envelope);
        let stated = message(&body, row.envelope);
        assert!(
            stated.starts_with("1 validation error detected: "),
            "case {case:?}: expected the one-error envelope, got {stated:?}"
        );
        assert!(
            stated.contains(&row.clause),
            "case {case:?}: expected a clause stating {:?}, got {stated:?}",
            row.clause
        );
        assert!(
            !stated.contains(&row.rejected),
            "case {case:?}: the clause must quote no rejected value, but {:?} appears in \
             {stated:?}",
            row.rejected
        );
        assert!(
            !stated.contains("Value '"),
            "case {case:?}: the clause must quote no value at all, got {stated:?}"
        );
    }
}

/// RULE: an enumerated type's admissible values are listed in the order the front door walks
/// its own table of them — neither the order the model declares them in nor lexicographic —
/// and the set is the one the member's type admits, whole.
///
/// Both enumerated types this service refuses a value of are covered: the eight-value
/// `APIName`, whose stated order differs from its declaration order, and the single-value
/// `ComparisonOperator`, where every order coincides. The expected order is the shared
/// derivation's (`service_common::validation_message::hashed_enum_order`) over the model's own
/// declared values, so no order is written out for the class and a model revision cannot leave
/// this asserting a stale list — and the ONE order a refusal of this service is known to have
/// stated ([`STATED_API_NAME_ORDER`]) is pinned as a literal beside it, so the derivation
/// itself is held to that observation rather than only compared with itself.
#[tokio::test]
async fn an_enum_value_set_is_listed_in_the_order_the_front_door_states_it() {
    let router = harness();

    assert_eq!(
        service_common::validation_message::hashed_enum_order(kvs::ops::types::A_P_I_NAME_VALUES)
            .join(", "),
        STATED_API_NAME_ORDER,
        "case \"the stated APIName order\": the derivation must state the order a refusal of \
         this member carries"
    );

    // (case, path, the request, the member path the clause names, the type's declared values)
    let rows: Vec<(&str, &str, serde_json::Value, &str, Vec<String>)> = vec![
        (
            "APIName, whose stated order is not its declaration order",
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "probe", "APIName": "NOT_AN_API" }),
            "aPIName",
            kvs::ops::types::A_P_I_NAME_VALUES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        ),
        (
            "ComparisonOperator, a single-value set",
            "/listStreams",
            serde_json::json!({
                "StreamNameCondition": { "ComparisonOperator": "ENDS_WITH" }
            }),
            "streamNameCondition.comparisonOperator",
            kvs::ops::types::COMPARISON_OPERATOR_VALUES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        ),
    ];

    for (case, path, request, member, declared) in &rows {
        let stated_order =
            service_common::validation_message::hashed_enum_order(declared).join(", ");
        let expected = format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{stated_order}]"
        );
        let (status, body, _) = call(&router, path, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        let stated = body
            .get(MODELLED_MEMBER)
            .or_else(|| body.get(RENDERED_MEMBER))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: a member the request did not carry is stated like every other refused member — the
/// member path and the requirement alone. The value half is dropped for a value the request
/// SENT and for the absence of one alike, so not even the `null` a framework renderer writes
/// for an absent member reaches the sentence.
///
/// The rows differ along which `@required` member is missing, on which operation, and which
/// stack fronts that operation.
#[tokio::test]
async fn an_absent_required_member_states_the_requirement_and_no_value() {
    let router = harness();

    // (case, path, request, the envelope, the expected message in full)
    let rows: Vec<(&str, &str, serde_json::Value, Envelope, String)> = vec![
        (
            "CreateStream without its required StreamName",
            "/createStream",
            serde_json::json!({ "MediaType": "video/h264" }),
            Envelope::Modelled,
            "1 validation error detected: Value at 'streamName' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "CreateSignalingChannel without its required ChannelName",
            "/createSignalingChannel",
            serde_json::json!({ "ChannelType": "SINGLE_MASTER" }),
            Envelope::Modelled,
            "1 validation error detected: Value at 'channelName' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "GetDataEndpoint without its required APIName",
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "probe" }),
            Envelope::StreamStack,
            "1 validation error detected: Value at 'aPIName' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
        (
            "TagResource without its required Tags",
            "/TagResource",
            serde_json::json!({
                "ResourceARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1700000000000"
            }),
            Envelope::Modelled,
            "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
    ];

    for (case, path, request, envelope, expected) in &rows {
        let (status, body, error_type) = call(&router, path, request.clone()).await;
        assert_envelope(case, status, &body, &error_type, *envelope);
        let stated = message(&body, *envelope);
        assert_eq!(
            &stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}
