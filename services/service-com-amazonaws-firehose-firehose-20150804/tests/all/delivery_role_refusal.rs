// SPDX-License-Identifier: CC0-1.0

//! What a delivery the destination's ROLE did not allow writes, and what it reports.
//!
//! The rule under test: Firehose makes a delivery under the role the destination
//! configuration names ("the Amazon Resource Name (ARN) of the Amazon Web Services
//! credentials" — `S3DestinationConfiguration$RoleARN`), and that role's permissions are
//! evaluated AFRESH ON EVERY ATTEMPT: while they do not allow Amazon S3's own write of the
//! object, no object is written and the attempt is reported through the destination's own
//! `CloudWatchLoggingOptions` as its `S3.AccessDenied` delivery error; once they allow it
//! again, the next attempt is authorized and delivers — so the decision is the one in force
//! at the moment of the flush rather than the one in force when the stream was created.
//!
//! The rows span what the rule generalises over: HOW the permission goes away (the inline
//! policy is deleted, replaced by one scoped to another bucket, replaced by one allowing only
//! a read), how many records are put while it is gone, and which destination member carries
//! the Amazon S3 location (`S3DestinationConfiguration`, `ExtendedS3DestinationConfiguration`).
//!
//! What becomes of the records a flush could not write is NOT this suite's subject.
//!
//! Four services are mounted on ONE shared engine, exactly as the router mounts them: the
//! role is created and changed through IAM's own wire and the decision is IAM's, the bucket
//! and the delivered objects are Amazon S3's, and the refusals are read back through
//! CloudWatch Logs' own `GetLogEvents`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, REGION};
use crate::destination_delivery_errors::{allow, authorization, form_encode};

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// How long a delivery or a refusal is waited for before a row fails: far longer than the
/// emulator's own delivery span, so a slow machine does not fail a row the clock would have
/// served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination, the buffer and the log stream are read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The bucket each row's destination delivers into.
const BUCKET: &str = "firehose-role-refusal";

/// Another bucket Amazon S3 holds, so a row can scope the role's grant to a bucket that is
/// not the destination's.
const OTHER_BUCKET: &str = "firehose-role-refusal-other";

/// The log group each row's `CloudWatchLoggingOptions` name.
const LOG_GROUP: &str = "/aws/kinesisfirehose/role-refusal";

/// The log stream they name — the one a Firehose destination's delivery errors go to.
const LOG_STREAM: &str = "DestinationDelivery";

/// The role every row delivers under.
const ROLE_NAME: &str = "firehose-role-refusal";

/// The one inline policy on that role, which a row removes and then restores.
const POLICY_NAME: &str = "s3-delivery-access";

/// The stream every row puts its records on.
const STREAM: &str = "role-refusal";

/// Where the delivered objects go.
const PREFIX: &str = "data/";

/// The `errorCode` a destination whose role does not allow the write reports.
const REFUSED_ERROR_CODE: &str = "S3.AccessDenied";

/// The record every row puts while the role still allows the write.
const BEFORE_REVOKE: &[u8] = b"before-revoke\n";

/// The record every row puts once the permission is restored.
const AFTER_RESTORE: &[u8] = b"after-restore\n";

/// One payload as the line a consumer reads back out of the destination.
fn line_of(payload: &[u8]) -> String {
    String::from_utf8_lossy(payload).trim().to_string()
}

/// `bucket` as the `BucketARN` a destination names it by.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// Firehose, Amazon S3, IAM and CloudWatch Logs on one shared engine.
struct Aws {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // The role a delivery is made under is IAM's to decide about, so IAM holds the role
        // these rows change and answers the question each delivery puts to it.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // The refusals are CloudWatch Logs': the destination's logging writes them there and
        // this suite reads them back through that service's own reader.
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// One `awsJson1_1` call: `service` is the routing key its requests are signed for and
    /// `target` the prefix of its `X-Amz-Target`.
    async fn json_rpc(
        &self,
        service: &str,
        target: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .header("authorization", authorization(service))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Firehose operation that must succeed.
    async fn firehose_ok(&self, operation: &str, body: Value) -> Value {
        let (status, answered) = self
            .json_rpc("firehose", "Firehose_20150804", operation, body)
            .await;
        assert_eq!(status, 200, "{operation} must succeed, got {answered}");
        answered
    }

    /// One CloudWatch Logs operation.
    async fn logs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.json_rpc("logs", "Logs_20140328", operation, body).await
    }

