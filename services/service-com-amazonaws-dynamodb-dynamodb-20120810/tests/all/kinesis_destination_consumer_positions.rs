// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for READING what a table's Kinesis streaming destination delivered:
//! the change records an item write publishes are read back from every starting position
//! the Kinesis data-stream consumer API hands out.
//!
//! The sibling suites pin that a write on a table whose destination is replicating
//! delivers one record per change (`kinesis_destination_record_delivery`) and that it does
//! so whichever operation made the change (`kinesis_destination_delivery`), each reading
//! the stream from its trim horizon. This suite widens the same rule along the axis neither
//! covers — WHERE in the shard the consumer starts — because that is the surface a
//! change-data-capture pipeline is built on. The rules pinned here:
//!
//! 1. the records a replicating destination delivers are readable from EVERY position
//!    `ShardIteratorType` declares
//!    (`smithy_query(query="shape",shape="ShardIteratorType",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")`:
//!    AT_SEQUENCE_NUMBER | AFTER_SEQUENCE_NUMBER | TRIM_HORIZON | LATEST | AT_TIMESTAMP),
//!    each reading exactly the delivered records at or after the position it names;
//! 2. a consumer already positioned at the END of the shard reads the changes made AFTER
//!    it took that position: replication keeps delivering for as long as the destination
//!    replicates, so a long-lived consumer never stops seeing a table's item changes.
//!
//! Each rule is asserted over a SET of inputs — the five positions a consumer may start
//! from, and the three modifications a write makes (`PutItem` inserting, `UpdateItem`
//! editing, `DeleteItem` removing) — rather than one example. Every call is driven over the
//! wire (`POST /` with `X-Amz-Target: <Service>.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, with the DynamoDB and the Kinesis
//! service mounted on ONE shared engine, and nothing seeded through either store: the table
//! is created, the destination enabled and the records read exactly as a client does it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::create_table;
use crate::emulator::{emulator, ensure_kinesis_stream, ok, ok_kinesis};

/// The partition-key attribute of the tables this suite creates
/// (`crate::contributor_insights::create_table`).
const KEY: &str = "Part";

/// The one item every case below changes, so each case's records are the changes of a
/// single item and land in one shard.
const ITEM: &str = "row";

/// How long a loop below waits for a status the clock owes before it fails the case.
///
/// GENEROUS on purpose: the only thing this bound decides is that a condition which never
/// holds fails instead of hanging, so it must not be tight enough for a loaded host to
/// trip it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often those loops look again.
const POLL_EVERY: Duration = Duration::from_millis(50);

/// How many `GetRecords` pages one read walks before the case fails: a bound, so a
/// position that never advances fails rather than looping for ever.
const MAX_PAGES: usize = 16;

/// Wait until `table` serves item writes, polling the reader that reports its status.
async fn table_is_active(router: &MultiServiceRouter, table: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = ok(router, "DescribeTable", json!({"TableName": table})).await;
        let status = described["Table"]["TableStatus"].clone();
        if status == json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case table {table:?}: expected it to reach ACTIVE, last saw {status}"
        );
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Start `table` replicating to `stream_arn` and wait until the destination reports
/// `ACTIVE`, which is the status AWS tells a client to poll for before it expects records
/// ("use DescribeKinesisStreamingDestination to check if streaming to the Kinesis data
/// stream is ACTIVE" — the `EnableKinesisStreamingDestination` doc).
async fn replicate(router: &MultiServiceRouter, table: &str, stream_arn: &str) {
    ok(
        router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": table, "StreamArn": stream_arn}),
    )
    .await;
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let report = ok(
            router,
            "DescribeKinesisStreamingDestination",
            json!({"TableName": table}),
        )
        .await;
        let destination = report["KinesisDataStreamDestinations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|destination| destination["StreamArn"] == json!(stream_arn))
            .unwrap_or(Value::Null);
        let status = destination["DestinationStatus"].clone();
        assert_ne!(
            status,
            json!("ENABLE_FAILED"),
            "case table {table:?}: the enable must not fail for a stream that is there, got \
             {destination}"
        );
        if status == json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case table {table:?}: expected the destination to reach ACTIVE, last saw {status}"
        );
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// The id of the single shard `stream` serves — the shard every record of one item is
/// delivered into.
async fn only_shard(router: &MultiServiceRouter, stream: &str) -> String {
    let listed = ok_kinesis(router, "ListShards", json!({"StreamName": stream})).await;
    let shards = listed["Shards"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        shards.len(),
        1,
        "case stream {stream:?}: the suite drives a one-shard stream, got {listed}"
    );
    shards[0]["ShardId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Mint one shard iterator at the position `position` names, which carries the
/// `ShardIteratorType` and whatever member that type reads its position from.
async fn iterator_at(
    router: &MultiServiceRouter,
    stream: &str,
    shard: &str,
    position: Value,
) -> String {
    let mut request = position;
    request["StreamName"] = json!(stream);
    request["ShardId"] = json!(shard);
    let minted = ok_kinesis(router, "GetShardIterator", request.clone()).await;
    let handed = minted["ShardIterator"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !handed.is_empty(),
        "case {request}: expected a position in the shard, got {minted}"
    );
    handed
}

/// Read from `iterator` until a page comes back empty: every record read, as its sequence
/// number and the JSON document its `Data` blob carries, plus the position the last page
/// handed back for a consumer that keeps reading.
async fn read_from(
    router: &MultiServiceRouter,
    case: &str,
    iterator: String,
) -> (Vec<(String, Value)>, Option<String>) {
    let mut iterator = iterator;
    let mut read: Vec<(String, Value)> = Vec::new();
    for _ in 0..MAX_PAGES {
        let page = ok_kinesis(router, "GetRecords", json!({"ShardIterator": iterator})).await;
        let batch = page["Records"].as_array().cloned().unwrap_or_default();
        for record in &batch {
            let sequence_number = record["SequenceNumber"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every record carries a SequenceNumber, got {record}")
                })
                .to_string();
            let encoded = record["Data"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every record carries Data, got {record}")
            });
            let decoded = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case:?}: a record's Data is base64, got {error}")
            });
            let document: Value = serde_json::from_slice(&decoded).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered record carries JSON, got {error}")
            });
            read.push((sequence_number, document));
        }
        let next = page["NextShardIterator"].as_str().map(str::to_string);
        match next {
            Some(next) if !batch.is_empty() => iterator = next,
            next => return (read, next),
        }
    }
    panic!(
        "case {case:?}: expected the shard's records within {MAX_PAGES} pages, got {} record(s)",
        read.len()
    );
}

