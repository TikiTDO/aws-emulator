// SPDX-License-Identifier: CC0-1.0

//! WHEN a stream's buffer becomes an Amazon S3 object: the destination's `BufferingHints`.
//!
//! The rule under test: Firehose holds the records a put accepted and delivers them once the
//! FIRST of the destination's two buffering conditions is met — `IntervalInSeconds` after the
//! buffer opened ("Buffer incoming data for the specified period of time, in seconds, before
//! delivering it to the destination") or as soon as the buffer holds `SizeInMBs` ("Buffer
//! incoming data to the specified size, in MiBs, before delivering it to the destination"),
//! with the documented defaults (300 s, 5 MiB) standing where the configuration names
//! neither. Each flushed buffer becomes ONE object.
//!
//! So a destination's bucket holds NOTHING for a record put inside a longer interval, holds
//! one object for the same record once that interval has passed, and holds one as soon as the
//! records cross the configured size however long the interval is. Every row below asserts
//! that window rather than any particular latency: the readings are taken at one checkpoint
//! inside the shortest configured interval and at one past it.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// The bucket every case here delivers into.
const BUCKET: &str = "buffering-hints-bucket";

/// The interval the rows that wait one out are configured with, in seconds.
///
/// Inside the published range of `IntervalInSeconds` (0..900) and long enough that a reading
/// taken at [`EARLY_CHECKPOINT`] is comfortably before it.
const SHORT_INTERVAL: i64 = 3;

/// How long after the puts the first reading is taken: past the window a buffer whose
/// interval is the published minimum is held for, and well inside [`SHORT_INTERVAL`].
const EARLY_CHECKPOINT: Duration = Duration::from_millis(1_000);

/// How long after the puts the second reading is taken: past [`SHORT_INTERVAL`], so a buffer
/// configured with it is due, while every longer interval still has minutes to run.
const LATE_CHECKPOINT: Duration = Duration::from_millis(5_500);

/// How long a delivery that IS due is waited for before a case fails, so a slow machine does
/// not fail a case the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination is polled while waiting for a due delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A record big enough that a few of them cross one MiB while each stays inside the
/// per-record limit of `Record$Data`.
const BIG_RECORD_BYTES: usize = 600 * 1024;

/// A record big enough that six of them cross the documented default `SizeInMBs` of 5 MiB.
const HUGE_RECORD_BYTES: usize = 900 * 1024;

/// When the destination of one row may first hold an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// At once: the configured interval is the published minimum, or the records put cross
    /// `SizeInMBs` so the size condition is the one that fires.
    AtOnce,
    /// Only once [`SHORT_INTERVAL`] has passed.
    AfterItsInterval,
    /// Not while this case runs: its interval is minutes away and its records never reach
    /// its size.
    NotYet,
}

/// How the row's records are put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Put {
    /// One `PutRecordBatch` carrying every record.
    OneBatch,
    /// One `PutRecord` per record.
    OneCallEach,
}

/// One buffering case.
struct Row {
    case: &'static str,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`; `None` leaves the member out
    /// altogether, which is the documented defaults.
    hints: Option<(i64, i64)>,
    /// How many records are put, and how many bytes of filler each carries.
    records: (usize, usize),
    put: Put,
    due: Due,
}