    /// One awsQuery call to IAM that must succeed.
    async fn iam(&self, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{HOST}/"))
            .header("host", HOST)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "{action} must succeed, got {status} with {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`, naming the Region this suite signs for.
    async fn create_bucket(&self, bucket: &str) {
        let payload = format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        );
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("https://{HOST}/{bucket}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::from(payload))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// Every line of every object [`BUCKET`] holds — what a consumer reading the whole
    /// destination back sees, which is what a record either is or is not among.
    fn delivered_lines(&self) -> Vec<String> {
        let Some(state) = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
        else {
            return Vec::new();
        };
        let store = state.lock();
        store
            .bucket(BUCKET)
            .map(|bucket| {
                bucket
                    .objects()
                    .flat_map(|(_, object)| {
                        String::from_utf8_lossy(&object.body)
                            .lines()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the destination holds a line carrying `payload`.
    fn delivered(&self, payload: &[u8]) -> bool {
        let expected = line_of(payload);
        self.delivered_lines().iter().any(|line| line == &expected)
    }

    /// The `errorCode`s the destination's logging has reported so far.
    async fn reported_codes(&self) -> Vec<String> {
        let (status, answered) = self
            .logs(
                "GetLogEvents",
                json!({
                    "logGroupName": LOG_GROUP,
                    "logStreamName": LOG_STREAM,
                    "startFromHead": true,
                }),
            )
            .await;
        if status != 200 {
            return Vec::new();
        }
        answered["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|event| event["message"].as_str())
            .filter_map(|message| serde_json::from_str::<Value>(message).ok())
            .filter_map(|document| {
                document["errorCode"]
                    .as_str()
                    .map(::std::string::ToString::to_string)
            })
            .collect()
    }

    /// `PutRecord` of one payload, which must be accepted.
    async fn put(&self, payload: &[u8]) {
        self.firehose_ok(
            "PutRecord",
            json!({
                "DeliveryStreamName": STREAM,
                "Record": { "Data": aws_smithy_types::base64::encode(payload) },
            }),
        )
        .await;
    }

    /// Create the role every row delivers under, with `policy` as its one inline policy.
    async fn create_role(&self, policy: &Value) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "firehose.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            "CreateRole",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.put_role_policy(policy).await;
        format!(
            "arn:aws:iam::{}:role/{ROLE_NAME}",
            engine::emulator_account_id()
        )
    }

    /// `PutRolePolicy` of the role's one inline policy.
    async fn put_role_policy(&self, policy: &Value) {
        self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }

    /// `DeleteRolePolicy` of that same policy.
    async fn delete_role_policy(&self) {
        self.iam(
            "DeleteRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
            ],
        )
        .await;
    }

    /// Wait for `ready` to hold without making any Firehose call, so what makes it hold is
    /// the emulator's own delivery clock.
    async fn wait_until<F>(&self, case: &str, expected: &str, mut ready: F)
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

    /// Wait for the destination's logging to report `code`, which is how a refused delivery
    /// becomes observable to an operator.
    async fn wait_for_report(&self, case: &str, code: &str) {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let reported = self.reported_codes().await;
            if reported.iter().any(|reported| reported == code) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {code:?} reported within {DELIVERY_DEADLINE:?}, got \
                 {reported:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The policy that lets the delivery be made: the write of any object of the destination
/// bucket.
fn delivery_policy() -> Value {
    allow("s3:PutObject", &format!("{}/*", bucket_arn(BUCKET)))
}

/// How a row takes the delivery permission away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revocation {
    /// `DeleteRolePolicy` removes the role's only S3 permissions.
    DeleteThePolicy,
    /// The policy is replaced by one granting the same action on ANOTHER bucket.
    ScopeToAnotherBucket,
    /// The policy is replaced by one granting only a read of the same bucket.
    AllowOnlyARead,
}
/// Which destination member carries the Amazon S3 location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// `S3DestinationConfiguration`.
    S3,
    /// `ExtendedS3DestinationConfiguration`.
    ExtendedS3,
}

/// One row of the table.
struct Row {
    case: &'static str,
    revocation: Revocation,
    /// The records put while the role does not allow their delivery.
    withheld: &'static [&'static [u8]],
    destination: Destination,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the policy is deleted",
            revocation: Revocation::DeleteThePolicy,
            withheld: &[b"after-revoke\n"],
            destination: Destination::ExtendedS3,
        },
        Row {
            case: "the policy is scoped to another bucket",
            revocation: Revocation::ScopeToAnotherBucket,
            withheld: &[b"after-revoke-one\n", b"after-revoke-two\n"],
            destination: Destination::S3,
        },
        Row {
            case: "the policy allows only a read",
            revocation: Revocation::AllowOnlyARead,
            withheld: &[b"after-revoke\n"],
            destination: Destination::ExtendedS3,
        },
    ]
}

#[tokio::test]
async fn a_record_the_delivery_role_withheld_is_delivered_once_the_role_allows_the_write() {
    for row in rows() {
        let case = row.case;
        let aws = Aws::new();
        aws.create_bucket(BUCKET).await;
        aws.create_bucket(OTHER_BUCKET).await;
        let (status, answered) = aws
            .logs("CreateLogGroup", json!({ "logGroupName": LOG_GROUP }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the log group must be created, got {status} {answered}"
        );
        let role_arn = aws.create_role(&delivery_policy()).await;

        // The shortest buffering the hints admit, so one put is one delivery attempt.
        let destination = json!({
            "RoleARN": role_arn,
            "BucketARN": bucket_arn(BUCKET),
            "Prefix": PREFIX,
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
            "CloudWatchLoggingOptions": {
                "Enabled": true,
                "LogGroupName": LOG_GROUP,
                "LogStreamName": LOG_STREAM,
            },
        });
        let member = match row.destination {
            Destination::S3 => "S3DestinationConfiguration",
            Destination::ExtendedS3 => "ExtendedS3DestinationConfiguration",
        };
        aws.firehose_ok(
            "CreateDeliveryStream",
            json!({ "DeliveryStreamName": STREAM, member: destination }),
        )
        .await;
        tokio::time::sleep(PAST_THE_TRANSITION).await;

        // With the policy in force the delivery is made, which is the control this row's
        // refusal is measured against.
        aws.put(BEFORE_REVOKE).await;
        aws.wait_until(case, "the record put under the policy is delivered", || {
            aws.delivered(BEFORE_REVOKE)
        })
        .await;

        // The permission goes away, and the records put while it is gone are not written.
        match row.revocation {
            Revocation::DeleteThePolicy => aws.delete_role_policy().await,
            Revocation::ScopeToAnotherBucket => {
                aws.put_role_policy(&allow(
                    "s3:PutObject",
                    &format!("{}/*", bucket_arn(OTHER_BUCKET)),
                ))
                .await;
            }
            Revocation::AllowOnlyARead => {
                aws.put_role_policy(&allow(
                    "s3:GetObject",
                    &format!("{}/*", bucket_arn(BUCKET)),
                ))
                .await;
            }
        }
        for payload in row.withheld {
            aws.put(payload).await;
        }
        // The attempt is made and reported, which is the only thing a producer can observe
        // of a delivery the role did not allow.
        aws.wait_for_report(case, REFUSED_ERROR_CODE).await;
        for payload in row.withheld {
            assert!(
                !aws.delivered(payload),
                "case {case:?}: a record the role may not deliver reaches no object while the \
                 permission is gone, got {:?}",
                aws.delivered_lines()
            );
        }

        // The permission is restored, identical to the one the first delivery was made
        // under, and the next attempt is authorized again: the record put after the repair
        // is delivered, so the decision was re-read rather than settled when the stream was
        // created. What becomes of the records withheld while the permission was gone is a
        // separate question this suite does not judge.
        aws.put_role_policy(&delivery_policy()).await;
        aws.put(AFTER_RESTORE).await;
        aws.wait_until(case, "the record put after the repair is delivered", || {
            aws.delivered(AFTER_RESTORE)
        })
        .await;

        // What the destination holds: the record delivered under the policy, and the one put
        // once the policy was back.
        let lines = aws.delivered_lines();
        for payload in [BEFORE_REVOKE, AFTER_RESTORE] {
            let expected = line_of(payload);
            assert!(
                lines.contains(&expected),
                "case {case:?}: the destination holds {expected:?}, got {lines:?}"
            );
        }
    }
}
