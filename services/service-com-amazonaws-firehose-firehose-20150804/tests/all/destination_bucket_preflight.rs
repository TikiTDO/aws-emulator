// SPDX-License-Identifier: CC0-1.0

//! The pre-flight `UpdateDestination` runs on the Amazon S3 location an update names.
//!
//! The rule under test: `UpdateDestination` resolves every Amazon S3 bucket the submitted
//! destination update names against Amazon S3 BEFORE it rewrites the stored destination, so
//! an update naming a bucket that does not exist is refused with HTTP 400
//! `InvalidArgumentException` and the message that names it, and the stream keeps the
//! destination, the `VersionId` and the delivery target it had. A bucket Amazon S3 does
//! hold is accepted, and an update that names no bucket at all has nothing to resolve.
//!
//! The rows span what the rule generalises over, not the one request a client sent: WHICH
//! member of WHICH destination type carries the `BucketARN` (a plain Amazon S3
//! destination's own, an extended one's, the `S3BackupUpdate` beside it, the `S3Update` of
//! another destination kind), whether the bucket was never created or was created and then
//! deleted (so the gate is Amazon S3's state at the moment of the call and not a list
//! written here), whether the bucket exists, and an update that carries no bucket at all.
//!
//! Both services are mounted on ONE shared engine, exactly as the router mounts them, so
//! the gate is exercised the way a client sees it: the bucket is created through Amazon
//! S3's own wire and resolved through the same state the delivery leg writes to.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    at, error_code, error_message, PAST_THE_TRANSITION, ROLE_ARN,
};
use crate::destination_delivery::Emulator;

/// The bucket every stream below is created against: the destination a refused update must
/// leave in place.
const ORIGIN: &str = "preflight-origin";

/// A second bucket Amazon S3 holds, so a row can repoint a destination at a bucket that
/// really is there.
const TARGET: &str = "preflight-target";

/// A bucket created during setup and DELETED before the table runs: the gate reads what
/// Amazon S3 holds now, so a destination can no longer be repointed at it.
const DELETED: &str = "preflight-deleted";

/// Bucket names nothing ever created.
const ABSENT_PRIMARY: &str = "preflight-absent-primary";
const ABSENT_BACKUP: &str = "preflight-absent-backup";
const ABSENT_NESTED: &str = "preflight-absent-nested";

/// `bucket` as the `BucketARN` a destination names it by.
///
/// `pub(crate)`, like this crate's other test helpers, so the sibling suite that extends
/// this table along the remaining members and name forms
/// (`tests/all/destination_bucket_resolution.rs`) reuses it instead of carrying a copy.
pub(crate) fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// An `S3DestinationConfiguration` delivering to `bucket`.
///
/// Its `BufferingHints` ask for the shortest wait `IntervalInSeconds` admits (published
/// range 0..900), so a case asserting that a delivery reaches the configured bucket does
/// not wait out a buffering interval it is not about.
pub(crate) fn s3_configuration(bucket: &str) -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": bucket_arn(bucket),
        "Prefix": "raw/",
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
    })
}