/// The modification each read record describes, in the order it was read.
fn events(records: &[(String, Value)]) -> Vec<&str> {
    records
        .iter()
        .map(|(_, document)| document["eventName"].as_str().unwrap_or_default())
        .collect()
}

/// Store the item `ITEM` of `table`, carrying `payload`.
async fn put_item(router: &MultiServiceRouter, table: &str, payload: &str) {
    ok(
        router,
        "PutItem",
        json!({
            "TableName": table,
            "Item": {KEY: {"S": ITEM}, "Payload": {"S": payload}},
        }),
    )
    .await;
}

/// Edit that item in place, so the change is a modification of an item that was there.
async fn update_item(router: &MultiServiceRouter, table: &str, payload: &str) {
    ok(
        router,
        "UpdateItem",
        json!({
            "TableName": table,
            "Key": {KEY: {"S": ITEM}},
            "UpdateExpression": "SET Payload = :payload",
            "ExpressionAttributeValues": {":payload": {"S": payload}},
        }),
    )
    .await;
}

/// Remove it, so the change is a removal.
async fn delete_item(router: &MultiServiceRouter, table: &str) {
    ok(
        router,
        "DeleteItem",
        json!({"TableName": table, "Key": {KEY: {"S": ITEM}}}),
    )
    .await;
}

/// An epoch second a whole day ahead of now — a position past every record this suite
/// delivers, which is what the AT_TIMESTAMP row that reads none is stated over.
fn a_day_ahead() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs()
        + 24 * 60 * 60
}

