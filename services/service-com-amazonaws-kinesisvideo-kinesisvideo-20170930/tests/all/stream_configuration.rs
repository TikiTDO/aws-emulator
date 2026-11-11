// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the stream-CONFIGURATION slice of Kinesis Video
//! Streams: `DescribeStreamStorageConfiguration`, `DescribeNotificationConfiguration`,
//! `DescribeImageGenerationConfiguration`, `DescribeMappedResourceConfiguration` and
//! `DescribeEdgeConfiguration`, together with the three updates that write the state
//! they report (`UpdateNotificationConfiguration`,
//! `UpdateImageGenerationConfiguration`, `UpdateMediaStorageConfiguration`) and
//! `StartEdgeConfigurationUpdate`.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through [`MultiServiceRouter::dispatch`], so a response here
//! is the same bytes a client on the socket would see — including the `restJson1`
//! error envelope (`Content-Type: application/json` plus `x-amzn-errortype`) every
//! failure has to carry.
//!
//! The harness mirrors the one in the sibling suites of this crate (each test target
//! module owns its own, since none of them is importable from another).
//!
//! Where a modeled `@pattern` has to be proved, the test feeds the generated value
//! BACK IN through an input member carrying that same pattern (`StreamARN`,
//! `ChannelARN`, `NextToken`). The boundary validator is then the oracle for the
//! pattern, instead of this file re-implementing one.

use std::sync::Arc;
use std::time::Duration;

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

/// The `Content-Type` `restJson1` answers with, success or failure.
const JSON_CONTENT_TYPE: &str = "application/json";

/// One answer from the router: HTTP status, the parsed JSON body, and the two response
/// headers the `restJson1` error envelope is made of.
struct Answer {
    status: u16,
    body: serde_json::Value,
    content_type: String,
    error_type: String,
}

impl Answer {
    /// The wire error code of a failure body (`__type`), with any Smithy namespace
    /// stripped.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .rsplit('#')
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// The member `name` of the response body, or `None` when it is absent or null —
    /// the two renderings of "this stream carries no such configuration".
    fn member(&self, name: &str) -> Option<&serde_json::Value> {
        self.body
            .get(name)
            .filter(|value| !value.is_null())
    }

    /// Whether `name` reached the wire as JSON `null`: the form a member the service
    /// has no value for takes, as distinct from an ABSENT key — the distinction an SDK
    /// caller branching on an unconfigured stream reads.
    fn reports_null(&self, name: &str) -> bool {
        self.body.get(name).is_some_and(serde_json::Value::is_null)
    }
}

/// POST one restJson1 call to `path`.
async fn call(router: &MultiServiceRouter, path: &str, body: serde_json::Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{ENDPOINT_HOST}{path}"))
        .header("host", ENDPOINT_HOST)
        .header("content-type", JSON_CONTENT_TYPE)
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
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let error_type = header("x-amzn-errortype");
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    Answer {
        status,
        body,
        content_type,
        error_type,
    }
}

/// Create a stream named `name` with `extra` merged into the request, and return its
/// `StreamARN`.
async fn create_stream(router: &MultiServiceRouter, name: &str, extra: serde_json::Value) -> String {
    let mut request = serde_json::json!({ "StreamName": name });
    if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    let answer = call(router, "/createStream", request).await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: CreateStream failed with {:?}",
        answer.body
    );
    answer
        .body
        .get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {name:?}: CreateStream returned no StreamARN: {:?}",
                answer.body
            )
        })
        .to_string()
}

/// Create a signaling channel named `name` and return its `ChannelARN`.
async fn create_channel(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "/createSignalingChannel",
        serde_json::json!({ "ChannelName": name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: CreateSignalingChannel failed with {:?}",
        answer.body
    );
    answer
        .body
        .get("ChannelARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {name:?}: CreateSignalingChannel returned no ChannelARN: {:?}",
                answer.body
            )
        })
        .to_string()
}

/// An ARN in the same account and region as `real`, naming a stream nothing created.
///
/// Built from a minted ARN rather than written out, so it satisfies `ResourceARN`'s
/// modeled `@pattern` by construction — the boundary validator refuses one that does
/// not, before any handler runs.
fn absent_stream_arn(real: &str) -> String {
    let (prefix, _) = real
        .split_once(":stream/")
        .unwrap_or_else(|| panic!("a minted StreamARN names a stream resource: {real:?}"));
    format!("{prefix}:stream/absent-stream/1700000000000")
}

