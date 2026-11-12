// SPDX-License-Identifier: CC0-1.0

//! Delivery to a destination in Splunk, end to end.
//!
//! The rule under test: a stream whose destination is a Splunk HTTP Event Collector has its
//! buffered records ATTEMPTED against the endpoint its `HECEndpoint` names once the buffer is
//! due, and the events the collector did not take are parked in the Amazon S3 location its
//! `S3Configuration` names once `RetryOptions$DurationInSeconds` — "the total amount of time
//! that Firehose spends on retries. This duration starts after the initial attempt to send
//! data to Splunk fails" — is spent. "Firehose writes any data that could not be indexed to
//! the configured Amazon S3 destination" (`SplunkS3BackupMode`), which holds under BOTH of
//! that member's values: `AllEvents` copies every incoming record there as well, while
//! `FailedEventsOnly` — the member's documented default — parks only what failed. A parked
//! event is written as the error-output document every destination family's failed records
//! are parked as: one JSON document per line carrying the record's own bytes base64-encoded
//! under `rawData` and an `errorCode` naming the Splunk failure class.
//!
//! The rows span what the rule generalises over: the backup mode (named, named as the
//! copy-everything value, and left to its default), the endpoint type (`Raw` and `Event`),
//! the `ErrorOutputPrefix` the backup configures, the records the buffer holds, and the retry
//! window (spent, and still open).
//!
//! The stream is created and the records are put over the real `awsJson1_1` wire and the
//! parked objects are read back over Amazon S3's own, through the Firehose-and-S3 harness of
//! [`crate::destination_delivery`]; the parking is driven by the emulator's OWN delivery clock
//! rather than by the test, since nothing this emulator serves answers at the collector URL of
//! a row — so what each case proves is what a producer that makes no further call observes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::Emulator;

/// How long a parked or delivered object is waited for before a case fails: far longer than
/// the emulator's delivery span, so a slow machine does not fail a case the clock would have
/// served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination is read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Where a row's delivered copies go, for the backup mode that copies every record.
const DELIVERED_PREFIX: &str = "delivered/";

/// The `firehose:error-output-type` a Splunk destination's failed events are written under,
/// and the prefix they get when the backup configures no `ErrorOutputPrefix`.
const FAILED_OUTPUT_TYPE: &str = "splunk-failed";

/// The `errorCode`s the error-output document of an event a Splunk collector did not take
/// carries — the collector's own failure classes, each named for the destination family.
/// Sourced from the `errorCode` values Firehose publishes for a Splunk destination.
const FAILED_ERROR_CODES: &[&str] = &[
    "Splunk.ConnectionClosed",
    "Splunk.AckTimeout",
    "Splunk.InvalidToken",
    "Splunk.InvalidDataFormat",
    "Splunk.ServerError",
];

/// The fixed prefix every one of those codes carries: the destination family.
const FAILED_ERROR_CODE_PREFIX: &str = "Splunk.";

/// The destination member a Splunk stream is created with.
const SPLUNK_MEMBER: &str = "SplunkDestinationConfiguration";

/// The bucket one case's destination backs its failed events up to: one per case, so what a
/// case reads back is its own delivery and not a sibling's.
fn backup_bucket(case: &str) -> String {
    format!("firehose-splunk-{case}")
}

/// That bucket as the `BucketARN` a destination names it by, in the form that member's
/// pattern requires.
fn backup_bucket_arn(case: &str) -> String {
    format!("arn:aws:s3:::{}", backup_bucket(case))
}

/// The URL of the collector one case delivers to. Nothing this emulator serves answers at
/// it, so every attempt against it fails and the events are parked.
fn collector_url(case: &str) -> String {
    format!("https://collector.splunk.test/{case}/services/collector/event")
}

/// The destination configuration of a Splunk collector, with the backup every row shares.
///
/// The buffering hints ask for the shortest wait their members admit
/// (`SplunkBufferingHints$IntervalInSeconds` 0..60), so a case does not wait out a buffering
/// interval it is not about.
fn splunk_configuration(case: &str, endpoint_type: &str, retry_seconds: i64) -> Value {
    json!({
        "HECEndpoint": collector_url(case),
        "HECEndpointType": endpoint_type,
        "HECToken": "00000000-0000-4000-8000-00000000000a",
        "HECAcknowledgmentTimeoutInSeconds": 180,
        "RetryOptions": { "DurationInSeconds": retry_seconds },
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        "S3Configuration": {
            "RoleARN": ROLE_ARN,
            "BucketARN": backup_bucket_arn(case),
            "Prefix": DELIVERED_PREFIX,
        },
    })
}

