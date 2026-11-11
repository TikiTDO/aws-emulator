// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the stream TAGGING surface of Kinesis Video Streams:
//! `TagStream`, `UntagStream`, `ListTagsForStream`, and the modelled refusals the
//! tagging inputs carry (`TagResource`'s `ResourceARN` included).
//!
//! The rule under test: a stream's tags are per-stream state, so `TagStream` MERGES
//! the request's key/value pairs into the ones the stream already holds (a repeated
//! key takes the new value), `UntagStream` removes exactly the listed keys, and
//! `ListTagsForStream` reports exactly the map those two left behind — for either
//! identifier form (`StreamName` or `StreamARN`) and for any key/value the modelled
//! `TagKey`/`TagValue` constraints admit.
//!
//! Each case is a ROW of a table spanning what the rule generalises over — tag count
//! (1, 3, and the modelled maximum of 50), value form (ordinary, empty, the modelled
//! maximum length), key form (the full modelled character class, maximum length) and
//! identifier form — rather than the single recorded request from the issue.
//!
//! Every case drives the real request walk (route -> deserialize -> validate ->
//! handle -> serialize) through [`MultiServiceRouter::dispatch`], so a response here
//! is the same bytes a client on the socket would see, headers included.

use std::collections::BTreeMap;
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

/// One response, as a client sees it.
struct Reply {
    status: u16,
    content_type: Option<String>,
    error_type: Option<String>,
    body: serde_json::Value,
}

/// POST one restJson1 call and return its status, the wire headers that carry the
/// error identity, and the parsed JSON body.
async fn call(router: &MultiServiceRouter, path: &str, body: serde_json::Value) -> Reply {
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
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    };
    let content_type = header("content-type");
    let error_type = header("x-amzn-errortype");
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    Reply {
        status,
        content_type,
        error_type,
        body: parsed,
    }
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

/// Assert a reply is the modelled failure `code` with `status`, in the restJson1
/// envelope this service speaks: `application/json` plus `x-amzn-errortype` naming
/// the modelled error.
fn assert_modeled_failure(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        error_code(&reply.body),
        code,
        "case {case:?}: expected error code {code}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.content_type.as_deref(),
        Some("application/json"),
        "case {case:?}: expected restJson1 content-type application/json, got {:?}",
        reply.content_type
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        reply.error_type
    );
}

/// Create `name` and return its `StreamARN`.
async fn create_stream(router: &MultiServiceRouter, name: &str) -> String {
    let reply = call(
        router,
        "/createStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: CreateStream failed with {:?}",
        reply.body
    );
    reply
        .body
        .get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {name:?}: CreateStream returned no StreamARN: {:?}",
                reply.body
            )
        })
        .to_string()
}

/// The tags `ListTagsForStream` reports for `identifier`, as a sorted map.
async fn read_tags(
    router: &MultiServiceRouter,
    case: &str,
    identifier: serde_json::Value,
) -> BTreeMap<String, String> {
    let reply = call(router, "/listTagsForStream", identifier).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListTagsForStream failed with {:?}",
        reply.body
    );
    assert_eq!(
        reply.content_type.as_deref(),
        Some("application/json"),
        "case {case:?}: expected restJson1 content-type application/json, got {:?}",
        reply.content_type
    );
    // A stream's tags are never truncated (at most 50 of them, and the modelled
    // `Tags` map holds 50), so the response carries no resume token — and the member
    // still reaches the wire, as JSON `null` rather than an absent key.
    assert_eq!(
        reply.body.get("NextToken"),
        Some(&serde_json::Value::Null),
        "case {case:?}: expected NextToken present and null, got {:?}",
        reply.body
    );
    let tags = reply
        .body
        .get("Tags")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListTagsForStream returned no Tags map: {:?}",
                reply.body
            )
        });
    tags.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: tag {key:?} is not a string: {value:?}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// A tag key built from every character class the modelled `TagKey` `@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits: letters (including non-ASCII), a
