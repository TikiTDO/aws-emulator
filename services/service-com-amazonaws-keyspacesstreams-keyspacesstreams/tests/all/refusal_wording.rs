// SPDX-License-Identifier: CC0-1.0

//! The WORDS this service refuses a request with, driven end to end over the real wire
//! path (an awsJson1_0 request through the multi-service router into the mounted service),
//! so what is asserted is the error envelope a client parses.
//!
//! THE RULES UNDER TEST, one per class of refusal:
//!
//! * a `shardIterator` this service did not issue names no position to read from, so it is
//!   an input-FORMAT failure — HTTP 400 `ValidationException` with `errorCode`
//!   `InvalidFormat` — and never the resource-missing bucket;
//! * a stream or shard this service holds no record of is `ResourceNotFoundException` with
//!   Amazon Keyspaces's fixed not-found sentence;
//! * a parameter value, or a combination of them, the operation does not accept is
//!   `ValidationException` with Amazon Keyspaces's fixed invalid-parameter sentence, which
//!   the `errorCode` member classifies.
//!
//! Each sentence is FIXED text: it names no member, no value and no identifier of the
//! request, so the tables below assert that nothing of the request — a keyspace or table
//! name, a shard id, a sequence number, an iterator, a stream ARN — appears in it, over a
//! set of requests that vary every one of those values.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::ops::types::{
    KeyspacesCellValue, SHARD_ITERATOR_TYPE_VALUES,
};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::state::{service_state, ChangeRecord};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Amazon Keyspaces's fixed sentence for a resource its stream APIs hold no record of.
const RESOURCE_NOT_FOUND: &str = "Requested resource not found";

/// Amazon Keyspaces's fixed sentence for a parameter value its stream APIs do not accept.
const INVALID_PARAMETER_VALUES: &str = "One or more parameter values were invalid";

/// Amazon Keyspaces's fixed sentence for a `shardIterator` it did not issue.
const INVALID_SHARD_ITERATOR: &str = "Invalid ShardIterator";

/// The Region the seeded streams belong to; the calls below are unsigned, so they name
/// none and every stream is visible to them.
const REGION: &str = "us-east-1";

/// The emulator under test: one shared engine, one service.
fn emulator() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// One awsJson1_0 call, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://cassandra-streams.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("KeyspacesStreams.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One captured change to a row, as the table's write path records it.
fn change(key: &str) -> ChangeRecord {
    let mut partition_keys = BTreeMap::new();
    partition_keys.insert(
        "id".to_string(),
        KeyspacesCellValue::textT(key.to_string()),
    );
    ChangeRecord {
        origin: "USER".to_string(),
        partition_keys,
        clustering_keys: BTreeMap::new(),
        new_image: None,
        old_image: None,
    }
}

/// One seeded stream: its ARN, its shard, a sequence number it really holds, and an
/// iterator at the start of that shard — all read back from the store, so no case depends
/// on how an identifier happens to be minted.
struct Seeded {
    stream_arn: String,
    shard_id: String,
    starting_sequence_number: String,
    iterator: String,
}

/// Enable CDC for one table, capture two changes on it and register an iterator at the
/// start of its shard, through `crate::state`'s write side — the Keyspaces control plane's
/// seam, and the registry `GetShardIterator` mints into.
fn given_stream(
    engine: &Arc<engine::InMemoryEngine>,
    keyspace_name: &str,
    table_name: &str,
) -> Seeded {
    let state = service_state(&**engine).expect("this service's store");
    let mut state = state.lock();
    let stream_arn = state
        .enable_stream(REGION, keyspace_name, table_name, "NEW_IMAGE")
        .expect("a modelled view type is accepted");
    for index in 0..2 {
        assert!(
            state
                .append_change_record(&stream_arn, change(&format!("r{index}")))
                .is_some(),
            "seeding {keyspace_name}.{table_name}: record {index} must be captured"
        );
    }
    let shard = state
        .stream(&stream_arn)
        .expect("the stream just enabled")
        .shards
        .first()
        .expect("a freshly enabled stream has one shard")
        .clone();
    let iterator = state.mint_shard_iterator(&stream_arn, &shard.shard_id, 0);
    Seeded {
        stream_arn,
        shard_id: shard.shard_id.clone(),
        starting_sequence_number: shard.starting_sequence_number.clone(),
        iterator,
    }
}

