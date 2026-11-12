// SPDX-License-Identifier: CC0-1.0

//! The Kinesis-data-stream-to-Firehose-to-Amazon-S3 data plane, driven over the wire.
//!
//! The rule under test: a Firehose stream whose `DeliveryStreamType` names a source service
//! takes its data from the source its source configuration names, and not from a put — for
//! `KinesisStreamAsSource` that is the Kinesis data stream of
//! `KinesisStreamSourceConfiguration{KinesisStreamARN, RoleARN}`, "the stream and role Amazon
//! Resource Names (ARNs) for a Kinesis data stream used as the source for a Firehose stream".
//! Every record that stream takes from the instant retrieval starts at
//! ("Firehose starts retrieving records from the Kinesis data stream starting with this
//! timestamp" — `KinesisStreamSourceDescription$DeliveryStartTimestamp`) is buffered exactly
//! once, in its shard's order, and is then delivered to the destination exactly as a
//! directly-put record is: an object under the destination's `Prefix` with the automatic
//! `YYYY/MM/DD/HH` expansion, on whichever `BufferingHints` condition is met first.
//!
//! Three services are mounted on ONE shared engine, as the router mounts them, so what is
//! exercised is the hand-off between them: the Kinesis writes and the Firehose calls as
//! `POST /` with `X-Amz-Target`, the destination reads as the path-style Amazon S3 requests a
//! client sends.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, REGION, ROLE_ARN};
use crate::destination_delivery::{base64, carries_time_prefix, Emulator};

/// The `X-Amz-Target` prefix of the Kinesis Data Streams wire.
const KINESIS_TARGET: &str = "Kinesis_20131202";

/// The routing key a Kinesis request is signed for.
const KINESIS_SERVICE: &str = "kinesis";

/// How long a delivery out of a source stream is waited for before the case fails.
///
/// Longer than the `DirectPut` suite's deadline: a record has to be CONSUMED from the source
/// before its buffer even opens, so the wait covers a source poll, the buffer's window and
/// the destination write, with room to spare on a slow machine.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long the destination is watched AFTER every record has arrived, to see that nothing
/// is delivered twice: several source polls and several buffer windows.
const SETTLE: Duration = Duration::from_millis(1_500);

/// One source-ingestion case: the source stream's shape, the destination the Firehose stream
/// delivers to, and the records a producer writes to the source.
struct SourceRow {
    case: &'static str,
    /// How many shards the source stream is created with, so the records of a case are
    /// spread over that many shards.
    shards: u32,
    /// The `CreateDeliveryStream` member carrying the destination.
    member: &'static str,
    /// The `Prefix` the destination is configured with, `None` to leave the member out.
    prefix: Option<&'static str>,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`, `None` to leave the member out
    /// and take the documented defaults (5 MiB, 300 s).
    hints: Option<(i64, i64)>,
    /// How many records the producer writes to the source.
    records: usize,
    /// How many bytes of filler each record carries beyond its marker.
    record_filler: usize,
}

/// The source-ingestion cases.
///
/// The rows differ along what the rule generalises over: how many shards the records are
/// spread over, which destination member carries the Amazon S3 location, whether the
/// destination names a `Prefix` of its own, the `BufferingHints` the stream was created with
/// — a size the records cross well inside a long interval, and each published bound of
/// `SizeInMBs` (1..128) under the shortest interval the member admits — and the
/// number and size of the records written to the source.
///
/// Every row whose records do not cross `SizeInMBs` asks for `IntervalInSeconds` 0, the
/// shortest wait the member admits, because what these cases assert is that a source's
/// records reach the destination at all; the wait a configured interval buys is asserted by
/// the `buffering_hints` cases.
fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            case: "one record, an extended destination, a 1 MiB buffer",
            shards: 1,
            member: "ExtendedS3DestinationConfiguration",
            prefix: None,
            hints: Some((1, 0)),
            records: 1,
            record_filler: 0,
        },
        SourceRow {
            case: "twenty ~60 KiB records crossing SizeInMBs=1 inside IntervalInSeconds=900",
            shards: 1,
            member: "S3DestinationConfiguration",
            prefix: None,
            hints: Some((1, 900)),
            records: 20,
            record_filler: 60 * 1024,
        },
        SourceRow {
            case: "eight records spread over four shards, the documented default size",
            shards: 4,
            member: "ExtendedS3DestinationConfiguration",
            prefix: Some("landing/"),
            hints: Some((5, 0)),
            records: 8,
            record_filler: 32,
        },
        SourceRow {
            case: "the largest SizeInMBs and the smallest IntervalInSeconds",
            shards: 2,
            member: "S3DestinationConfiguration",
            prefix: Some("edge/bounds/"),
            hints: Some((128, 0)),
            records: 5,
            record_filler: 16,
        },
    ]
}

