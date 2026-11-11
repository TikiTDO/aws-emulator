// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the stateful stream slice of Kinesis Video Streams:
//! `CreateStream`, `DescribeStream`, `ListStreams`, `GetDataEndpoint`, `DeleteStream`.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through [`MultiServiceRouter::dispatch`], so a response here
//! is the same bytes a client on the socket would see.
//!
//! Where a modeled `@pattern` has to be proved, the test feeds the generated value
//! BACK IN through an input member carrying that same pattern (`StreamARN`,
//! `CurrentVersion`, `NextToken`). The boundary validator is then the oracle for the
//! pattern, instead of this file re-implementing one.

use std::collections::BTreeSet;
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

/// The control-plane endpoint every case addresses. `GetDataEndpoint` resolves a
/// stream's media calls to a DATA host instead of this one — the plane-by-plane rule
/// `stream_data_endpoint.rs` covers.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// POST one restJson1 call and return its status and parsed JSON body.
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
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), with any Smithy namespace
/// stripped.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit('#')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Create `name` and return its `StreamARN`, failing the test if the call did not
/// succeed.
async fn create_stream(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "/createStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(status, 200, "case {name:?}: CreateStream failed with {body:?}");
    body.get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: CreateStream returned no StreamARN: {body:?}"))
        .to_string()
}

/// Describe `name` and return its `StreamInfo`.
async fn describe_stream(router: &MultiServiceRouter, name: &str) -> serde_json::Value {
    let (status, body) = call(
        router,
        "/describeStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: DescribeStream failed with {body:?}"
    );
    body.get("StreamInfo")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: DescribeStream returned no StreamInfo: {body:?}"))
}

/// Wait out [`kvs::state::STREAM_ACTIVATION_SPAN`] so the clock owes every freshly
/// created stream its move to `ACTIVE`.
pub(crate) fn wait_for_activation() {
    std::thread::sleep(kvs::state::STREAM_ACTIVATION_SPAN + Duration::from_millis(150));
}

/// Wait out [`kvs::state::STREAM_DELETION_SPAN`] so the clock owes every stream
/// marked for deletion its removal from the store.
pub(crate) fn wait_for_deletion() {
    std::thread::sleep(kvs::state::STREAM_DELETION_SPAN + Duration::from_millis(150));
}

