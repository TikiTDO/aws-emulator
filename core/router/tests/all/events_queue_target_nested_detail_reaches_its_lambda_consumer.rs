// SPDX-License-Identifier: CC0-1.0

//! A typed, nested `PutEvents` detail crossing BOTH hops of the EventBridge → Amazon SQS →
//! Lambda-event-source-mapping chain, and arriving in Amazon DynamoDB as the consumer wrote
//! it.
//!
//! The rule pinned here: **an event published onto an event bus is delivered to every target
//! of every enabled rule whose `EventPattern` matches it, and the envelope a queue target
//! receives carries `id` as the `EventId` the publication was answered with, `source` and
//! `detail-type` exactly as submitted, and `detail` as the submitted JSON document with every
//! value's JSON type preserved — "a valid JSON object … the JSON object may contain fields and
//! nested sub-objects" (`PutEventsRequestEntry$Detail`) — so nested sub-objects, arrays,
//! fractional numbers, integers at the published bounds, booleans, nulls and strings needing
//! escapes all arrive as the same JSON values rather than stringified or coerced; an enabled
//! event source mapping on that queue then hands its function each message's body in
//! `Records[].body`, so the same document reaches the function's own writes, and the messages
//! the attempt processed are deleted, which drains the queue.**
//!
//! Six crates have to agree for that to hold (EventBridge matches and hands over, Amazon SQS
//! holds the queue and its policy, IAM answers for the execution role, CloudFormation archives
//! the inline handler into the function's deployment package, Lambda polls the queue and RUNS
//! the handler, DynamoDB serves the table it writes), so the suite lives here rather than in
//! any one of them. Every observation is one a client makes: the message on the rule's target
//! queue, the item in the table, the queue's own attributes.
//!
//! The rows differ along what the rule generalises over — the SHAPE of the submitted detail:
//! the nested domain payload (a sub-object, an array of objects, a fractional number, a
//! boolean and a JSON null), an object with no members at all, containers nested through
//! arrays and objects together, numbers at both published integer bounds beside empty
//! containers and an empty string, strings carrying quotes, backslashes, control characters
//! and non-ASCII text, and an array of mixed JSON types. The recorded case is ONE row.
//!
//! Each row asserts BOTH hops of the same publication, so a chain that delivers the envelope
//! but never invokes the consumer, and one that invokes it with a mangled document, are told
//! apart — and both are told apart from silence, which is what the whole chain failing looks
//! like from a client.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_OK;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for, and the Region the shared stack reader
/// ([`wait_for_stack_status`]) signs for.
const REGION: &str = "us-east-1";

/// The `StackStatus` the consumer's stack has to reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `Source` every event this suite publishes carries, and the one the rule matches on.
const SOURCE: &str = "sim2real.nested-detail.suite";

/// The names this suite's resources are created under.
const BUS: &str = "nested-detail-bus";
const RULE: &str = "nested-detail-rule";
const TABLE: &str = "nested-detail-landings";
const ROLE_NAME: &str = "nested-detail-consumer-role";
const FUNCTION: &str = "nested-detail-consumer";
const STACK: &str = "nested-detail-consumer-stack";
/// The queue the suite READS the envelope off, and the queue the mapping consumes. Both are
/// targets of the one rule, so one publication is observed on both hops without the suite's
/// own read competing with the poller for a message.
const INSPECT_QUEUE: &str = "nested-detail-inspect";
const CONSUME_QUEUE: &str = "nested-detail-consume";

/// How long each queue hides a message handed to a consumer for, as `VisibilityTimeout`
/// carries it: well past the function's own `Timeout`, so a message the mapping holds does not
/// come back visible mid-row.
const VISIBILITY_TIMEOUT: &str = "30";
/// See [`VISIBILITY_TIMEOUT`].
const FUNCTION_TIMEOUT: i64 = 10;

