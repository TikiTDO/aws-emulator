// SPDX-License-Identifier: CC0-1.0

//! What a LOGGING trail delivers for the calls of the account's OTHER services.
//!
//! Three services are mounted on ONE engine, because the behaviour spans them: CloudTrail
//! holds the trail and decides the delivery, Step Functions serves calls of its own, and the
//! object store is where the log files land. Every call goes over the wire the way a client
//! makes it, and the bucket is created, policied, listed and read through Amazon S3's OWN API
//! over the same in-process callback a handler uses — so a delivered log file counts only if
//! the object store itself answers with it.
//!
//! The rule under test: while a trail is logging, EVERY management call the account serves
//! that the trail's selectors admit is delivered into that trail's bucket, under the trail's
//! own `S3KeyPrefix`, whichever service served it — each record naming the SERVING service as
//! its `eventSource`, the operation as its `eventName`, that operation's read/write semantics
//! as `readOnly`, and the id the CALLER was answered under as `requestID`, so a client holding
//! a response finds the log file of that very call. A trail that has stopped logging has
//! nothing delivered for a later call.
//!
//! The neighbouring `trail_log_file_delivery` suite pins the same delivery over this
//! service's own calls and over the key layout; the rows here differ along the SERVICE that
//! served the call and along that call's read/write kind, which is what the rule generalises
//! over.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::emulator::{cloudtrail_bucket_policy, TARGET_PREFIX};

/// The Region every call below is signed for: the Region the trail is homed in, and so the
/// Region its log files are delivered under.
const REGION: &str = "us-east-1";

/// Step Functions' `X-Amz-Target` prefix (that service's model declares the service shape
/// `AWSStepFunctions`), and the `awsJson` version it speaks.
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";
const SFN_JSON_VERSION: &str = "1.0";

/// Step Functions' `sigv4Name`, which is what routes a request to it.
const SFN_SIGNING_NAME: &str = "states";

/// The `eventSource` CloudTrail stamps on a Step Functions record
/// (`smithy_query(query="service_info")` on that service ->
/// `awsService.cloudTrailEventSource: states.amazonaws.com`).
const SFN_EVENT_SOURCE: &str = "states.amazonaws.com";

/// This service's own `eventSource`, for the row served by the service that holds the trail.
const CLOUDTRAIL_EVENT_SOURCE: &str = "cloudtrail.amazonaws.com";

/// The `eventCategory` of a control-plane call's record, which is the category the trail's
/// advanced selector below admits.
const MANAGEMENT_CATEGORY: &str = "Management";

/// The header a response names the request it answers in, lowercased as HTTP compares it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The member a log file's document holds its records in.
const RECORDS_MEMBER: &str = "Records";

/// The first segment of a log-file key under the trail's own prefix, and the segment naming
/// the service whose files it holds.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";
const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";

/// What a log file's name ends with.
const LOG_FILE_SUFFIX: &str = ".json.gz";

/// Which service serves a row's call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Serves {
    /// Step Functions, over `awsJson1_0`.
    StepFunctions,
    /// CloudTrail itself, over `awsJson1_1`.
    CloudTrail,
}

/// One call of the account, and what the record delivered for it has to say.
struct Row {
    case: &'static str,
    serves: Serves,
    /// The operation called, which is also the `eventName` its record carries.
    operation: &'static str,
    /// The request body, built per row so a row can address what an earlier row created.
    body: Value,
    /// The `eventSource` the record names the serving service by.
    event_source: &'static str,
    /// The operation's own semantics, which is the record's `readOnly`.
    read_only: bool,
}

/// One answer as its caller reads it: the parsed body and the id the response named the
/// request by.
struct Answered {
    body: Value,
    request_id: String,
}

