// SPDX-License-Identifier: CC0-1.0

//! The CDK pattern a bucket's event notifications are deployed by: a stack whose
//! `Custom::S3BucketNotifications` resource is backed by a Lambda function that configures
//! the bucket through the AWS SDK its own runtime supplies.
//!
//! The rule under test: **the AWS SDK a managed Lambda runtime supplies binds the API of the
//! service a handler names, so a provider function configuring a bucket's notifications
//! reaches Amazon S3 itself — the configuration it puts is the configuration the bucket
//! carries, and the destination it names receives the events** — which is what makes the
//! custom resource behind `bucket.addEventNotification(...)` deployable at all.
//!
//! Four crates have to agree for that to hold (CloudFormation sends the provider request and
//! holds the resource until the provider answers, Lambda runs the handler and supplies it an
//! SDK bound to S3's API, S3 validates the destination and records the configuration, and
//! Amazon SQS holds the queue whose policy decides the validation and receives the
//! notification), so the suite lives here: a crate-local suite can only ever see one side of
//! it. The Lambda crate's `handler_bucket_administration_sdk` suite pins what the runtime's
//! SDK binds, and the S3 crate's `notification_destination_validation` suite pins the
//! validation; what only this suite can reach is a stack deploying the whole chain.
//!
//! The rows are the three stages of one deployment, each asserted where a client sees it: the
//! stack reaching `CREATE_COMPLETE`, the configuration the bucket reports afterwards, and the
//! events the queue receives — the prefix-filtered write that must reach it and the write
//! outside the filter that must not.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{element, elements, form_encode, members_of, outputs_of};

/// The Region every request here is signed for. It is the Region the shared stack reader
/// ([`stack_status`]) signs for too, so the stack this suite creates is the stack it reports.
const REGION: &str = "us-east-1";

/// The status the deployment of this suite must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long this suite waits for work the clock owes — a stack's resources coming up, a
/// provider answering, a notification being delivered — and how often it looks while waiting.
const WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// How long a read that must see NOTHING keeps looking before concluding nothing came.
const QUIET_WINDOW: Duration = Duration::from_secs(5);

/// The prefix the deployed configuration filters on, a key under it, and a key outside it.
const FILTERED_PREFIX: &str = "reports/";
const MATCHING_KEY: &str = "reports/first.json";
const UNMATCHED_KEY: &str = "archive/first.json";

/// The event class the deployed configuration publishes, and the two event names a reader of
/// the queue sees: the test notification a validated destination receives, and the record of
/// an object write.
const PUBLISHED_EVENT: &str = "s3:ObjectCreated:*";
const TEST_EVENT: &str = "s3:TestEvent";
const CREATED_EVENT_NAME: &str = "ObjectCreated:Put";

/// The provider function's source: the handler the CDK notifications custom resource ships,
/// in the shape it has — the request type decides the container, the container is put through
/// the runtime's own AWS SDK, and the outcome is PUT to the request's `ResponseURL`.
const PROVIDER_SOURCE: &str = r#"
import json
import logging
import urllib.request

import boto3

s3 = boto3.client("s3")

EVENTBRIDGE_CONFIGURATION = "EventBridgeConfiguration"
CONFIGURATION_TYPES = [
    "TopicConfigurations",
    "QueueConfigurations",
    "LambdaFunctionConfigurations",
]


def handler(event, context):
    response_status = "SUCCESS"
    error_message = ""
    try:
        props = event["ResourceProperties"]
        bucket = props["BucketName"]
        notification_configuration = props["NotificationConfiguration"]
        request_type = event["RequestType"]
        managed = props.get("Managed", "true").lower() == "true"
        skip_validation = props.get("SkipDestinationValidation", "false").lower() == "true"
        stack_id = event["StackId"]

        if managed:
            config = handle_managed(request_type, notification_configuration)
        else:
            config = handle_unmanaged(
                bucket, stack_id, request_type, notification_configuration
            )
        s3.put_bucket_notification_configuration(
            Bucket=bucket,
            NotificationConfiguration=config,
            SkipDestinationValidation=skip_validation,
        )
    except Exception as error:
        logging.exception("Failed to put bucket notification configuration")
        response_status = "FAILED"
        error_message = "Error: %s. " % (error,)
    finally:
        submit_response(event, context, response_status, error_message)


