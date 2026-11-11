// SPDX-License-Identifier: CC0-1.0

//! What a handler ADMINISTERING an Amazon S3 bucket can do with the AWS SDK the runtime
//! supplies: Lambda, IAM, the token service, Amazon S3 and Amazon SQS on ONE engine, a
//! Python function whose code configures the bucket's event notifications, tags it, lists
//! the versions it holds and empties it, and every result read back where a client reads it.
//!
//! RULE: the AWS SDK a managed runtime supplies binds a service's own API, so a handler's
//! call on a bucket's configuration subresources or on its objects as a set is BUILT, signed
//! and dispatched to the emulated Amazon S3 service, and what comes back is that service's
//! own answer — the operation's output members for a success, and S3's own error code,
//! message and status for a refusal. No call of a service this emulator serves is answered
//! by the SDK layer itself.
//!
//! The rows differ along what the rule generalises over: WHICH operation class is called (a
//! subresource read whose answer is a nested container, a subresource replacement whose
//! input is one, a listing of every version a bucket holds, a removal of a set of objects in
//! one call), WHAT the container the call carries holds (one destination with a prefix rule,
//! one with a suffix rule and two events and an id of its own, one beside the EventBridge
//! container, the empty container that turns notifications off, one naming no events at
//! all), whether the destination VALIDATES or is refused, whether the validation is skipped,
//! and whether the bucket the objects are listed out of is versioning-enabled (so a version
//! carries a minted id) or not (so it carries the null one). Every success is read back
//! through Amazon S3's OWN API, so a call that answered without doing the work fails here,
//! and two last rows show the other half of the rule: a container whose own REQUIRED member
//! the handler's document does not carry stops the call rather than travelling emptied.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::given_role;
use crate::event_source_mappings::{await_function_active, call, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The bucket whose configuration subresources the handler administers.
const BUCKET: &str = "handler-administered-bucket";

/// The two buckets the handler empties, and the objects each holds: one versioning-enabled,
/// so each version carries a minted id, and one that is not, so each carries the null one.
const VERSIONED_BUCKET: &str = "handler-emptied-versioned";
const PLAIN_BUCKET: &str = "handler-emptied-plain";
const OBJECT_KEYS: [&str; 2] = ["reports/first.json", "reports/second.json"];

/// The id S3 reports for a version written while the bucket was not versioning-enabled
/// ("objects stored in your bucket before you set the versioning state have a version ID of
/// null", `ListObjectVersions`).
const NULL_VERSION: &str = "null";

/// Lambda, IAM, the token service, Amazon S3 and Amazon SQS over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own calls are dispatched back into
/// this same router, so the bucket it configures is the bucket this suite reads and the queue
/// it names is the queue Amazon SQS holds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One request to Amazon S3, made as a CLIENT makes one: its protocol is `restXml`, so the
/// request is a method and a path and the answer is a document.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One Amazon SQS call, which speaks an AWS JSON protocol, asserted to answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/sqs/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Create the queue `name` and answer the ARN Amazon SQS keyed it by, granting Amazon S3 the
/// `sqs:SendMessage` its notification publication needs when `grant_s3` says so.
///
/// S3 "verifies that any … Amazon Simple Queue Service (Amazon SQS) destination exists, and
/// that the bucket owner has permission to publish to it"
/// (`PutBucketNotificationConfiguration`), so a queue that granted it nothing is a
/// destination a configuration may NOT be recorded with — which is what the refused row
/// below rests on.
async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    grant_s3: bool,
) -> String {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateQueue must answer a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetQueueAttributes must answer a QueueArn, got {attributes}")
        })
        .to_string();
    if grant_s3 {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "s3.amazonaws.com" },
                "Action": "sqs:SendMessage",
                "Resource": arn,
            }],
        })
        .to_string();
        sqs(
            router,
            case,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": document } }),
        )
        .await;
    }
    arn
}

/// The handler this suite runs: one step per operation class the rule covers, each reported
/// under its own name so what is asserted is what the runtime gave the guest.
///
/// Only primitive values are reported, because the report travels back as the invocation's
/// JSON answer.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def _refusal(error):
    """The service's own code and sentence, as the SDK raised them."""
    return {
        "ok": False,
        "code": error.response["Error"]["Code"],
        "message": error.response["Error"]["Message"],
    }