/// separator, digits, and each punctuation character it lists.
const WIDE_KEY: &str = "Ünicode Key_0.9:a/b=c+d-e@f";

/// How the stream under test is named in the tagging request: the rule holds for
/// either modelled identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Name,
    Arn,
}

/// One row of the tagging table.
struct Case {
    name: &'static str,
    stream: &'static str,
    by: By,
    /// The tags one `TagStream` call sets, as `(key, value)` pairs.
    tags: &'static [(&'static str, &'static str)],
    /// A second `TagStream` call, overlapping the first where the row exercises the
    /// merge.
    retag: &'static [(&'static str, &'static str)],
    /// The keys `UntagStream` then removes — including, in some rows, a key the
    /// stream never carried.
    untag: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        name: "a single tag, named by StreamName",
        stream: "tags-single",
        by: By::Name,
        tags: &[("Department", "Engineering")],
        retag: &[],
        untag: &["Department"],
    },
    Case {
        name: "three tags, named by StreamARN",
        stream: "tags-three",
        by: By::Arn,
        tags: &[("env", "prod"), ("owner", "video"), ("tier", "gold")],
        retag: &[],
        untag: &["owner"],
    },
    Case {
        name: "a repeated key takes the new value and a new key is added",
        stream: "tags-merge",
        by: By::Name,
        tags: &[("env", "staging"), ("owner", "video")],
        retag: &[("env", "prod"), ("region", "us-east-1")],
        untag: &["owner", "absent-key"],
    },
    Case {
        name: "an empty tag value, which TagValue's length 0..256 admits",
        stream: "tags-empty-value",
        by: By::Arn,
        tags: &[("marker", ""), ("env", "prod")],
        retag: &[],
        untag: &["marker"],
    },
    Case {
        name: "a key spanning TagKey's whole character class",
        stream: "tags-wide-key",
        by: By::Name,
        tags: &[(WIDE_KEY, "set")],
        retag: &[(WIDE_KEY, "reset")],
        untag: &[],
    },
    Case {
        name: "removing every tag leaves an empty map, not a missing one",
        stream: "tags-drained",
        by: By::Name,
        tags: &[("a", "1"), ("b", "2")],
        retag: &[],
        untag: &["a", "b"],
    },
    Case {
        name: "untagging a key the stream never carried changes nothing",
        stream: "tags-untag-absent",
        by: By::Arn,
        tags: &[("keep", "me")],
        retag: &[],
        untag: &["never-set"],
    },
];

/// The request body naming the stream under test the way the row asks for.
fn identifier(by: By, stream: &str, arn: &str) -> serde_json::Value {
    match by {
        By::Name => serde_json::json!({ "StreamName": stream }),
        By::Arn => serde_json::json!({ "StreamARN": arn }),
    }
}

/// `identifier` with `extra`'s members merged in.
fn with(mut body: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let extra = extra
        .as_object()
        .expect("the extra members are an object")
        .clone();
    let target = body.as_object_mut().expect("the body is an object");
    for (key, value) in extra {
        target.insert(key, value);
    }
    body
}

/// A `(key, value)` slice as a JSON tag map.
fn tag_map(tags: &[(&str, &str)]) -> serde_json::Value {
    serde_json::Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), serde_json::Value::from(*value)))
            .collect(),
    )
}

