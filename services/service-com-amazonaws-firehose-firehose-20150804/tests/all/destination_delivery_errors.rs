// SPDX-License-Identifier: CC0-1.0

//! What a destination that did NOT take a delivery reports, and what it withholds.
//!
//! The rule under test: Firehose writes a delivered object under the role the destination
//! configuration names ("the Amazon Resource Name (ARN) of the AWS credentials" —
//! `S3DestinationConfiguration$RoleARN`), so the write happens only where the permissions in
//! force on that role allow Amazon S3's own write of that object; a delivery the destination
//! did not take writes no object, leaves the records buffered, and — where the destination's
//! `CloudWatchLoggingOptions` are enabled — is reported as ONE log event in the
//! `LogGroupName` / `LogStreamName` they name, carrying the stream, the destination, the
//! stream's version and the condition's own `errorCode`. A destination that took the delivery
//! reports nothing at all, which is what tells a misconfigured stream from a healthy one.
//!
//! The rows span what the rule generalises over: what the role's permissions say (an allow of
//! the write, no policy at all, an allow of another action, an allow scoped to another
//! bucket), which condition the delivery failed on (the role, and a destination bucket that
//! does not exist), and whether the destination's logging options report anything (enabled,
//! disabled, naming no stream).
//!
//! Four services are mounted on ONE shared engine, exactly as the router mounts them: the
//! role is created through IAM's own wire and the decision is IAM's, the bucket and the
//! delivered objects are Amazon S3's, and the reported events are read back through
//! CloudWatch Logs' own `GetLogEvents`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, REGION};

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// How long a reported event is waited for before a row fails: far longer than the
/// emulator's own delivery span.
const REPORT_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination and the log stream are polled while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Comfortably past the window a buffer is held for (`state::delivery::DELIVERY_SPAN`) and
/// one retry beyond it, so a read that finds nothing is reading something that was never
/// written rather than something still owed.
const PAST_THE_DELIVERY_WINDOW: Duration = Duration::from_millis(2_000);

/// The bucket each row's destination delivers into.
const BUCKET: &str = "firehose-error-reporting";

/// A bucket no row ever creates, for the destination whose location does not exist.
const ABSENT_BUCKET: &str = "firehose-error-reporting-absent";

/// Another bucket Amazon S3 holds, so a row can grant the write on a bucket that is not the
/// destination's.
const OTHER_BUCKET: &str = "firehose-error-reporting-other";

/// The log group each row's `CloudWatchLoggingOptions` name.
const LOG_GROUP: &str = "/aws/kinesisfirehose/reporting";

/// The log stream they name — the one a Firehose destination's delivery errors go to.
const LOG_STREAM: &str = "DestinationDelivery";

/// The stream every row puts its record on.
const STREAM: &str = "reporting";

/// Where the delivered objects go.
const PREFIX: &str = "data/";

/// `bucket` as the `BucketARN` a destination names it by.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// Firehose, Amazon S3, IAM and CloudWatch Logs on one shared engine.
struct Aws {
    router: MultiServiceRouter,
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
        // The role a delivery is made under is IAM's to decide about, so IAM holds the roles
        // these rows create and answers the question the delivery puts to it.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // The reported events are CloudWatch Logs': the destination's logging writes them
        // there and this suite reads them back through that service's own reader.
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
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