/// Every value of the request that must NOT appear in a refusal's message: the names the
/// stream was created with, and the identifiers the request itself carries.
fn request_values(seeded: &Seeded, keyspace_name: &str, table_name: &str) -> Vec<String> {
    vec![
        keyspace_name.to_string(),
        table_name.to_string(),
        seeded.stream_arn.clone(),
        seeded.shard_id.clone(),
        seeded.starting_sequence_number.clone(),
        seeded.iterator.clone(),
    ]
}

/// The refusal a case expects: the HTTP status, the wire error code, the `errorCode`
/// member when the class carries one, and the exact sentence.
#[derive(Debug, Clone, Copy)]
struct Expected {
    status: u16,
    code: &'static str,
    error_code: Option<&'static str>,
    message: &'static str,
}

impl Expected {
    /// The input-format refusal of a `shardIterator` this service did not issue.
    fn invalid_shard_iterator() -> Self {
        Self {
            status: 400,
            code: "ValidationException",
            error_code: Some("InvalidFormat"),
            message: INVALID_SHARD_ITERATOR,
        }
    }

    /// The refusal of a resource this service holds no record of.
    fn not_found() -> Self {
        Self {
            status: 404,
            code: "ResourceNotFoundException",
            error_code: None,
            message: RESOURCE_NOT_FOUND,
        }
    }

    /// The refusal of a parameter value the operation does not accept, classified by the
    /// `ValidationExceptionType` it carries.
    fn invalid_parameters(error_code: &'static str) -> Self {
        Self {
            status: 400,
            code: "ValidationException",
            error_code: Some(error_code),
            message: INVALID_PARAMETER_VALUES,
        }
    }
}

/// Assert one refusal: the status, the wire error code, the `errorCode` member when the
/// class carries one, the exact sentence, and that no value of the request leaked into it.
fn assert_refusal(
    case: &str,
    status: u16,
    body: &Value,
    expected: Expected,
    request_values: &[String],
) {
    let Expected {
        status: expected_status,
        code: expected_code,
        error_code: expected_error_code,
        message: expected_message,
    } = expected;
    assert_eq!(
        status, expected_status,
        "case {case}: expected HTTP {expected_status}, got {status} {body}"
    );
    assert_eq!(
        body["__type"], expected_code,
        "case {case}: expected {expected_code}, got {body}"
    );
    match expected_error_code {
        Some(expected_error_code) => assert_eq!(
            body["errorCode"], expected_error_code,
            "case {case}: expected errorCode {expected_error_code}, got {body}"
        ),
        None => assert!(
            body.get("errorCode").is_none(),
            "case {case}: this error class carries no errorCode member, got {body}"
        ),
    }
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case}: expected the fixed sentence {expected_message:?}, got {body}"
    );
    for value in request_values {
        assert!(
            !message.contains(value.as_str()),
            "case {case}: the sentence interpolates nothing of the request, but {value:?} is in \
             {message:?}"
        );
    }
}