/// Wait out [`kvs::state::STREAM_ACTIVATION_SPAN`] and
/// [`kvs::state::CHANNEL_ACTIVATION_SPAN`] so the clock owes every freshly created
/// stream and channel its move to `ACTIVE`.
fn wait_for_activation() {
    let span = kvs::state::STREAM_ACTIVATION_SPAN.max(kvs::state::CHANNEL_ACTIVATION_SPAN);
    std::thread::sleep(span + Duration::from_millis(150));
}

/// The five configuration reads this suite covers, as request paths.
const CONFIGURATION_READ_PATHS: &[&str] = &[
    "/describeStreamStorageConfiguration",
    "/describeNotificationConfiguration",
    "/describeImageGenerationConfiguration",
    "/describeMappedResourceConfiguration",
    "/describeEdgeConfiguration",
];

/// RULE: a request that violates a modelled constraint is refused with HTTP 400 whose
/// message is the framework's validation envelope, naming the member path and the
/// requirement that member failed, so the caller can fix the request from the message
/// alone. The CODE is the one the stack fronting the operation states a refusal with (this
/// crate's `service::refusal_envelope`): the generic validation one for the
/// stream-addressed operations, this service's modelled `InvalidArgumentException` for the
/// rest.
///
/// The rows differ along which constraint class the request violates (enum value set,
/// `@range`, `@pattern`, `@length`), which operation carries it, and which stack fronts
/// that operation, since the envelope is a property of the stack rather than of one
/// constraint.
#[tokio::test]
async fn a_constraint_violation_is_refused_as_an_invalid_argument() {
    let router = harness();

    // A `StreamName` of 300 characters: past `StreamName`'s modeled `@length` maximum
    // of 256, while still satisfying its `@pattern`, so the length is the one
    // constraint the request violates.
    let over_long_name = "n".repeat(300);

    // (case, path, request, the member the clause names, the requirement, the code the
    // stack fronting that operation refuses with)
    let cases: Vec<(&str, &str, serde_json::Value, &str, &str, &str)> = vec![
        (
            "a ComparisonOperator outside the enum value set",
            "/listStreams",
            serde_json::json!({
                "StreamNameCondition": {
                    "ComparisonOperator": "ENDS_WITH",
                    "ComparisonValue": "demo"
                }
            }),
            "streamNameCondition.comparisonOperator",
            "Member must satisfy enum value set: [BEGINS_WITH]",
            "InvalidArgumentException",
        ),
        (
            "a MaxResults below the modeled range",
            "/listStreams",
            serde_json::json!({ "MaxResults": 0 }),
            "maxResults",
            "Member must have value greater than or equal to 1",
            "InvalidArgumentException",
        ),
        (
            "a StreamARN that is not an ARN at all",
            "/describeStreamStorageConfiguration",
            serde_json::json!({ "StreamARN": "not-an-arn" }),
            "streamARN",
            "Member must satisfy regular expression pattern:",
            "ValidationException",
        ),
        (
            "a StreamName past the modeled length",
            "/describeNotificationConfiguration",
            serde_json::json!({ "StreamName": over_long_name }),
            "streamName",
            "Member must have length less than or equal to 256",
            "ValidationException",
        ),
    ];

    for (case, path, request, member, requirement, code) in &cases {
        let answer = call(&router, path, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: got {:?}",
            answer.body
        );
        let message = answer
            .body
            .get("Message")
            .or_else(|| answer.body.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let expected_clause =
            format!("at '{member}' failed to satisfy constraint: {requirement}");
        assert!(
            message.starts_with("1 validation error detected: ")
                && message.contains(&expected_clause),
            "case {case:?}: expected one validation error stating {expected_clause:?}, got {message:?}"
        );
    }
}

/// RULE: a configuration read reports the configuration the addressed stream actually
/// holds, so a stream nobody has configured answers 200 with that configuration's
/// "unconfigured" shape — the member on the wire as JSON `null`, an empty list, or the
/// stored default of a `@required` member — and only the edge configuration, whose
/// absence the model gives its own error, answers 404.
///
/// The rows differ along what the rule generalises over: which members the create named,
/// and the value of the one configuration a create can set. The storage tiers are
/// `DefaultStorageTier`'s whole value set from
/// `smithy_query(query="shape", shape="DefaultStorageTier")` (`HOT`, `WARM`); the issue
/// cited only `HOT`.
#[tokio::test]
async fn a_fresh_stream_reports_the_unconfigured_shape_of_every_configuration() {
    let router = harness();

    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        ("no storage configuration named", serde_json::json!({}), "HOT"),
        (
            "the HOT tier named explicitly",
            serde_json::json!({ "StreamStorageConfiguration": { "DefaultStorageTier": "HOT" } }),
            "HOT",
        ),
        (
            "the WARM tier named explicitly",
            serde_json::json!({ "StreamStorageConfiguration": { "DefaultStorageTier": "WARM" } }),
            "WARM",
        ),
        (
            "retention and a media type named, no storage configuration",
            serde_json::json!({ "DataRetentionInHours": 24, "MediaType": "video/h264" }),
            "HOT",
        ),
    ];

    for (index, (case, extra, expected_tier)) in cases.iter().enumerate() {
        let name = format!("fresh-stream-{index}");
        let arn = create_stream(&router, &name, extra.clone()).await;

        // Storage: `DefaultStorageTier` is `@required`, so the answer carries the tier
        // the create named, or the default when it named none.
        for identifier in [
            serde_json::json!({ "StreamName": name }),
            serde_json::json!({ "StreamARN": arn }),
        ] {
            let answer = call(
                &router,
                "/describeStreamStorageConfiguration",
                identifier.clone(),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} via {identifier:?}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.content_type, JSON_CONTENT_TYPE,
                "case {case:?} via {identifier:?}: expected Content-Type {JSON_CONTENT_TYPE}, got {:?}",
                answer.content_type
            );
            assert_eq!(
                answer.body.get("StreamName").and_then(serde_json::Value::as_str),
                Some(name.as_str()),
                "case {case:?} via {identifier:?}: expected StreamName {name:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body.get("StreamARN").and_then(serde_json::Value::as_str),
                Some(arn.as_str()),
                "case {case:?} via {identifier:?}: expected StreamARN {arn:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer
                    .body
                    .pointer("/StreamStorageConfiguration/DefaultStorageTier")
                    .and_then(serde_json::Value::as_str),
                Some(*expected_tier),
                "case {case:?} via {identifier:?}: expected DefaultStorageTier {expected_tier:?}, got {:?}",
                answer.body
            );
        }

        // Notification and image generation: a stream nobody configured reports NO
        // structure, which is a 200 with the member absent.
        for (path, member) in [
            (
                "/describeNotificationConfiguration",
                "NotificationConfiguration",
            ),
            (
                "/describeImageGenerationConfiguration",
                "ImageGenerationConfiguration",
            ),
        ] {
            let answer = call(&router, path, serde_json::json!({ "StreamName": name })).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} on {path}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.content_type, JSON_CONTENT_TYPE,
                "case {case:?} on {path}: expected Content-Type {JSON_CONTENT_TYPE}, got {:?}",
                answer.content_type
            );
            assert!(
                answer.reports_null(member),
                "case {case:?} on {path}: expected {member} to reach the wire as null, got {:?}",
                answer.body
            );
        }

        // Mapped resources: nothing is mapped to a fresh stream, which is an EMPTY
        // list and no token, not a failure.
        let answer = call(
            &router,
            "/describeMappedResourceConfiguration",
            serde_json::json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} on /describeMappedResourceConfiguration: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer
                .body
                .get("MappedResourceConfigurationList")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {case:?}: expected an empty MappedResourceConfigurationList, got {:?}",
            answer.body
        );
        assert!(
            answer.reports_null("NextToken"),
            "case {case:?}: expected NextToken as null on the only page, got {:?}",
            answer.body
        );

        // Edge: its absence is the one the model gives its own 404 error for, which is
        // what lets a client tell "no edge configuration" from "no such stream".
        let answer = call(
            &router,
            "/describeEdgeConfiguration",
            serde_json::json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?} on /describeEdgeConfiguration: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "StreamEdgeConfigurationNotFoundException",
            "case {case:?} on /describeEdgeConfiguration: got {:?}",
            answer.body
        );
        assert_eq!(
            answer.error_type, "StreamEdgeConfigurationNotFoundException",
            "case {case:?}: expected x-amzn-errortype to name the modeled error, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.content_type, JSON_CONTENT_TYPE,
            "case {case:?}: expected Content-Type {JSON_CONTENT_TYPE}, got {:?}",
            answer.content_type
        );
    }
}