def _configured(client, bucket):
    """What the bucket's notification container holds, read back through the SDK."""
    try:
        read = client.get_bucket_notification_configuration(Bucket=bucket)
    except botocore.exceptions.ClientError as error:
        return _refusal(error)
    queues = []
    for one in read.get("QueueConfigurations", []):
        rules = []
        key = (one.get("Filter") or {}).get("Key") or {}
        for rule in key.get("FilterRules", []):
            rules.append([rule.get("Name"), rule.get("Value")])
        queues.append(
            {
                "id": one.get("Id"),
                "arn": one.get("QueueArn"),
                "events": one.get("Events"),
                "rules": rules,
            }
        )
    return {
        "ok": True,
        "queues": queues,
        "topics": len(read.get("TopicConfigurations", [])),
        "functions": len(read.get("LambdaFunctionConfigurations", [])),
        "event_bridge": "EventBridgeConfiguration" in read,
    }


def lambda_handler(event, context):
    client = boto3.client("s3")
    bucket = event["bucket"]
    report = {"notifications": [], "emptied": []}

    # One row per notification container: what the put answered, and what the bucket carries
    # once it has.
    for row in event["containers"]:
        put = {"ok": True}
        try:
            client.put_bucket_notification_configuration(
                Bucket=bucket,
                NotificationConfiguration=row["configuration"],
                SkipDestinationValidation=row["skip_validation"],
            )
        except botocore.exceptions.ClientError as error:
            put = _refusal(error)
        except botocore.exceptions.SdkCallNotMade as error:
            # A call the SDK could not build at all, which is not an answer from the
            # service: it is reported as a call that was not made, carrying why.
            put = {"ok": False, "code": "SdkCallNotMade", "message": str(error)}
        report["notifications"].append(
            {"case": row["case"], "put": put, "read": _configured(client, bucket)}
        )

    # The bucket's own tag set: what it carries before one is written, the write, and what it
    # carries after.
    tagging = {}
    try:
        client.get_bucket_tagging(Bucket=bucket)
        tagging["before"] = {"ok": True}
    except botocore.exceptions.ClientError as error:
        tagging["before"] = _refusal(error)
    client.put_bucket_tagging(Bucket=bucket, Tagging={"TagSet": event["tags"]})
    read = client.get_bucket_tagging(Bucket=bucket)
    tagging["after"] = sorted([[tag["Key"], tag["Value"]] for tag in read.get("TagSet", [])])
    report["tagging"] = tagging

    # A container whose own REQUIRED member the document does not carry: the SDK does not
    # build the call at all, so nothing is replaced or removed.
    probes = {}
    attempts = (
        ("tagging", lambda: client.put_bucket_tagging(Bucket=bucket, Tagging={})),
        ("removal", lambda: client.delete_objects(Bucket=bucket, Delete={})),
    )
    for label, attempt in attempts:
        try:
            attempt()
            probes[label] = {"ok": True}
        except botocore.exceptions.ClientError as error:
            probes[label] = _refusal(error)
        except botocore.exceptions.SdkCallNotMade as error:
            probes[label] = {"ok": False, "code": "SdkCallNotMade", "message": str(error)}
    report["not_made"] = probes

    # Every version a bucket holds, removed in one call — the pass a provider that empties a
    # bucket makes.
    for row in event["object_buckets"]:
        held = client.list_object_versions(Bucket=row["bucket"])
        contents = list(held.get("Versions", [])) + list(held.get("DeleteMarkers", []))
        records = [{"Key": one["Key"], "VersionId": one["VersionId"]} for one in contents]
        removed = client.delete_objects(
            Bucket=row["bucket"], Delete={"Objects": records}
        )
        left = client.list_object_versions(Bucket=row["bucket"])
        report["emptied"].append(
            {
                "case": row["case"],
                "name": held.get("Name"),
                "listed": sorted([one["Key"] for one in contents]),
                "version_ids": sorted(set([one["VersionId"] for one in contents])),
                "truncated": held.get("IsTruncated"),
                "truncated_as_boolean": isinstance(held.get("IsTruncated"), bool),
                "deleted": sorted([one.get("Key") for one in removed.get("Deleted", [])]),
                "errors": [one.get("Code") for one in removed.get("Errors", [])],
                "left": [one["Key"] for one in left.get("Versions", [])],
            }
        )
    return report