#[tokio::test]
async fn tagging_a_stream_merges_removes_and_reads_back_through_either_identifier() {
    let router = harness();

    for case in CASES {
        let arn = create_stream(&router, case.stream).await;
        let id = identifier(case.by, case.stream, &arn);

        // Nothing has been tagged yet: the report is an empty map, not a 404 and not
        // a missing member.
        let initial = read_tags(&router, case.name, id.clone()).await;
        assert!(
            initial.is_empty(),
            "case {:?}: a freshly created stream carries no tags, got {initial:?}",
            case.name
        );

        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for round in [case.tags, case.retag] {
            if round.is_empty() {
                continue;
            }
            let reply = call(
                &router,
                "/tagStream",
                with(id.clone(), serde_json::json!({ "Tags": tag_map(round) })),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: TagStream failed with {:?}",
                case.name, reply.body
            );
            assert_eq!(
                reply.body,
                serde_json::json!({}),
                "case {:?}: TagStream models no output members, got {:?}",
                case.name,
                reply.body
            );
            for (key, value) in round {
                expected.insert((*key).to_string(), (*value).to_string());
            }

            let observed = read_tags(&router, case.name, id.clone()).await;
            assert_eq!(
                observed, expected,
                "case {:?}: after TagStream expected {expected:?}, got {observed:?}",
                case.name
            );
        }

        if !case.untag.is_empty() {
            let reply = call(
                &router,
                "/untagStream",
                with(
                    id.clone(),
                    serde_json::json!({ "TagKeyList": case.untag }),
                ),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: UntagStream failed with {:?}",
                case.name, reply.body
            );
            assert_eq!(
                reply.body,
                serde_json::json!({}),
                "case {:?}: UntagStream models no output members, got {:?}",
                case.name,
                reply.body
            );
            for key in case.untag {
                expected.remove(*key);
            }

            let observed = read_tags(&router, case.name, id.clone()).await;
            assert_eq!(
                observed, expected,
                "case {:?}: after UntagStream expected {expected:?}, got {observed:?}",
                case.name
            );
        }

        // Both identifier forms name the one stored record, whichever the tagging
        // calls used.
        let by_other = match case.by {
            By::Name => identifier(By::Arn, case.stream, &arn),
            By::Arn => identifier(By::Name, case.stream, &arn),
        };
        let observed = read_tags(&router, case.name, by_other).await;
        assert_eq!(
            observed, expected,
            "case {:?}: the other identifier form must report the same tags, got {observed:?}",
            case.name
        );
    }
}

