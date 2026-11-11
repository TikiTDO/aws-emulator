// SPDX-License-Identifier: CC0-1.0

//! The request-parameter bounds this service enforces BEYOND what its model
//! declares, and the refusal a value outside one of them draws.
//!
//! The rule pinned here: a parameter value outside a bound the service enforces but
//! the model does not express is REFUSED — HTTP 400, error code
//! `ValidationException`, in that operation's own fixed wording — and never quietly
//! clamped to the bound or served as sent. The bounds:
//!
//! * `DescribeStreamInput$Limit` — "The maximum number of shard objects to return.
//!   The upper limit is 100."
//! * `GetRecordsInput$Limit` — "The maximum number of records to return from the
//!   shard. The upper limit is 1000."
//! * `GetShardIteratorInput$Timestamp` — "If the timestamp is in the future, the
//!   request returns a ValidationException."
//!
//! Each `Limit` row set spans the class rather than one value: the smallest the
//! declared range allows, a value inside the bound, the published bound itself, one
//! past it, and far past it (up to the widest integer the member carries). The
//! timestamp rows span the clock: instants well before, just before and at the
//! current second, one second ahead, and far ahead — plus the two neighbouring cases
//! the same member documents as NOT refusals, a timestamp older than the trim
//! horizon and a future timestamp sent with an iterator type that does not read it.
//!
//! The two `Limit` sentences are deliberately different texts: each operation words
//! its own cap, so they are asserted separately and never through one template.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;
use smithy_server_runtime::MultiServiceRouter;
use streams::ops::types::{AttributeValue, KeySchemaElement};
use streams::state::{
    AttributeMap, ComAmazonawsDynamodbstreamsDynamodbstreams20120810State as StreamsState,
    OperationType, RecordAppend, StreamRegistration, StreamViewType, MAX_GET_RECORDS_LIMIT,
    MAX_PAGE_LIMIT,
};

/// A SigV4 credential scope naming `dynamodb`, which is how a DynamoDB Streams
/// request is signed. Request authentication is off here, so only its routing signal
/// matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

const REGION: &str = "us-east-1";

/// The whole refusal `DescribeStream` answers for a `Limit` above its cap.
const DESCRIBE_STREAM_LIMIT_REFUSAL: &str =
    "One or more parameter values were invalid: Limit cannot be greater than the maximum: 100";

/// The whole refusal `GetRecords` answers for a `Limit` above its cap — a different
/// sentence for the same kind of bound.
const GET_RECORDS_LIMIT_REFUSAL: &str =
    "One or more parameter values were invalid: Limit should be less than or equal to 1000";

/// The whole refusal `GetShardIterator` answers for an `AT_TIMESTAMP` start point
/// that has not happened yet.
const TIMESTAMP_REFUSAL: &str = "One or more parameter values were invalid: \
     Timestamp parameter cannot be greater than the current timestamp";