/// RULE: every configuration read resolves the `StreamName`/`StreamARN` pair by the one
/// rule — either identifier names the record, neither is a bad argument (400), a pair
/// naming two records is a bad argument, and a well-formed identifier for no stored
/// stream is `ResourceNotFoundException` (404) — and renders the failure in the
/// `restJson1` envelope.
///
/// The rows differ along which identifier form the request carries; the columns are the
/// five reads, so the rule is asserted on each of them rather than on one.
#[tokio::test]
async fn a_configuration_read_refuses_an_unknown_or_contradictory_stream_identifier() {
    let router = harness();

    let first = create_stream(&router, "configured-stream", serde_json::json!({})).await;
    let second = create_stream(&router, "other-stream", serde_json::json!({})).await;
    let absent = absent_stream_arn(&first);

    let cases: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "a stream name nothing created",
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed ARN nothing created",
            serde_json::json!({ "StreamARN": absent }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "neither identifier",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
        ),
        (
            "a name and an ARN naming different streams",
            serde_json::json!({ "StreamName": "configured-stream", "StreamARN": second }),
            400,
            "InvalidArgumentException",
        ),
    ];

    for path in CONFIGURATION_READ_PATHS {
        for (case, request, expected_status, expected_code) in &cases {
            let answer = call(&router, path, request.clone()).await;
            assert_eq!(
                answer.status, *expected_status,
                "case {case:?} on {path}: expected {expected_status}, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                *expected_code,
                "case {case:?} on {path}: expected {expected_code}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type, *expected_code,
                "case {case:?} on {path}: expected x-amzn-errortype {expected_code}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.content_type, JSON_CONTENT_TYPE,
                "case {case:?} on {path}: expected Content-Type {JSON_CONTENT_TYPE}, got {:?}",
                answer.content_type
            );
            assert!(
                answer
                    .body
                    .get("Message")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?} on {path}: expected a non-empty Message, got {:?}",
                answer.body
            );
        }
    }

    // The stream that DOES exist is unaffected by the refusals above: the reads are
    // reporting operations.
    let answer = call(
        &router,
        "/describeStreamStorageConfiguration",
        serde_json::json!({ "StreamARN": first }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the stored stream still reads back: got {} {:?}",
        answer.status, answer.body
    );
}

