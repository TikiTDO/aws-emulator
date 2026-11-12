// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's SHARD MAP, end to end over the wire: `CreateStream` ->
//! `ListShards`.
//!
//! RULE: `ListShards` reports the STORED shard map of the stream its request addresses, in
//! shard-map order, carrying for each shard exactly what `DescribeStream` reports for it;
//! the shards it reports are the ones the request's `ShardFilter` selects (every shard
//! inside the retention window when the request carries none), started after the shard an
//! `ExclusiveStartShardId` — or an `AFTER_SHARD_ID` filter's `ShardId` — names, and served
//! one page of at most `MaxResults` (default and maximum 1000) at a time, with an opaque
//! `NextToken` exactly when a selected shard is left over, which resumes precisely where
//! the page stopped. It resolves its stream by `StreamName`, by `StreamARN`, by both or by
//! the `NextToken` that already identifies one, inside the region the request was signed
//! for; a request naming a stream this account and region do not hold is
//! `ResourceNotFoundException`, one naming no stream at all (or two different streams) is
//! `InvalidArgumentException`, one against a stream that is not `ACTIVE` yet is
//! `ResourceInUseException`, a filter property its own type does not admit is
//! `InvalidArgumentException`, a `NextToken` this service never issued is
//! `InvalidArgumentException` with "Invalid NextToken.", and one it DID issue that is past its
//! documented lifetime is `ExpiredNextTokenException`.
//!
//! The recorded flow this was derived from lists the shards of ONE stream once it is active
//! (`list_shards(StreamName=…)["Shards"]`) and is one row of the tables below. The rows
//! differ along what the rule generalises over: how many shards the stream holds, its
//! capacity mode, the page size the request asks for (both published bounds and past the
//! maximum), how many pages the listing takes, which filter type selects the shards, the
//! identifier the stream is resolved by, the region the request is signed for, and the
//! status the stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    next_token_expired_message, shard_id, stream_creation_time_not_found_message,
    stream_identifier_mismatch_message, stream_not_active_message, stream_not_found_message,
    ComAmazonawsKinesisKinesis20131202State, ShardPageToken, ACTIVATION_DELAY,
    DEFAULT_LIST_SHARDS_LIMIT, INVALID_NEXT_TOKEN_MESSAGE, LIST_SHARDS_TOKEN_TTL,
    MAX_LIST_SHARDS_LIMIT, NEXT_TOKEN_STREAM_MISMATCH_MESSAGE, STREAM_CREATING,
    STREAM_IDENTIFIER_REQUIRED_MESSAGE, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The published bounds of `MaxResults`
/// (`smithy_query(query="operation",operation="ListShards")` -> `MaxResults` range
/// 1..10000).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 10_000);

/// The `ShardFilterType` values the model declares
/// (`smithy_query(query="shape",shape="ShardFilterType")`).
const SHARD_FILTER_TYPES: &[&str] = &[
    "AFTER_SHARD_ID",
    "AT_LATEST",
    "AT_TIMESTAMP",
    "AT_TRIM_HORIZON",
    "FROM_TIMESTAMP",
    "FROM_TRIM_HORIZON",
];

/// How long a case waits before treating a freshly created stream as ready: past the
/// instant the create armed the `CREATING` -> `ACTIVE` transition for.
fn past_activation() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(400)
}