/// How long an observation of work the CLOCK owes — the table coming up, the mapping settling,
/// a delivery, an invocation's own write, the queue draining — waits before giving up, and how
/// long between two looks.
const BUDGET: Duration = Duration::from_secs(45);
/// See [`BUDGET`].
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The handler the stack deploys: it writes the `detail` of every record it is handed back out
/// as JSON text, keyed by the envelope's own event id, so what the second hop carried is
/// readable through DynamoDB's own strongly consistent read.
const CONSUMER_SOURCE: &str = r#"
import json
import os

import boto3


def handler(event, context):
    client = boto3.client("dynamodb")
    written = 0
    for record in event["Records"]:
        envelope = json.loads(record["body"])
        client.put_item(
            TableName=os.environ["TABLE"],
            Item={
                "pk": {"S": envelope["id"]},
                "detail_json": {"S": json.dumps(envelope["detail"])},
                "source": {"S": envelope["source"]},
                "detail_type": {"S": envelope["detail-type"]},
            },
        )
        written += 1
    return {"written": written}
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched
/// back into this same router, so the table it writes is the table this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: every call carries a credential scope so
    // the router can name the service it belongs to.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire, with whatever status it answered.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> (u16, Bytes) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    (parts.status.as_u16(), body)
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, answered) = dispatch(
        router,
        "POST",
        &format!("{signing_name}.{REGION}.amazonaws.com"),
        "/",
        &[
            (
                "content-type",
                &format!("application/x-amz-json-{json_version}"),
            ),
            ("x-amz-target", &format!("{target_prefix}.{operation}")),
            ("authorization", &authorization(signing_name)),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (status, parsed)
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer
/// 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "dynamodb",
        "DynamoDB_20120810",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One AWS Lambda call over its `restJson1` wire, with whatever status it answered.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let (status, answered) = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (status, parsed)
}

/// One awsQuery call of `signing_name` at `host`, as its status and XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
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
    let (status, answered) = dispatch(
        router,
        "POST",
        host,
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization(signing_name)),
        ],
        Bytes::from(body),
    )
    .await;
    (status, String::from_utf8_lossy(&answered).to_string())
}

/// One IAM call that must answer 200.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(
        router,
        "iam",
        "iam.amazonaws.com",
        "2010-05-08",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} with {body}"
    );
    body
}

/// One CloudFormation call that must answer 200.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "2010-05-15",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// The table the consumer writes, with the one string hash key the recorded client used;
/// answers the ARN DynamoDB reports for it.
async fn given_table(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
        router,
        "setup",
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateTable reports no TableArn, got {created}"))
        .to_string()
}

/// Wait until DynamoDB first reports the table `ACTIVE`: "you can perform read and write
/// operations only on an ACTIVE table".
async fn await_table_active(router: &MultiServiceRouter) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(router, "setup", "DescribeTable", &json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: DynamoDB never reported {TABLE} ACTIVE, last read {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A queue hiding a delivered message for [`VISIBILITY_TIMEOUT`]; answers its url and the ARN
/// Amazon SQS itself reports for it.
async fn given_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(
        router,
        "setup",
        "CreateQueue",
        &json!({
            "QueueName": name,
            "Attributes": { "VisibilityTimeout": VISIBILITY_TIMEOUT },
        }),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateQueue {name:?} reports no QueueUrl, got {created}"))
        .to_string();
    let arn = queue_attributes(router, "setup", &url, &["QueueArn"]).await["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the queue {name:?} reports no QueueArn"))
        .to_string();
    (url, arn)
}

/// The named attributes of the queue `url` addresses, as `GetQueueAttributes` reports them.
async fn queue_attributes(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    names: &[&str],
) -> Value {
    let read = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": names }),
    )
    .await;
    read["Attributes"].clone()
}

/// Put the queue policy EventBridge's delivery needs on the queue `url` addresses: the
/// `sqs:SendMessage` grant to the service principal, scoped to the rule making the delivery —
/// "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
/// resource-based policies" (`PutTargets`).
async fn allow_the_rule_to_send(
    router: &MultiServiceRouter,
    url: &str,
    queue_arn: &str,
    rule_arn: &str,
) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridgeRule",
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": rule_arn } },
        }],
    });
    sqs(
        router,
        "setup",
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
}