def handle_managed(request_type, notification_configuration):
    if request_type == "Delete":
        return {}
    return notification_configuration


def handle_unmanaged(bucket, stack_id, request_type, notification_configuration):
    external = find_external_notifications(bucket, stack_id)
    if request_type == "Delete":
        return external
    notifications = {}
    for one in CONFIGURATION_TYPES:
        incoming = notification_configuration.get(one, [])
        notifications[one] = external.get(one, []) + incoming
    if EVENTBRIDGE_CONFIGURATION in notification_configuration:
        notifications[EVENTBRIDGE_CONFIGURATION] = notification_configuration[
            EVENTBRIDGE_CONFIGURATION
        ]
    elif EVENTBRIDGE_CONFIGURATION in external:
        notifications[EVENTBRIDGE_CONFIGURATION] = external[EVENTBRIDGE_CONFIGURATION]
    return notifications


def find_external_notifications(bucket, stack_id):
    existing = s3.get_bucket_notification_configuration(Bucket=bucket)
    external = {}
    for one in CONFIGURATION_TYPES:
        external[one] = [
            held
            for held in existing.get(one, [])
            if not held.get("Id", "").startswith("%s-" % (stack_id,))
        ]
    if EVENTBRIDGE_CONFIGURATION in existing:
        external[EVENTBRIDGE_CONFIGURATION] = existing[EVENTBRIDGE_CONFIGURATION]
    return external


def submit_response(event, context, response_status, error_message):
    body = json.dumps(
        {
            "Status": response_status,
            "Reason": error_message + "See the details in the function's log stream",
            "PhysicalResourceId": event.get("PhysicalResourceId")
            or event["LogicalResourceId"],
            "StackId": event["StackId"],
            "RequestId": event["RequestId"],
            "LogicalResourceId": event["LogicalResourceId"],
            "NoEcho": False,
        }
    ).encode("utf-8")
    headers = {"content-type": "", "content-length": str(len(body))}
    request = urllib.request.Request(
        url=event["ResponseURL"], headers=headers, data=body, method="PUT"
    )
    with urllib.request.urlopen(request) as answered:
        answered.read()
"#;