/// A router carrying exactly this service, over one shared engine: the same assembly
/// `core/router` performs, with one service mounted.
struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the mounted service reads, so a case can read the instant a create
    /// stored without going through a projection of it.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The instant the create stored for the stream `name` of the default test region.
    fn stream_created_at(&self, case: &str, name: &str) -> SystemTime {
        let state = self.engine.state::<ComAmazonawsKinesisKinesis20131202State>();
        let store = state.lock();
        store
            .stream_in(SystemTime::now(), REGION, name)
            .unwrap_or_else(|| panic!("case {case}: the store holds no stream named {name}"))
            .created_at
    }

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// [`Self::call_in`] in the default test region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation` in the default test region, asserting a 200 and answering the body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// Create a stream of `shard_count` shards (or an on-demand stream when `shard_count`
    /// is `None`), wait out the transition the create armed, and answer its ARN.
    async fn active_stream(&self, case: &str, name: &str, shard_count: Option<i64>) -> String {
        let mut body = json!({ "StreamName": name });
        match shard_count {
            Some(count) => {
                body["ShardCount"] = json!(count);
                body["StreamModeDetails"] = json!({ "StreamMode": STREAM_MODE_PROVISIONED });
            }
            None => body["StreamModeDetails"] = json!({ "StreamMode": STREAM_MODE_ON_DEMAND }),
        }
        self.ok(case, "CreateStream", &body).await;
        tokio::time::sleep(past_activation()).await;
        let summary = self
            .ok(
                case,
                "DescribeStreamSummary",
                &json!({ "StreamName": name }),
            )
            .await;
        summary["StreamDescriptionSummary"]["StreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
            .to_string()
    }

    /// `ListShards` with `body`, asserting a 200 and answering the body.
    async fn list(&self, case: &str, body: &Value) -> Value {
        self.ok(case, "ListShards", body).await
    }
}

