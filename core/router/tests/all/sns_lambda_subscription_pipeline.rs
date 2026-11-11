// SPDX-License-Identifier: CC0-1.0

//! A stack-deployed topic→function pipeline really runs the function: the stack is deployed in
//! CloudFormation, the subscription is a subscriber Amazon SNS holds, and a `Publish` to the
//! topic INVOKES the subscribed function, whose handler writes into an Amazon SQS queue the
//! stack never touched.
//!
//! The rule under test: **SNS delivers each published message to every confirmed subscription
//! of the topic over that subscription's own protocol, and for `lambda` ("delivery of
//! JSON-encoded message to an Lambda function", `SubscribeInput$Protocol`) the delivery is an
//! asynchronous invocation of the endpoint function with the notification inside the `Records`
//! event — performed as the service principal `sns.amazonaws.com`, so it happens only where
//! the FUNCTION's own resource-based policy grants `lambda:InvokeFunction` to that principal.**
//! Five crates have to agree for that to hold (CloudFormation makes the calls, IAM answers for
//! the execution role, SNS performs the delivery, Lambda holds the policy and runs the
//! function, SQS receives what the handler wrote), so the suite lives here: a crate-local test
//! can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the stack declares the
//! `AWS::Lambda::Permission` that grants SNS the invoke at all, and whether the grant it
//! declares is unscoped, scoped to the publishing topic, or scoped to another topic — each row
//! a stack of its own, so the grant a stack was CREATED with is what decides its deliveries.
//! Each row asserts both halves — the publish is answered with a MessageId either way, and the
//! handler's write is there or is not — so a delivery that never happens and a delivery nothing
//! authorized are told apart.
//!
//! The sibling suites state the same rule in the other two shapes it is observable in:
//! `sns_lambda_subscription_stack_invokes_its_function.rs` over a stack created with and
//! without the grant, and `sns_lambda_subscription_permission_boundary.rs` over ONE stack
//! UPDATED to add it. What this one adds is the grant's SCOPE as a stack declares it — the
//! unscoped form and the form scoped to another topic — over the assembled router
//! (`aws_emulator_router::build_router`), which is the seam a deployed client really reaches.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, elements, form_encode, outputs_of};

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to. It is the Region the shared stack waiter
/// ([`wait_for_stack_status`]) signs for too, so the stack this suite creates is the stack
/// that waiter polls.
const REGION: &str = "us-east-1";

/// The `StackStatus` a deployed row must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long to wait for work the clock owes, and how often to look while waiting.
const SETTLE_WAIT: Duration = Duration::from_secs(25);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row that must receive NOTHING keeps looking before concluding that nothing came
/// — well past the span an authorized delivery of the same publish takes (the invocation is
/// made before the publish is answered, and the handler runs a few hundred milliseconds later).
const QUIET_WINDOW: Duration = Duration::from_secs(5);

/// How long a row that must RECEIVE something keeps looking for it.
///
/// The same budget as the clock-owed work above ([`SETTLE_WAIT`]), because the delivery is the
/// same kind of work: the publish is answered before the function is invoked, and the handler
/// then runs in-process alongside every other test of this target. The loop below leaves as
/// soon as the sink holds what the row expects, so this bound only decides how long a LOADED
/// machine is given — it is not a window any assertion depends on. A row expecting nothing
/// keeps the quiet window above, so nothing about an unauthorized delivery is relaxed by it.
const DELIVERY_WAIT: Duration = Duration::from_secs(25);

/// How long EVERY row waits after the sink first holds what it expects, before the count is
/// asserted: a delivery that happened twice lands inside it.
const SETTLE_SLACK: Duration = Duration::from_secs(1);

/// The handler every row deploys: it forwards each notification it is invoked with to the
/// queue the stack passed it, through the AWS SDK the runtime supplies.
const FORWARDING_HANDLER: &str = r#"
import json
import os

import boto3


def handler(event, context):
    client = boto3.client("sqs")
    for record in event["Records"]:
        client.send_message(
            QueueUrl=os.environ["QUEUE_URL"],
            MessageBody=json.dumps(record["Sns"]),
        )
    return {"forwarded": len(event["Records"])}
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's SDK call is dispatched back
/// into this same router, so the queue it writes is the queue this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One awsQuery call of `signing_name`.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
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
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Amazon SQS call over its `awsJson1_0` wire, which must be answered.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
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
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The notifications waiting on the queue at `url`, as the `Sns` object each body carries.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.to_string()))
        })
        .collect()
}