/// RULE: a configuration the caller writes is reported back unchanged by its read, and a
/// write carrying no structure deletes it, so the read reports it absent again.
///
/// The rows differ along the configuration's status, the form of its destination URI and
/// which optional members it carries. `ConfigurationStatus`, `ImageSelectorType` and
/// `Format` are covered across the rows from their whole modeled value sets
/// (`smithy_query(query="shape", shape="ConfigurationStatus")` — `ENABLED`, `DISABLED`;
/// `ImageSelectorType` — `SERVER_TIMESTAMP`, `PRODUCER_TIMESTAMP`; `Format` — `JPEG`,
/// `PNG`).
#[tokio::test]
async fn a_written_configuration_reads_back_and_a_cleared_one_reads_absent() {
    let router = harness();
    let name = "configured-stream";
    create_stream(
        &router,
        name,
        serde_json::json!({ "DataRetentionInHours": 24 }),
    )
    .await;

    struct Case {
        case: &'static str,
        update_path: &'static str,
        read_path: &'static str,
        member: &'static str,
        configuration: serde_json::Value,
    }

    let cases = vec![
        Case {
            case: "an enabled notification configuration delivering to an SNS topic",
            update_path: "/updateNotificationConfiguration",
            read_path: "/describeNotificationConfiguration",
            member: "NotificationConfiguration",
            configuration: serde_json::json!({
                "Status": "ENABLED",
                "DestinationConfig": {
                    "Uri": "arn:aws:sns:us-east-1:000000000000:kvs-fragment-notifications"
                }
            }),
        },
        Case {
            case: "a disabled notification configuration delivering to an HTTPS endpoint",
            update_path: "/updateNotificationConfiguration",
            read_path: "/describeNotificationConfiguration",
            member: "NotificationConfiguration",
            configuration: serde_json::json!({
                "Status": "DISABLED",
                "DestinationConfig": { "Uri": "https://example.test/fragments" }
            }),
        },
        Case {
            case: "an enabled image generation configuration with every optional member",
            update_path: "/updateImageGenerationConfiguration",
            read_path: "/describeImageGenerationConfiguration",
            member: "ImageGenerationConfiguration",
            configuration: serde_json::json!({
                "Status": "ENABLED",
                "ImageSelectorType": "SERVER_TIMESTAMP",
                "DestinationConfig": {
                    "Uri": "s3://kvs-images/stream",
                    "DestinationRegion": "us-east-1"
                },
                // `SamplingInterval` is "the time interval in milliseconds (ms) at which
                // the images need to be generated"; the next row uses its documented
                // minimum of 200 ms.
                "SamplingInterval": 3000,
                "Format": "JPEG",
                "FormatConfig": { "JPEGQuality": "80" },
                "WidthPixels": 1920,
                "HeightPixels": 1080
            }),
        },
        Case {
            case: "a disabled image generation configuration with only required members",
            update_path: "/updateImageGenerationConfiguration",
            read_path: "/describeImageGenerationConfiguration",
            member: "ImageGenerationConfiguration",
            configuration: serde_json::json!({
                "Status": "DISABLED",
                "ImageSelectorType": "PRODUCER_TIMESTAMP",
                "DestinationConfig": {
                    "Uri": "s3://kvs-images-west/stream",
                    "DestinationRegion": "us-west-2"
                },
                "SamplingInterval": 200,
                "Format": "PNG"
            }),
        },
    ];

    for case in &cases {
        let mut request = serde_json::json!({ "StreamName": name });
        request[case.member] = case.configuration.clone();
        let answer = call(&router, case.update_path, request).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the update to answer 200, got {} {:?}",
            case.case, answer.status, answer.body
        );

        let answer = call(
            &router,
            case.read_path,
            serde_json::json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the read to answer 200, got {} {:?}",
            case.case, answer.status, answer.body
        );
        assert_eq!(
            answer.member(case.member),
            Some(&case.configuration),
            "case {:?}: expected {} to read back as written, got {:?}",
            case.case,
            case.member,
            answer.body
        );

        // Writing no structure deletes the configuration from the stream, so the read
        // reports it absent again.
        let answer = call(
            &router,
            case.update_path,
            serde_json::json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the clearing update to answer 200, got {} {:?}",
            case.case, answer.status, answer.body
        );
        let answer = call(
            &router,
            case.read_path,
            serde_json::json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the read after clearing to answer 200, got {} {:?}",
            case.case, answer.status, answer.body
        );
        assert!(
            answer.reports_null(case.member),
            "case {:?}: expected {} to reach the wire as null after clearing, got {:?}",
            case.case,
            case.member,
            answer.body
        );
    }
}