/// CloudTrail, Step Functions and the object store over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region and the service.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// One `X-Amz-Target` RPC call: its status, parsed body and the id it was answered under.
    async fn rpc(
        &self,
        signing_name: &str,
        json_version: &str,
        target: &str,
        body: &Value,
    ) -> (u16, Answered) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEMULATOR/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("x-amz-target", target)
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (
            status,
            Answered {
                body: parsed,
                request_id,
            },
        )
    }

    /// One CloudTrail call that has to be served.
    async fn cloudtrail(&self, case: &str, operation: &str, body: &Value) -> Answered {
        let (status, answered) = self
            .rpc(
                "cloudtrail",
                "1.1",
                &format!("{TARGET_PREFIX}{operation}"),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CloudTrail {operation}, got {status} with {}",
            answered.body
        );
        answered
    }

    /// One Step Functions call that has to be served.
    async fn step_functions(&self, case: &str, operation: &str, body: &Value) -> Answered {
        let (status, answered) = self
            .rpc(
                SFN_SIGNING_NAME,
                SFN_JSON_VERSION,
                &format!("{SFN_TARGET_PREFIX}.{operation}"),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Step Functions {operation}, got {status} \
             with {}",
            answered.body
        );
        answered
    }

    /// The call a row names, served by the service that owns it.
    async fn served(&self, row: &Row) -> Answered {
        match row.serves {
            Serves::StepFunctions => self.step_functions(row.case, row.operation, &row.body).await,
            Serves::CloudTrail => self.cloudtrail(row.case, row.operation, &row.body).await,
        }
    }

    /// The Amazon S3 client, over the same in-process callback the delivery uses — built by
    /// the same shared constructor, so the test's transport is the delivery's by
    /// construction.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create `bucket` and attach the policy that lets CloudTrail deliver into it — the
    /// `CreateBucket` and `PutBucketPolicy` a client makes before pointing a trail at it.
    async fn log_bucket(&self, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
        self.s3()
            .put_bucket_policy()
            .bucket(bucket)
            .policy(cloudtrail_bucket_policy(bucket))
            .send()
            .await
            .unwrap_or_else(|error| panic!("policying {bucket} must succeed, got {error:?}"));
    }

    /// Every object key the bucket holds under `prefix`, as Amazon S3 lists them.
    async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .send()
            .await
            .unwrap_or_else(|error| panic!("listing {bucket}/{prefix} must succeed, got {error:?}"));
        let mut keys: Vec<String> = listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect();
        keys.sort();
        keys
    }

    /// The records one delivered log file holds, read through `GetObject` and gunzipped.
    async fn records(&self, bucket: &str, key: &str) -> Vec<Value> {
        let object = self
            .s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("reading {key} must succeed, got {error:?}"));
        let body = object
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reading {key}'s body must succeed, got {error:?}"))
            .into_bytes();
        let mut document = String::new();
        flate2::read::GzDecoder::new(body.as_ref())
            .read_to_string(&mut document)
            .unwrap_or_else(|error| panic!("{key} must be a gzip stream, got {error}"));
        let parsed: Value = serde_json::from_str(&document)
            .unwrap_or_else(|error| panic!("{key} must gunzip to JSON, got {error}"));
        parsed
            .get(RECORDS_MEMBER)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("{key} must hold {RECORDS_MEMBER}, got {parsed}"))
    }

    /// Every record the trail delivered under `prefix`, with the key each one arrived in.
    async fn delivered(&self, bucket: &str, prefix: &str) -> Vec<(String, Value)> {
        let mut delivered = Vec::new();
        for key in self.keys_under(bucket, prefix).await {
            for record in self.records(bucket, &key).await {
                delivered.push((key.clone(), record));
            }
        }
        delivered
    }
}

/// The account every trail, bucket and record below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The trail's whole delivery area under the key prefix it was created with:
/// `<S3KeyPrefix>/AWSLogs/<account-id>/CloudTrail/`, which is where a consumer of the trail
/// looks for its files.
fn delivery_prefix(key_prefix: &str) -> String {
    format!(
        "{key_prefix}/{LOG_FILE_ROOT_SEGMENT}/{}/{LOG_FILE_SERVICE_SEGMENT}/",
        account()
    )
}

/// The text a record holds at `member`, or `""` for one that holds none.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The rows: one call per serving service and per read/write kind, each addressing only what
/// it created itself.
///
/// `ListStateMachines` and `DescribeActivity` only REPORT state and `CreateActivity` CHANGES
/// it, which is what each record's `readOnly` has to say; `GetTrailStatus` is the same
/// question asked of the service that holds the trail, so a trail logs its own service's
/// calls exactly as it logs another's.
fn rows(activity: &str, activity_arn: &str, trail: &str) -> Vec<Row> {
    vec![
        Row {
            case: "another service's mutating call",
            serves: Serves::StepFunctions,
            operation: "CreateActivity",
            body: json!({"name": activity}),
            event_source: SFN_EVENT_SOURCE,
            read_only: false,
        },
        Row {
            case: "another service's reporting call on a resource it holds",
            serves: Serves::StepFunctions,
            operation: "DescribeActivity",
            body: json!({"activityArn": activity_arn}),
            event_source: SFN_EVENT_SOURCE,
            read_only: true,
        },
        Row {
            case: "another service's reporting call over a collection",
            serves: Serves::StepFunctions,
            operation: "ListStateMachines",
            body: json!({}),
            event_source: SFN_EVENT_SOURCE,
            read_only: true,
        },
        Row {
            case: "a call of the service that holds the trail",
            serves: Serves::CloudTrail,
            operation: "GetTrailStatus",
            body: json!({"Name": trail}),
            event_source: CLOUDTRAIL_EVENT_SOURCE,
            read_only: true,
        },
    ]
}

