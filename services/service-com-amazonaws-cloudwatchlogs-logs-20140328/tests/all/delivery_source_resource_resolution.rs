// SPDX-License-Identifier: CC0-1.0

//! A delivery source's `resourceArn` is RESOLVED against the service that owns the resource.
//!
//! CloudWatch Logs and Amazon S3 are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so the bucket a row names either is, or is not, one the object
//! store itself holds: nothing here fakes the owning service's answer.
//!
//! RULE: `PutDeliverySource` creates "a logical object that represents the resource that is
//! actually sending the logs", so the reference is resolved where the owning service holds it
//! before a source is stored — a put naming a resource that service reports it does not hold is
//! refused with `ResourceNotFoundException`, carrying that service's own wording for the
//! missing resource, and stores nothing. A refusal has to be GROUNDED in what the owning
//! service answered, so a resource of a service this emulator cannot ask, and a wildcard ARN
//! (which names a kind of resource rather than one), are stored as before.
//!
//! The rows differ along what the rule generalises over: whether the owning service holds the
//! resource, whether this emulator can ask that service at all, and whether the ARN names one
//! resource or a wildcard — each paired with a log type its own service sends.
//!
//! The arm that needs a SECOND owning service mounted beside this one — a CloudFront
//! distribution, resolved with `GetDistribution` — lives in
//! `core/router/tests/all/logs_delivery_source_cloudfront_resource.rs`.

use serde_json::json;

use crate::export_task_objects::Emulator;
use crate::harness::error_code;

/// The two operations under test.
const PUT: &str = "PutDeliverySource";
const GET: &str = "GetDeliverySource";

/// The refusal a resource no owning service holds is answered with, and the one a read of a
/// source the account does not hold answers.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// What one row names and what the put must do with it.
struct Row {
    case: &'static str,
    /// Names this row's delivery source, so no row collides with another.
    name: &'static str,
    /// The bucket the object store must hold before the put, if any.
    bucket: Option<&'static str>,
    /// The `resourceArn` the put names.
    resource_arn: &'static str,
    /// The `logType` that resource's own service sends. Source: the `logType` member of
    /// `PutDeliverySourceRequest`
    /// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
    log_type: &'static str,
    /// The refusal message the put must carry, or `None` for a put that must be accepted.
    refused_with: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket the object store holds",
            name: "held-bucket",
            bucket: Some("vended-logs-held"),
            resource_arn: "arn:aws:s3:::vended-logs-held",
            log_type: "S3_SERVER_ACCESS_LOGS",
            refused_with: None,
        },
        Row {
            case: "a bucket the object store does not hold",
            name: "absent-bucket",
            bucket: None,
            resource_arn: "arn:aws:s3:::vended-logs-never-created",
            log_type: "S3_SERVER_ACCESS_LOGS",
            // Amazon S3's own wording for a bucket it does not hold (`NoSuchBucket`).
            refused_with: Some("The specified bucket does not exist"),
        },
        Row {
            case: "another bucket the object store does not hold, named nothing like the first",
            name: "absent-bucket-elsewhere",
            bucket: None,
            resource_arn: "arn:aws:s3:::obs-pipeline-staging-1",
            log_type: "S3_SERVER_ACCESS_LOGS",
            refused_with: Some("The specified bucket does not exist"),
        },
        Row {
            case: "a bucket the object store holds beside the absent ones",
            name: "second-held-bucket",
            bucket: Some("obs-pipeline-staging-2"),
            resource_arn: "arn:aws:s3:::obs-pipeline-staging-2",
            log_type: "S3_SERVER_ACCESS_LOGS",
            refused_with: None,
        },
        Row {
            case: "a resource of a service this emulator cannot ask",
            name: "unaskable-service",
            bucket: None,
            resource_arn:
                "arn:aws:workmail:eu-west-2:123456789012:organization/m-1234EXAMPLEabcd1234",
            log_type: "WORKMAIL_MAILBOX_ACCESS_LOGS",
            refused_with: None,
        },
        Row {
            case: "a wildcard ARN, which names a kind of resource rather than one",
            name: "wildcard-hub",
            bucket: None,
            resource_arn: "arn:aws:securityhub:eu-west-2:111122223333:hubv2/*",
            log_type: "SECURITY_FINDING_LOGS",
            refused_with: None,
        },
    ]
}

#[tokio::test]
async fn a_delivery_source_is_created_only_over_a_resource_its_service_holds() {
    let emulator = Emulator::new();
    let rows = rows();

    for row in &rows {
        if let Some(bucket) = row.bucket {
            emulator.create_bucket(row.case, bucket).await;
        }
    }

    for row in &rows {
        let case = row.case;
        let body = json!({
            "name": row.name,
            "resourceArn": row.resource_arn,
            "logType": row.log_type,
        });
        let (status, answered) = emulator.logs(PUT, &body).await;
        let (read_status, read) = emulator
            .logs(GET, &json!({"name": row.name}))
            .await;

        match row.refused_with {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the put to be accepted, got {status} {answered}"
                );
                assert_eq!(
                    read_status, 200,
                    "case {case:?}: expected the source to be readable back, got {read_status} \
                     {read}"
                );
                assert_eq!(
                    read["deliverySource"]["resourceArns"],
                    json!([row.resource_arn]),
                    "case {case:?}: expected the stored source to represent {:?}, got {read}",
                    row.resource_arn
                );
                assert_eq!(
                    read["deliverySource"]["logType"].as_str(),
                    Some(row.log_type),
                    "case {case:?}: expected the stored source to send {:?}, got {read}",
                    row.log_type
                );
            }
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the put to be refused, got {status} {answered}"
                );
                let code = error_code(&answered);
                assert_eq!(
                    code, NOT_FOUND,
                    "case {case:?}: expected {NOT_FOUND}, got {code} {answered}"
                );
                assert_eq!(
                    answered["message"].as_str(),
                    Some(message),
                    "case {case:?}: expected the owning service's wording {message:?}, got \
                     {answered}"
                );
                // The refusal leaves nothing behind: the source is not readable afterwards.
                assert_eq!(
                    read_status, 400,
                    "case {case:?}: a refused put stores nothing, got {read_status} {read}"
                );
                let read_code = error_code(&read);
                assert_eq!(
                    read_code, NOT_FOUND,
                    "case {case:?}: expected the read to be {NOT_FOUND}, got {read_code} {read}"
                );
            }
        }
    }
}