"#;

/// The policy of the role the function runs as: a handler administering a bucket needs the
/// bucket's own operations, which is what this grants.
fn bucket_administration() -> Value {
    json!([{ "Effect": "Allow", "Action": "s3:*", "Resource": "*" }])
}

/// Create the function `name`, running as `role_arn`, out of [`HANDLER_SOURCE`].
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, name).await;
}

/// One row of the notification table: the container the handler puts, whether it asks for
/// the destination validation to be skipped, and what the bucket must carry afterwards.
struct Container {
    /// What the row is called in a failure.
    case: &'static str,
    /// The container the handler passes, as the operation's own input member.
    configuration: Value,
    /// Whether the row asks S3 to skip validating the destinations it names.
    skip_validation: bool,
    /// The code S3 refuses this row with, or `None` for a row it serves.
    refused_with: Option<&'static str>,
    /// What the refusal's own sentence must carry, where the row states it.
    refusal_names: Option<&'static str>,
    /// The queue destinations the bucket carries once the row has run.
    expected_queues: Vec<ExpectedQueue>,
    /// Whether the bucket carries the EventBridge container afterwards.
    expected_event_bridge: bool,
}

/// One queue destination a configured bucket carries, as a reader of it sees it.
struct ExpectedQueue {
    /// The ARN of the queue the notifications are published to.
    arn: String,
    /// The events it is published for.
    events: Vec<&'static str>,
    /// Its key-name filter rules, each a name and the value it matches on.
    rules: Vec<(&'static str, &'static str)>,
}

/// The notification containers the handler puts, in order: each row leaves the bucket
/// carrying what the row after it reads, so the refused row is also where the atomicity of a
/// put is observed.
fn containers(queue_arn: &str, ungranted_arn: &str, absent_arn: &str) -> Vec<Container> {
    vec![
        Container {
            case: "one queue destination with a prefix rule",
            configuration: json!({
                "QueueConfigurations": [{
                    "QueueArn": queue_arn,
                    "Events": ["s3:ObjectCreated:*"],
                    "Filter": { "Key": { "FilterRules": [
                        { "Name": "prefix", "Value": "reports/" },
                    ] } },
                }],
            }),
            skip_validation: false,
            refused_with: None,
            refusal_names: None,
            // The request vocabulary of `FilterRuleName` is lower cased (`prefix`,
            // `suffix`), and what S3 stores and reports to every reader of the bucket's
            // notification document is the capitalised spelling, so a rule is read back
            // under that one.
            expected_queues: vec![ExpectedQueue {
                arn: queue_arn.to_string(),
                events: vec!["s3:ObjectCreated:*"],
                rules: vec![("Prefix", "reports/")],
            }],
            expected_event_bridge: false,
        },
        Container {
            case: "an id of its own, two events and a suffix rule",
            configuration: json!({
                "QueueConfigurations": [{
                    "Id": "named-by-the-caller",
                    "QueueArn": queue_arn,
                    "Events": ["s3:ObjectCreated:Put", "s3:ObjectRemoved:Delete"],
                    "Filter": { "Key": { "FilterRules": [
                        { "Name": "suffix", "Value": ".json" },
                    ] } },
                }],
            }),
            skip_validation: false,
            refused_with: None,
            refusal_names: None,
            expected_queues: vec![ExpectedQueue {
                arn: queue_arn.to_string(),
                events: vec!["s3:ObjectCreated:Put", "s3:ObjectRemoved:Delete"],
                rules: vec![("Suffix", ".json")],
            }],
            expected_event_bridge: false,
        },
        Container {
            case: "a queue destination beside the EventBridge container",
            configuration: json!({
                "QueueConfigurations": [{
                    "QueueArn": queue_arn,
                    "Events": ["s3:ObjectCreated:*"],
                }],
                "EventBridgeConfiguration": {},
            }),
            skip_validation: false,
            refused_with: None,
            refusal_names: None,
            expected_queues: vec![ExpectedQueue {
                arn: queue_arn.to_string(),
                events: vec!["s3:ObjectCreated:*"],
                rules: Vec::new(),
            }],
            expected_event_bridge: true,
        },
        Container {
            case: "the empty container, which turns notifications off",
            configuration: json!({}),
            skip_validation: false,
            refused_with: None,
            refusal_names: None,
            expected_queues: Vec::new(),
            expected_event_bridge: false,
        },
        Container {
            case: "a queue whose policy lets Amazon S3 publish nothing",
            configuration: json!({
                "QueueConfigurations": [{
                    "QueueArn": ungranted_arn,
                    "Events": ["s3:ObjectCreated:*"],
                }],
            }),
            skip_validation: false,
            // The put is atomic, so the bucket still carries what the row before it left.
            refused_with: Some("InvalidArgument"),
            // The sentence S3 sends for this refusal is fixed text; which destinations
            // failed travels in the numbered argument series beside it.
            refusal_names: Some("Unable to validate the following destination configurations"),
            expected_queues: Vec::new(),
            expected_event_bridge: false,
        },
        Container {
            case: "a destination naming no events at all",
            configuration: json!({
                "QueueConfigurations": [{ "QueueArn": ungranted_arn }],
            }),
            skip_validation: false,
            // `Events` is a required member of a destination, so the SDK does not build the
            // call at all: it is reported as a call that was NOT MADE, naming the member,
            // rather than sent with that member emptied.
            refused_with: Some("SdkCallNotMade"),
            refusal_names: Some("Events"),
            expected_queues: Vec::new(),
            expected_event_bridge: false,
        },
        Container {
            case: "a destination that does not exist, with its validation skipped",
            configuration: json!({
                "QueueConfigurations": [{
                    "QueueArn": absent_arn,
                    "Events": ["s3:ObjectCreated:*"],
                }],
            }),
            skip_validation: true,
            refused_with: None,
            refusal_names: None,
            expected_queues: vec![ExpectedQueue {
                arn: absent_arn.to_string(),
                events: vec!["s3:ObjectCreated:*"],
                rules: Vec::new(),
            }],
            expected_event_bridge: false,
        },
    ]
}