/// The per-resource ceiling is the MERGED tag count, so a stream can hold the
/// modelled maximum and one key past it is refused with the error the model declares
/// for exactly that case — leaving the stored tags untouched.
#[tokio::test]
async fn a_stream_holds_the_modeled_maximum_tags_and_refuses_one_more() {
    let router = harness();
    let stream = "tags-at-the-limit";
    create_stream(&router, stream).await;

    let max = kvs::state::MAX_TAGS_PER_RESOURCE;
    // The ceiling is the MODEL's, not the implementation's: `ResourceTags` is
    // `@length(min: 1, max: 50)` (`smithy_query(query="shape", shape="ResourceTags")`),
    // so the literal is asserted here and every bound below is taken from it.
    assert_eq!(
        max, 50,
        "case \"the modelled ceiling\": ResourceTags' modelled maximum is 50, got {max}"
    );
    let full: Vec<(String, String)> = (0..max)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let as_json = serde_json::Value::Object(
        full.iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::from(value.clone())))
            .collect(),
    );

    let reply = call(
        &router,
        "/tagStream",
        serde_json::json!({ "StreamName": stream, "Tags": as_json }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"{max} tags\": TagStream failed with {:?}",
        reply.body
    );

    let expected: BTreeMap<String, String> = full.into_iter().collect();
    let observed = read_tags(
        &router,
        "the modeled maximum",
        serde_json::json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        observed.len(),
        max,
        "case \"{max} tags\": expected {max} tags, got {}",
        observed.len()
    );
    assert_eq!(
        observed, expected,
        "case \"{max} tags\": expected {expected:?}, got {observed:?}"
    );

    // One key past the ceiling: refused, and nothing is written.
    let reply = call(
        &router,
        "/tagStream",
        serde_json::json!({
            "StreamName": stream,
            "Tags": { "one-too-many": "x" }
        }),
    )
    .await;
    assert_modeled_failure(
        "one tag past the per-resource limit",
        &reply,
        400,
        "TagsPerResourceExceededLimitException",
    );
    let observed = read_tags(
        &router,
        "after the refused TagStream",
        serde_json::json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        observed, expected,
        "case \"one tag past the per-resource limit\": the refusal must not write, \
         expected {expected:?}, got {observed:?}"
    );

    // Re-setting a key the stream already holds stays within the ceiling.
    let reply = call(
        &router,
        "/tagStream",
        serde_json::json!({
            "StreamName": stream,
            "Tags": { "key-0": "replaced" }
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"re-set an existing key at the limit\": TagStream failed with {:?}",
        reply.body
    );
    let observed = read_tags(
        &router,
        "after re-setting a key at the limit",
        serde_json::json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        observed.get("key-0").map(String::as_str),
        Some("replaced"),
        "case \"re-set an existing key at the limit\": expected the new value, got {observed:?}"
    );
    assert_eq!(
        observed.len(),
        max,
        "case \"re-set an existing key at the limit\": expected {max} tags, got {}",
        observed.len()
    );
}

/// What a refused tagging request must answer.
///
/// `code` is the error shape the refusal names, which for a request refused BEFORE the
/// handler ran is the one the stack fronting that operation states a refusal with (this
/// crate's `service::refusal_envelope`): the generic validation one on the stream-addressed
/// operations, this service's modelled `InvalidArgumentException` on the rest and on every
/// refusal a handler states itself.
struct Refusal {
    name: &'static str,
    path: &'static str,
    body: fn(&str) -> serde_json::Value,
    status: u16,
    code: &'static str,
}

/// A `StreamARN` that is well formed per `ResourceARN`'s modelled `@pattern` but
/// names no stored stream: a real ARN shape for a stream nobody created.
const UNKNOWN_ARN: &str = "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent-stream/1";

/// A `StreamARN` the modelled `@pattern` rejects (no `arn:` prefix, no partition, no
/// resource path).
const MALFORMED_ARN: &str = "not-an-arn";

/// A tag key one character past `TagKey`'s modelled maximum length.
fn over_long_tag_key() -> String {
    "k".repeat(kvs::state::MAX_TAG_KEY_LENGTH + 1)
}

/// A one-entry tag map whose key is one character past `TagKey`'s modelled maximum
/// length.
fn over_long_tag_map() -> serde_json::Value {
    serde_json::Value::Object(
        [(over_long_tag_key(), serde_json::Value::from("1"))]
            .into_iter()
            .collect(),
    )
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        name: "ListTagsForStream for a stream that does not exist",
        path: "/listTagsForStream",
        body: |_| serde_json::json!({ "StreamName": "no-such-stream" }),
        status: 404,
        code: "ResourceNotFoundException",
    },
    Refusal {
        name: "ListTagsForStream for a well-formed ARN naming no stream",
        path: "/listTagsForStream",
        body: |_| serde_json::json!({ "StreamARN": UNKNOWN_ARN }),
        status: 404,
        code: "ResourceNotFoundException",
    },
    Refusal {
        name: "TagStream for a stream that does not exist",
        path: "/tagStream",
        body: |_| serde_json::json!({ "StreamName": "no-such-stream", "Tags": { "a": "1" } }),
        status: 404,
        code: "ResourceNotFoundException",
    },
    Refusal {
        name: "UntagStream for a stream that does not exist",
        path: "/untagStream",
        body: |_| serde_json::json!({ "StreamName": "no-such-stream", "TagKeyList": ["a"] }),
        status: 404,
        code: "ResourceNotFoundException",
    },
    Refusal {
        name: "ListTagsForStream with a malformed streamARN",
        path: "/listTagsForStream",
        body: |_| serde_json::json!({ "StreamARN": MALFORMED_ARN }),
        status: 400,
        code: "ValidationException",
    },
    Refusal {
        name: "TagStream with a malformed streamARN",
        path: "/tagStream",
        body: |_| serde_json::json!({ "StreamARN": MALFORMED_ARN, "Tags": { "a": "1" } }),
        status: 400,
        code: "ValidationException",
    },
    Refusal {
        name: "UntagStream with a malformed streamARN",
        path: "/untagStream",
        body: |_| serde_json::json!({ "StreamARN": MALFORMED_ARN, "TagKeyList": ["a"] }),
        status: 400,
        code: "ValidationException",
    },
    Refusal {
        name: "TagResource with a malformed resourceARN",
        path: "/TagResource",
        body: |_| {
            serde_json::json!({
                "ResourceARN": MALFORMED_ARN,
                "Tags": [{ "Key": "a", "Value": "1" }]
            })
        },
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "ListTagsForStream naming neither identifier",
        path: "/listTagsForStream",
        body: |_| serde_json::json!({}),
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "TagStream naming neither identifier",
        path: "/tagStream",
        body: |_| serde_json::json!({ "Tags": { "a": "1" } }),
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "UntagStream naming neither identifier",
        path: "/untagStream",
        body: |_| serde_json::json!({ "TagKeyList": ["a"] }),
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "ListTagsForStream whose StreamName and StreamARN disagree",
        path: "/listTagsForStream",
        body: |arn| serde_json::json!({ "StreamName": "other-stream", "StreamARN": arn }),
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "ListTagsForStream with a NextToken this listing never issued",
        path: "/listTagsForStream",
        body: |_| {
            serde_json::json!({ "StreamName": "refusals-stream", "NextToken": "bm90LWEtdG9rZW4=" })
        },
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "TagStream with an empty tag key, below TagKey's minimum length",
        path: "/tagStream",
        body: |_| serde_json::json!({ "StreamName": "refusals-stream", "Tags": { "": "1" } }),
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "UntagStream with an empty tag key, below TagKey's minimum length",
        path: "/untagStream",
        body: |_| {
            serde_json::json!({ "StreamName": "refusals-stream", "TagKeyList": ["keep", ""] })
        },
        status: 400,
        code: "ValidationException",
    },
    Refusal {
        name: "TagStream with a tag key one past TagKey's maximum length",
        path: "/tagStream",
        body: |_| {
            serde_json::json!({
                "StreamName": "refusals-stream",
                "Tags": over_long_tag_map()
            })
        },
        status: 400,
        code: "InvalidArgumentException",
    },
    Refusal {
        name: "UntagStream with a tag key one past TagKey's maximum length",
        path: "/untagStream",
        body: |_| {
            serde_json::json!({
                "StreamName": "refusals-stream",
                "TagKeyList": [over_long_tag_key()]
            })
        },
        status: 400,
        code: "ValidationException",
    },
    Refusal {
        name: "TagStream with an empty Tags map, below ResourceTags' minimum length",
        path: "/tagStream",
        body: |_| serde_json::json!({ "StreamName": "refusals-stream", "Tags": {} }),
        status: 400,
        code: "ValidationException",
    },
    Refusal {
        name: "UntagStream with an empty TagKeyList, below its minimum length",
        path: "/untagStream",
        body: |_| serde_json::json!({ "StreamName": "refusals-stream", "TagKeyList": [] }),
        status: 400,
        code: "ValidationException",
    },
];

/// Every modelled refusal of the tagging surface, in the restJson1 envelope: the
/// status and the error code are the contract, and both must arrive as
/// `application/json` with `x-amzn-errortype` naming the modelled error.
#[tokio::test]
async fn the_tagging_surface_refuses_bad_requests_with_its_modeled_errors() {
    let router = harness();
    let arn = create_stream(&router, "refusals-stream").await;

    for refusal in REFUSALS {
        let reply = call(&router, refusal.path, (refusal.body)(&arn)).await;
        assert_modeled_failure(refusal.name, &reply, refusal.status, refusal.code);
    }
}

/// A tag key and value AT each modelled length bound are accepted and read back
/// unchanged; one character past the key's bound is refused.
///
/// The bounds are `TagKey` `@length(min: 1, max: 128)` and `TagValue`
/// `@length(min: 0, max: 256)`, from
/// `smithy_query(query="shape", shape="TagKey,TagValue")`.
#[tokio::test]
async fn tag_keys_and_values_at_the_modeled_length_bounds_round_trip() {
    let router = harness();
    let stream = "tags-at-the-length-bounds";
    create_stream(&router, stream).await;

    let shortest_key = "k".repeat(kvs::state::MIN_TAG_KEY_LENGTH);
    let longest_key = "k".repeat(kvs::state::MAX_TAG_KEY_LENGTH);
    // `TagValue`'s modelled bounds: an empty value and a 256-character one.
    let shortest_value = String::new();
    let longest_value = "v".repeat(256);

    let expected: BTreeMap<String, String> = [
        (shortest_key.clone(), longest_value.clone()),
        (longest_key.clone(), shortest_value.clone()),
    ]
    .into_iter()
    .collect();
    let tags = serde_json::Value::Object(
        expected
            .iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::from(value.clone())))
            .collect(),
    );

    let reply = call(
        &router,
        "/tagStream",
        serde_json::json!({ "StreamName": stream, "Tags": tags }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case \"keys and values at the modelled bounds\": TagStream failed with {:?}",
        reply.body
    );

    let observed = read_tags(
        &router,
        "keys and values at the modelled bounds",
        serde_json::json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        observed, expected,
        "case \"keys and values at the modelled bounds\": expected {expected:?}, \
         got {observed:?}"
    );

    // One character past the key bound, on the same stream: refused, and the stored
    // tags are unchanged.
    let reply = call(
        &router,
        "/tagStream",
        serde_json::json!({ "StreamName": stream, "Tags": over_long_tag_map() }),
    )
    .await;
    assert_modeled_failure(
        "a key one past TagKey's maximum length",
        &reply,
        400,
        "InvalidArgumentException",
    );
    let observed = read_tags(
        &router,
        "after the refused over-long key",
        serde_json::json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        observed, expected,
        "case \"a key one past TagKey's maximum length\": the refusal must not write, \
         expected {expected:?}, got {observed:?}"
    );
}