/// RULE: a `shardIterator` this service did not issue is an input-format failure — 400
/// `ValidationException`, `errorCode` `InvalidFormat`, with Amazon Keyspaces's fixed
/// sentence — whatever shape the presented value has, while an iterator it DID issue is
/// served.
#[tokio::test]
async fn an_iterator_this_service_did_not_issue_is_an_input_format_failure() {
    let (engine, router) = emulator();
    let seeded = given_stream(&engine, "shop", "orders");
    let values = request_values(&seeded, "shop", "orders");

    // The class spans every way a value of `ShardIterator`'s length bound (1..4096) can
    // fail to be an iterator this service holds: arbitrary text, the shortest and the
    // longest value the model admits, a value shaped like this service's own tokens, one
    // that differs from a real iterator in a single character, and another service's
    // iterator spelling.
    let real = seeded.iterator.clone();
    let mut nearly = real.clone();
    nearly.pop();
    nearly.push(if real.ends_with('a') { 'b' } else { 'a' });
    let cases: Vec<(&str, String)> = vec![
        (
            "arbitrary text",
            format!("not-a-real-iterator-{}", "x".repeat(40)),
        ),
        ("the shortest value the model admits", "x".to_string()),
        (
            "the longest value the model admits",
            "x".repeat(4096),
        ),
        (
            "this service's own token shape, never minted",
            format!("ksstreams-iterator-{}", "0".repeat(80)),
        ),
        ("one character off a real iterator", nearly),
        (
            "another service's iterator spelling",
            format!("arn:aws:dynamodb:{REGION}:000000000000:table/orders/stream/1|1|AAAA"),
        ),
    ];

    for (case, shard_iterator) in cases {
        let (status, body) = call(
            &router,
            "GetRecords",
            &json!({ "shardIterator": shard_iterator }),
        )
        .await;
        assert_refusal(
            case,
            status,
            &body,
            Expected::invalid_shard_iterator(),
            &values,
        );
    }

    // The iterator this service DID issue still reads the shard, so the rule above refuses
    // the request rather than the operation.
    let (status, body) = call(&router, "GetRecords", &json!({ "shardIterator": real })).await;
    assert_eq!(
        status, 200,
        "case \"a real iterator\": an iterator this service issued is served, got {status} {body}"
    );
    assert_eq!(
        body["changeRecords"].as_array().map(Vec::len),
        Some(2),
        "case \"a real iterator\": the read serves what the stream captured, got {body}"
    );
}