/// The buffering cases.
///
/// The rows differ along what the rule generalises over: the configured `IntervalInSeconds`
/// (its published minimum 0, a short one, its published maximum 900, and the member left out
/// for the documented default of 300), the configured `SizeInMBs` (1, the published maximum
/// 128, and the member left out for the documented default of 5), whether the records put
/// CROSS that size, how many records there are and how big each is, and whether they are put
/// in one batch or one call each.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest interval the member admits, with a tiny record",
            hints: Some((1, 0)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::AtOnce,
        },
        Row {
            case: "a short interval, with a payload far below SizeInMBs",
            hints: Some((1, SHORT_INTERVAL)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "the same short interval under the largest SizeInMBs the member admits",
            hints: Some((128, SHORT_INTERVAL)),
            records: (3, 0),
            put: Put::OneCallEach,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "no BufferingHints at all, so the documented 300 s default decides",
            hints: None,
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::NotYet,
        },
        Row {
            case: "the longest interval the member admits, with a tiny record",
            hints: Some((1, 900)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::NotYet,
        },
        Row {
            case: "the longest interval with the largest size, and records far below it",
            hints: Some((128, 900)),
            records: (2, BIG_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::NotYet,
        },
        Row {
            case: "records crossing SizeInMBs=1 inside the longest interval",
            hints: Some((1, 900)),
            records: (2, BIG_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
        Row {
            case: "records crossing the documented default SizeInMBs with no hints at all",
            hints: None,
            records: (6, HUGE_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
    ]
}

/// One row's stream under test, with the records it put and where they land.
struct Leg {
    case: &'static str,
    stream: String,
    /// The `Prefix` the row's destination is configured with, which isolates its objects.
    prefix: String,
    /// The records put, in arrival order.
    records: Vec<Vec<u8>>,
    put: Put,
    due: Due,
}

#[tokio::test]
async fn a_buffer_is_delivered_on_the_first_buffering_hint_it_meets_and_not_before() {
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row.
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in rows().into_iter().enumerate() {
        let stream = format!("buffering-stream-{ordinal}");
        let prefix = format!("row-{ordinal}/");
        let mut config = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
            "Prefix": prefix,
            "CompressionFormat": "UNCOMPRESSED",
        });
        if let Some((size, interval)) = row.hints {
            config["BufferingHints"] = json!({
                "SizeInMBs": size,
                "IntervalInSeconds": interval,
            });
        }
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": config,
                }),
            )
            .await;
        let (count, filler) = row.records;
        legs.push(Leg {
            case: row.case,
            stream,
            prefix,
            records: (0..count)
                .map(|index| payload(ordinal, index, filler))
                .collect(),
            put: row.put,
            due: row.due,
        });
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let put_at = Instant::now();
    for leg in &legs {
        match leg.put {
            Put::OneBatch => {
                let (status, answered) = emulator.put_batch(&leg.stream, &leg.records).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the batch must be accepted, got {answered:?}",
                    leg.case
                );
            }
            Put::OneCallEach => {
                for record in &leg.records {
                    let (status, answered) = emulator.put_record(&leg.stream, record).await;
                    assert_eq!(
                        status, 200,
                        "case {:?}: the put must be accepted, got {answered:?}",
                        leg.case
                    );
                }
            }
        }
    }

    // Inside the shortest configured interval: only a buffer whose own condition has
    // already fired may have become an object.
    tokio::time::sleep_until((put_at + EARLY_CHECKPOINT).into()).await;
    for leg in &legs {
        if leg.due == Due::AtOnce {
            continue;
        }
        let keys = emulator.keys_under(BUCKET, &leg.prefix).await;
        assert!(
            keys.is_empty(),
            "case {:?}: nothing may be delivered before the buffer's own hints are met, \
             got {keys:?}",
            leg.case
        );
    }
    for leg in legs.iter().filter(|leg| leg.due == Due::AtOnce) {
        assert_one_object_holding(&emulator, leg).await;
    }

    // Past the short interval: the buffers configured with it are now objects, and the ones
    // whose interval is minutes away still hold nothing.
    tokio::time::sleep_until((put_at + LATE_CHECKPOINT).into()).await;
    for leg in legs.iter().filter(|leg| leg.due == Due::NotYet) {
        let keys = emulator.keys_under(BUCKET, &leg.prefix).await;
        assert!(
            keys.is_empty(),
            "case {:?}: a buffer whose interval has not elapsed is still not delivered, \
             got {keys:?}",
            leg.case
        );
    }
    for leg in legs.iter().filter(|leg| leg.due == Due::AfterItsInterval) {
        assert_one_object_holding(&emulator, leg).await;
    }
}

/// Require `leg`'s destination to hold exactly one object, carrying every record of the
/// flushed buffer in arrival order — "one object per flushed batch".
async fn assert_one_object_holding(emulator: &Emulator, leg: &Leg) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let keys = loop {
        let keys = emulator.keys_under(BUCKET, &leg.prefix).await;
        if !keys.is_empty() {
            break keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected a delivered object under {:?}",
            leg.case,
            leg.prefix
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        keys.len(),
        1,
        "case {:?}: one flushed buffer is one object, got {keys:?}",
        leg.case
    );
    let body = emulator.object(BUCKET, &keys[0]).await;
    let expected = leg.records.concat();
    assert_eq!(
        body.len(),
        expected.len(),
        "case {:?}: the object holds every record of the buffer ({} bytes), got {} bytes",
        leg.case,
        expected.len(),
        body.len()
    );
    assert!(
        body == expected,
        "case {:?}: the object holds the buffer's records in arrival order",
        leg.case
    );
}

/// The bytes of one record: a marker naming where it came from, padded with `filler` bytes.
fn payload(row: usize, record: usize, filler: usize) -> Vec<u8> {
    let mut data = format!("row-{row}-record-{record}-").into_bytes();
    data.extend(std::iter::repeat_n(b'x', filler));
    data
}