/// The trust policy an execution role needs: Lambda has to be able to assume it.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// One row: what the stack declares about the grant SNS needs, and whether the published
/// notification must therefore reach the handler.
struct Row {
    case: &'static str,
    /// The `AWS::Lambda::Permission` the template declares, built from nothing — `None` for
    /// the bare subscription that declares no permission at all.
    permission: Option<fn() -> Value>,
    /// How many notifications the handler must have forwarded after one publish.
    forwarded: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "a bare subscription with no AWS::Lambda::Permission for SNS",
        permission: None,
        forwarded: 0,
    },
    Row {
        case: "a subscription beside the permission that grants SNS the invoke",
        permission: Some(|| {
            json!({
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                    "Principal": "sns.amazonaws.com",
                },
            })
        }),
        forwarded: 1,
    },
    Row {
        case: "that permission scoped to the publishing topic",
        permission: Some(|| {
            json!({
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                    "Principal": "sns.amazonaws.com",
                    "SourceArn": { "Ref": "Topic" },
                },
            })
        }),
        forwarded: 1,
    },
    Row {
        // The scoping is what the two rows above cannot tell apart on their own: a grant tied
        // to ANOTHER topic admits that topic's notifications and none of this topic's.
        case: "that permission scoped to ANOTHER topic",
        permission: Some(|| {
            json!({
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                    "Principal": "sns.amazonaws.com",
                    "SourceArn": { "Fn::Sub": "arn:aws:sns:${AWS::Region}:${AWS::AccountId}:elsewhere" },
                },
            })
        }),
        forwarded: 0,
    },
];

/// The template a row deploys: the execution role, the forwarding function, the topic, the
/// `lambda` subscription on it, and — where the row declares one — the permission that grants
/// SNS the invoke.
fn template(row: &Row, queue_url: &str) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "ExecRole".to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "AssumeRolePolicyDocument": trust_policy(),
                // A function calls another service with temporary credentials for this
                // role, so the role has to GRANT the write its handler makes: without the
                // grant the handler's own `sqs:SendMessage` is refused, which is a
                // different rule than the one these rows are about.
                "Policies": [{
                    "PolicyName": "forward-to-the-sink",
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": "sqs:SendMessage",
                            "Resource": "*",
                        }],
                    },
                }],
            },
        }),
    );
    resources.insert(
        "Forwarder".to_string(),
        json!({
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Role": { "Fn::GetAtt": ["ExecRole", "Arn"] },
                "Timeout": 30,
                "Environment": { "Variables": { "QUEUE_URL": queue_url } },
                "Code": { "ZipFile": FORWARDING_HANDLER },
            },
        }),
    );
    resources.insert(
        "Topic".to_string(),
        json!({ "Type": "AWS::SNS::Topic", "Properties": {} }),
    );
    resources.insert(
        "Subscription".to_string(),
        json!({
            "Type": "AWS::SNS::Subscription",
            "Properties": {
                "TopicArn": { "Ref": "Topic" },
                "Protocol": "lambda",
                "Endpoint": { "Fn::GetAtt": ["Forwarder", "Arn"] },
            },
        }),
    );
    if let Some(permission) = row.permission {
        resources.insert("InvokePermission".to_string(), permission());
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(resources),
        "Outputs": {
            "TopicArn": { "Value": { "Ref": "Topic" } },
            "FunctionName": { "Value": { "Ref": "Forwarder" } },
        },
    })
    .to_string()
}

/// Poll Lambda until it reports `name` `Active` — the state an invocation needs — and answer
/// the state it reported then. The span a created function spends `Pending` is that service's
/// own, so no fixed sleep names the instant it ends.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> String {
    let looks = SETTLE_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut last = String::new();
    for _ in 0..looks {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("/2015-03-31/functions/{name}/configuration"))
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("authorization", authorization("lambda"))
            .body(Bytes::new())
            .expect("a well-formed request");
        let response = router.dispatch(request).await;
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        last = parsed["State"].as_str().unwrap_or_default().to_string();
        if last == "Active" {
            return last;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    last
}