/// RULE: a stream or shard this service holds no record of is `ResourceNotFoundException`
/// with Amazon Keyspaces's fixed not-found sentence, interpolating nothing of the request —
/// so the sentence is the same whichever resource, and whichever operation, named it.
#[tokio::test]
async fn a_resource_this_service_does_not_hold_is_the_fixed_not_found() {
    let (engine, router) = emulator();
    let seeded = given_stream(&engine, "shop", "orders");
    let values = request_values(&seeded, "shop", "orders");

    // A well-formed stream ARN of a table nothing ever created, and one of a keyspace
    // nothing ever created: the class is "this service holds no such resource", not one
    // spelling of an absent one.
    let absent_table_arn = ::engine::emulator_arn(
        "cassandra",
        REGION,
        "/keyspace/shop/table/absent/stream/2026-10-30T07:59:02.000",
    );
    let absent_keyspace_arn = ::engine::emulator_arn(
        "cassandra",
        REGION,
        "/keyspace/absent/table/orders/stream/2026-10-30T07:59:02.000",
    );
    let absent_shard = format!("shardId-{:020}-deadbeef", 1_u64);

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "GetStream on a stream of a table that was never created",
            "GetStream",
            json!({ "streamArn": absent_table_arn }),
        ),
        (
            "GetStream on a stream of a keyspace that was never created",
            "GetStream",
            json!({ "streamArn": absent_keyspace_arn }),
        ),
        (
            "GetShardIterator on a stream that was never created",
            "GetShardIterator",
            json!({
                "streamArn": absent_table_arn,
                "shardId": seeded.shard_id,
                "shardIteratorType": "TRIM_HORIZON",
            }),
        ),
        (
            "GetShardIterator on a shard the stream does not hold",
            "GetShardIterator",
            json!({
                "streamArn": seeded.stream_arn,
                "shardId": absent_shard,
                "shardIteratorType": "LATEST",
            }),
        ),
        (
            "GetStream filtering on a parent shard the stream does not hold",
            "GetStream",
            json!({
                "streamArn": seeded.stream_arn,
                "shardFilter": { "type": "CHILD_SHARDS", "shardId": absent_shard },
            }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_refusal(case, status, &body, Expected::not_found(), &values);
    }

    // The stream this service DOES hold is still described, so the rule refuses the
    // request rather than the operation.
    let (status, body) = call(
        &router,
        "GetStream",
        &json!({ "streamArn": seeded.stream_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the stream this service holds\": it must still be described, got {status} {body}"
    );
}

/// RULE: a parameter value, or a combination of them, the operation does not accept is
/// `ValidationException` with Amazon Keyspaces's fixed invalid-parameter sentence — the
/// same sentence whichever parameter is at fault, with the `errorCode` member carrying the
/// class — while the same request with an acceptable value is served.
#[tokio::test]
async fn a_parameter_value_the_operation_does_not_accept_is_the_fixed_invalid_parameters() {
    let (engine, router) = emulator();
    let seeded = given_stream(&engine, "shop", "orders");
    let values = request_values(&seeded, "shop", "orders");

    // One below the shard's own starting sequence number, read back from the shard rather
    // than assumed, so the case stays "older than the shard retains" whatever order ids
    // are minted in.
    let starting: u64 = seeded
        .starting_sequence_number
        .parse()
        .expect("a sequence number is decimal");
    assert!(
        starting > 0,
        "the shard's starting sequence number must leave room below it, got {starting}"
    );
    let trimmed = format!("{:021}", starting - 1);
    // Inside `SequenceNumber`'s 21..48 length bound, and above every sequence number this
    // store has minted, so the shard never held it.
    let never_held = "9".repeat(21);

    // (case, operation, request, the ValidationExceptionType the refusal carries)
    let mut cases: Vec<(String, &str, Value, &str)> = vec![
        (
            "ListStreams with a table name and no keyspace name".to_string(),
            "ListStreams",
            json!({ "tableName": "orders" }),
            "InvalidFormat",
        ),
        (
            "GetStream with a shard filter carrying no type".to_string(),
            "GetStream",
            json!({ "streamArn": seeded.stream_arn, "shardFilter": { "shardId": seeded.shard_id } }),
            "InvalidFormat",
        ),
        (
            "GetStream with a child-shard filter naming no parent".to_string(),
            "GetStream",
            json!({ "streamArn": seeded.stream_arn, "shardFilter": { "type": "CHILD_SHARDS" } }),
            "InvalidFormat",
        ),
        (
            "GetShardIterator at a sequence number the shard never held".to_string(),
            "GetShardIterator",
            json!({
                "streamArn": seeded.stream_arn,
                "shardId": seeded.shard_id,
                "shardIteratorType": "AT_SEQUENCE_NUMBER",
                "sequenceNumber": never_held,
            }),
            "InvalidFormat",
        ),
        (
            "GetShardIterator at a sequence number the shard has trimmed".to_string(),
            "GetShardIterator",
            json!({
                "streamArn": seeded.stream_arn,
                "shardId": seeded.shard_id,
                "shardIteratorType": "AT_SEQUENCE_NUMBER",
                "sequenceNumber": trimmed,
            }),
            "TrimmedDataAccess",
        ),
    ];
    // Both `ShardIteratorType` values the model documents a `sequenceNumber` as required
    // for — the crate's generated catalog of the model's enum, so the table cannot drift
    // from it — with that member omitted.
    for iterator_type in SHARD_ITERATOR_TYPE_VALUES
        .iter()
        .filter(|value| value.ends_with("SEQUENCE_NUMBER"))
    {
        cases.push((
            format!("GetShardIterator {iterator_type} with no sequenceNumber"),
            "GetShardIterator",
            json!({
                "streamArn": seeded.stream_arn,
                "shardId": seeded.shard_id,
                "shardIteratorType": iterator_type,
            }),
            "InvalidFormat",
        ));
    }

    for (case, operation, request, error_code) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_refusal(
            &case,
            status,
            &body,
            Expected::invalid_parameters(error_code),
            &values,
        );
    }

    // The same operations with acceptable values are served, so the rule refuses the
    // parameter and not the operation.
    for (case, operation, request) in [
        (
            "ListStreams filtered by keyspace and table",
            "ListStreams",
            json!({ "keyspaceName": "shop", "tableName": "orders" }),
        ),
        (
            "GetStream filtering on a parent shard the stream holds",
            "GetStream",
            json!({
                "streamArn": seeded.stream_arn,
                "shardFilter": { "type": "CHILD_SHARDS", "shardId": seeded.shard_id },
            }),
        ),
        (
            "GetShardIterator at a sequence number the shard holds",
            "GetShardIterator",
            json!({
                "streamArn": seeded.stream_arn,
                "shardId": seeded.shard_id,
                "shardIteratorType": "AT_SEQUENCE_NUMBER",
                "sequenceNumber": seeded.starting_sequence_number,
            }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an acceptable value must be served, got {status} {body}"
        );
    }
}