/// The data of every record still buffered for `stream`, in arrival order — read out of this
/// service's own store, which is the only place a buffered record exists.
fn buffered(emulator: &Emulator, stream: &str) -> Vec<Vec<u8>> {
    let state = emulator.engine().state::<
        service_com_amazonaws_firehose_firehose_20150804::ComAmazonawsFirehoseFirehose20150804State,
    >();
    let store = state.lock();
    store
        .streams
        .get(stream)
        .map(|stream| {
            stream
                .records
                .iter()
                .map(|record| record.data.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Wait for `ready` to hold without making any Firehose call, so what makes it hold is the
/// emulator's own delivery clock.
async fn wait_until<F>(case: &str, expected: &str, mut ready: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {expected} within {DELIVERY_DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The objects of `bucket` once one of them is written under `prefix`, keyed by key, or a
/// failure naming the case once [`DELIVERY_DEADLINE`] has passed.
async fn parked_under(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    prefix: &str,
) -> BTreeMap<String, Vec<u8>> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = emulator.keys_under(bucket, prefix).await;
        if !keys.is_empty() {
            return emulator.delivered_bodies(bucket).await;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the events parked under {prefix:?} of {bucket} within \
             {DELIVERY_DEADLINE:?}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the backup table.
struct BackupRow {
    case: &'static str,
    /// `S3BackupMode`, or `None` to configure none at all — whose documented default is
    /// `FailedEventsOnly`.
    backup_mode: Option<&'static str>,
    /// `HECEndpointType`: "either \"Raw\" or \"Event.\""
    endpoint_type: &'static str,
    /// The backup's `ErrorOutputPrefix`, or `None` to configure none — in which case the
    /// error output type is itself the prefix.
    error_output_prefix: Option<&'static str>,
    /// The records the buffer holds.
    payloads: &'static [&'static [u8]],
    /// Whether every record is also COPIED to the backup location, which is what the
    /// copy-everything mode asks for.
    copies_every_record: bool,
}

#[tokio::test]
async fn events_a_splunk_collector_never_took_are_parked_in_the_destination_backup() {
    // Each row varies what the rule generalises over: which `S3BackupMode` the destination
    // names (including none, so the documented default decides), which endpoint type the
    // attempt is made against, the `ErrorOutputPrefix` the parked object is written under,
    // and the records the buffer holds. The retry window of every row is zero
    // (`DurationInSeconds`'s own minimum), so one attempt is made, fails, and the events are
    // parked by the emulator's own delivery clock without any further call.
    let rows = [
        BackupRow {
            case: "failed-events-only",
            backup_mode: Some("FailedEventsOnly"),
            endpoint_type: "Event",
            error_output_prefix: None,
            payloads: &[b"{\"event\":\"the-one-event\"}\n"],
            copies_every_record: false,
        },
        BackupRow {
            case: "default-backup-mode",
            backup_mode: None,
            endpoint_type: "Event",
            error_output_prefix: None,
            payloads: &[b"alpha", b"beta", b"gamma"],
            copies_every_record: false,
        },
        BackupRow {
            case: "raw-endpoint-with-error-prefix",
            backup_mode: Some("FailedEventsOnly"),
            endpoint_type: "Raw",
            error_output_prefix: Some("failed/"),
            payloads: &[b"a raw event", &[0x00, 0x01, 0xff]],
            copies_every_record: false,
        },
        BackupRow {
            case: "error-prefix-with-output-type",
            backup_mode: Some("FailedEventsOnly"),
            endpoint_type: "Event",
            error_output_prefix: Some("failed/!{firehose:error-output-type}/"),
            payloads: &[b""],
            copies_every_record: false,
        },
        BackupRow {
            case: "all-events",
            backup_mode: Some("AllEvents"),
            endpoint_type: "Event",
            error_output_prefix: None,
            payloads: &[b"copied-and-parked"],
            copies_every_record: true,
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let case = row.case;
        let bucket = backup_bucket(case);
        emulator.create_bucket(&bucket).await;
        let mut config = splunk_configuration(case, row.endpoint_type, 0);
        if let Some(mode) = row.backup_mode {
            config["S3BackupMode"] = json!(mode);
        }
        if let Some(prefix) = row.error_output_prefix {
            config["S3Configuration"]["ErrorOutputPrefix"] = json!(prefix);
        }
        emulator
            .create_active_stream(case, SPLUNK_MEMBER, config)
            .await;

        // Where the parked object is written: the configured `ErrorOutputPrefix`, with the
        // error-output-type namespace expanded, and the error output type itself where the
        // backup configures no prefix at all.
        let expected_prefix = row.error_output_prefix.map_or_else(
            || format!("{FAILED_OUTPUT_TYPE}/"),
            |configured| configured.replace("!{firehose:error-output-type}", FAILED_OUTPUT_TYPE),
        );
        let records: Vec<Vec<u8>> = row.payloads.iter().map(|data| data.to_vec()).collect();
        let (status, answered) = emulator.put_batch(case, &records).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the batch must be accepted, got {answered}"
        );
        assert_eq!(
            answered["FailedPutCount"].as_i64(),
            Some(0),
            "case {case:?}: every record of the batch must be accepted, got {answered}"
        );

        let parked = parked_under(&emulator, case, &bucket, &expected_prefix).await;
        let (key, body) = parked
            .iter()
            .find(|(key, _)| key.starts_with(&expected_prefix))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: an object under {expected_prefix:?} holds the failed \
                     events, got {:?}",
                    parked.keys().collect::<Vec<_>>()
                )
            });
        assert!(
            key.starts_with(&expected_prefix),
            "case {case:?}: the failed events are written under {expected_prefix:?}, got {key:?}"
        );
        // One error-output document per record, each naming how the delivery failed and
        // carrying the record's own bytes, so nothing of a parked event is lost.
        let documents: Vec<Value> = String::from_utf8_lossy(body)
            .lines()
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: every parked line is a JSON document, got {error}")
                })
            })
            .collect();
        assert_eq!(
            documents.len(),
            row.payloads.len(),
            "case {case:?}: one document per failed event, got {documents:?}"
        );
        for (document, payload) in documents.iter().zip(row.payloads) {
            let code = document["errorCode"].as_str().unwrap_or_default();
            assert!(
                code.starts_with(FAILED_ERROR_CODE_PREFIX),
                "case {case:?}: the document's errorCode names the destination family \
                 {FAILED_ERROR_CODE_PREFIX:?}, got {code:?}"
            );
            assert!(
                FAILED_ERROR_CODES.contains(&code),
                "case {case:?}: the document names how the Splunk delivery failed, expected \
                 one of {FAILED_ERROR_CODES:?}, got {code:?}"
            );
            assert!(
                document["errorMessage"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the document states what the code means, got {document:?}"
            );
            let raw = document["rawData"].as_str().unwrap_or_default();
            let decoded = aws_smithy_types::base64::decode(raw).unwrap_or_else(|error| {
                panic!("case {case:?}: rawData is base64, got {error} for {raw:?}")
            });
            assert_eq!(
                decoded,
                payload.to_vec(),
                "case {case:?}: the document carries the event's own bytes, got {decoded:?}"
            );
            assert!(
                document["arrivalTimestamp"].as_u64().unwrap_or_default() > 0,
                "case {case:?}: the document says when the record arrived, got {document:?}"
            );
        }
        // The copy-everything mode delivers every incoming record to the backup location as
        // well, which is the one thing that separates it from the failed-events-only mode.
        let copied = emulator.keys_under(&bucket, DELIVERED_PREFIX).await;
        assert_eq!(
            !copied.is_empty(),
            row.copies_every_record,
            "case {case:?}: expected copies_every_record={}, got the delivered objects {copied:?}",
            row.copies_every_record
        );
        assert!(
            buffered(&emulator, case).is_empty(),
            "case {case:?}: a parked buffer is no longer waiting for the collector, got {:?}",
            buffered(&emulator, case)
        );
    }
}