#[tokio::test]
async fn a_kinesis_source_streams_records_are_delivered_to_the_destination() {
    for (ordinal, row) in source_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("source-bucket-{ordinal}");
        let source = format!("source-stream-{ordinal}");
        let stream = format!("source-fed-stream-{ordinal}");
        emulator.create_bucket(&bucket).await;
        let source_arn = create_source_stream(&emulator, &source, row.shards).await;

        create_source_fed_stream(&emulator, &stream, &source_arn, &row, &bucket).await;

        // Nothing has been written to the source yet, so there is nothing to consume and
        // nothing to deliver.
        let before = emulator.keys_under(&bucket, "").await;
        assert!(
            before.is_empty(),
            "case {:?}: an idle source delivers nothing, got {before:?}",
            row.case
        );

        // What the producer writes to the SOURCE — never to Firehose, which refuses a put to
        // a stream fed by a configured source.
        let payloads: Vec<Vec<u8>> = (0..row.records)
            .map(|index| payload(ordinal, index, row.record_filler))
            .collect();
        for (index, payload) in payloads.iter().enumerate() {
            put_to_source(&emulator, &source, &format!("pk-{index}"), payload).await;
        }

        let bodies = delivered_payloads(&emulator, row.case, &bucket, &payloads).await;
        let prefix = row.prefix.unwrap_or_default();
        for key in bodies.keys() {
            assert!(
                key.starts_with(prefix),
                "case {:?}: expected a key under {prefix:?}, got {key:?}",
                row.case
            );
            assert!(
                carries_time_prefix(key, prefix),
                "case {:?}: a delivered key carries the automatic YYYY/MM/DD/HH prefix, \
                 got {key:?}",
                row.case
            );
            assert!(
                key.contains(&stream),
                "case {:?}: a delivered object is named for its Firehose stream, got {key:?}",
                row.case
            );
        }

        // Each record of the source reached the destination exactly once: a consumed record
        // is not consumed again, and a delivered buffer is not delivered again.
        tokio::time::sleep(SETTLE).await;
        let settled = concatenated(&emulator, &bucket).await;
        for (index, payload) in payloads.iter().enumerate() {
            let held = occurrences(&settled, payload);
            assert_eq!(
                held, 1,
                "case {:?}: record {index} of the source is delivered exactly once, got {held}",
                row.case
            );
        }
        assert_eq!(
            settled.len(),
            payloads.iter().map(Vec::len).sum::<usize>(),
            "case {:?}: the destination holds the source's records and nothing else",
            row.case
        );
    }
}

#[tokio::test]
async fn retrieval_starts_at_the_delivery_start_timestamp() {
    let emulator = Emulator::new();
    let bucket = "delivery-start-bucket".to_string();
    let source = "delivery-start-source".to_string();
    let stream = "delivery-start-stream".to_string();
    emulator.create_bucket(&bucket).await;
    let source_arn = create_source_stream(&emulator, &source, 1).await;

    // A record the source held BEFORE the Firehose stream existed: retrieval starts at the
    // stream's own `DeliveryStartTimestamp`, so this one is not its data.
    let before = b"written-before-the-firehose-stream".to_vec();
    put_to_source(&emulator, &source, "pk-before", &before).await;

    let row = SourceRow {
        case: "retrieval starts at the delivery start timestamp",
        shards: 1,
        member: "ExtendedS3DestinationConfiguration",
        prefix: None,
        hints: Some((1, 0)),
        records: 0,
        record_filler: 0,
    };
    create_source_fed_stream(&emulator, &stream, &source_arn, &row, &bucket).await;

    let after = b"written-after-the-firehose-stream".to_vec();
    put_to_source(&emulator, &source, "pk-after", &after).await;

    let delivered =
        delivered_payloads(&emulator, row.case, &bucket, std::slice::from_ref(&after)).await;
    let held: Vec<u8> = delivered.values().flatten().copied().collect();
    assert_eq!(
        occurrences(&held, &before),
        0,
        "a record the source held before the Firehose stream was created is not retrieved, \
         got {:?}",
        String::from_utf8_lossy(&held)
    );

    // And it stays unretrieved: the position the first poll took is past it for good.
    tokio::time::sleep(SETTLE).await;
    let settled = concatenated(&emulator, &bucket).await;
    assert_eq!(
        occurrences(&settled, &before),
        0,
        "a record before the delivery start timestamp is never retrieved, got {:?}",
        String::from_utf8_lossy(&settled)
    );
    assert_eq!(
        occurrences(&settled, &after),
        1,
        "the record written after it is delivered exactly once, got {:?}",
        String::from_utf8_lossy(&settled)
    );
}