/// The tag set the handler writes on the bucket.
const TAGS: [(&str, &str); 2] = [("owner", "platform"), ("auto-delete", "true")];

/// Invoke `name`, handing it the table of containers, the tag set and the buckets to empty.
async fn invoked(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    rows: &[Container],
) -> Value {
    let containers: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "case": row.case,
                "configuration": row.configuration,
                "skip_validation": row.skip_validation,
            })
        })
        .collect();
    let tags: Vec<Value> = TAGS
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect();
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &json!({
            "bucket": BUCKET,
            "containers": containers,
            "tags": tags,
            "object_buckets": [
                { "case": "a versioning-enabled bucket", "bucket": VERSIONED_BUCKET },
                { "case": "a bucket that is not versioning-enabled", "bucket": PLAIN_BUCKET },
            ],
        }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// Create the bucket `name`, and the objects of [`OBJECT_KEYS`] in it, turning versioning on
/// first when `versioned` says so.
async fn given_bucket(router: &MultiServiceRouter, case: &str, name: &str, versioned: bool) {
    let (created, body) = s3(router, "PUT", &format!("/{name}"), b"").await;
    assert_eq!(
        created, 200,
        "case {case:?}: expected CreateBucket to answer 200, got {created}: {body}"
    );
    if versioned {
        let (enabled, body) = s3(
            router,
            "PUT",
            &format!("/{name}?versioning"),
            b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
        )
        .await;
        assert_eq!(
            enabled, 200,
            "case {case:?}: expected PutBucketVersioning to answer 200, got {enabled}: {body}"
        );
    }
    for key in OBJECT_KEYS {
        let (stored, body) = s3(
            router,
            "PUT",
            &format!("/{name}/{key}"),
            format!("{{\"key\": \"{key}\"}}").as_bytes(),
        )
        .await;
        assert_eq!(
            stored, 200,
            "case {case:?}: expected PutObject to answer 200 for {key}, got {stored}: {body}"
        );
    }
}

/// Everything a handler administering a bucket does, in one invocation: the notification
/// container in each of its shapes, the tag set, the version listing and the set removal —
/// and every one of them read back through Amazon S3's own API.
#[tokio::test]
async fn a_handler_administers_a_bucket_with_what_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler administering a bucket through the runtime's SDK";
    let router = emulator();
    given_bucket(&router, case, BUCKET, false).await;
    given_bucket(&router, case, VERSIONED_BUCKET, true).await;
    given_bucket(&router, case, PLAIN_BUCKET, false).await;
    let queue_arn = given_queue(&router, case, "handler-notified-queue", true).await;
    let ungranted_arn = given_queue(&router, case, "handler-ungranted-queue", false).await;
    // A queue ARN Amazon SQS holds no queue for: the destination a skipped validation is
    // asked for over.
    let absent_arn = queue_arn.replace("handler-notified-queue", "handler-absent-queue");

    let role = given_role(
        &router,
        case,
        "handler-bucket-admin-role",
        Some(bucket_administration()),
    )
    .await;
    let function = "handler-bucket-administrator";
    given_function(&router, case, function, &role).await;
    let rows = containers(&queue_arn, &ungranted_arn, &absent_arn);
    let report = invoked(&router, case, function, &rows).await;

    // One assertion block per notification row: what the put answered, and what the bucket
    // carries once it has.
    let answered = report["notifications"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a report per row, got {report}"));
    assert_eq!(
        answered.len(),
        rows.len(),
        "case {case:?}: expected one report per container, got {report}"
    );
    for (row, reported) in rows.iter().zip(answered) {
        let put = &reported["put"];
        match row.refused_with {
            None => assert_eq!(
                put["ok"],
                json!(true),
                "case {:?}: expected the put to be served, got {put}",
                row.case
            ),
            Some(code) => {
                assert_eq!(
                    put["code"],
                    json!(code),
                    "case {:?}: expected the service's own error code, got {put}",
                    row.case
                );
                if let Some(named) = row.refusal_names {
                    let message = put["message"].as_str().unwrap_or_default();
                    assert!(
                        message.contains(named),
                        "case {:?}: expected the refusal to carry {named:?}, got {message:?}",
                        row.case
                    );
                }
            }
        }
        let read = &reported["read"];
        assert_eq!(
            read["ok"],
            json!(true),
            "case {:?}: expected the container to be read back, got {read}",
            row.case
        );
        let queues = read["queues"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: expected the queue list, got {read}", row.case));
        assert_eq!(
            queues.len(),
            row.expected_queues.len(),
            "case {:?}: expected {} queue destination(s), got {read}",
            row.case,
            row.expected_queues.len()
        );
        for (carried, expected) in queues.iter().zip(&row.expected_queues) {
            assert_eq!(
                carried["arn"],
                json!(expected.arn),
                "case {:?}: expected the destination the put named, got {carried}",
                row.case
            );
            assert_eq!(
                carried["events"],
                json!(expected.events),
                "case {:?}: expected the events the put named, got {carried}",
                row.case
            );
            let rules: Vec<Value> = expected
                .rules
                .iter()
                .map(|(name, value)| json!([name, value]))
                .collect();
            assert_eq!(
                carried["rules"],
                json!(rules),
                "case {:?}: expected the key-name filter the put named, got {carried}",
                row.case
            );
            // S3 "mints an id for every configuration that named none", so a destination
            // always reads back under one.
            assert!(
                carried["id"].as_str().is_some_and(|id| !id.is_empty()),
                "case {:?}: expected the destination to carry an id, got {carried}",
                row.case
            );
        }
        assert_eq!(
            read["event_bridge"],
            json!(row.expected_event_bridge),
            "case {:?}: expected the EventBridge container to be {}, got {read}",
            row.case,
            row.expected_event_bridge
        );
    }

    // The configuration the LAST row left, read through Amazon S3's own API: what the handler
    // configured is what a client of the service sees.
    let (status, document) = s3(&router, "GET", &format!("/{BUCKET}?notification"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBucketNotificationConfiguration to answer 200, got \
         {status}: {document}"
    );
    assert!(
        document.contains(&absent_arn),
        "case {case:?}: expected the store to hold the destination the handler configured \
         ({absent_arn}), got {document}"
    );

    // The tag set: absent before the write, S3's own `NoSuchTagSet` for the read that found
    // none, and the set itself afterwards — in the store as well as in the handler's report.
    let tagging = &report["tagging"];
    assert_eq!(
        tagging["before"]["code"],
        json!("NoSuchTagSet"),
        "case {case:?}: expected the service's own code for a bucket carrying no tag set, got \
         {tagging}"
    );
    let mut expected: Vec<Value> = TAGS
        .iter()
        .map(|(key, value)| json!([key, value]))
        .collect();
    expected.sort_by_key(ToString::to_string);
    assert_eq!(
        tagging["after"],
        json!(expected),
        "case {case:?}: expected the tag set the handler wrote, got {tagging}"
    );
    let (status, document) = s3(&router, "GET", &format!("/{BUCKET}?tagging"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBucketTagging to answer 200, got {status}: {document}"
    );
    for (key, value) in TAGS {
        assert!(
            document.contains(key) && document.contains(value),
            "case {case:?}: expected the store to hold the tag {key}={value}, got {document}"
        );
    }

    // A container whose own required member the document does not carry stops the call: the
    // required member is never sent emptied, and the tag set the store holds above proves
    // the call that would have replaced it was not made.
    for (probe, named) in [("tagging", "TagSet"), ("removal", "Objects")] {
        let reported = &report["not_made"][probe];
        assert_eq!(
            reported["code"],
            json!("SdkCallNotMade"),
            "case {probe:?}: expected the call not to be made, got {reported}"
        );
        let message = reported["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(named),
            "case {probe:?}: expected the refusal to name {named:?}, got {message:?}"
        );
    }

    // One assertion block per emptied bucket: what was listed, what was removed, and what the
    // store holds afterwards.
    let emptied = report["emptied"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a report per bucket, got {report}"));
    assert_eq!(
        emptied.len(),
        2,
        "case {case:?}: expected one report per emptied bucket, got {report}"
    );
    for (reported, (bucket, versioned)) in emptied
        .iter()
        .zip([(VERSIONED_BUCKET, true), (PLAIN_BUCKET, false)])
    {
        let row = reported["case"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            reported["name"],
            json!(bucket),
            "case {row:?}: expected the listing to name the bucket, got {reported}"
        );
        assert_eq!(
            reported["listed"],
            json!(OBJECT_KEYS),
            "case {row:?}: expected every version the bucket holds, got {reported}"
        );
        assert_eq!(
            reported["truncated_as_boolean"],
            json!(true),
            "case {row:?}: expected IsTruncated as the boolean it is typed as, got {reported}"
        );
        assert_eq!(
            reported["truncated"],
            json!(false),
            "case {row:?}: expected one whole page, got {reported}"
        );
        let ids = reported["version_ids"]
            .as_array()
            .unwrap_or_else(|| panic!("case {row:?}: expected the version ids, got {reported}"))
            .clone();
        if versioned {
            assert_eq!(
                ids.len(),
                OBJECT_KEYS.len(),
                "case {row:?}: expected a minted id per version, got {reported}"
            );
            assert!(
                ids.iter().all(|id| id != &json!(NULL_VERSION)),
                "case {row:?}: expected every version to carry a minted id, got {reported}"
            );
        } else {
            assert_eq!(
                ids,
                vec![json!(NULL_VERSION)],
                "case {row:?}: expected the null version id, got {reported}"
            );
        }
        assert_eq!(
            reported["deleted"],
            json!(OBJECT_KEYS),
            "case {row:?}: expected every key to be reported removed, got {reported}"
        );
        assert_eq!(
            reported["errors"],
            json!([] as [Value; 0]),
            "case {row:?}: expected no per-object error, got {reported}"
        );
        assert_eq!(
            reported["left"],
            json!([] as [Value; 0]),
            "case {row:?}: expected the bucket to hold no version afterwards, got {reported}"
        );
        // What the STORE holds: every object the handler removed is gone from it.
        for key in OBJECT_KEYS {
            let (status, _) = s3(&router, "GET", &format!("/{bucket}/{key}"), b"").await;
            assert_eq!(
                status, 404,
                "case {row:?}: expected {key} to be gone from the store, got {status}"
            );
        }
    }
}