/// One row: the stream under test, the update applied to it, and what the pre-flight must
/// answer.
struct Case {
    case: &'static str,
    stream: &'static str,
    /// The `CreateDeliveryStream` member carrying the stream's destination.
    config_member: &'static str,
    config: Value,
    /// The `UpdateDestination` member carrying the update.
    update_member: &'static str,
    update: Value,
    /// The `/`-separated path, inside the sole `DestinationDescription`, of the `BucketARN`
    /// the destination delivers to.
    bucket_path: &'static str,
    /// The bucket the destination must name AFTER the call: the origin for a refused
    /// update, the update's own for an accepted one.
    bucket_after: &'static str,
    /// The bucket the refusal must name, or `None` for a row the pre-flight accepts.
    refused_bucket: Option<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "an extended S3 destination repointed at a bucket that was never created",
            stream: "preflight-extended",
            config_member: "ExtendedS3DestinationConfiguration",
            config: s3_configuration(ORIGIN),
            update_member: "ExtendedS3DestinationUpdate",
            update: json!({ "BucketARN": bucket_arn(ABSENT_PRIMARY) }),
            bucket_path: "ExtendedS3DestinationDescription/BucketARN",
            bucket_after: ORIGIN,
            refused_bucket: Some(ABSENT_PRIMARY),
        },
        Case {
            // The same absent bucket carried by the OTHER Amazon S3 update member: the gate
            // is the bucket, not one member of one destination type.
            case: "a plain S3 destination repointed at a bucket that was never created",
            stream: "preflight-plain",
            config_member: "S3DestinationConfiguration",
            config: s3_configuration(ORIGIN),
            update_member: "S3DestinationUpdate",
            update: json!({ "BucketARN": bucket_arn(ABSENT_PRIMARY) }),
            bucket_path: "S3DestinationDescription/BucketARN",
            bucket_after: ORIGIN,
            refused_bucket: Some(ABSENT_PRIMARY),
        },
        Case {
            case: "an extended S3 destination repointed at a bucket that was deleted",
            stream: "preflight-deleted-target",
            config_member: "ExtendedS3DestinationConfiguration",
            config: s3_configuration(ORIGIN),
            update_member: "ExtendedS3DestinationUpdate",
            update: json!({ "BucketARN": bucket_arn(DELETED) }),
            bucket_path: "ExtendedS3DestinationDescription/BucketARN",
            bucket_after: ORIGIN,
            refused_bucket: Some(DELETED),
        },
        Case {
            // Not the member the recording cited: the backup location beside the primary
            // one is a bucket Firehose writes objects to as well, so an absent one is
            // refused even though the destination's own bucket is there.
            case: "an extended S3 update whose backup bucket was never created",
            stream: "preflight-backup",
            config_member: "ExtendedS3DestinationConfiguration",
            config: s3_configuration(ORIGIN),
            update_member: "ExtendedS3DestinationUpdate",
            update: json!({
                "BucketARN": bucket_arn(TARGET),
                "S3BackupMode": "Enabled",
                "S3BackupUpdate": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": bucket_arn(ABSENT_BACKUP),
                    "Prefix": "backup/",
                },
            }),
            bucket_path: "ExtendedS3DestinationDescription/BucketARN",
            bucket_after: ORIGIN,
            refused_bucket: Some(ABSENT_BACKUP),
        },
        Case {
            // Nor is the nested Amazon S3 configuration of a destination that is not an
            // Amazon S3 destination at all.
            case: "a Splunk destination whose nested S3 update names no such bucket",
            stream: "preflight-splunk",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(ORIGIN),
            }),
            update_member: "SplunkDestinationUpdate",
            update: json!({ "S3Update": { "BucketARN": bucket_arn(ABSENT_NESTED) } }),
            bucket_path: "SplunkDestinationDescription/S3DestinationDescription/BucketARN",
            bucket_after: ORIGIN,
            refused_bucket: Some(ABSENT_NESTED),
        },
        Case {
            case: "an extended S3 destination repointed at a bucket Amazon S3 holds",
            stream: "preflight-accepted",
            config_member: "ExtendedS3DestinationConfiguration",
            config: s3_configuration(ORIGIN),
            update_member: "ExtendedS3DestinationUpdate",
            update: json!({ "BucketARN": bucket_arn(TARGET) }),
            bucket_path: "ExtendedS3DestinationDescription/BucketARN",
            bucket_after: TARGET,
            refused_bucket: None,
        },
        Case {
            case: "an update that names no bucket at all",
            stream: "preflight-no-bucket",
            config_member: "ExtendedS3DestinationConfiguration",
            config: s3_configuration(ORIGIN),
            update_member: "ExtendedS3DestinationUpdate",
            update: json!({ "Prefix": "moved/" }),
            bucket_path: "ExtendedS3DestinationDescription/BucketARN",
            bucket_after: ORIGIN,
            refused_bucket: None,
        },
    ]
}