/// The whole client script from the issue, over one store: create, read back, list,
/// get a data endpoint, delete, watch the deletion settle, and read back the absence.
#[tokio::test]
async fn a_stream_is_created_read_listed_endpointed_and_deleted() {
    let router = harness();

    let (status, body) = call(
        &router,
        "/createStream",
        serde_json::json!({
            "StreamName": "demo-stream",
            "MediaType": "video/h264",
            "DataRetentionInHours": 24
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateStream: {body:?}");
    let arn = body
        .get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .expect("CreateStream returns a StreamARN")
        .to_string();

    // The ARN segments the modeled `@pattern`
    // `^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`
    // calls for, with the region taken from the request's credential scope.
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "StreamARN must have six colon-separated fields, got {arn:?}"
    );
    assert_eq!(fields[2], "kinesisvideo", "ARN service field: got {arn:?}");
    assert_eq!(fields[3], "us-east-1", "ARN region field: got {arn:?}");
    assert!(
        fields[4].chars().all(|c| c.is_ascii_digit()),
        "ARN account field must be digits, got {arn:?}"
    );
    let resource: Vec<&str> = fields[5].split('/').collect();
    assert_eq!(
        resource.len(),
        3,
        "ARN resource must be stream/<name>/<digits>, got {arn:?}"
    );
    assert_eq!(resource[0], "stream", "ARN resource type: got {arn:?}");
    assert_eq!(resource[1], "demo-stream", "ARN resource name: got {arn:?}");
    assert!(
        resource[2].chars().all(|c| c.is_ascii_digit()) && !resource[2].is_empty(),
        "ARN trailing segment must be digits, got {arn:?}"
    );

    wait_for_activation();

    let info = describe_stream(&router, "demo-stream").await;
    assert_eq!(
        info.get("StreamName").and_then(serde_json::Value::as_str),
        Some("demo-stream"),
        "DescribeStream reads back the created name: {info:?}"
    );
    assert_eq!(
        info.get("StreamARN").and_then(serde_json::Value::as_str),
        Some(arn.as_str()),
        "DescribeStream reads back the minted ARN: {info:?}"
    );
    assert_eq!(
        info.get("MediaType").and_then(serde_json::Value::as_str),
        Some("video/h264"),
        "DescribeStream reads back MediaType: {info:?}"
    );
    assert_eq!(
        info.get("DataRetentionInHours")
            .and_then(serde_json::Value::as_i64),
        Some(24),
        "DescribeStream reads back DataRetentionInHours: {info:?}"
    );
    assert_eq!(
        info.get("Status").and_then(serde_json::Value::as_str),
        Some("ACTIVE"),
        "a stream past its activation span is ACTIVE: {info:?}"
    );
    assert!(
        info.get("CreationTime").is_some(),
        "DescribeStream reports a CreationTime: {info:?}"
    );
    let version = info
        .get("Version")
        .and_then(serde_json::Value::as_str)
        .expect("DescribeStream reports a Version")
        .to_string();

    // Describing by the minted ARN must reach the same record. It also proves the ARN
    // satisfies `ResourceARN`'s pattern: an ARN that did not would be refused at the
    // boundary before this handler ran.
    let (status, body) = call(
        &router,
        "/describeStream",
        serde_json::json!({ "StreamARN": arn }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream by ARN: {body:?}");
    assert_eq!(
        body.get("StreamInfo"),
        Some(&info),
        "name and ARN must resolve to the same record: {body:?}"
    );

    let (status, body) = call(&router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams: {body:?}");
    let listed = body
        .get("StreamInfoList")
        .and_then(serde_json::Value::as_array)
        .expect("ListStreams returns StreamInfoList");
    assert_eq!(listed, &vec![info.clone()], "ListStreams: {body:?}");
    assert!(
        body.get("NextToken")
            .is_some_and(serde_json::Value::is_null),
        "the last page carries the NextToken member as null: {body:?}"
    );

    let (status, body) = call(
        &router,
        "/getDataEndpoint",
        serde_json::json!({ "StreamName": "demo-stream", "APIName": "PUT_MEDIA" }),
    )
    .await;
    assert_eq!(status, 200, "GetDataEndpoint: {body:?}");
    assert_ne!(
        body.get("DataEndpoint").and_then(serde_json::Value::as_str),
        Some(format!("https://{ENDPOINT_HOST}").as_str()),
        "the data endpoint is the stream's ingest plane, not the control plane the call \
         was made to: {body:?}"
    );

    let (status, body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": arn, "CurrentVersion": version }),
    )
    .await;
    assert_eq!(status, 200, "DeleteStream with the current version: {body:?}");

    // The delete MARKED the stream for deletion: it is still describable, with every
    // member carried over unchanged and only the status moved.
    let deleting = describe_stream(&router, "demo-stream").await;
    let mut expected = info.clone();
    expected["Status"] = serde_json::json!("DELETING");
    assert_eq!(
        deleting, expected,
        "a stream marked for deletion keeps its record with Status DELETING: \
         expected {expected:?}, got {deleting:?}"
    );

    let (status, body) = call(&router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams inside the deletion span: {body:?}");
    assert_eq!(
        body.get("StreamInfoList")
            .and_then(serde_json::Value::as_array),
        Some(&vec![deleting.clone()]),
        "ListStreams reports the same record DescribeStream does: {body:?}"
    );

    wait_for_deletion();

    let (status, body) = call(
        &router,
        "/describeStream",
        serde_json::json!({ "StreamName": "demo-stream" }),
    )
    .await;
    assert_eq!(
        status, 404,
        "DescribeStream once the deletion fell due: {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "DescribeStream once the deletion fell due: {body:?}"
    );

    let (status, body) = call(&router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams after the deletion span: {body:?}");
    assert_eq!(
        body.get("StreamInfoList")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(0),
        "ListStreams must not report a removed stream: {body:?}"
    );
}

/// Every member `CreateStream` accepts is persisted and read back unchanged, and the
/// members it does not receive take their documented defaults. The rows differ along
/// what the rule generalises over: which optional members are present, and the value,
/// form and size of each.
#[tokio::test]
async fn describe_stream_reads_back_exactly_what_create_stream_persisted() {
    /// `KmsKeyId` docs: "If no key ID is specified, the default, Kinesis
    /// Video-managed key (aws/kinesisvideo) is used."
    const DEFAULT_KEY: &str = "arn:aws:kms:us-east-1:000000000000:alias/aws/kinesisvideo";

    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "only the required name",
            serde_json::json!({ "StreamName": "bare" }),
            // `DataRetentionInHours`: "The default value is 0".
            serde_json::json!({
                "DeviceName": null, "MediaType": null,
                "KmsKeyId": DEFAULT_KEY, "DataRetentionInHours": 0
            }),
        ),
        (
            "device name and a single media type",
            serde_json::json!({
                "StreamName": "cam.1", "DeviceName": "device_01", "MediaType": "video/h264"
            }),
            serde_json::json!({
                "DeviceName": "device_01", "MediaType": "video/h264",
                "KmsKeyId": DEFAULT_KEY, "DataRetentionInHours": 0
            }),
        ),
        (
            "a multi-part media type and a caller-chosen key",
            serde_json::json!({
                "StreamName": "cam-2", "MediaType": "video/h264,audio/aac",
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abcd-1234",
                "DataRetentionInHours": 1
            }),
            serde_json::json!({
                "DeviceName": null, "MediaType": "video/h264,audio/aac",
                "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abcd-1234",
                "DataRetentionInHours": 1
            }),
        ),
        (
            "retention at its modeled minimum, with tags",
            serde_json::json!({
                "StreamName": "zero_retention", "DataRetentionInHours": 0,
                "Tags": { "env": "test", "owner": "" }
            }),
            serde_json::json!({
                "DeviceName": null, "MediaType": null,
                "KmsKeyId": DEFAULT_KEY, "DataRetentionInHours": 0
            }),
        ),
        (
            "a large retention and a storage configuration",
            serde_json::json!({
                "StreamName": "warm-tier", "DataRetentionInHours": 87_600,
                "StreamStorageConfiguration": { "DefaultStorageTier": "WARM" }
            }),
            serde_json::json!({
                "DeviceName": null, "MediaType": null,
                "KmsKeyId": DEFAULT_KEY, "DataRetentionInHours": 87_600
            }),
        ),
    ];

    let router = harness();
    for (case, request, expected) in cases {
        let name = request
            .get("StreamName")
            .and_then(serde_json::Value::as_str)
            .expect("each row names a stream")
            .to_string();

        let (status, body) = call(&router, "/createStream", request.clone()).await;
        assert_eq!(status, 200, "case {case:?}: CreateStream gave {body:?}");

        let info = describe_stream(&router, &name).await;
        for (member, want) in expected.as_object().expect("expected is an object") {
            let got = info.get(member).unwrap_or(&serde_json::Value::Null);
            assert_eq!(
                got, want,
                "case {case:?}: {member} expected {want:?}, got {got:?}"
            );
        }
        assert_eq!(
            info.get("StreamName").and_then(serde_json::Value::as_str),
            Some(name.as_str()),
            "case {case:?}: StreamName round-trip, got {info:?}"
        );
    }
}

/// `CreateStream` refuses a name the modeled `@length`/`@pattern` on `StreamName`
/// excludes, with the service's own `InvalidArgumentException`; it accepts every name
/// inside them. Both published length bounds are covered, plus one past each.
#[tokio::test]
async fn create_stream_enforces_the_modeled_stream_name_constraint() {
    let at_min = "a".to_string();
    let at_max = "n".repeat(256);
    let past_max = "n".repeat(257);

    let cases: Vec<(&str, &str, u16)> = vec![
        ("the length minimum, 1 character", at_min.as_str(), 200),
        ("every allowed character class", "Stream_name.9-x", 200),
        ("the length maximum, 256 characters", at_max.as_str(), 200),
        ("one past the length minimum, empty", "", 400),
        ("one past the length maximum, 257", past_max.as_str(), 400),
        ("a space, outside the pattern", "bad name", 400),
        ("a slash, outside the pattern", "bad/name", 400),
    ];

    let router = harness();
    for (case, name, expected_status) in cases {
        let (status, body) = call(
            &router,
            "/createStream",
            serde_json::json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {body:?}"
            );
        }
    }
}

/// A stream name is unique per account and region, so a second create under a name
/// already in the store is refused and leaves the stored record untouched.
#[tokio::test]
async fn a_duplicate_stream_name_is_refused_as_resource_in_use() {
    let router = harness();
    let arn = create_stream(&router, "taken").await;

    for case in ["identical request", "different optional members"] {
        let request = if case == "identical request" {
            serde_json::json!({ "StreamName": "taken" })
        } else {
            serde_json::json!({ "StreamName": "taken", "MediaType": "video/h265" })
        };
        let (status, body) = call(&router, "/createStream", request).await;
        assert_eq!(status, 400, "case {case:?}: got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {case:?}: got {body:?}"
        );
    }

    let info = describe_stream(&router, "taken").await;
    assert_eq!(
        info.get("StreamARN").and_then(serde_json::Value::as_str),
        Some(arn.as_str()),
        "a refused duplicate must not replace the stored record: {info:?}"
    );
    assert_eq!(
        info.get("MediaType").unwrap_or(&serde_json::Value::Null),
        &serde_json::Value::Null,
        "a refused duplicate must not alter the stored record: {info:?}"
    );
}

/// How a reader states an identifier it cannot resolve: a pair that does not name
/// EXACTLY one identifier — neither, or both, even two that agree — is a bad argument
/// (400), a well-formed identifier for no stored stream is a missing resource (404).
/// Asserted for both readers that take the pair.
#[tokio::test]
async fn an_unresolvable_identifier_is_stated_the_same_by_every_reader() {
    let router = harness();
    let arn = create_stream(&router, "present").await;
    let other_arn = create_stream(&router, "other").await;

    let cases: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "neither identifier",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
        ),
        (
            "a name that is not in the store",
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN that is not in the store",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/ghost/1"
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a name and an ARN naming different streams",
            serde_json::json!({ "StreamName": "present", "StreamARN": other_arn }),
            400,
            "InvalidArgumentException",
        ),
        (
            "a name and its own ARN",
            serde_json::json!({ "StreamName": "present", "StreamARN": arn }),
            400,
            "InvalidArgumentException",
        ),
    ];

    for (case, mut identifier, expected_status, expected_code) in cases {
        let (status, body) = call(&router, "/describeStream", identifier.clone()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} on DescribeStream: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on DescribeStream: got {body:?}"
            );
        }

        // `GetDataEndpoint` takes the same pair and must state the same verdict.
        identifier
            .as_object_mut()
            .expect("each row is an object")
            .insert("APIName".to_string(), serde_json::json!("GET_MEDIA"));
        let (status, body) = call(&router, "/getDataEndpoint", identifier).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} on GetDataEndpoint: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on GetDataEndpoint: got {body:?}"
            );
        }
    }
}