/// The execution role the consumer runs as: trusted by Lambda, granting the poller's own reads
/// of the consumed queue and the write the handler makes. Answers its ARN as IAM reports it.
async fn given_role(router: &MultiServiceRouter, queue_arn: &str, table_arn: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    let arn = element(&created, "Arn");
    // The reads a queue mapping's poller makes on its source, and the write the handler makes
    // with temporary credentials for this role.
    let permissions = json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": [
                    "sqs:ReceiveMessage",
                    "sqs:DeleteMessage",
                    "sqs:GetQueueAttributes",
                ],
                "Resource": queue_arn,
            },
            {
                "Effect": "Allow",
                "Action": "dynamodb:PutItem",
                "Resource": table_arn,
            },
        ],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "consume-the-queue"),
            ("PolicyDocument", &permissions),
        ],
    )
    .await;
    arn
}

/// Deploy the consumer: a stack holding the one function, whose inline code CloudFormation
/// archives into the deployment package Lambda runs ("the cfnresponse module is available only
/// when you use the `ZipFile` property to write your function's source code" — inline code is
/// the form a template declares a runnable package in).
async fn given_function(router: &MultiServiceRouter, role_arn: &str) {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Consumer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Environment": { "Variables": { "TABLE": TABLE } },
                    "Code": { "ZipFile": CONSUMER_SOURCE },
                },
            },
        },
    })
    .to_string();
    cloudformation(
        router,
        "CreateStack",
        &[("StackName", STACK), ("TemplateBody", &template)],
    )
    .await;
    let (reached, described) = wait_for_stack_status(router, STACK, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "setup: expected the consumer's stack to reach {CREATE_COMPLETE}, got {described}"
    );
    await_function_active(router).await;
}