/// RULE: the resources `DescribeMappedResourceConfiguration` reports for a stream are
/// exactly the signaling channels whose enabled media-storage configuration points at
/// it, each carrying the resource type its own ARN names; a stream already holding
/// another channel's media, or persisting nothing, cannot take a new mapping.
#[tokio::test]
async fn a_mapped_resource_is_reported_from_the_mapping_that_created_it() {
    let router = harness();

    let stream_arn = create_stream(
        &router,
        "ingest-stream",
        serde_json::json!({ "DataRetentionInHours": 24 }),
    )
    .await;
    let transient_arn = create_stream(
        &router,
        "no-retention-stream",
        serde_json::json!({ "DataRetentionInHours": 0 }),
    )
    .await;
    let channel_arn = create_channel(&router, "ingest-channel").await;
    let other_channel_arn = create_channel(&router, "other-channel").await;
    wait_for_activation();

    let enable = |channel: &str, stream: &str| {
        serde_json::json!({
            "ChannelARN": channel,
            "MediaStorageConfiguration": { "Status": "ENABLED", "StreamARN": stream }
        })
    };

    let answer = call(
        &router,
        "/updateMediaStorageConfiguration",
        enable(&channel_arn, &stream_arn),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"map a channel to a retaining stream\": expected 200, got {} {:?}",
        answer.status, answer.body
    );

    // Both sides of the one mapping report it: the stream side names the channel, the
    // channel side names the stream.
    for request in [
        serde_json::json!({ "StreamName": "ingest-stream" }),
        serde_json::json!({ "StreamARN": stream_arn }),
        serde_json::json!({ "StreamName": "ingest-stream", "MaxResults": 1 }),
    ] {
        let answer = call(&router, "/describeMappedResourceConfiguration", request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {request:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("MappedResourceConfigurationList"),
            Some(&serde_json::json!([{ "Type": "CHANNEL", "ARN": channel_arn }])),
            "case {request:?}: expected the mapped channel, got {:?}",
            answer.body
        );
        assert!(
            answer.reports_null("NextToken"),
            "case {request:?}: expected NextToken as null on the only page, got {:?}",
            answer.body
        );
    }

    let answer = call(
        &router,
        "/describeMediaStorageConfiguration",
        serde_json::json!({ "ChannelARN": channel_arn }),
    )
    .await;
    assert_eq!(
        answer.body.get("MediaStorageConfiguration"),
        Some(&serde_json::json!({ "Status": "ENABLED", "StreamARN": stream_arn })),
        "case \"the channel side of the mapping\": got {:?}",
        answer.body
    );

    // A stream holds at most one channel's media — the list is modeled `@length(0..1)`
    // — so a second channel claiming it is refused.
    let answer = call(
        &router,
        "/updateMediaStorageConfiguration",
        enable(&other_channel_arn, &stream_arn),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"a second channel claiming a mapped stream\": expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceInUseException",
        "case \"a second channel claiming a mapped stream\": got {:?}",
        answer.body
    );

    // "In order for WebRTC Ingestion to work, the stream must have data retention
    // enabled."
    let answer = call(
        &router,
        "/updateMediaStorageConfiguration",
        enable(&other_channel_arn, &transient_arn),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"a stream that persists nothing\": expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NoDataRetentionException",
        "case \"a stream that persists nothing\": got {:?}",
        answer.body
    );

    // A token this listing never issued is not a resume point.
    let answer = call(
        &router,
        "/describeMappedResourceConfiguration",
        serde_json::json!({ "StreamName": "ingest-stream", "NextToken": "bm90LWEtdG9rZW4=" }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"a NextToken this listing never issued\": expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgumentException",
        "case \"a NextToken this listing never issued\": got {:?}",
        answer.body
    );

    // Disabling the configuration drops the mapping: "If StorageStatus is disabled, no
    // data will be stored, and the StreamARN parameter will not be needed."
    let answer = call(
        &router,
        "/updateMediaStorageConfiguration",
        serde_json::json!({
            "ChannelARN": channel_arn,
            "MediaStorageConfiguration": { "Status": "DISABLED" }
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"disable the mapping\": expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let answer = call(
        &router,
        "/describeMappedResourceConfiguration",
        serde_json::json!({ "StreamName": "ingest-stream" }),
    )
    .await;
    assert_eq!(
        answer.body.get("MappedResourceConfigurationList"),
        Some(&serde_json::json!([])),
        "case \"after disabling the mapping\": expected an empty list, got {:?}",
        answer.body
    );
}

/// RULE: a stream's edge configuration is the one the latest
/// `StartEdgeConfigurationUpdate` submitted; it is reported `SYNCING` until the clock
/// owes it the terminal status, a second update during that window is refused, and
/// `CreationTime` stays at the first submission while `LastUpdatedTime` moves.
///
/// The two rows differ along which optional members of `EdgeConfig` they carry.
#[tokio::test]
async fn an_edge_configuration_round_trips_and_reaches_its_terminal_sync_status() {
    let router = harness();
    create_stream(
        &router,
        "edge-stream",
        serde_json::json!({ "DataRetentionInHours": 24 }),
    )
    .await;
    wait_for_activation();

    // Every value here satisfies the modeled constraint of its member, so the boundary
    // validator is the oracle: `HubDeviceArn`'s IoT-thing pattern,
    // `MediaUriSecretArn`'s Secrets Manager pattern, `MediaUriType`'s enum,
    // `ScheduleExpression`'s 11..100 length, `DurationInSeconds`' 60..3600 range,
    // `EdgeRetentionInHours`' 1..720 and `MaxLocalMediaSizeInMB`' 64..2000000.
    let full = serde_json::json!({
        "HubDeviceArn": "arn:aws:iot:us-east-1:000000000000:thing/edge-hub",
        "RecorderConfig": {
            "MediaSourceConfig": {
                "MediaUriSecretArn":
                    "arn:aws:secretsmanager:us-east-1:000000000000:secret:camera-credentials",
                "MediaUriType": "RTSP_URI"
            },
            "ScheduleConfig": { "ScheduleExpression": "0 0 12 * * ?", "DurationInSeconds": 120 }
        },
        "UploaderConfig": {
            "ScheduleConfig": { "ScheduleExpression": "0 30 * * * ?", "DurationInSeconds": 3600 }
        },
        "DeletionConfig": {
            "EdgeRetentionInHours": 24,
            "LocalSizeConfig": {
                "MaxLocalMediaSizeInMB": 128,
                "StrategyOnFullSize": "DELETE_OLDEST_MEDIA"
            },
            "DeleteAfterUpload": true
        }
    });
    let minimal = serde_json::json!({
        "HubDeviceArn": "arn:aws:iot:us-east-1:000000000000:thing/edge-hub-2",
        "RecorderConfig": {
            "MediaSourceConfig": {
                "MediaUriSecretArn":
                    "arn:aws:secretsmanager:us-east-1:000000000000:secret:file-credentials",
                "MediaUriType": "FILE_URI"
            }
        }
    });

    let mut first_creation_time: Option<serde_json::Value> = None;

    for (case, config) in [
        ("every optional member carried", &full),
        ("only the required members carried", &minimal),
    ] {
        let answer = call(
            &router,
            "/startEdgeConfigurationUpdate",
            serde_json::json!({ "StreamName": "edge-stream", "EdgeConfig": config }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("SyncStatus").and_then(serde_json::Value::as_str),
            Some("SYNCING"),
            "case {case:?}: a submitted configuration starts SYNCING, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.get("EdgeConfig"),
            Some(config),
            "case {case:?}: expected the submitted EdgeConfig back, got {:?}",
            answer.body
        );
        let creation_time = answer
            .body
            .get("CreationTime")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime: {:?}", answer.body));

        // Read straight back: the caller still observes the transient sync status, and
        // the configuration it submitted.
        let answer = call(
            &router,
            "/describeEdgeConfiguration",
            serde_json::json!({ "StreamName": "edge-stream" }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the read to answer 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("SyncStatus").and_then(serde_json::Value::as_str),
            Some("SYNCING"),
            "case {case:?}: expected SYNCING inside the sync span, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.get("EdgeConfig"),
            Some(config),
            "case {case:?}: expected the stored EdgeConfig, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.get("StreamName").and_then(serde_json::Value::as_str),
            Some("edge-stream"),
            "case {case:?}: expected the StreamName, got {:?}",
            answer.body
        );
        // Nothing has failed and no Edge Agent has reported a job, so these members
        // reach the wire as null rather than as absent keys or invented status.
        for member in ["FailedStatusDetails", "EdgeAgentStatus"] {
            assert!(
                answer.reports_null(member),
                "case {case:?}: expected {member} to reach the wire as null, got {:?}",
                answer.body
            );
        }

        // A second submission inside the sync window is refused.
        let answer = call(
            &router,
            "/startEdgeConfigurationUpdate",
            serde_json::json!({ "StreamName": "edge-stream", "EdgeConfig": config }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected a second update inside the sync span to be refused, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceInUseException",
            "case {case:?}: got {:?}",
            answer.body
        );

        // The clock, not a reader, is what moves the sync status on.
        std::thread::sleep(kvs::state::EDGE_SYNC_SPAN + Duration::from_millis(150));
        let answer = call(
            &router,
            "/describeEdgeConfiguration",
            serde_json::json!({ "StreamName": "edge-stream" }),
        )
        .await;
        assert_eq!(
            answer.body.get("SyncStatus").and_then(serde_json::Value::as_str),
            Some("IN_SYNC"),
            "case {case:?}: expected IN_SYNC past the sync span, got {:?}",
            answer.body
        );

        match &first_creation_time {
            None => first_creation_time = Some(creation_time),
            Some(first) => assert_eq!(
                &creation_time, first,
                "case {case:?}: expected CreationTime to stay at the first submission {first:?}, got {creation_time:?}"
            ),
        }
    }

    // A stream of its own carries its own (absent) edge configuration.
    create_stream(&router, "edge-free-stream", serde_json::json!({})).await;
    let answer = call(
        &router,
        "/describeEdgeConfiguration",
        serde_json::json!({ "StreamName": "edge-free-stream" }),
    )
    .await;
    assert_eq!(
        answer.status, 404,
        "case \"a stream with no edge configuration\": expected 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "StreamEdgeConfigurationNotFoundException",
        "case \"a stream with no edge configuration\": got {:?}",
        answer.body
    );
}