/// Every row: the deployed pipeline delivers exactly where the function's policy grants the
/// invocation.
#[tokio::test]
async fn a_stack_declared_lambda_subscription_runs_the_function_it_names() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = build_router();

        // The sink is created OUT OF BAND, as the recorded pipeline's is: it belongs to no
        // stack, and what lands in it is the handler's own writing.
        let queue_name = format!("sns-lambda-sink-{index}");
        let created = sqs(
            &router,
            case,
            "CreateQueue",
            // The sink is READ several times per row — a row that must receive nothing polls
            // its whole quiet window — so a message it already holds must not come back
            // visible and be counted twice: the visibility window is set well past the row.
            &json!({
                "QueueName": queue_name,
                "Attributes": { "VisibilityTimeout": "300" },
            }),
        )
        .await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();

        let stack = format!("sns-lambda-pipeline-{index}");
        let body = template(row, &queue_url);
        let (status, answered) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &body),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {answered}"
        );
        assert!(
            !element(&answered, "StackId").is_empty(),
            "case {case:?}: CreateStack must answer the new stack's id, got {answered}"
        );
        let (reached, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        let failures = if reached == CREATE_COMPLETE {
            Vec::new()
        } else {
            let (_, events) = query(
                &router,
                "cloudformation",
                "2010-05-15",
                "DescribeStackEvents",
                &[("StackName", &stack)],
            )
            .await;
            elements(&events, "ResourceStatusReason")
        };
        assert_eq!(
            reached, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
             resource failures {failures:?}"
        );

        let outputs = outputs_of(&described);
        let topic = outputs
            .get("TopicArn")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no TopicArn: {described}"))
            .clone();
        let function = outputs
            .get("FunctionName")
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stack published no FunctionName: {described}")
            })
            .clone();
        let state = wait_until_active(&router, &function).await;
        assert_eq!(
            state, "Active",
            "case {case:?}: expected Lambda to report {function} Active, got {state:?}"
        );

        let marker = format!("marker-{index}");
        let (status, published) = query(
            &router,
            "sns",
            "2010-03-31",
            "Publish",
            &[("TopicArn", &topic), ("Message", &marker)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Publish, got {status} with {published}"
        );
        let message_id = element(&published, "MessageId");
        assert!(
            !message_id.is_empty(),
            "case {case:?}: Publish must answer a MessageId whether or not the delivery is \
             authorized, got {published}"
        );

        // The invocation is asynchronous and the handler's write is owed by the CLOCK, so the
        // sink is POLLED rather than read once. A row that must receive nothing is polled for
        // the whole quiet window, so an empty sink is not a sink read too early; a row that
        // must receive something is polled for [`DELIVERY_WAIT`] and once more after its
        // expected count is reached, so a SECOND delivery of one published message fails the
        // count below rather than being cut off by the break.
        let budget = if row.forwarded > 0 {
            DELIVERY_WAIT
        } else {
            QUIET_WINDOW
        };
        let looks = budget.as_millis() / LOOK_INTERVAL.as_millis().max(1);
        let mut forwarded: Vec<Value> = Vec::new();
        for _ in 0..looks {
            forwarded.extend(drain(&router, case, &queue_url).await);
            if row.forwarded > 0 && forwarded.len() >= row.forwarded {
                break;
            }
            tokio::time::sleep(LOOK_INTERVAL).await;
        }
        tokio::time::sleep(SETTLE_SLACK).await;
        forwarded.extend(drain(&router, case, &queue_url).await);
        assert_eq!(
            forwarded.len(),
            row.forwarded,
            "case {case:?}: expected {} forwarded notification(s), got {forwarded:?}",
            row.forwarded
        );
        for notification in &forwarded {
            assert_eq!(
                notification["Message"].as_str(),
                Some(marker.as_str()),
                "case {case:?}: the handler must have been invoked with the published message, \
                 got {notification}"
            );
            assert_eq!(
                notification["TopicArn"].as_str(),
                Some(topic.as_str()),
                "case {case:?}: the notification must name the stack's topic, got {notification}"
            );
        }
    }
}