/// The emulator as it is ASSEMBLED, on one engine: the provider's own SDK call is dispatched
/// back into this same router, so the bucket it configures is the bucket this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation call (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Amazon SQS call (`awsJson1_0`), as its status and parsed body.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Dispatch one request against S3's REST surface, path-style, as its status and body text.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// Every resource the stack reports a FAILED status for, with the reason it gave — what a
/// client reads to see why a deployment did not complete, and what this suite quotes when it
/// did not.
async fn reported_failures(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (status, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    if status != 200 {
        return vec![format!("DescribeStackEvents answered {status}: {body}")];
    }
    members_of(&body)
        .into_iter()
        .filter_map(|member| {
            let resource_status = elements(member, "ResourceStatus").into_iter().next()?;
            if !resource_status.contains("FAILED") {
                return None;
            }
            let logical = elements(member, "LogicalResourceId")
                .into_iter()
                .next()
                .unwrap_or_default();
            let reason = elements(member, "ResourceStatusReason")
                .into_iter()
                .next()
                .unwrap_or_default();
            Some(format!("{logical} {resource_status}: {reason}"))
        })
        .collect()
}

/// The template this suite deploys: the queue and the policy that lets Amazon S3 publish to
/// it, the bucket, the provider function with its execution role, and the custom resource
/// that configures the bucket's notifications through that provider.
fn template(bucket: &str, queue: &str) -> Value {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": queue },
            },
            // The grant S3 needs to publish this bucket's notifications, scoped to the
            // publishing bucket exactly as the generated template scopes it.
            "QueuePolicy": {
                "Type": "AWS::SQS::QueuePolicy",
                "Properties": {
                    "Queues": [{ "Ref": "Queue" }],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "s3.amazonaws.com" },
                            "Action": "sqs:SendMessage",
                            "Resource": { "Fn::GetAtt": ["Queue", "Arn"] },
                            "Condition": {
                                "ArnLike": {
                                    "aws:SourceArn": { "Fn::GetAtt": ["Bucket", "Arn"] },
                                },
                            },
                        }],
                    },
                },
            },
            "Bucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": bucket },
            },
            "HandlerRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "configure-the-bucket",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "s3:PutBucketNotification",
                                    "s3:GetBucketNotification",
                                    "logs:*",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Handler": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["HandlerRole", "Arn"] },
                    "Timeout": 60,
                    "Code": { "ZipFile": PROVIDER_SOURCE },
                },
            },
            "BucketNotifications": {
                "Type": "Custom::S3BucketNotifications",
                // The configuration is validated against the queue's own policy, so the
                // grant has to be in place before the provider puts it.
                "DependsOn": ["QueuePolicy", "Handler"],
                "Properties": {
                    "ServiceToken": { "Fn::GetAtt": ["Handler", "Arn"] },
                    "BucketName": { "Ref": "Bucket" },
                    "NotificationConfiguration": {
                        "QueueConfigurations": [{
                            "Events": [PUBLISHED_EVENT],
                            "Filter": {
                                "Key": {
                                    "FilterRules": [
                                        { "Name": "prefix", "Value": FILTERED_PREFIX },
                                    ],
                                },
                            },
                            "QueueArn": { "Fn::GetAtt": ["Queue", "Arn"] },
                        }],
                    },
                    "Managed": "true",
                    "SkipDestinationValidation": "false",
                },
            },
        },
        "Outputs": {
            "QueueUrl": { "Value": { "Ref": "Queue" } },
            "BucketName": { "Value": { "Ref": "Bucket" } },
        },
    })
}

/// Create `stack` from `template`, wait for `CREATE_COMPLETE` and answer its outputs. A stack
/// that did not get there is quoted with every resource failure it reported.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> BTreeMap<String, String> {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let reported = awaited_status(router, stack, CREATE_COMPLETE).await;
    if reported.0 != CREATE_COMPLETE {
        let failures = reported_failures(router, stack).await;
        panic!(
            "case {case:?}: the stack must reach {CREATE_COMPLETE}, got {:?}; the failures it \
             reported: {failures:?}",
            reported.0
        );
    }
    outputs_of(&reported.1)
}

/// Poll `stack` until it reports `wanted`, and answer the last status it reported with the
/// answer it was read from.
async fn awaited_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = stack_status(router, stack).await;
    for _ in 0..looks {
        if reported.0 == wanted {
            return reported;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        reported = stack_status(router, stack).await;
    }
    reported
}

