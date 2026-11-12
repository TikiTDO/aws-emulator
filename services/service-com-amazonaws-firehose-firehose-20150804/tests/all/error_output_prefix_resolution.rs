// SPDX-License-Identifier: CC0-1.0

//! What an `ErrorOutputPrefix` carrying a partition-key expression delivers.
//!
//! The rule under test: a record whose partition keys could not be placed is written under
//! the destination's `ErrorOutputPrefix` ("A prefix that Firehose evaluates and adds to
//! failed records before writing them to S3" — `ErrorOutputPrefix`), so that prefix is
//! always resolvable — a partition key cannot be asked of it, because a record is there
//! precisely BECAUSE its keys could not be derived, and such an expression is carried
//! through as written like any other namespace the expander does not evaluate. The object
//! reporting the failure is therefore written on the first attempt, rather than the
//! destination being left owing a delivery that could never be made.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// How long the error output is waited for before a case fails.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One error-output case: the configured `ErrorOutputPrefix` and what it must expand to.
struct ErrorPrefixRow {
    case: &'static str,
    /// The prefix as configured, `{leg}` standing for the case's own first segment.
    configured: &'static str,
    /// What that prefix expands to, `{leg}` likewise.
    expanded: &'static str,
}

/// The error-output cases.
///
/// The rows differ along what the rule generalises over: which partition-key namespace the
/// prefix names (`partitionKeyFromQuery`, `partitionKeyFromLambda`, both at once — the two
/// the expander resolves only for a delivered object's prefix), where in the prefix it sits,
/// and whether the prefix also carries an expression that IS evaluated there
/// (`!{firehose:error-output-type}`, `!{timestamp:...}`). The last row is the control: a
/// prefix naming no partition key at all.
fn error_prefix_rows() -> Vec<ErrorPrefixRow> {
    vec![
        ErrorPrefixRow {
            case: "a partition key from the query, after the error output type",
            configured: "{leg}!{firehose:error-output-type}/!{partitionKeyFromQuery:dept}/",
            expanded: "{leg}metadata-extraction-failed/!{partitionKeyFromQuery:dept}/",
        },
        ErrorPrefixRow {
            case: "a partition key from a data transformation, as the whole second segment",
            configured: "{leg}!{partitionKeyFromLambda:dept}/",
            expanded: "{leg}!{partitionKeyFromLambda:dept}/",
        },
        ErrorPrefixRow {
            case: "both partition-key namespaces beside a timestamp that is evaluated",
            configured: "{leg}!{partitionKeyFromQuery:dept}/!{partitionKeyFromLambda:team}/\
                         y=!{timestamp:yyyy}/",
            expanded: "{leg}!{partitionKeyFromQuery:dept}/!{partitionKeyFromLambda:team}/y=",
        },
        ErrorPrefixRow {
            case: "no partition key at all, the control",
            configured: "{leg}!{firehose:error-output-type}/",
            expanded: "{leg}metadata-extraction-failed/",
        },
    ]
}

#[tokio::test]
async fn an_error_output_prefix_naming_a_partition_key_still_writes_the_error_output() {
    let emulator = Emulator::new();
    let bucket = "error-output-prefix-bucket";
    emulator.create_bucket(bucket).await;

    let rows = error_prefix_rows();
    let mut legs: Vec<(String, String, &'static str)> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        let stream = format!("err-prefix-{ordinal}");
        let leg = format!("{stream}-err/");
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{bucket}"),
                        // A key only a Lambda data transformation could supply, and this
                        // emulator runs none: every record fails to be placed and goes to
                        // the error output.
                        "Prefix": format!("{stream}/!{{partitionKeyFromLambda:dept}}/"),
                        "ErrorOutputPrefix": row.configured.replace("{leg}", &leg),
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                        "CompressionFormat": "UNCOMPRESSED",
                        "DynamicPartitioningConfiguration": { "Enabled": true },
                        "ProcessingConfiguration": {
                            "Enabled": true,
                            "Processors": [{
                                "Type": "MetadataExtraction",
                                "Parameters": [
                                    {
                                        "ParameterName": "MetadataExtractionQuery",
                                        "ParameterValue": "{dept:.dept}",
                                    },
                                    {
                                        "ParameterName": "JsonParsingEngine",
                                        "ParameterValue": "JQ-1.6",
                                    },
                                ],
                            }],
                        },
                    },
                }),
            )
            .await;
        legs.push((stream, leg, row.case));
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (stream, _, case) in &legs {
        let (status, answered) = emulator.put_record(stream, b"{\"dept\":\"sales\"}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must be accepted, got {answered:?}"
        );
    }

    for ((_, leg, case), row) in legs.iter().zip(&rows) {
        let expected = row.expanded.replace("{leg}", leg);
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        let key = loop {
            let keys = emulator.keys_under(bucket, leg).await;
            if let Some(key) = keys.into_iter().next() {
                break key;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the error output must be written under {leg:?}, and the \
                 destination must not be left owing a delivery it can never make"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert!(
            key.starts_with(&expected),
            "case {case:?}: expected the error output under {expected:?}, got {key:?}"
        );
    }
}