#[tokio::test]
async fn a_source_fed_stream_takes_no_direct_put() {
    let emulator = Emulator::new();
    let bucket = "no-direct-put-bucket".to_string();
    let source = "no-direct-put-source".to_string();
    let stream = "no-direct-put-stream".to_string();
    emulator.create_bucket(&bucket).await;
    let source_arn = create_source_stream(&emulator, &source, 1).await;
    let row = SourceRow {
        case: "a source-fed stream takes no direct put",
        shards: 1,
        member: "S3DestinationConfiguration",
        prefix: None,
        hints: Some((1, 0)),
        records: 0,
        record_filler: 0,
    };
    create_source_fed_stream(&emulator, &stream, &source_arn, &row, &bucket).await;

    // The ingestion operations are not available on a stream fed by a configured source, so
    // the destination's data can only have come from the source.
    let (status, answered) = emulator.put_record(&stream, b"direct").await;
    assert_eq!(
        status, 400,
        "a put to a KinesisStreamAsSource stream is refused, got {answered:?}"
    );

    let through_the_source = b"through-the-source".to_vec();
    put_to_source(&emulator, &source, "pk-1", &through_the_source).await;
    let delivered = delivered_payloads(
        &emulator,
        "a source-fed stream takes no direct put",
        &bucket,
        std::slice::from_ref(&through_the_source),
    )
    .await;
    let held: Vec<u8> = delivered.values().flatten().copied().collect();
    assert_eq!(
        occurrences(&held, b"direct"),
        0,
        "the refused put is nowhere in the destination, got {:?}",
        String::from_utf8_lossy(&held)
    );
}

/// Create the source Kinesis data stream `name` with `shards` shards and answer its
/// `StreamARN`, once the stream reports `ACTIVE`.
async fn create_source_stream(emulator: &Emulator, name: &str, shards: u32) -> String {
    let (status, answered) = emulator
        .json_rpc(
            KINESIS_SERVICE,
            KINESIS_TARGET,
            "CreateStream",
            json!({ "StreamName": name, "ShardCount": shards }),
        )
        .await;
    assert_eq!(status, 200, "create source stream {name}: got {answered:?}");

    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let (status, answered) = emulator
            .json_rpc(
                KINESIS_SERVICE,
                KINESIS_TARGET,
                "DescribeStreamSummary",
                json!({ "StreamName": name }),
            )
            .await;
        assert_eq!(status, 200, "describe source stream {name}: got {answered:?}");
        let summary = &answered["StreamDescriptionSummary"];
        if summary["StreamStatus"] == json!("ACTIVE") {
            return summary["StreamARN"]
                .as_str()
                .unwrap_or_else(|| panic!("the source stream reports its ARN, got {answered:?}"))
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "the source stream {name} must reach ACTIVE, got {answered:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create the `KinesisStreamAsSource` Firehose stream `name` reading `source_arn` and
/// delivering to `bucket`, and wait for it to be `ACTIVE`.
async fn create_source_fed_stream(
    emulator: &Emulator,
    name: &str,
    source_arn: &str,
    row: &SourceRow,
    bucket: &str,
) {
    let mut destination = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": format!("arn:aws:s3:::{bucket}"),
    });
    if let Some(prefix) = row.prefix {
        destination["Prefix"] = json!(prefix);
    }
    if let Some((size, interval)) = row.hints {
        destination["BufferingHints"] = json!({
            "SizeInMBs": size,
            "IntervalInSeconds": interval,
        });
    }
    let mut request = json!({
        "DeliveryStreamName": name,
        "DeliveryStreamType": "KinesisStreamAsSource",
        "KinesisStreamSourceConfiguration": {
            "KinesisStreamARN": source_arn,
            "RoleARN": ROLE_ARN,
        },
    });
    request[row.member] = destination;
    let answered = emulator.firehose_ok("CreateDeliveryStream", request).await;
    assert!(
        answered["DeliveryStreamARN"]
            .as_str()
            .is_some_and(|arn| arn.contains(name)),
        "case {:?}: the create answers the stream's ARN, got {answered:?}",
        row.case
    );
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The source the stream reports is the one it was created with, read back through the
    // service's own describe.
    let described = emulator
        .firehose_ok(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": name }),
        )
        .await;
    let description = &described["DeliveryStreamDescription"];
    assert_eq!(
        description["DeliveryStreamStatus"],
        json!("ACTIVE"),
        "case {:?}: the stream must be ACTIVE, got {described:?}",
        row.case
    );
    assert_eq!(
        description["Source"]["KinesisStreamSourceDescription"]["KinesisStreamARN"],
        json!(source_arn),
        "case {:?}: the stream reports the source it reads, got {described:?}",
        row.case
    );
    assert!(
        description["Source"]["KinesisStreamSourceDescription"]["DeliveryStartTimestamp"]
            .is_number(),
        "case {:?}: the stream reports when retrieval starts, got {described:?}",
        row.case
    );
}