/// `GetDataEndpoint` answers for every `APIName` the model publishes, and refuses one
/// it does not. The catalog is the model's own
/// [`kvs::ops::types::A_P_I_NAME_VALUES`], so a model change cannot leave a value
/// untested.
#[tokio::test]
async fn get_data_endpoint_serves_every_modeled_api_name() {
    let router = harness();
    create_stream(&router, "endpointed").await;

    assert!(
        kvs::ops::types::A_P_I_NAME_VALUES.len() >= 8,
        "the model should publish the full APIName set, got {:?}",
        kvs::ops::types::A_P_I_NAME_VALUES
    );

    for api_name in kvs::ops::types::A_P_I_NAME_VALUES {
        let (status, body) = call(
            &router,
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "endpointed", "APIName": api_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {api_name:?}: expected HTTP 200, got {status} with {body:?}"
        );
        assert_ne!(
            body.get("DataEndpoint").and_then(serde_json::Value::as_str),
            Some(format!("https://{ENDPOINT_HOST}").as_str()),
            "case {api_name:?}: the data endpoint must not be the control-plane one: \
             {body:?}"
        );
    }

    for case in ["NOT_AN_API", "put_media", ""] {
        let (status, body) = call(
            &router,
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "endpointed", "APIName": case }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            // Refused at the boundary, which the stream stack states as the generic
            // validation refusal (this crate's `service::refusal_envelope`).
            "ValidationException",
            "case {case:?}: got {body:?}"
        );
    }

    let (status, body) = call(
        &router,
        "/getDataEndpoint",
        serde_json::json!({ "StreamName": "endpointed" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an absent required APIName must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "an absent required APIName: got {body:?}"
    );
}

/// `MaxResults` is a real page bound: draining the pages yields every stream exactly
/// once, in sorted name order, with a `NextToken` on every page but the last. The
/// rows vary the page size against the store size, including the published bounds of
/// `MaxResults` (1 and 10000).
#[tokio::test]
async fn list_streams_honours_max_results_as_a_page_bound() {
    let cases: Vec<(&str, usize, i64)> = vec![
        ("one stream, page of 1", 1, 1),
        ("five streams, page of 1 (the modeled minimum)", 5, 1),
        ("five streams, page of 2", 5, 2),
        ("five streams, page of 5 (exactly the store)", 5, 5),
        ("five streams, page of 6 (larger than the store)", 5, 6),
        ("nine streams, page of 4", 9, 4),
        ("three streams, page of 10000 (the modeled maximum)", 3, 10_000),
    ];

    for (case, total, max_results) in cases {
        let router = harness();
        let mut expected = Vec::new();
        for index in 0..total {
            let name = format!("s-{index:02}");
            create_stream(&router, &name).await;
            expected.push(name);
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = serde_json::json!({ "MaxResults": max_results });
            if let Some(token) = token.as_deref() {
                request
                    .as_object_mut()
                    .expect("the request is an object")
                    .insert("NextToken".to_string(), serde_json::json!(token));
            }
            let (status, body) = call(&router, "/listStreams", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListStreams page {pages} gave {body:?}"
            );
            let page = body
                .get("StreamInfoList")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no StreamInfoList in {body:?}"));
            let page_size = usize::try_from(max_results).unwrap_or(usize::MAX);
            let remaining = expected.len() - seen.len();
            assert_eq!(
                page.len(),
                remaining.min(page_size),
                "case {case:?}: page {pages} size, got {body:?}"
            );
            for info in page {
                seen.push(
                    info.get("StreamName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_else(|| panic!("case {case:?}: no StreamName in {info:?}"))
                        .to_string(),
                );
            }
            pages += 1;
            // Feeding the token straight back in also proves it satisfies
            // `NextToken`'s `^[a-zA-Z0-9+/=]*$` pattern: one that did not would be
            // refused at the boundary.
            token = body
                .get("NextToken")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: draining every page must yield each stream once, in name order"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case {case:?}: a stream was reported on two pages: {seen:?}"
        );
    }
}

/// `MaxResults` outside its modeled `@range` 1..10000 is refused rather than clamped.
#[tokio::test]
async fn list_streams_refuses_a_max_results_outside_its_modeled_range() {
    let router = harness();
    create_stream(&router, "any").await;

    for (case, max_results, expected_status) in [
        ("the range minimum", 1, 200),
        ("the range maximum", 10_000, 200),
        ("one past the minimum", 0, 400),
        ("one past the maximum", 10_001, 400),
    ] {
        let (status, body) = call(
            &router,
            "/listStreams",
            serde_json::json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({max_results}): expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidArgumentException",
                "case {case:?}: got {body:?}"
            );
        }
    }
}

/// `StreamNameCondition` is a server-side filter: BEGINS_WITH on `ComparisonValue`
/// selects exactly the stored names with that prefix, and it composes with the page
/// bound.
#[tokio::test]
async fn list_streams_applies_stream_name_condition_server_side() {
    let router = harness();
    for name in [
        "alpha-one",
        "alpha-two",
        "alpha.three",
        "beta-one",
        "gamma_1",
    ] {
        create_stream(&router, name).await;
    }

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a prefix shared by several names",
            "alpha",
            vec!["alpha-one", "alpha-two", "alpha.three"],
        ),
        ("a longer prefix narrowing it", "alpha-", vec!["alpha-one", "alpha-two"]),
        ("a prefix matching one name", "beta", vec!["beta-one"]),
        ("a whole name as the prefix", "gamma_1", vec!["gamma_1"]),
        ("a prefix matching nothing", "delta", vec![]),
        (
            "a one-character prefix",
            "a",
            vec!["alpha-one", "alpha-two", "alpha.three"],
        ),
    ];

    for (case, prefix, want) in cases {
        let mut expected: Vec<String> = want.iter().map(ToString::to_string).collect();
        expected.sort();

        let (status, body) = call(
            &router,
            "/listStreams",
            serde_json::json!({
                "StreamNameCondition": {
                    "ComparisonOperator": "BEGINS_WITH",
                    "ComparisonValue": prefix
                }
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {body:?}");
        let got: Vec<String> = body
            .get("StreamInfoList")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no StreamInfoList in {body:?}"))
            .iter()
            .filter_map(|info| info.get("StreamName").and_then(serde_json::Value::as_str))
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );

        // The filter is applied before the page bound, not after it.
        let (status, body) = call(
            &router,
            "/listStreams",
            serde_json::json!({
                "MaxResults": 1,
                "StreamNameCondition": {
                    "ComparisonOperator": "BEGINS_WITH",
                    "ComparisonValue": prefix
                }
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?} with MaxResults=1: got {body:?}");
        let first_page = body
            .get("StreamInfoList")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no StreamInfoList in {body:?}"));
        assert_eq!(
            first_page.len(),
            expected.len().min(1),
            "case {case:?} with MaxResults=1: got {body:?}"
        );
        // A resume token is minted only while matches remain; on the last page the
        // member still reaches the wire, as JSON `null`.
        assert_eq!(
            body.get("NextToken")
                .is_some_and(|token| !token.is_null()),
            expected.len() > 1,
            "case {case:?}: NextToken carries a token only while matches remain, got {body:?}"
        );
    }

    // A condition with no value to compare against cannot be evaluated.
    let (status, body) = call(
        &router,
        "/listStreams",
        serde_json::json!({ "StreamNameCondition": { "ComparisonOperator": "BEGINS_WITH" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a condition with no ComparisonValue must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidArgumentException",
        "a condition with no ComparisonValue: got {body:?}"
    );
}

/// A `NextToken` this listing never issued is refused rather than silently treated as
/// a resume point; the modeled empty token means "from the beginning", and a token
/// this listing did mint resumes where it left off.
#[tokio::test]
async fn list_streams_states_its_verdict_on_a_next_token() {
    let router = harness();
    for name in ["t-1", "t-2"] {
        create_stream(&router, name).await;
    }

    // A token this listing minted is honoured: it resumes after the page it ended.
    let (status, body) = call(
        &router,
        "/listStreams",
        serde_json::json!({ "MaxResults": 1 }),
    )
    .await;
    assert_eq!(status, 200, "the first page: got {body:?}");
    let issued = body
        .get("NextToken")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("the first page carries a NextToken: {body:?}"))
        .to_string();
    let (status, body) = call(
        &router,
        "/listStreams",
        serde_json::json!({ "NextToken": issued }),
    )
    .await;
    assert_eq!(status, 200, "a token this listing minted: got {body:?}");
    assert_eq!(
        body.get("StreamInfoList")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(1),
        "a token this listing minted resumes after its page: got {body:?}"
    );

    // Everything else: the empty token is the modeled "from the beginning", and a
    // token this listing did not mint is refused — including one that merely happens
    // to be base64, and one carrying another listing's marker.
    for (case, token, expected_status, expected_len) in [
        ("the modeled empty token", "", 200, 2),
        // base64 of "t-1", i.e. a bare stream name with no marker.
        ("base64 this listing never minted", "dC0x", 400, 0),
        // base64 of "other/v1/t-1": a marker that is not this listing's.
        ("another listing's marker", "b3RoZXIvdjEvdC0x", 400, 0),
        ("a token that is not base64", "a", 400, 0),
    ] {
        let (status, body) = call(
            &router,
            "/listStreams",
            serde_json::json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 200 {
            assert_eq!(
                body.get("StreamInfoList")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len),
                Some(expected_len),
                "case {case:?}: got {body:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "InvalidArgumentException",
                "case {case:?}: got {body:?}"
            );
        }
    }
}

/// `CurrentVersion` is an optimistic-concurrency check: a version that is not the
/// stored one refuses the delete and leaves the stream untouched; the stored one (or
/// no version at all) marks it for deletion.
#[tokio::test]
async fn delete_stream_checks_current_version_before_marking_the_record() {
    let router = harness();

    // A version that is not the stored one, in several shapes: all inside
    // `Version`'s modeled `@length` 1..64 and `^[a-zA-Z0-9]+$` pattern.
    let long_version = "z".repeat(64);
    let cases = ["bogus1", "A", long_version.as_str()];

    // One stream per row, so no row meets a record another row marked for deletion,
    // and all of them are created before the single wait for the activation span: the
    // version guard is only reached by a stream whose status allows a delete at all.
    let mut rows: Vec<(&str, String, String)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("guarded-{index}");
        let arn = create_stream(&router, &name).await;
        rows.push((case, name, arn));
    }
    wait_for_activation();

    for (case, name, arn) in &rows {
        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": arn, "CurrentVersion": case }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "VersionMismatchException",
            "case {case:?}: got {body:?}"
        );

        let info = describe_stream(&router, name).await;
        assert_eq!(
            info.get("StreamARN").and_then(serde_json::Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: a refused delete must leave the stream intact, got {info:?}"
        );
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: a refused delete must not mark the stream, got {info:?}"
        );

        // The stored version does mark it for deletion. Passing it back in also proves
        // it satisfies `Version`'s pattern: one that did not would be refused at the
        // boundary.
        let stored = info
            .get("Version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no Version in {info:?}"))
            .to_string();
        assert_ne!(
            &stored, case,
            "case {case:?}: the row must differ from the stored version"
        );
        let (status, body) = call(
            &router,
            "/deleteStream",
            serde_json::json!({ "StreamARN": arn, "CurrentVersion": stored }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored version must delete, got {status} with {body:?}"
        );
        let info = describe_stream(&router, name).await;
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some("DELETING"),
            "case {case:?}: an accepted delete marks the stream DELETING, got {info:?}"
        );
    }

    // No `CurrentVersion` at all: an unconditional delete.
    let arn = create_stream(&router, "unguarded").await;
    wait_for_activation();
    let (status, body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": arn }),
    )
    .await;
    assert_eq!(status, 200, "an unconditional delete: got {body:?}");

    // A stream already marked for deletion is present but not ACTIVE, so a second
    // delete is refused as in use rather than as absent.
    let (status, body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "deleting a stream that is already DELETING: got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "deleting a stream that is already DELETING: got {body:?}"
    );

    // Once the deletion has fallen due, the same ARN resolves to nothing at all, and
    // THAT is the missing-resource refusal.
    wait_for_deletion();
    let (status, body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": arn }),
    )
    .await;
    assert_eq!(status, 404, "deleting a removed stream: got {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "deleting a removed stream: got {body:?}"
    );
}

/// `CreateStream` is documented as asynchronous, so the stream is returned in the
/// non-terminal `CREATING` status and the clock owes it `ACTIVE` after
/// [`kvs::state::STREAM_ACTIVATION_SPAN`]. A caller reading immediately sees the
/// transient status; one reading after the span sees `ACTIVE`, through either reader.
#[tokio::test]
async fn a_created_stream_reports_creating_until_the_clock_owes_it_active() {
    let router = harness();
    for name in ["async-1", "async-2"] {
        create_stream(&router, name).await;

        let info = describe_stream(&router, name).await;
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some("CREATING"),
            "case {name:?}: a stream read inside its activation span is CREATING: {info:?}"
        );
    }

    let (status, body) = call(&router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams inside the span: {body:?}");
    for info in body
        .get("StreamInfoList")
        .and_then(serde_json::Value::as_array)
        .expect("ListStreams returns StreamInfoList")
    {
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some("CREATING"),
            "ListStreams reports the same status DescribeStream does: {info:?}"
        );
    }

    wait_for_activation();

    for name in ["async-1", "async-2"] {
        let info = describe_stream(&router, name).await;
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some("ACTIVE"),
            "case {name:?}: a stream past its activation span is ACTIVE: {info:?}"
        );
    }

    let (status, body) = call(&router, "/listStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams after the span: {body:?}");
    for info in body
        .get("StreamInfoList")
        .and_then(serde_json::Value::as_array)
        .expect("ListStreams returns StreamInfoList")
    {
        assert_eq!(
            info.get("Status").and_then(serde_json::Value::as_str),
            Some("ACTIVE"),
            "ListStreams reports the same status DescribeStream does: {info:?}"
        );
    }
}