/// Every message waiting on the queue at `url`, as the documents the bodies carry. The read
/// takes each message off the queue, so a later read sees only what arrived after it.
async fn drained(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let mut found = Vec::new();
    loop {
        let (status, answered) = sqs(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "ReceiveMessage({url:?}) must be answered, got {status} with {answered}"
        );
        let messages = answered["Messages"].as_array().cloned().unwrap_or_default();
        if messages.is_empty() {
            return found;
        }
        for message in messages {
            let body = message["Body"].as_str().unwrap_or_default();
            found.push(serde_json::from_str(body).unwrap_or(Value::Null));
            if let Some(receipt) = message["ReceiptHandle"].as_str() {
                let (status, answered) = sqs(
                    router,
                    "DeleteMessage",
                    &json!({ "QueueUrl": url, "ReceiptHandle": receipt }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "DeleteMessage({url:?}) must be answered, got {status} with {answered}"
                );
            }
        }
    }
}

/// Wait until the queue at `url` holds a message, and answer every message it then holds;
/// a read that must see nothing stops looking after [`QUIET_WINDOW`].
async fn awaited_messages(
    router: &MultiServiceRouter,
    url: &str,
    expected: bool,
) -> Vec<Value> {
    let wait = if expected { WAIT } else { QUIET_WINDOW };
    let looks = wait.as_millis() / LOOK_INTERVAL.as_millis();
    let mut found = Vec::new();
    for _ in 0..looks {
        found.extend(drained(router, url).await);
        if expected && !found.is_empty() {
            return found;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    found
}

/// The whole deployment: the stack completes, the bucket reports the configuration the
/// provider put, the validated destination received the test notification, and the write the
/// filter admits reaches the queue while the write outside it does not.
#[tokio::test]
async fn a_stack_configures_bucket_notifications_through_its_provider_function() {
    let case = "a custom resource configuring a bucket's notifications";
    let router = build_router();
    let stack = "cdk-bucket-notifications";
    let bucket = format!("{stack}-bucket");
    let queue = format!("{stack}-queue");
    let outputs = deployed(
        &router,
        case,
        stack,
        &template(&bucket, &queue).to_string(),
    )
    .await;
    let queue_url = outputs
        .get("QueueUrl")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the queue's url"));

    // What the BUCKET carries, read through Amazon S3's own API: the destination and the
    // prefix rule the provider's call named.
    let (status, document) = s3(&router, "GET", &format!("/{bucket}?notification"), "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetBucketNotificationConfiguration must answer 200, got {status}: \
         {document}"
    );
    assert!(
        document.contains(&queue) && document.contains(FILTERED_PREFIX),
        "case {case:?}: expected the bucket to carry the queue destination and the prefix \
         rule, got {document}"
    );
    assert_eq!(
        element(&document, "Event"),
        PUBLISHED_EVENT,
        "case {case:?}: expected the event class the configuration named, got {document}"
    );

    // The test notification a validated queue destination receives, which is the observable
    // half of the validation S3 performed before recording the configuration.
    let delivered = awaited_messages(&router, &queue_url, true).await;
    assert!(
        delivered
            .iter()
            .any(|message| message["Event"].as_str() == Some(TEST_EVENT)),
        "case {case:?}: expected the validated destination to hold the test notification, got \
         {delivered:?}"
    );

    // The write the filter admits reaches the queue as an object-created record.
    let (status, body) = s3(
        &router,
        "PUT",
        &format!("/{bucket}/{MATCHING_KEY}"),
        "the object the notification names",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({MATCHING_KEY:?}) must succeed, got {status}: {body}"
    );
    let delivered = awaited_messages(&router, &queue_url, true).await;
    let records: Vec<Value> = delivered
        .iter()
        .filter_map(|message| message["Records"].as_array().cloned())
        .flatten()
        .collect();
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected one record for the write the filter admits, got {delivered:?}"
    );
    assert_eq!(
        records[0]["eventName"].as_str(),
        Some(CREATED_EVENT_NAME),
        "case {case:?}: expected the record of a put, got {}",
        records[0]
    );
    assert_eq!(
        records[0]["s3"]["object"]["key"].as_str(),
        Some(MATCHING_KEY),
        "case {case:?}: expected the record to name the written key, got {}",
        records[0]
    );

    // And the write OUTSIDE the filter reaches it with nothing.
    let (status, body) = s3(
        &router,
        "PUT",
        &format!("/{bucket}/{UNMATCHED_KEY}"),
        "the object no rule admits",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({UNMATCHED_KEY:?}) must succeed, got {status}: {body}"
    );
    let quiet = awaited_messages(&router, &queue_url, false).await;
    assert!(
        quiet.is_empty(),
        "case {case:?}: the filter excludes {UNMATCHED_KEY:?}, so no notification may be \
         published for it, got {quiet:?}"
    );
}