/// Write one record to the source stream, as a producer does.
async fn put_to_source(emulator: &Emulator, source: &str, partition_key: &str, data: &[u8]) {
    let (status, answered) = emulator
        .json_rpc(
            KINESIS_SERVICE,
            KINESIS_TARGET,
            "PutRecord",
            json!({
                "StreamName": source,
                "PartitionKey": partition_key,
                "Data": base64(data),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "put to the source stream {source} in {REGION}: got {answered:?}"
    );
    assert!(
        answered["SequenceNumber"].is_string(),
        "the source stream answers the sequence number it stored the record under, \
         got {answered:?}"
    );
}

/// Every object the destination holds once each of `payloads` is in one of them, keyed by
/// object key — or a failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
async fn delivered_payloads(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    payloads: &[Vec<u8>],
) -> BTreeMap<String, Vec<u8>> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let bodies = bodies(emulator, bucket).await;
        let held: Vec<u8> = bodies.values().flatten().copied().collect();
        let missing: Vec<usize> = payloads
            .iter()
            .enumerate()
            .filter(|(_, payload)| occurrences(&held, payload) == 0)
            .map(|(index, _)| index)
            .collect();
        if missing.is_empty() {
            return bodies;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected every record of the source in {bucket}, missing the \
             records at {missing:?} of {} (the keys delivered are {:?})",
            payloads.len(),
            bodies.keys().collect::<Vec<_>>()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The bytes of every object the destination holds, keyed by key.
async fn bodies(emulator: &Emulator, bucket: &str) -> BTreeMap<String, Vec<u8>> {
    let mut bodies = BTreeMap::new();
    for key in emulator.keys_under(bucket, "").await {
        let body = emulator.object(bucket, &key).await;
        bodies.insert(key, body);
    }
    bodies
}

/// The bytes of every object the destination holds, in key order, as one slab — what the
/// records of a source are counted in.
async fn concatenated(emulator: &Emulator, bucket: &str) -> Vec<u8> {
    bodies(emulator, bucket)
        .await
        .values()
        .flatten()
        .copied()
        .collect()
}

/// How many times `needle` occurs in `held`.
fn occurrences(held: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || held.len() < needle.len() {
        return 0;
    }
    held.windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// The `index`th record of the `ordinal`th case: a marker no other record of the suite
/// carries, padded to the row's record size so a case can cross a size hint.
///
/// The filler is a repeated digit, which cannot hold another record's marker, so counting a
/// marker in the concatenated objects counts that record and nothing else.
fn payload(ordinal: usize, index: usize, filler: usize) -> Vec<u8> {
    let mut payload = format!("case-{ordinal}-record-{index}:").into_bytes();
    payload.extend(std::iter::repeat_n(b'7', filler));
    payload
}