/// Point a trail at `bucket` under `key_prefix`, give it the advanced selector that admits
/// management events, and start it logging.
async fn logging_trail(emulator: &Emulator, trail: &str, bucket: &str, key_prefix: &str) {
    emulator
        .cloudtrail(
            "setup",
            "CreateTrail",
            &json!({
                "Name": trail,
                "S3BucketName": bucket,
                "S3KeyPrefix": key_prefix,
                "IsMultiRegionTrail": false,
                "IncludeGlobalServiceEvents": false,
            }),
        )
        .await;
    emulator
        .cloudtrail(
            "setup",
            "PutEventSelectors",
            &json!({
                "TrailName": trail,
                "AdvancedEventSelectors": [{
                    "Name": "management",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": [MANAGEMENT_CATEGORY]}],
                }],
            }),
        )
        .await;
    emulator
        .cloudtrail("setup", "StartLogging", &json!({"Name": trail}))
        .await;
    let status = emulator
        .cloudtrail("setup", "GetTrailStatus", &json!({"Name": trail}))
        .await;
    assert_eq!(
        status.body.get("IsLogging"),
        Some(&json!(true)),
        "setup: expected the trail to report itself logging, got {}",
        status.body
    );
}

/// RULE: a logging trail delivers the record of EVERY management call the account serves,
/// whichever service served it, under the trail's own key prefix — naming the serving
/// service, the operation, its read/write kind and the id the caller was answered under.
#[tokio::test]
async fn a_logging_trail_delivers_the_calls_of_every_service_of_the_account() {
    let emulator = Emulator::new();
    let bucket = "cross-service-trail-logs";
    let trail = "cross-service-trail";
    let key_prefix = "obs/trail";
    let activity = "delivered-activity";
    let activity_arn = format!("arn:aws:states:{REGION}:{}:activity:{activity}", account());
    emulator.log_bucket(bucket).await;
    logging_trail(&emulator, trail, bucket, key_prefix).await;

    let rows = rows(activity, &activity_arn, trail);
    let mut answered_as: Vec<String> = Vec::new();
    for row in &rows {
        let answered = emulator.served(row).await;
        assert!(
            !answered.request_id.is_empty(),
            "case {:?}: the answer must name the request it answers, got {}",
            row.case,
            answered.body
        );
        answered_as.push(answered.request_id);
    }

    let delivered = emulator.delivered(bucket, &delivery_prefix(key_prefix)).await;
    for (row, request_id) in rows.iter().zip(&answered_as) {
        let found = delivered
            .iter()
            .find(|(_, record)| text(record, "requestID") == *request_id);
        let Some((key, record)) = found else {
            let names: Vec<String> = delivered
                .iter()
                .map(|(_, record)| text(record, "eventName"))
                .collect();
            panic!(
                "case {:?}: expected a delivered record of the call answered as \
                 {request_id:?}, got records of {names:?}",
                row.case
            );
        };
        assert_eq!(
            (
                text(record, "eventName").as_str(),
                text(record, "eventSource").as_str(),
                text(record, "eventCategory").as_str(),
            ),
            (row.operation, row.event_source, MANAGEMENT_CATEGORY),
            "case {:?}: expected the record of {} by {}, got {record}",
            row.case,
            row.operation,
            row.event_source
        );
        assert_eq!(
            record.get("readOnly"),
            Some(&json!(row.read_only)),
            "case {:?}: expected readOnly {}, got {record}",
            row.case,
            row.read_only
        );
        assert_eq!(
            (
                text(record, "awsRegion").as_str(),
                text(record, "recipientAccountId").as_str(),
            ),
            (REGION, account().as_str()),
            "case {:?}: expected the account's own record of this Region, got {record}",
            row.case
        );
        // The file it arrived in is under the trail's configured prefix and is a log file:
        // the object a consumer of this trail lists and reads.
        assert!(
            key.starts_with(&delivery_prefix(key_prefix)) && key.ends_with(LOG_FILE_SUFFIX),
            "case {:?}: expected the record under the trail's prefix in a {LOG_FILE_SUFFIX} \
             file, got {key:?}",
            row.case
        );
    }
}

/// RULE's other side: a trail that has STOPPED logging delivers nothing for a later call of
/// another service, while the same call before the stop was delivered.
#[tokio::test]
async fn a_stopped_trail_delivers_nothing_for_a_later_call_of_another_service() {
    let emulator = Emulator::new();
    let bucket = "stopped-trail-logs";
    let trail = "stopped-trail";
    let key_prefix = "obs/stopped";
    let prefix = delivery_prefix(key_prefix);
    emulator.log_bucket(bucket).await;
    logging_trail(&emulator, trail, bucket, key_prefix).await;

    let while_logging = emulator
        .step_functions("while logging", "ListStateMachines", &json!({}))
        .await;
    let delivered = emulator.delivered(bucket, &prefix).await;
    assert!(
        delivered
            .iter()
            .any(|(_, record)| text(record, "requestID") == while_logging.request_id),
        "case while logging: expected the call answered as {:?} delivered, got {} record(s)",
        while_logging.request_id,
        delivered.len()
    );

    emulator
        .cloudtrail("stopping", "StopLogging", &json!({"Name": trail}))
        .await;
    let before = emulator.keys_under(bucket, &prefix).await;
    let after_stop = emulator
        .step_functions("after the stop", "ListStateMachines", &json!({}))
        .await;
    let after = emulator.keys_under(bucket, &prefix).await;
    assert_eq!(
        after, before,
        "case after the stop: expected no further log file for the call answered as {:?}, \
         got {after:?}",
        after_stop.request_id
    );
}