/// One `TagStream` carrying more tags than `ResourceTags`' modelled maximum is
/// refused at the boundary, and the stream is left untagged.
#[tokio::test]
async fn one_tag_stream_call_past_the_modeled_tags_maximum_is_refused() {
    let router = harness();
    let stream = "tags-over-the-request-limit";
    create_stream(&router, stream).await;

    let max = kvs::state::MAX_TAGS_PER_RESOURCE;
    for count in [max, max + 1] {
        let tags = serde_json::Value::Object(
            (0..count)
                .map(|index| {
                    (
                        format!("key-{index}"),
                        serde_json::Value::from(format!("value-{index}")),
                    )
                })
                .collect(),
        );
        let reply = call(
            &router,
            "/tagStream",
            serde_json::json!({ "StreamName": stream, "Tags": tags }),
        )
        .await;
        if count <= max {
            assert_eq!(
                reply.status, 200,
                "case \"{count} tags in one call\": expected HTTP 200, got {} with {:?}",
                reply.status, reply.body
            );
        } else {
            assert_modeled_failure(
                "51 tags in one call",
                &reply,
                400,
                // Refused at the boundary on a stream-addressed operation, which the stream
                // stack states as the generic validation refusal (this crate's
                // `service::refusal_envelope`).
                "ValidationException",
            );
        }
    }

    let observed = read_tags(
        &router,
        "after the refused oversized TagStream",
        serde_json::json!({ "StreamName": stream }),
    )
    .await;
    assert_eq!(
        observed.len(),
        max,
        "case \"51 tags in one call\": the refusal must not write, expected {max} tags, got {}",
        observed.len()
    );
}