/// Wait until Lambda first reports the function `Active` — the state an invocation needs.
async fn await_function_active(router: &MultiServiceRouter) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if status == 200 && configuration["State"].as_str() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: Lambda never reported {FUNCTION} Active, last read {status} {configuration}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The enabled event source mapping on the consumed queue, one message per batch as the
/// recorded client created it; answers its `UUID` once Lambda reports it `Enabled`.
async fn given_mapping(router: &MultiServiceRouter, queue_arn: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/event-source-mappings",
        Some(&json!({
            "FunctionName": FUNCTION,
            "EventSourceArn": queue_arn,
            "BatchSize": 1,
            "Enabled": true,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: expected HTTP 202 from CreateEventSourceMapping, got {status} with {created}"
    );
    let uuid = created["UUID"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created mapping reports no UUID, got {created}"))
        .to_string();
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/event-source-mappings/{uuid}"),
            None,
        )
        .await;
        if status == 200 && read["State"].as_str() == Some("Enabled") {
            return uuid;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the mapping {uuid} never reported Enabled, last read {status} {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The ONE message waiting on the queue `url` addresses, as its parsed body — polled for as
/// long as [`BUDGET`], and deleted, so what a later look finds is a later delivery.
async fn await_one_message(router: &MultiServiceRouter, case: &str, url: &str) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let received = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if !messages.is_empty() {
            assert_eq!(
                messages.len(),
                1,
                "case {case:?}: expected exactly one envelope message per matched rule, got \
                 {messages:?}"
            );
            let message = &messages[0];
            let text = message["Body"].as_str().unwrap_or_default().to_string();
            sqs(
                router,
                case,
                "DeleteMessage",
                &json!({
                    "QueueUrl": url,
                    "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
                }),
            )
            .await;
            return serde_json::from_str(&text).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected the EventBridge envelope as a JSON document, got \
                     {text:?} ({error})"
                )
            });
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no message ever arrived on the rule's queue target {url}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The item the consumer wrote under `key`, read through DynamoDB's own strongly consistent
/// read and polled for as long as [`BUDGET`].
async fn await_item(router: &MultiServiceRouter, case: &str, key: &str) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = dynamodb(
            router,
            case,
            "GetItem",
            &json!({
                "TableName": TABLE,
                "Key": { "pk": { "S": key } },
                "ConsistentRead": true,
            }),
        )
        .await;
        if read["Item"].is_object() {
            return read["Item"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no DynamoDB write for the event {key:?}, last read {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the queue `url` addresses holds nothing and has nothing in flight: the counts a
/// client reads a drained queue by.
async fn await_drained(router: &MultiServiceRouter, case: &str, url: &str) {
    let names = [
        "ApproximateNumberOfMessages",
        "ApproximateNumberOfMessagesNotVisible",
    ];
    let deadline = Instant::now() + BUDGET;
    loop {
        let attributes = queue_attributes(router, case, url, &names).await;
        let counted = |name: &str| attributes[name].as_str().unwrap_or("0").to_string();
        if names.iter().all(|name| counted(name) == "0") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the consumed queue to drain to 0/0, it reports \
             {attributes}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the `DetailType` it publishes under, and the `Detail` document it submits.
struct Row {
    case: &'static str,
    detail_type: &'static str,
    detail: Value,
}

/// The rows: one per SHAPE of submitted detail the rule has to carry through unchanged.
///
/// The integer bounds in the fourth row are the ones this operation publishes for the JSON
/// number (integer) data type — "a minimum value of -9,223,372,036,854,775,808 … a maximum
/// value of 9,223,372,036,854,775,807" (`smithy_query(query="doc",shape="PutEvents")`) — so
/// the row carries each published bound rather than a value in the middle of the range.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a nested domain payload: sub-object, array of objects, fraction, boolean, null",
            detail_type: "sim2real.nested-detail.test",
            detail: json!({
                "marker": "row-nested",
                "total": 19.5,
                "expedited": true,
                "giftNote": null,
                "customer": { "name": "Ada Lovelace", "tier": "gold" },
                "items": [
                    { "sku": "sku-alpha", "qty": 2 },
                    { "sku": "sku-beta", "qty": 7 },
                ],
            }),
        },
        Row {
            case: "an object with no members at all",
            detail_type: "sim2real.nested-detail.empty",
            detail: json!({}),
        },
        Row {
            case: "containers nested through arrays and objects together",
            detail_type: "sim2real.nested-detail.deep",
            detail: json!({
                "marker": "row-deep",
                "a": { "b": { "c": { "d": [[1, 2], [3, [4, { "e": null }]]] } } },
            }),
        },
        Row {
            case: "numbers at both published integer bounds, beside empty containers",
            detail_type: "sim2real.nested-detail.numbers",
            detail: json!({
                "marker": "row-numbers",
                "minimum": i64::MIN,
                "maximum": i64::MAX,
                "zero": 0,
                "negativeFraction": -12.25,
                "emptyString": "",
                "emptyArray": [],
                "emptyObject": {},
            }),
        },
        Row {
            case: "strings carrying quotes, backslashes, control characters and non-ASCII text",
            detail_type: "sim2real.nested-detail.strings",
            detail: json!({
                "marker": "row-strings",
                "quoted": "she said \"ship it\"",
                "escaped": "a\\b",
                "control": "first\nsecond\ttabbed",
                "unicode": "café ☕ 日本語",
                "slashed": "a/b",
            }),
        },
        Row {
            case: "an array of mixed JSON types",
            detail_type: "sim2real.nested-detail.mixed",
            detail: json!({
                "marker": "row-mixed",
                "flags": [true, false, null],
                "mixed": [1, "two", { "three": 3.5 }, [4]],
            }),
        },
    ]
}

/// RULE: every row's submitted `detail` reaches the rule's queue target as the same JSON
/// document, and reaches the function the queue's event source mapping invokes, whose own
/// write holds that document too.
#[tokio::test]
async fn a_nested_detail_survives_the_queue_target_and_the_lambda_mapping_that_consumes_it() {
    let router = emulator();

    // The table the consumer writes, the two queues the rule targets, and the bus and rule
    // that match the publications below.
    let table_arn = given_table(&router).await;
    await_table_active(&router).await;
    let (inspect_url, inspect_arn) = given_queue(&router, INSPECT_QUEUE).await;
    let (consume_url, consume_arn) = given_queue(&router, CONSUME_QUEUE).await;
    events(&router, "setup", "CreateEventBus", &json!({ "Name": BUS })).await;
    let rule = events(
        &router,
        "setup",
        "PutRule",
        &json!({
            "Name": RULE,
            "EventBusName": BUS,
            "EventPattern": json!({ "source": [SOURCE] }).to_string(),
        }),
    )
    .await;
    let rule_arn = rule["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: PutRule reports no RuleArn, got {rule}"))
        .to_string();
    for (url, arn) in [(&inspect_url, &inspect_arn), (&consume_url, &consume_arn)] {
        allow_the_rule_to_send(&router, url, arn, &rule_arn).await;
    }
    let targeted = events(
        &router,
        "setup",
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": BUS,
            "Targets": [
                { "Id": "inspect", "Arn": inspect_arn },
                { "Id": "consume", "Arn": consume_arn },
            ],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "setup: both queue targets must be accepted, got {targeted}"
    );

    // The consumer of the second queue: the role it runs as, the function itself, and the
    // enabled mapping that polls the queue for it.
    let role_arn = given_role(&router, &consume_arn, &table_arn).await;
    given_function(&router, &role_arn).await;
    let mapping = given_mapping(&router, &consume_arn).await;

    for row in rows() {
        let case = row.case;
        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "EventBusName": BUS,
                    "Source": SOURCE,
                    "DetailType": row.detail_type,
                    "Detail": row.detail.to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: the entry must be accepted, got {published}"
        );
        let event_id = published["Entries"][0]["EventId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: an accepted entry must carry an EventId, got {published}")
            })
            .to_string();

        // The first hop: the envelope on the rule's queue target.
        let envelope = await_one_message(&router, case, &inspect_url).await;
        assert_eq!(
            envelope["source"].as_str(),
            Some(SOURCE),
            "case {case:?}: expected the submitted source, got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"].as_str(),
            Some(row.detail_type),
            "case {case:?}: expected the submitted detail-type, got {envelope}"
        );
        assert_eq!(
            envelope["id"].as_str(),
            Some(event_id.as_str()),
            "case {case:?}: expected the envelope to carry the answered EventId \
             {event_id:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail"], row.detail,
            "case {case:?}: expected the submitted detail unchanged, got {}",
            envelope["detail"]
        );

        // The second hop: what the function the mapping invoked wrote for the same event.
        let item = await_item(&router, case, &event_id).await;
        let written = item["detail_json"]["S"].as_str().unwrap_or_default();
        let parsed: Value = serde_json::from_str(written).unwrap_or_else(|error| {
            panic!("case {case:?}: the consumer writes JSON text, got {written:?} ({error})")
        });
        assert_eq!(
            parsed, row.detail,
            "case {case:?}: expected the consumer to have been handed the submitted detail, \
             got {parsed}"
        );
        assert_eq!(
            item["source"]["S"].as_str(),
            Some(SOURCE),
            "case {case:?}: expected the consumer to have been handed the submitted source, \
             got {item}"
        );
        assert_eq!(
            item["detail_type"]["S"].as_str(),
            Some(row.detail_type),
            "case {case:?}: expected the consumer to have been handed the submitted \
             detail-type, got {item}"
        );
    }

    // What the mapping consumed it also deleted, so the queue it polls drains — and the
    // inspected queue holds nothing beyond the one message per publication read above.
    await_drained(&router, "the consumed queue", &consume_url).await;
    await_drained(&router, "the inspected queue", &inspect_url).await;
    // And the mapping itself reports the attempts it made as processed, which is what a
    // consumer that was handed every message, and whose function took each one, reports.
    let (status, read) = lambda(
        &router,
        "GET",
        &format!("/2015-03-31/event-source-mappings/{mapping}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the mapping to still be readable, got {status} with {read}"
    );
    assert_eq!(
        read["LastProcessingResult"].as_str(),
        Some(LAST_PROCESSING_RESULT_OK),
        "expected the mapping to report {LAST_PROCESSING_RESULT_OK:?} as the result of its \
         last processing attempt, got {read}"
    );
}