fn build() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(streams::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header(
            "x-amz-target",
            format!("DynamoDBStreams_20120810.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// The wire error code of a refusal, as an SDK client matches it.
fn error_code(body: &serde_json::Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carries, under either of the casings a client reads it by.
fn message(body: &serde_json::Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Assert one row that the rule says is refused: HTTP 400, `ValidationException`,
/// and the operation's own whole sentence.
fn assert_refused(
    case: &str,
    status: u16,
    body: &serde_json::Value,
    expected_message: &str,
) {
    assert_eq!(
        status, 400,
        "case {case}: a parameter outside the service's bound must be refused with \
         400, got {status} and {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected error code ValidationException, got {body}"
    );
    assert_eq!(
        message(body),
        expected_message,
        "case {case}: expected message {expected_message:?}, got {:?}",
        message(body)
    );
}

fn seconds_ago(seconds: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(now - seconds)
}

fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the instant is after the Unix epoch")
        .as_secs()
}

fn register(engine: &engine::InMemoryEngine, table_name: &str, at: SystemTime) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .register_stream(
            &StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type: StreamViewType::NewImage,
                key_schema: vec![KeySchemaElement {
                    attribute_name: "Id".to_string(),
                    key_type: "HASH".to_string(),
                }],
                region: REGION.to_string(),
            },
            at,
        )
        .expect("the registry accepts the registration")
}

fn keys_of(id: &str) -> AttributeMap {
    HashMap::from([("Id".to_string(), AttributeValue::S(id.to_string()))])
}

/// Publish one `INSERT` onto `stream_arn` — the seam DynamoDB's item writes call.
fn append(engine: &engine::InMemoryEngine, stream_arn: &str, id: &str, at: SystemTime) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .append_record(
            stream_arn,
            &RecordAppend {
                event_name: OperationType::Insert,
                keys: keys_of(id),
                new_image: Some(keys_of(id)),
                old_image: None,
            },
            at,
        )
        .unwrap_or_else(|rejected| panic!("the registry must take the record, got {rejected}"))
}

/// Close the stream's open shard and open a child on it, so the stream serves more
/// than one shard to page over.
fn roll_shard(engine: &engine::InMemoryEngine, stream_arn: &str, at: SystemTime) {
    engine
        .state::<StreamsState>()
        .lock()
        .roll_shard(stream_arn, at)
        .expect("the stream is registered and has an open shard");
}

/// Every shard id `DescribeStream` reports for the stream, unpaged.
async fn all_shard_ids(router: &MultiServiceRouter, stream_arn: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream answered {status}: {body}");
    body["StreamDescription"]["Shards"]
        .as_array()
        .unwrap_or_else(|| panic!("no Shards in {body}"))
        .iter()
        .map(|shard| {
            shard["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("no ShardId in {shard}"))
                .to_string()
        })
        .collect()
}

/// `DescribeStream` honours a `Limit` up to its published cap and REFUSES anything
/// above it, rather than narrowing the page silently.
#[tokio::test]
async fn describe_stream_refuses_a_limit_above_its_published_cap() {
    let (engine, router) = build();
    let base = seconds_ago(3_600);
    let stream_arn = register(&engine, "ledger", base);
    // Four shards: three rolled-over (closed) ones and the open child, so a page
    // narrower than the shard list is observably truncated.
    for index in 0..3_u64 {
        append(&engine, &stream_arn, &format!("item-{index}"), base);
        roll_shard(&engine, &stream_arn, base + Duration::from_secs(index + 1));
    }
    let shards = all_shard_ids(&router, &stream_arn).await;
    assert_eq!(
        shards.len(),
        4,
        "the fixture must serve four shards, got {shards:?}"
    );

    let cap = i64::from(MAX_PAGE_LIMIT);
    // Each row: the `Limit` sent, and the page size the rule requires — or None
    // where the rule requires a refusal. The published bound, one past it, far past
    // it, and the widest value the member carries.
    let cases: [(&str, i64, Option<usize>); 7] = [
        ("Limit 1, the smallest the declared range allows", 1, Some(1)),
        ("Limit 3, inside both the cap and the shard list", 3, Some(3)),
        ("Limit 50, inside the cap but past the shard list", 50, Some(4)),
        ("Limit 100, the published cap", cap, Some(4)),
        ("Limit 101, one past the cap", cap + 1, None),
        ("Limit 5000, far past the cap", 5_000, None),
        (
            "Limit 2147483647, the widest the member carries",
            i64::from(i32::MAX),
            None,
        ),
    ];

    for (case, limit, expected) in cases {
        let (status, body) = call(
            &router,
            "DescribeStream",
            serde_json::json!({ "StreamArn": stream_arn, "Limit": limit }),
        )
        .await;
        match expected {
            Some(page_size) => {
                assert_eq!(
                    status, 200,
                    "case {case}: a Limit inside the cap must be served, got {status} and {body}"
                );
                let page: Vec<String> = body["StreamDescription"]["Shards"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: no Shards in {body}"))
                    .iter()
                    .map(|shard| shard["ShardId"].as_str().unwrap_or_default().to_string())
                    .collect();
                assert_eq!(
                    page,
                    shards[..page_size].to_vec(),
                    "case {case}: expected the first {page_size} shards, got {page:?}"
                );
            }
            None => assert_refused(case, status, &body, DESCRIBE_STREAM_LIMIT_REFUSAL),
        }
    }
}

/// `GetRecords` honours a `Limit` up to its published cap and REFUSES anything above
/// it, rather than serving a page of a size AWS would never serve.
#[tokio::test]
async fn get_records_refuses_a_limit_above_its_published_cap() {
    let (engine, router) = build();
    let base = seconds_ago(3_600);
    let stream_arn = register(&engine, "ledger", base);
    let appended: Vec<String> = (0..5_u64)
        .map(|index| {
            append(
                &engine,
                &stream_arn,
                &format!("item-{index}"),
                base + Duration::from_secs(index),
            )
        })
        .collect();
    let shard_id = all_shard_ids(&router, &stream_arn)
        .await
        .first()
        .cloned()
        .expect("the stream serves a shard");

    let cap = i64::from(MAX_GET_RECORDS_LIMIT);
    // Each row: the `Limit` sent, and the number of records the rule requires — or
    // None where the rule requires a refusal.
    let cases: [(&str, i64, Option<usize>); 7] = [
        ("Limit 1, the smallest the declared range allows", 1, Some(1)),
        ("Limit 4, inside both the cap and the shard", 4, Some(4)),
        ("Limit 999, just inside the cap", cap - 1, Some(5)),
        ("Limit 1000, the published cap", cap, Some(5)),
        ("Limit 1001, one past the cap", cap + 1, None),
        ("Limit 50000, far past the cap", 50_000, None),
        (
            "Limit 2147483647, the widest the member carries",
            i64::from(i32::MAX),
            None,
        ),
    ];

    for (case, limit, expected) in cases {
        // A fresh iterator per row, so no row depends on another row's position.
        let (status, body) = call(
            &router,
            "GetShardIterator",
            serde_json::json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: GetShardIterator answered {status}: {body}"
        );
        let iterator = body["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no ShardIterator in {body}"))
            .to_string();

        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": iterator, "Limit": limit }),
        )
        .await;
        match expected {
            Some(count) => {
                assert_eq!(
                    status, 200,
                    "case {case}: a Limit inside the cap must be served, got {status} and {body}"
                );
                let delivered: Vec<String> = body["Records"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: no Records in {body}"))
                    .iter()
                    .map(|record| {
                        record["dynamodb"]["SequenceNumber"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert_eq!(
                    delivered,
                    appended[..count].to_vec(),
                    "case {case}: expected the first {count} records, got {delivered:?}"
                );
            }
            None => assert_refused(case, status, &body, GET_RECORDS_LIMIT_REFUSAL),
        }
    }
}

/// `AT_TIMESTAMP` positions at a past instant and REFUSES one that has not happened
/// yet, so a client reading from a skewed clock is told its start point is invalid
/// instead of being handed an iterator it never asked for.
#[tokio::test]
async fn get_shard_iterator_refuses_an_at_timestamp_in_the_future() {
    let (engine, router) = build();
    let base = seconds_ago(3_600);
    let stream_arn = register(&engine, "ledger", base);
    let appended: Vec<String> = (0..3_u64)
        .map(|index| {
            append(
                &engine,
                &stream_arn,
                &format!("item-{index}"),
                base + Duration::from_secs(index * 60),
            )
        })
        .collect();
    let shard_id = all_shard_ids(&router, &stream_arn)
        .await
        .first()
        .cloned()
        .expect("the stream serves a shard");

    // Each row: the offset in seconds from the current second, and the records the
    // minted iterator must deliver — or None where the rule requires a refusal. The
    // instant's distance from now is what the rows vary; the sign of that distance
    // is what the rule turns on.
    //
    // The future rows start five seconds out and the current second is read again for
    // each row, so crossing a second boundary between reading the clock here and the
    // handler reading its own cannot turn a future instant into a past one.
    let cases: [(&str, i64, Option<usize>); 7] = [
        ("an hour before now, before every record", -3_600, Some(0)),
        ("a minute before now, after every record", -60, Some(3)),
        ("one second before now", -1, Some(3)),
        ("the current second", 0, Some(3)),
        ("five seconds ahead of now", 5, None),
        ("a minute ahead of now", 60, None),
        ("a day ahead of now", 86_400, None),
    ];

    for (case, offset, expected) in cases {
        // A row at or before the current second is measured from the instant the records
        // above were laid out from, so a slow run cannot carry the start point past a
        // record the row requires to be delivered; a row AHEAD of now is measured from the
        // current second, so it is still ahead of the handler's own reading of the clock.
        let from = if offset < 0 {
            i64::try_from(epoch_secs(base)).unwrap_or(i64::MAX) + 3_600
        } else {
            i64::try_from(epoch_secs(SystemTime::now())).unwrap_or(i64::MAX)
        };
        let timestamp = from + offset;
        let (status, body) = call(
            &router,
            "GetShardIterator",
            serde_json::json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "AT_TIMESTAMP",
                "Timestamp": timestamp,
            }),
        )
        .await;
        match expected {
            Some(skipped) => {
                assert_eq!(
                    status, 200,
                    "case {case}: a past AT_TIMESTAMP must be served, got {status} and {body}"
                );
                let iterator = body["ShardIterator"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: no ShardIterator in {body}"))
                    .to_string();
                let (status, page) = call(
                    &router,
                    "GetRecords",
                    serde_json::json!({ "ShardIterator": iterator }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: GetRecords answered {status}: {page}"
                );
                let delivered: Vec<String> = page["Records"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: no Records in {page}"))
                    .iter()
                    .map(|record| {
                        record["dynamodb"]["SequenceNumber"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert_eq!(
                    delivered,
                    appended[skipped..].to_vec(),
                    "case {case}: the iterator must start at the first record at or after \
                     the timestamp, got {delivered:?}"
                );
            }
            None => assert_refused(case, status, &body, TIMESTAMP_REFUSAL),
        }
    }
}

/// The two neighbouring cases the same member documents as NOT refusals: a timestamp
/// older than the trim horizon positions at the oldest untrimmed record, and a future
/// `Timestamp` sent with an iterator type that does not read it is simply unused.
#[tokio::test]
async fn a_future_timestamp_is_refused_only_where_the_iterator_type_reads_it() {
    let (engine, router) = build();
    let base = seconds_ago(3_600);
    let stream_arn = register(&engine, "ledger", base);
    for index in 0..3_u64 {
        append(
            &engine,
            &stream_arn,
            &format!("item-{index}"),
            base + Duration::from_secs(index),
        );
    }
    let shard_id = all_shard_ids(&router, &stream_arn)
        .await
        .first()
        .cloned()
        .expect("the stream serves a shard");
    let future = i64::try_from(epoch_secs(SystemTime::now())).unwrap_or(i64::MAX) + 86_400;

    // Catalog source: the modelled `ShardIteratorType` enum. Only AT_TIMESTAMP reads
    // `Timestamp`, so only it can be refused for one; the sequence-number forms are
    // left out because they draw their own refusal for a missing SequenceNumber.
    let cases: [(&str, serde_json::Value); 2] = [
        ("TRIM_HORIZON with a future Timestamp", serde_json::json!("TRIM_HORIZON")),
        ("LATEST with a future Timestamp", serde_json::json!("LATEST")),
    ];

    for (case, iterator_type) in cases {
        let (status, body) = call(
            &router,
            "GetShardIterator",
            serde_json::json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": iterator_type,
                "Timestamp": future,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the Timestamp member is unused by this iterator type, so the \
             request stands; got {status} and {body}"
        );
        assert!(
            body["ShardIterator"].is_string(),
            "case {case}: expected a minted iterator, got {body}"
        );
    }

    // A timestamp before the oldest record the shard holds is NOT a refusal: it
    // positions at the trim horizon, which is what the member documents.
    let (status, body) = call(
        &router,
        "GetShardIterator",
        serde_json::json!({
            "StreamArn": stream_arn,
            "ShardId": shard_id,
            "ShardIteratorType": "AT_TIMESTAMP",
            "Timestamp": 0,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a timestamp older than the trim horizon must be served, got {status} and {body}"
    );
    let iterator = body["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("no ShardIterator in {body}"))
        .to_string();
    let (status, page) = call(
        &router,
        "GetRecords",
        serde_json::json!({ "ShardIterator": iterator }),
    )
    .await;
    assert_eq!(status, 200, "GetRecords answered {status}: {page}");
    assert_eq!(
        page["Records"].as_array().map(Vec::len),
        Some(3),
        "an iterator at the trim horizon must deliver every retained record, got {page}"
    );
}