    /// One path-style Amazon S3 request.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (parts.status.as_u16(), body)
    }

    /// `PUT /<bucket>` — S3's `CreateBucket`, naming the Region this suite signs for.
    async fn create_bucket(&self, bucket: &str) {
        let (status, body) = self
            .s3(
                "PUT",
                &format!("/{bucket}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Every key `bucket` holds, read through `ListObjectsV2`.
    async fn keys(&self, bucket: &str) -> Vec<String> {
        let (status, body) = self
            .s3("GET", &format!("/{bucket}?list-type=2"), b"")
            .await;
        assert_eq!(
            status,
            200,
            "list {bucket}: got {}",
            String::from_utf8_lossy(&body)
        );
        let document = String::from_utf8_lossy(&body).into_owned();
        let mut reader = Reader::from_str(&document);
        let mut keys = Vec::new();
        let mut element = String::new();
        loop {
            match reader
                .read_event()
                .unwrap_or_else(|error| panic!("the listing must be well-formed XML: {error}"))
            {
                Event::Start(start) => {
                    element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                }
                Event::Text(text) => {
                    if element == "Key" {
                        keys.push(text.xml10_content().unwrap_or_default().into_owned());
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        keys
    }

    /// The events `group`/`stream` holds, read through `GetLogEvents`.
    ///
    /// A stream the account does not hold holds no event, which is what a group nothing was
    /// ever reported into looks like.
    async fn log_events(&self, group: &str, stream: &str) -> Vec<Value> {
        let (status, answered) = self
            .logs(
                "GetLogEvents",
                json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "startFromHead": true,
                }),
            )
            .await;
        if status != 200 {
            return Vec::new();
        }
        answered["events"].as_array().cloned().unwrap_or_default()
    }

    /// Create the role `name` with a trust policy Firehose can assume, plus `policy` as its
    /// one inline policy where the row carries one.
    async fn create_role(&self, name: &str, policy: Option<Value>) -> String {
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
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.iam(
                "PutRolePolicy",
                &[
                    ("RoleName", name.to_string()),
                    ("PolicyName", "delivery".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        format!(
            "arn:aws:iam::{}:role/{name}",
            engine::emulator_account_id()
        )
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// An identity-based policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}],
    })
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The role's one inline policy, or `None` for a role carrying none at all.
    policy: Option<Value>,
    /// The bucket the destination delivers to.
    bucket: &'static str,
    /// The destination's `CloudWatchLoggingOptions`, as the create submits them.
    logging: Value,
    /// Whether the delivery reaches the destination bucket.
    delivers: bool,
    /// The `errorCode` the destination's logging reports, or `None` where it reports
    /// nothing.
    reported: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    let enabled = json!({
        "Enabled": true,
        "LogGroupName": LOG_GROUP,
        "LogStreamName": LOG_STREAM,
    });
    vec![
        Row {
            case: "a role allowing the write delivers and reports nothing",
            policy: Some(allow("s3:PutObject", &format!("{}/*", bucket_arn(BUCKET)))),
            bucket: BUCKET,
            logging: enabled.clone(),
            delivers: true,
            reported: None,
        },
        Row {
            case: "a role allowing every action on every resource delivers",
            policy: Some(allow("*", "*")),
            bucket: BUCKET,
            logging: enabled.clone(),
            delivers: true,
            reported: None,
        },
        Row {
            case: "a role carrying no policy at all is refused the write",
            policy: None,
            bucket: BUCKET,
            logging: enabled.clone(),
            delivers: false,
            reported: Some("S3.AccessDenied"),
        },
        Row {
            case: "a role allowing only a read is refused the write",
            policy: Some(allow("s3:GetObject", &format!("{}/*", bucket_arn(BUCKET)))),
            bucket: BUCKET,
            logging: enabled.clone(),
            delivers: false,
            reported: Some("S3.AccessDenied"),
        },
        Row {
            case: "a role whose grant is scoped to another bucket is refused the write",
            policy: Some(allow(
                "s3:PutObject",
                &format!("{}/*", bucket_arn(OTHER_BUCKET)),
            )),
            bucket: BUCKET,
            logging: enabled.clone(),
            delivers: false,
            reported: Some("S3.AccessDenied"),
        },
        Row {
            case: "a destination bucket that does not exist reports the missing bucket",
            policy: Some(allow("s3:PutObject", &format!("{}/*", bucket_arn(ABSENT_BUCKET)))),
            bucket: ABSENT_BUCKET,
            logging: enabled,
            delivers: false,
            reported: Some("S3.NoSuchBucket"),
        },
        Row {
            case: "a refused write reports nothing where the logging options are disabled",
            policy: None,
            bucket: BUCKET,
            logging: json!({
                "Enabled": false,
                "LogGroupName": LOG_GROUP,
                "LogStreamName": LOG_STREAM,
            }),
            delivers: false,
            reported: None,
        },
        Row {
            case: "a refused write reports nothing where the options name no log stream",
            policy: None,
            bucket: BUCKET,
            logging: json!({ "Enabled": true, "LogGroupName": LOG_GROUP }),
            delivers: false,
            reported: None,
        },
    ]
}

/// The events of [`LOG_STREAM`] once at least one is there, or a failure naming the row once
/// [`REPORT_DEADLINE`] has passed.
async fn reported(aws: &Aws, case: &str) -> Vec<Value> {
    let deadline = Instant::now() + REPORT_DEADLINE;
    loop {
        let events = aws.log_events(LOG_GROUP, LOG_STREAM).await;
        if !events.is_empty() {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a reported delivery error within {REPORT_DEADLINE:?}, got \
             none"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The keys `bucket` holds once at least one is there, or a failure naming the row.
async fn delivered(aws: &Aws, case: &str, bucket: &str) -> Vec<String> {
    let deadline = Instant::now() + REPORT_DEADLINE;
    loop {
        let keys = aws.keys(bucket).await;
        if !keys.is_empty() {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object within {REPORT_DEADLINE:?}, got none"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_destination_reports_the_delivery_it_did_not_take_and_nothing_else() {
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
        let role_arn = aws.create_role("firehose-delivery", row.policy.clone()).await;

        let stream_arn = aws
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": STREAM,
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": role_arn,
                        "BucketARN": bucket_arn(row.bucket),
                        "Prefix": PREFIX,
                        // The shortest buffering the hints admit, so one put is one
                        // delivery attempt.
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                        "CloudWatchLoggingOptions": row.logging.clone(),
                    },
                }),
            )
            .await["DeliveryStreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers the stream's ARN"))
            .to_string();
        tokio::time::sleep(PAST_THE_TRANSITION).await;

        aws.firehose_ok(
            "PutRecord",
            json!({
                "DeliveryStreamName": STREAM,
                "Record": { "Data": aws_smithy_types::base64::encode(b"an archived record") },
            }),
        )
        .await;

        if row.delivers {
            let keys = delivered(&aws, case, row.bucket).await;
            assert!(
                keys.iter().all(|key| key.starts_with(PREFIX)),
                "case {case:?}: every delivered object goes under {PREFIX:?}, got {keys:?}"
            );
        }

        match row.reported {
            Some(error_code) => {
                let events = reported(&aws, case).await;
                let message = events[0]["message"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: an event carries a message"))
                    .to_string();
                let document: Value = serde_json::from_str(&message).unwrap_or_else(|error| {
                    panic!("case {case:?}: the event is a JSON document: {error} in {message}")
                });
                assert_eq!(
                    document["errorCode"],
                    json!(error_code),
                    "case {case:?}: expected errorCode {error_code:?}, got {document}"
                );
                assert_eq!(
                    document["deliveryStreamARN"],
                    json!(stream_arn),
                    "case {case:?}: the event names the stream it belongs to, got {document}"
                );
                assert_eq!(
                    document["destination"],
                    json!(bucket_arn(row.bucket)),
                    "case {case:?}: the event names the destination, got {document}"
                );
                assert_eq!(
                    document["deliveryStreamVersionId"],
                    json!(1),
                    "case {case:?}: the event names the stream's version, got {document}"
                );
                assert!(
                    document["message"].as_str().is_some_and(|message| {
                        !message.is_empty() && message != error_code
                    }),
                    "case {case:?}: the event states what the code means, got {document}"
                );
                // Nothing of a refused delivery reaches the destination. A destination whose
                // bucket does not exist holds nothing to read, which is the condition this
                // row is about.
                if row.bucket != ABSENT_BUCKET {
                    let keys = aws.keys(row.bucket).await;
                    assert!(
                        keys.is_empty(),
                        "case {case:?}: a refused delivery writes no object, got {keys:?}"
                    );
                }
            }
            None => {
                // Read after the buffer's own window and a retry beyond it, so this is the
                // absence of a report rather than one still owed.
                tokio::time::sleep(PAST_THE_DELIVERY_WINDOW).await;
                let events = aws.log_events(LOG_GROUP, LOG_STREAM).await;
                assert!(
                    events.is_empty(),
                    "case {case:?}: expected nothing reported, got {events:?}"
                );
                if !row.delivers {
                    // A delivery the role may not write is withheld whether the destination
                    // reports it or not.
                    let keys = aws.keys(row.bucket).await;
                    assert!(
                        keys.is_empty(),
                        "case {case:?}: a refused delivery writes no object, got {keys:?}"
                    );
                }
            }
        }
    }
}