#[tokio::test]
async fn nothing_is_parked_while_the_splunk_retry_window_is_still_open() {
    // The same destination with a retry window that is still open: the attempt fails, and
    // "the total amount of time that Firehose spends on retries"
    // (`SplunkRetryOptions$DurationInSeconds`) has not been spent, so the events are neither
    // delivered nor parked — they wait for a further attempt. The rows span the two endpoint
    // types, so what separates them from the parking case above is the retry window alone.
    let rows = [
        ("retrying-an-event-endpoint", "Event"),
        ("retrying-a-raw-endpoint", "Raw"),
    ];
    let payloads: Vec<Vec<u8>> = vec![b"first".to_vec(), b"second".to_vec()];

    let emulator = Emulator::new();
    for (case, endpoint_type) in rows {
        let bucket = backup_bucket(case);
        emulator.create_bucket(&bucket).await;
        // A retry window far wider than this case runs for, so the events stay owed.
        let config = splunk_configuration(case, endpoint_type, 7_200);
        emulator
            .create_active_stream(case, SPLUNK_MEMBER, config)
            .await;

        let (status, answered) = emulator.put_batch(case, &payloads).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the batch must be accepted, got {answered}"
        );
        // The buffer is taken out of the store for the duration of an attempt, so this waits
        // for the restored buffer rather than reading once.
        wait_until(case, "the events are back in the buffer", || {
            buffered(&emulator, case) == payloads
        })
        .await;
        let parked = emulator.keys_under(&bucket, "").await;
        assert!(
            parked.is_empty(),
            "case {case:?}: nothing is parked while the retry window is open, got {parked:?}"
        );
    }
}