/// `(VersionId, DestinationId)` of a stream's sole destination, with the description it was
/// read from.
///
/// `pub(crate)` for the same reason as [`bucket_arn`]: the sibling suite reads a stream's
/// version and destination id exactly this way.
pub(crate) async fn version_and_destination(
    emulator: &Emulator,
    stream: &str,
) -> (String, String, Value) {
    let described = emulator
        .firehose_ok(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    let description = described["DeliveryStreamDescription"].clone();
    let version = description["VersionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let destination_id = description["Destinations"][0]["DestinationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !version.is_empty() && !destination_id.is_empty(),
        "stream {stream}: expected a version and a destination id, got {description}"
    );
    (version, destination_id, description)
}

#[tokio::test]
async fn an_update_naming_a_bucket_that_does_not_exist_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    for bucket in [ORIGIN, TARGET, DELETED] {
        emulator.create_bucket(bucket).await;
    }
    // The bucket that WAS there: the pre-flight asks Amazon S3 at the moment of the call,
    // so once this is gone an update may no longer name it.
    let removed = emulator.s3("DELETE", &format!("/{DELETED}")).await;
    assert!(
        matches!(removed.status, 200 | 204),
        "setup: deleting {DELETED} must succeed, got {} with body {}",
        removed.status,
        String::from_utf8_lossy(&removed.body)
    );

    let cases = cases();
    for case in &cases {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.config_member] = case.config.clone();
        emulator.firehose_ok("CreateDeliveryStream", request).await;
    }
    // Every stream is created, so one wait covers the clock-owed CREATING -> ACTIVE span:
    // only an ACTIVE stream's destination can be updated.
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in &cases {
        let name = case.case;
        let (version, destination_id, before) =
            version_and_destination(&emulator, case.stream).await;
        let mut request = json!({
            "DeliveryStreamName": case.stream,
            "CurrentDeliveryStreamVersionId": version,
            "DestinationId": destination_id,
        });
        request[case.update_member] = case.update.clone();
        let (status, answered) = emulator.firehose("UpdateDestination", request).await;

        match case.refused_bucket {
            Some(bucket) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "InvalidArgumentException"),
                    "case {name:?}: expected 400 InvalidArgumentException, got {status} \
                     with {answered}"
                );
                assert_eq!(
                    error_message(&answered),
                    format!("S3 Bucket {bucket} does not exist."),
                    "case {name:?}: expected the refusal to name {bucket:?}, got {answered}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {name:?}: expected the update to be accepted, got {status} with \
                 {answered}"
            ),
        }

        let (after_version, _, after) = version_and_destination(&emulator, case.stream).await;
        let expected_version = if case.refused_bucket.is_some() {
            version_number(&before)
        } else {
            version_number(&before) + 1
        };
        assert_eq!(
            version_number(&after),
            expected_version,
            "case {name:?}: expected VersionId {expected_version} after the call, got \
             {after_version:?}"
        );
        assert_eq!(
            at(&after["Destinations"][0], case.bucket_path),
            &json!(bucket_arn(case.bucket_after)),
            "case {name:?}: expected the destination to deliver to {:?}, got {after}",
            case.bucket_after
        );
    }
}

/// A stream's `VersionId` as the number it is: `DeliveryStreamVersionId` is a decimal
/// string, so a case states the version it expects rather than its spelling.
fn version_number(description: &Value) -> u64 {
    description["VersionId"]
        .as_str()
        .unwrap_or_default()
        .parse()
        .unwrap_or_else(|_| panic!("the description carries a decimal VersionId: {description}"))
}

#[tokio::test]
async fn a_refused_update_leaves_the_stream_delivering_to_its_original_bucket() {
    let emulator = Emulator::new();
    let stream = "preflight-live";
    let origin = "preflight-live-origin";
    let absent = "preflight-live-absent";
    emulator.create_bucket(origin).await;
    emulator
        .create_active_stream(
            stream,
            "ExtendedS3DestinationConfiguration",
            s3_configuration(origin),
        )
        .await;

    let (status, put) = emulator.put_record(stream, b"before-the-refusal").await;
    assert_eq!(status, 200, "the first put got {put}");
    let delivered = emulator
        .delivered_keys("before the refusal", origin, 1)
        .await;
    assert_eq!(delivered.len(), 1, "before the refusal: got {delivered:?}");

    let (version, destination_id, _) = version_and_destination(&emulator, stream).await;
    let (status, refused) = emulator
        .firehose(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": stream,
                "CurrentDeliveryStreamVersionId": version,
                "DestinationId": destination_id,
                "ExtendedS3DestinationUpdate": { "BucketARN": bucket_arn(absent) },
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidArgumentException"),
        "the update onto an absent bucket must be refused, got {status} with {refused}"
    );

    // The refusal changed nothing, so what is put next still reaches the original bucket.
    let (status, put) = emulator.put_record(stream, b"after-the-refusal").await;
    assert_eq!(status, 200, "the second put got {put}");
    let delivered = emulator.delivered_keys("after the refusal", origin, 2).await;
    assert_eq!(delivered.len(), 2, "after the refusal: got {delivered:?}");
    let bodies = emulator.delivered_bodies(origin).await;
    let text: String = bodies
        .values()
        .map(|body| String::from_utf8_lossy(body).into_owned())
        .collect();
    assert!(
        text.contains("before-the-refusal") && text.contains("after-the-refusal"),
        "the original bucket holds both records, got {text:?}"
    );

    // And nothing was written anywhere else: the bucket the refused update named is still
    // not a bucket at all.
    let listed = emulator.s3("GET", &format!("/{absent}?list-type=2")).await;
    assert_eq!(
        listed.status,
        404,
        "listing {absent} must report no such bucket, got {} with body {}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
}