/// Rule 1: every position a consumer may start from reads the delivered change records
/// that lie at or after it.
#[tokio::test]
async fn every_consumer_position_reads_the_delivered_change_records() {
    let router = emulator();
    let stream = "cdc-positions";
    let stream_arn = ensure_kinesis_stream(&router, stream).await;
    create_table(&router, "Positions", &[]).await;
    table_is_active(&router, "Positions").await;
    replicate(&router, "Positions", &stream_arn).await;
    let shard = only_shard(&router, stream).await;

    // Rule 1's LATEST row has to be minted BEFORE the changes it reads: "start reading just
    // after the most recent record in the shard" names the end of the shard at the instant
    // the position is handed out.
    let pinned_before = iterator_at(
        &router,
        stream,
        &shard,
        json!({"ShardIteratorType": "LATEST"}),
    )
    .await;

    // One of each modification an item write makes, in order.
    put_item(&router, "Positions", "first").await;
    update_item(&router, "Positions", "second").await;
    delete_item(&router, "Positions").await;
    let delivered = ["INSERT", "MODIFY", "REMOVE"];

    // What the shard holds, read from its trim horizon: the sequence numbers the
    // sequence-number positions below are stated over are read off these records.
    let from_horizon = iterator_at(
        &router,
        stream,
        &shard,
        json!({"ShardIteratorType": "TRIM_HORIZON"}),
    )
    .await;
    let (held, _) = read_from(&router, "the whole shard", from_horizon).await;
    assert_eq!(
        events(&held),
        delivered.to_vec(),
        "case \"the whole shard\": expected one record per modification, got {held:?}"
    );
    let second = held[1].0.clone();

    // (case, the position, the modifications the consumer reads from it)
    let positions: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "TRIM_HORIZON, the oldest record the shard holds",
            json!({"ShardIteratorType": "TRIM_HORIZON"}),
            delivered.to_vec(),
        ),
        (
            "AT_TIMESTAMP, a time stamp before every change",
            json!({"ShardIteratorType": "AT_TIMESTAMP", "Timestamp": 0}),
            delivered.to_vec(),
        ),
        (
            "AT_TIMESTAMP, a time stamp after every change",
            json!({"ShardIteratorType": "AT_TIMESTAMP", "Timestamp": a_day_ahead()}),
            Vec::new(),
        ),
        (
            "AT_SEQUENCE_NUMBER, the second record delivered",
            json!({
                "ShardIteratorType": "AT_SEQUENCE_NUMBER",
                "StartingSequenceNumber": second.clone(),
            }),
            vec!["MODIFY", "REMOVE"],
        ),
        (
            "AFTER_SEQUENCE_NUMBER, the second record delivered",
            json!({
                "ShardIteratorType": "AFTER_SEQUENCE_NUMBER",
                "StartingSequenceNumber": second.clone(),
            }),
            vec!["REMOVE"],
        ),
        (
            "LATEST, pinned at the end of the shard before any change",
            Value::Null,
            delivered.to_vec(),
        ),
    ];

    for (case, position, expected) in positions {
        // The LATEST row reads from the position minted above; every other row mints its
        // own now.
        let iterator = match position {
            Value::Null => pinned_before.clone(),
            position => iterator_at(&router, stream, &shard, position).await,
        };
        let (records, _) = read_from(&router, case, iterator).await;
        assert_eq!(
            events(&records),
            expected,
            "case {case:?}: expected the modifications {expected:?}, got {records:?}"
        );
        for (_, document) in &records {
            assert_eq!(
                document["dynamodb"]["Keys"],
                json!({KEY: {"S": ITEM}}),
                "case {case:?}: every record carries the key of the item it is about, got \
                 {document}"
            );
            assert_eq!(
                document["tableName"],
                json!("Positions"),
                "case {case:?}: every record names the table the change was made on, got \
                 {document}"
            );
        }
    }
}

/// Rule 2: a consumer that holds a position at the end of the shard reads the changes made
/// after it took that position — which is what a change-data-capture pipeline does between
/// two `GetRecords` calls.
#[tokio::test]
async fn a_consumer_at_the_end_of_the_shard_reads_the_next_changes() {
    let router = emulator();
    let stream = "cdc-tail";
    let stream_arn = ensure_kinesis_stream(&router, stream).await;
    create_table(&router, "Tailed", &[]).await;
    table_is_active(&router, "Tailed").await;
    replicate(&router, "Tailed", &stream_arn).await;
    let shard = only_shard(&router, stream).await;

    // One change first, so the position below is taken on a shard that ALREADY holds a
    // record rather than on an empty one.
    put_item(&router, "Tailed", "first").await;
    let iterator = iterator_at(
        &router,
        stream,
        &shard,
        json!({"ShardIteratorType": "LATEST"}),
    )
    .await;
    let (caught_up, next) = read_from(&router, "caught up with the shard", iterator).await;
    assert!(
        caught_up.is_empty(),
        "case \"caught up with the shard\": a position at the end of the shard reads none of \
         the records before it, got {caught_up:?}"
    );
    let mut iterator = next.expect(
        "case \"caught up with the shard\": a consumer that is caught up is handed the \
         position to carry on from",
    );

    // Each later change, read through the position the previous read handed back: the
    // rows differ along the modification made while the consumer held its position.
    for (case, expected) in [
        ("the edit made after it caught up", "MODIFY"),
        ("the removal made after that", "REMOVE"),
    ] {
        match expected {
            "MODIFY" => update_item(&router, "Tailed", "second").await,
            _ => delete_item(&router, "Tailed").await,
        }
        let (records, next) = read_from(&router, case, iterator).await;
        assert_eq!(
            events(&records),
            vec![expected],
            "case {case:?}: expected the change made while the consumer waited, got {records:?}"
        );
        iterator = next.unwrap_or_else(|| {
            panic!("case {case:?}: the consumer is handed the position to carry on from")
        });
    }
}