/// The stream index carries the emulator's own ceiling
/// ([`kvs::state::MAX_STREAMS`]): a create past it is refused with the error the
/// model declares for exactly this case, the store keeps exactly the accepted
/// streams, and a delete frees a slot again.
#[tokio::test]
async fn create_stream_refuses_a_create_past_the_stream_ceiling() {
    let router = harness();

    // Fill to the ceiling. Every create is a real HTTP call, so the count the
    // ceiling is checked against is the one the store actually holds.
    for index in 0..kvs::state::MAX_STREAMS {
        create_stream(&router, &format!("bound-{index:06}")).await;
    }

    // One past it, in several name shapes — the refusal is about the COUNT, not
    // about the name.
    for case in ["bound-overflow", "b", &"n".repeat(256)] {
        let (status, body) = call(
            &router,
            "/createStream",
            serde_json::json!({ "StreamName": case }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 past the ceiling, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccountStreamLimitExceededException",
            "case {case:?}: got {body:?}"
        );

        // A refused create persisted nothing.
        let (status, body) = call(
            &router,
            "/describeStream",
            serde_json::json!({ "StreamName": case }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store no record, got {status} with {body:?}"
        );
    }

    // The store holds exactly the accepted streams: `MaxResults` is above the
    // ceiling, so one page is the whole index.
    let (status, body) = call(
        &router,
        "/listStreams",
        serde_json::json!({ "MaxResults": 10_000 }),
    )
    .await;
    assert_eq!(status, 200, "ListStreams at the ceiling: {body:?}");
    let listed = body
        .get("StreamInfoList")
        .and_then(serde_json::Value::as_array)
        .expect("ListStreams returns StreamInfoList")
        .len();
    assert_eq!(
        listed,
        kvs::state::MAX_STREAMS,
        "the store must hold exactly the accepted streams, got {listed}"
    );

    // Deleting one frees exactly one slot: the record is marked for deletion and the
    // slot comes back when its removal falls due, after which the next create
    // succeeds and the one after it is refused again. Only an `ACTIVE` stream can be
    // deleted, so the activation span is waited out rather than assumed to have
    // passed while the store filled.
    wait_for_activation();
    let doomed = describe_stream(&router, "bound-000000").await;
    assert_eq!(
        doomed.get("Status").and_then(serde_json::Value::as_str),
        Some("ACTIVE"),
        "the stream to be deleted must be ACTIVE, got {doomed:?}"
    );
    let arn = doomed
        .get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .expect("the stored stream has an ARN")
        .to_string();
    let (status, body) = call(
        &router,
        "/deleteStream",
        serde_json::json!({ "StreamARN": arn }),
    )
    .await;
    assert_eq!(status, 200, "freeing a slot: got {status} with {body:?}");
    wait_for_deletion();

    let (status, body) = call(
        &router,
        "/createStream",
        serde_json::json!({ "StreamName": "bound-reused" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a freed slot must accept one create, got {status} with {body:?}"
    );
    let (status, body) = call(
        &router,
        "/createStream",
        serde_json::json!({ "StreamName": "bound-reused-again" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the slot is used up again, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "AccountStreamLimitExceededException",
        "the slot is used up again: got {body:?}"
    );
}

/// Reading does not change what a later caller sees: repeated reads of an unchanged
/// stream report the identical record, including its `Version`.
#[tokio::test]
async fn repeated_reads_of_an_unchanged_stream_are_identical() {
    let router = harness();
    create_stream(&router, "stable").await;
    wait_for_activation();

    let first = describe_stream(&router, "stable").await;
    for round in 1..=3 {
        let again = describe_stream(&router, "stable").await;
        assert_eq!(
            again, first,
            "case read {round}: a read must not change the record, expected {first:?}, got {again:?}"
        );
        let (status, body) = call(
            &router,
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "stable", "APIName": "GET_MEDIA" }),
        )
        .await;
        assert_eq!(status, 200, "case read {round}: GetDataEndpoint: {body:?}");
    }
}