/// The `__type` an awsJson1_1 error body carries, reduced to the error's shape name.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an awsJson1_1 error body carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `Shards` a listing reported, as a JSON array.
fn shards(answer: &Value) -> Vec<Value> {
    answer
        .get("Shards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The shard ids a listing reported, in the order it reported them.
fn shard_ids(answer: &Value) -> Vec<String> {
    shards(answer)
        .iter()
        .map(|shard| {
            shard
                .get("ShardId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `NextToken` a listing reported, absent when the body carries none or null.
fn next_token(answer: &Value) -> Option<String> {
    answer
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every shard the stream `name` holds, as `DescribeStream` reports it — the reader this
/// listing has to agree with.
async fn described_shards(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let described = emulator
        .ok(
            case,
            "DescribeStream",
            &json!({ "StreamName": name, "Limit": MAX_RESULTS_BOUNDS.0.max(100) }),
        )
        .await;
    described["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Page through `ListShards` from `body`, following `NextToken` to the end, and answer
/// every shard id in the order it arrived together with how many pages it took.
async fn page_through(
    emulator: &Emulator,
    case: &str,
    body: &Value,
    expected_total: usize,
) -> (Vec<String>, usize) {
    let mut listed: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    // One page per shard is the worst case; a page beyond that means the listing never
    // stopped handing out tokens.
    let page_budget = expected_total + 2;
    while pages <= page_budget {
        let mut page_body = body.clone();
        if let Some(token) = &token {
            page_body["NextToken"] = json!(token);
            // A `NextToken` already identifies the stream, so the resuming call names
            // neither it nor a starting shard.
            page_body.as_object_mut().map(|members| {
                members.remove("StreamName");
                members.remove("ExclusiveStartShardId")
            });
        }
        let answer = emulator.list(case, &page_body).await;
        pages += 1;
        listed.extend(shard_ids(&answer));
        match next_token(&answer) {
            None => return (listed, pages),
            Some(next) => {
                assert!(
                    !shards(&answer).is_empty(),
                    "case {case}: a page that hands back a NextToken must carry shards, got \
                     {answer}"
                );
                token = Some(next);
            }
        }
    }
    panic!("case {case}: ListShards never stopped handing out a NextToken after {pages} pages");
}

/// One shard-map row: a stream of this many shards in this capacity mode.
struct MapCase {
    label: &'static str,
    stream_name: &'static str,
    /// `ShardCount`, absent on an on-demand row whose count the service manages.
    shard_count: Option<i64>,
    /// The shards the stored stream holds.
    expected_shards: usize,
}

// 1. The listing reports the stored shard map, and agrees with `DescribeStream`
//
// The rows differ along the shard count and the capacity mode — what the rule generalises
// over — and the recorded single-shard stream is the first of them.
#[tokio::test]
async fn list_shards_reports_the_stored_shard_map_that_describe_stream_reports() {
    let emulator = Emulator::new();
    let cases = [
        MapCase {
            label: "one shard, provisioned (the recorded case)",
            stream_name: "one-shard",
            shard_count: Some(1),
            expected_shards: 1,
        },
        MapCase {
            label: "three shards, provisioned",
            stream_name: "three.shards",
            shard_count: Some(3),
            expected_shards: 3,
        },
        MapCase {
            label: "eight shards, provisioned",
            stream_name: "eight_shards",
            shard_count: Some(8),
            expected_shards: 8,
        },
        MapCase {
            label: "on-demand, service-managed shard count",
            stream_name: "on-demand",
            shard_count: None,
            expected_shards: 4,
        },
    ];

    for case in cases {
        let arn = emulator
            .active_stream(case.label, case.stream_name, case.shard_count)
            .await;
        let answer = emulator
            .list(case.label, &json!({ "StreamName": case.stream_name }))
            .await;
        let listed = shards(&answer);
        assert_eq!(
            listed.len(),
            case.expected_shards,
            "case {:?}: expected {} shards, got {}",
            case.label,
            case.expected_shards,
            listed.len()
        );
        assert!(
            next_token(&answer).is_none(),
            "case {:?}: a listing that fits one page carries no NextToken, got {answer}",
            case.label
        );

        // The reader this listing has to agree with, shard for shard.
        let described = described_shards(&emulator, case.label, case.stream_name).await;
        assert_eq!(
            listed, described,
            "case {:?}: ListShards must report exactly the shards DescribeStream reports, \
             got {listed:?} against {described:?}",
            case.label
        );

        // Each shard carries the id its position mints, an open sequence number range and
        // a hash key range that continues the previous shard's without a gap.
        let mut expected_start: u128 = 0;
        for (index, shard) in listed.iter().enumerate() {
            let position = u32::try_from(index).expect("a shard index fits in u32");
            assert_eq!(
                shard["ShardId"].as_str().unwrap_or_default(),
                shard_id(position),
                "case {:?}: shard {index} must carry the id its position mints, got {shard}",
                case.label
            );
            assert!(
                shard["SequenceNumberRange"]["EndingSequenceNumber"].is_null(),
                "case {:?}: an open shard reports no EndingSequenceNumber, got {shard}",
                case.label
            );
            let starting: u128 = shard["HashKeyRange"]["StartingHashKey"]
                .as_str()
                .unwrap_or_default()
                .parse()
                .unwrap_or_else(|_| {
                    panic!(
                        "case {:?}: shard {index} must report a decimal StartingHashKey, \
                         got {shard}",
                        case.label
                    )
                });
            let ending: u128 = shard["HashKeyRange"]["EndingHashKey"]
                .as_str()
                .unwrap_or_default()
                .parse()
                .unwrap_or_else(|_| {
                    panic!(
                        "case {:?}: shard {index} must report a decimal EndingHashKey, got \
                         {shard}",
                        case.label
                    )
                });
            assert_eq!(
                starting, expected_start,
                "case {:?}: shard {index} must start where the previous one ended, got \
                 {starting} against {expected_start}",
                case.label
            );
            assert!(
                ending >= starting,
                "case {:?}: shard {index} must span a non-empty hash key range, got \
                 {starting}..{ending}",
                case.label
            );
            // The last shard ends at 2^128-1, so one past it wraps back to zero: that is
            // what the assertion below reads as "the whole space is covered".
            expected_start = ending.wrapping_add(1);
        }
        assert_eq!(
            expected_start, 0,
            "case {:?}: the shard map must cover the whole 128-bit hash key space, it ended \
             one past {}",
            case.label,
            expected_start.wrapping_sub(1)
        );

        // The same map is reported when the stream is resolved by ARN, or by both members.
        for (how, body) in [
            ("by StreamARN", json!({ "StreamARN": arn })),
            (
                "by both members",
                json!({ "StreamName": case.stream_name, "StreamARN": arn }),
            ),
        ] {
            let resolved = emulator.list(case.label, &body).await;
            assert_eq!(
                shards(&resolved),
                listed,
                "case {:?} {how}: the same stream must report the same shard map, got {resolved}",
                case.label
            );
        }
    }
}

/// One pagination row: how many shards the stream holds and the page size asked for.
struct PageCase {
    label: &'static str,
    stream_name: &'static str,
    shard_count: i64,
    /// `MaxResults`, absent on a row that names none and takes the documented default.
    max_results: Option<i64>,
    /// How many pages the whole listing must take.
    expected_pages: usize,
}

// 2. `MaxResults` bounds a page, and `NextToken` resumes exactly where it stopped
#[tokio::test]
async fn max_results_bounds_a_page_and_next_token_resumes_exactly_once() {
    let emulator = Emulator::new();
    let shards_held = 9i64;
    let cases = [
        PageCase {
            label: "the smallest published page size",
            stream_name: "page-of-one",
            shard_count: shards_held,
            max_results: Some(MAX_RESULTS_BOUNDS.0),
            expected_pages: 9,
        },
        PageCase {
            label: "a page of two",
            stream_name: "page-of-two",
            shard_count: shards_held,
            max_results: Some(2),
            expected_pages: 5,
        },
        PageCase {
            label: "a page that divides the map exactly",
            stream_name: "page-of-three",
            shard_count: shards_held,
            max_results: Some(3),
            expected_pages: 3,
        },
        PageCase {
            label: "a page larger than the map",
            stream_name: "page-of-many",
            shard_count: shards_held,
            max_results: Some(shards_held + 1),
            expected_pages: 1,
        },
        PageCase {
            label: "the largest published page size",
            stream_name: "page-of-max",
            shard_count: shards_held,
            max_results: Some(MAX_RESULTS_BOUNDS.1),
            expected_pages: 1,
        },
        PageCase {
            label: "no page size at all (the recorded case)",
            stream_name: "page-default",
            shard_count: shards_held,
            max_results: None,
            expected_pages: 1,
        },
    ];

    // The documented default and maximum are the same 1000, so neither a request naming no
    // page size nor one naming the modelled maximum truncates a map of nine shards.
    assert_eq!(
        DEFAULT_LIST_SHARDS_LIMIT, MAX_LIST_SHARDS_LIMIT,
        "the documented default page size is also the documented maximum"
    );

    for case in cases {
        emulator
            .active_stream(case.label, case.stream_name, Some(case.shard_count))
            .await;
        let mut body = json!({ "StreamName": case.stream_name });
        if let Some(max_results) = case.max_results {
            body["MaxResults"] = json!(max_results);
        }
        let expected_total = usize::try_from(case.shard_count).expect("a shard count fits");
        let (listed, pages) = page_through(&emulator, case.label, &body, expected_total).await;

        let expected_ids: Vec<String> = (0..case.shard_count)
            .map(|index| shard_id(u32::try_from(index).expect("a shard index fits in u32")))
            .collect();
        assert_eq!(
            listed, expected_ids,
            "case {:?}: paging must report every shard exactly once in shard-map order, got \
             {listed:?}",
            case.label
        );
        assert_eq!(
            pages, case.expected_pages,
            "case {:?}: expected {} pages, got {pages}",
            case.label, case.expected_pages
        );
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>().len(),
            listed.len(),
            "case {:?}: no shard may arrive twice, got {listed:?}",
            case.label
        );

        // Each page carries at most the page size asked for.
        if let Some(max_results) = case.max_results {
            let first = emulator.list(case.label, &body).await;
            let page = shards(&first).len();
            assert!(
                i64::try_from(page).is_ok_and(|page| page <= max_results.min(1_000)),
                "case {:?}: a page carries at most {max_results} shards, got {page}",
                case.label
            );
        }
    }
}

/// One shard-filter row: the filter a request carries and the shards it must select from a
/// stream of open shards.
struct FilterCase {
    label: &'static str,
    /// `ShardFilter`, absent on the row that names none and takes the documented default.
    filter: Option<Value>,
    /// Which shard ids of a four-shard stream the filter must select.
    expected: Vec<usize>,
}

// 3. A `ShardFilter` selects which shards the listing reports
//
// Every shard of a freshly created stream is OPEN and inside the retention window, so every
// type that selects by the retention window or by openness selects all four, one by
// timestamp selects by where that timestamp falls, and `AFTER_SHARD_ID` starts after the
// shard it names.
#[tokio::test]
async fn a_shard_filter_selects_which_shards_are_reported() {
    let emulator = Emulator::new();
    let stream = "filtered";
    emulator.active_stream("filters", stream, Some(4)).await;

    let before_creation = epoch_seconds(SystemTime::now() - Duration::from_secs(3_600));
    let now = epoch_seconds(SystemTime::now());
    let cases = [
        FilterCase {
            label: "no filter at all (the recorded case)",
            filter: None,
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "FROM_TRIM_HORIZON, the documented default",
            filter: Some(json!({ "Type": "FROM_TRIM_HORIZON" })),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "AT_TRIM_HORIZON: the shards open at the horizon",
            filter: Some(json!({ "Type": "AT_TRIM_HORIZON" })),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "AT_LATEST: only the currently open shards",
            filter: Some(json!({ "Type": "AT_LATEST" })),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "AT_TIMESTAMP at an instant the shards span",
            filter: Some(json!({ "Type": "AT_TIMESTAMP", "Timestamp": now })),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "AT_TIMESTAMP before any shard existed",
            filter: Some(json!({ "Type": "AT_TIMESTAMP", "Timestamp": before_creation })),
            expected: vec![],
        },
        FilterCase {
            label: "FROM_TIMESTAMP at an instant inside the window",
            filter: Some(json!({ "Type": "FROM_TIMESTAMP", "Timestamp": now })),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "FROM_TIMESTAMP older than the trim horizon, corrected to it",
            filter: Some(json!({ "Type": "FROM_TIMESTAMP", "Timestamp": before_creation })),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            label: "AFTER_SHARD_ID: the shards after the first",
            filter: Some(json!({ "Type": "AFTER_SHARD_ID", "ShardId": shard_id(0) })),
            expected: vec![1, 2, 3],
        },
        FilterCase {
            label: "AFTER_SHARD_ID: the shards after the last, which is none",
            filter: Some(json!({ "Type": "AFTER_SHARD_ID", "ShardId": shard_id(3) })),
            expected: vec![],
        },
    ];

    // Every modelled filter type is covered by the table above, including the ones the
    // issue did not cite.
    for filter_type in SHARD_FILTER_TYPES {
        assert!(
            cases.iter().any(|case| case
                .filter
                .as_ref()
                .and_then(|filter| filter["Type"].as_str())
                .is_some_and(|named| &named == filter_type)),
            "every modelled ShardFilterType must be exercised; {filter_type} is not"
        );
    }

    for case in cases {
        let mut body = json!({ "StreamName": stream });
        if let Some(filter) = &case.filter {
            body["ShardFilter"] = filter.clone();
        }
        let answer = emulator.list(case.label, &body).await;
        let expected: Vec<String> = case
            .expected
            .iter()
            .map(|index| shard_id(u32::try_from(*index).expect("a shard index fits in u32")))
            .collect();
        assert_eq!(
            shard_ids(&answer),
            expected,
            "case {:?}: expected the shards {expected:?}, got {answer}",
            case.label
        );
    }
}

/// Epoch seconds, as awsJson1_1 carries a `Timestamp` on the wire.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// One refusal row: a request and the modelled error it must answer.
struct RefusalCase {
    label: &'static str,
    body: Value,
    expected_code: &'static str,
    /// A fragment the message must carry, when the rule that produces it fixes one.
    expected_message: Option<String>,
}

// 4. Every refusal the rule names, over the requests that trigger it
#[tokio::test]
async fn list_shards_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    let stream = "refusals";
    let arn = emulator.active_stream("refusals", stream, Some(3)).await;
    let other = "refusals-other";
    let other_arn = emulator.active_stream("refusals", other, Some(1)).await;

    let expired = ShardPageToken::new(
        &arn,
        emulator.stream_created_at("refusals", stream),
        &shard_id(1),
        SystemTime::now() - LIST_SHARDS_TOKEN_TTL - Duration::from_secs(1),
    )
    .encode();
    let live = ShardPageToken::new(
        &arn,
        emulator.stream_created_at("refusals", stream),
        &shard_id(1),
        SystemTime::now(),
    )
    .encode();
    // A token minted at an instant that has not arrived names no page this service served.
    let future = ShardPageToken::new(
        &arn,
        emulator.stream_created_at("refusals", stream),
        &shard_id(1),
        SystemTime::now() + Duration::from_secs(3_600),
    )
    .encode();
    // A token naming this ARN but another creation instant names a stream this account and
    // region do not hold: a name a delete released and a later create took again.
    let other_incarnation = ShardPageToken::new(
        &arn,
        emulator.stream_created_at("refusals", stream) - Duration::from_secs(60),
        &shard_id(1),
        SystemTime::now(),
    )
    .encode();

    let cases = [
        RefusalCase {
            label: "no stream identifier at all",
            body: json!({}),
            expected_code: "InvalidArgumentException",
            expected_message: Some(STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string()),
        },
        RefusalCase {
            label: "a StreamName and a StreamARN naming different streams",
            body: json!({ "StreamName": stream, "StreamARN": other_arn }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(stream_identifier_mismatch_message(stream, &other_arn)),
        },
        RefusalCase {
            label: "a stream name this account and region do not hold",
            body: json!({ "StreamName": "never-created" }),
            expected_code: "ResourceNotFoundException",
            expected_message: Some(stream_not_found_message("never-created")),
        },
        RefusalCase {
            label: "a StreamARN this account and region do not hold",
            body: json!({ "StreamARN": format!("{arn}-elsewhere") }),
            expected_code: "ResourceNotFoundException",
            expected_message: None,
        },
        RefusalCase {
            label: "an ExclusiveStartShardId that is no shard of the stream",
            body: json!({ "StreamName": stream, "ExclusiveStartShardId": shard_id(99) }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "an AFTER_SHARD_ID filter naming no shard of the stream",
            body: json!({
                "StreamName": stream,
                "ShardFilter": { "Type": "AFTER_SHARD_ID", "ShardId": shard_id(99) },
            }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "an AFTER_SHARD_ID filter carrying no ShardId",
            body: json!({ "StreamName": stream, "ShardFilter": { "Type": "AFTER_SHARD_ID" } }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a ShardId on a filter type that does not admit one",
            body: json!({
                "StreamName": stream,
                "ShardFilter": { "Type": "AT_LATEST", "ShardId": shard_id(0) },
            }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a Timestamp on a filter type that does not admit one",
            body: json!({
                "StreamName": stream,
                "ShardFilter": { "Type": "AT_TRIM_HORIZON", "Timestamp": 1.0_f64 },
            }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "an AT_TIMESTAMP filter carrying no Timestamp",
            body: json!({ "StreamName": stream, "ShardFilter": { "Type": "AT_TIMESTAMP" } }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a FROM_TIMESTAMP filter carrying no Timestamp",
            body: json!({ "StreamName": stream, "ShardFilter": { "Type": "FROM_TIMESTAMP" } }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a NextToken this service never issued",
            body: json!({ "NextToken": "not-a-token" }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(INVALID_NEXT_TOKEN_MESSAGE.to_string()),
        },
        RefusalCase {
            label: "a NextToken past its documented lifetime",
            body: json!({ "NextToken": expired }),
            expected_code: "ExpiredNextTokenException",
            expected_message: Some(next_token_expired_message()),
        },
        RefusalCase {
            label: "a NextToken minted at an instant that has not arrived",
            body: json!({ "NextToken": future }),
            expected_code: "ExpiredNextTokenException",
            expected_message: Some(next_token_expired_message()),
        },
        RefusalCase {
            label: "a NextToken naming another incarnation of the same stream name",
            body: json!({ "NextToken": other_incarnation }),
            expected_code: "ResourceNotFoundException",
            expected_message: None,
        },
        RefusalCase {
            label: "a live NextToken paired with the StreamName it already identifies",
            body: json!({ "NextToken": live, "StreamName": stream }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a live NextToken paired with an ExclusiveStartShardId",
            body: json!({ "NextToken": live, "ExclusiveStartShardId": shard_id(0) }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a live NextToken paired with a StreamCreationTimestamp",
            body: json!({ "NextToken": live, "StreamCreationTimestamp": 1.0_f64 }),
            expected_code: "InvalidArgumentException",
            expected_message: None,
        },
        RefusalCase {
            label: "a live NextToken paired with a StreamARN naming another stream",
            body: json!({ "NextToken": live, "StreamARN": other_arn }),
            expected_code: "InvalidArgumentException",
            expected_message: Some(NEXT_TOKEN_STREAM_MISMATCH_MESSAGE.to_string()),
        },
        RefusalCase {
            label: "a StreamCreationTimestamp that is not this stream's",
            body: json!({ "StreamName": stream, "StreamCreationTimestamp": 1.0_f64 }),
            expected_code: "ResourceNotFoundException",
            // The refusal names the creation time the request asked for, which is the
            // member that narrowed the lookup past the name.
            expected_message: Some(stream_creation_time_not_found_message(stream, 1)),
        },
    ];

    for case in cases {
        let (status, response) = emulator.call("ListShards", &case.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the modelled client refusal, got {status} with body \
             {response}",
            case.label
        );
        assert_eq!(
            error_code(&response),
            case.expected_code,
            "case {:?}: expected {}, got {response}",
            case.label,
            case.expected_code
        );
        if let Some(expected) = &case.expected_message {
            assert_eq!(
                &error_message(&response),
                expected,
                "case {:?}: expected the documented message, got {response}",
                case.label
            );
        }
    }

    // The live token is still a usable cursor on its own: the refusals above are about the
    // members paired with it, not the token.
    let resumed = emulator
        .list("a live NextToken on its own", &json!({ "NextToken": live }))
        .await;
    assert_eq!(
        shard_ids(&resumed),
        vec![shard_id(1), shard_id(2)],
        "a live NextToken must resume at the shard it names, got {resumed}"
    );
}

// 5. A stream that is not ACTIVE yet has no shard map to serve
#[tokio::test]
async fn a_stream_that_is_not_active_yet_is_refused_until_the_transition_falls_due() {
    let emulator = Emulator::new();
    let stream = "not-ready";
    emulator
        .ok(
            "not ready",
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 2 }),
        )
        .await;

    let (status, response) = emulator
        .call("ListShards", &json!({ "StreamName": stream }))
        .await;
    assert_eq!(
        status, 400,
        "case creating: a stream whose creation has not fallen due must be refused, got \
         {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case creating: expected ResourceInUseException, got {response}"
    );
    assert_eq!(
        error_message(&response),
        stream_not_active_message(stream, STREAM_CREATING),
        "case creating: expected the documented message, got {response}"
    );

    tokio::time::sleep(past_activation()).await;
    let answer = emulator
        .list("active", &json!({ "StreamName": stream }))
        .await;
    assert_eq!(
        shard_ids(&answer),
        vec![shard_id(0), shard_id(1)],
        "case active: once the transition has fallen due the whole shard map is served, got \
         {answer}"
    );
}

// 6. A stream is listed only in the region it was created in
#[tokio::test]
async fn a_stream_is_listed_only_in_the_region_it_was_created_in() {
    let emulator = Emulator::new();
    let stream = "regional";
    emulator.active_stream("regional", stream, Some(2)).await;

    for region in ["us-west-2", "eu-central-1"] {
        let (status, response) = emulator
            .call_in(region, "ListShards", &json!({ "StreamName": stream }))
            .await;
        assert_eq!(
            status, 400,
            "case {region}: a stream of another region must not be listed, got {status} with \
             body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {region}: expected ResourceNotFoundException, got {response}"
        );
    }

    // The same name in another region is another stream, with its own shard map.
    let elsewhere = json!({ "StreamName": stream, "ShardCount": 5 });
    let (status, response) = emulator
        .call_in("us-west-2", "CreateStream", &elsewhere)
        .await;
    assert_eq!(
        status, 200,
        "case us-west-2: the same name in another region is another stream, got {status} with \
         body {response}"
    );
    tokio::time::sleep(past_activation()).await;
    let (status, listed) = emulator
        .call_in("us-west-2", "ListShards", &json!({ "StreamName": stream }))
        .await;
    assert_eq!(
        status, 200,
        "case us-west-2: expected the other region's stream to be listed, got {status} with \
         body {listed}"
    );
    assert_eq!(
        shard_ids(&listed).len(),
        5,
        "case us-west-2: the other region's stream holds its own five shards, got {listed}"
    );
}

// 7. `ExclusiveStartShardId` and a matching `StreamCreationTimestamp`
#[tokio::test]
async fn an_exclusive_start_shard_id_starts_after_it_and_a_matching_creation_timestamp_lists() {
    let emulator = Emulator::new();
    let stream = "cursors";
    emulator.active_stream("cursors", stream, Some(4)).await;

    for (start, expected) in [
        (shard_id(0), vec![shard_id(1), shard_id(2), shard_id(3)]),
        (shard_id(2), vec![shard_id(3)]),
        (shard_id(3), Vec::new()),
    ] {
        let answer = emulator
            .list(
                "exclusive start",
                &json!({ "StreamName": stream, "ExclusiveStartShardId": start }),
            )
            .await;
        assert_eq!(
            shard_ids(&answer),
            expected,
            "case start after {start}: expected {expected:?}, got {answer}"
        );
    }

    // The creation timestamp the stream itself reports resolves it, while any other one
    // names a stream this account and region do not hold (asserted in the refusal table).
    let summary = emulator
        .ok(
            "creation timestamp",
            "DescribeStreamSummary",
            &json!({ "StreamName": stream }),
        )
        .await;
    let created = summary["StreamDescriptionSummary"]["StreamCreationTimestamp"].clone();
    let answer = emulator
        .list(
            "creation timestamp",
            &json!({ "StreamName": stream, "StreamCreationTimestamp": created }),
        )
        .await;
    assert_eq!(
        shard_ids(&answer).len(),
        4,
        "case creation timestamp: the stream's own creation timestamp resolves it, got {answer}"
    );
}

// 8. Listing is a READ: it changes nothing a later caller sees
#[tokio::test]
async fn listing_shards_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let stream = "read-only";
    emulator.active_stream("read only", stream, Some(5)).await;

    let before = emulator
        .ok(
            "read only",
            "DescribeStream",
            &json!({ "StreamName": stream }),
        )
        .await;
    // Page the whole map through a one-shard page size, which mints a token per page.
    let (listed, _) = page_through(
        &emulator,
        "read only",
        &json!({ "StreamName": stream, "MaxResults": 1 }),
        5,
    )
    .await;
    assert_eq!(
        listed.len(),
        5,
        "case read only: the whole map must page through, got {listed:?}"
    );
    let after = emulator
        .ok(
            "read only",
            "DescribeStream",
            &json!({ "StreamName": stream }),
        )
        .await;
    assert_eq!(
        before["StreamDescription"], after["StreamDescription"],
        "case read only: listing shards must leave the stored stream exactly as it was"
    );
}
