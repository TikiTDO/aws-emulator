// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed SNS -> Lambda subscription INVOKES the function it names, and
//! only while the stack also grants Amazon SNS the invocation — the permission boundary a
//! client validates a fan-out's wiring by.
//!
//! The rule under test: **a publish to a topic delivers the message to every confirmed
//! `lambda`-protocol subscription of that topic ("lambda – delivery of JSON-encoded message to
//! an Lambda function", `SubscribeInput$Protocol`), by invoking the subscribed function
//! asynchronously with the message in the `Records` envelope a function's event carries — and
//! that invocation is made only where the FUNCTION's own resource-based policy grants
//! `lambda:InvokeFunction` to the service principal `sns.amazonaws.com` for that topic, which
//! is what an `AWS::Lambda::Permission` declares. So a stack that declares the grant and one
//! that does not differ in what a client can observe downstream of the function.**
//!
//! Four crates have to agree for that to hold (CloudFormation provisions the topic, the
//! function, the subscription and the statement; SNS fans the publish out; Lambda runs the
//! handler; Amazon SQS holds what the handler wrote), so the suite lives here rather than in
//! any one crate. The observation is made where a client makes it: on the sink queue the
//! function forwards to, never on an emulator internal.
//!
//! The two halves differ along exactly what the rule generalises over — whether the stack
//! declares the `AWS::Lambda::Permission` — and the second half is deployed as an UPDATE of
//! the same stack, so the grant is the only thing that changed between them. The same boundary
//! over two INDEPENDENTLY deployed stacks is
//! [`crate::sns_lambda_subscription_permission`], which drives this suite's harness rather
//! than a copy of it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode, outputs_of};

/// The Region every request here is signed for — the one `wait_for_stack_status` polls in.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The stack statuses this suite waits for.
pub(crate) const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The execution role the declared function is created with, provisioned in IAM before the
/// stack so the template names a role that service really holds.
pub(crate) const ROLE_NAME: &str = "sns-lambda-boundary-exec";

/// How long an observation of a delivery waits before giving up on it.
pub(crate) const DELIVERY_BUDGET: Duration = Duration::from_secs(20);

/// How long the half that must observe NOTHING waits before concluding nothing came.
const QUIET_BUDGET: Duration = Duration::from_secs(3);

/// How often either look.
pub(crate) const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// CloudFormation, SNS, Lambda, Amazon SQS and IAM over ONE engine, as
/// `core/router/src/lib.rs` mounts them.
///
/// `pub(crate)` with the helpers below: a sibling suite that reads the same boundary over two
/// independently deployed stacks drives THIS harness rather than a second copy of it.
pub(crate) fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One awsQuery call against `signing_name`, with whatever status it answered.
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
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation call that must answer 200.
pub(crate) async fn cloudformation(
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
        "{action}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One SNS call that must answer 200.
pub(crate) async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(
        router,
        "sns",
        &format!("sns.{REGION}.amazonaws.com"),
        "2010-03-31",
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "{action}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One IAM call that must answer 200.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> String {
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
        "{action}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One Amazon SQS call (`awsJson1_0`) that must answer 200.
pub(crate) async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
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
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "SQS {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as the status it answered and its parsed body.
///
/// `pub(crate)` with the callers above and for the same reason: every suite that reads this rule
/// has to reach Lambda's own control plane — to publish a version, to add or remove a grant, to
/// ask whether the function is ready — and one caller for that wire serves them all.
pub(crate) async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(body) => Bytes::from(body.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
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

/// The trust policy an execution role has to carry for Lambda to be handed it.
pub(crate) fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The identity policy that same role has to carry for its FUNCTION's own calls to be
/// served: a handler calls another service with temporary credentials for its execution
/// role, so each call it makes — resolving the sink's URL by name, and the write itself — is
/// judged against the policies in force on that role.
fn forwarding_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["sqs:GetQueueUrl", "sqs:SendMessage"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Create the execution role the templates of these suites name, where IAM holds it:
/// trusted by Lambda, and granting the forward its handler makes.
pub(crate) async fn execution_role(router: &MultiServiceRouter) {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "forward-to-the-sink"),
            ("PolicyDocument", &forwarding_policy()),
        ],
    )
    .await;
}

/// The handler the stack declares inline: it forwards the message of every SNS record it is
/// handed to the queue the stack also declares, which is where the delivery is observed.
///
/// The queue is addressed by NAME, resolved through the service's own `GetQueueUrl`, so the
/// source carries nothing the deployment had to compute.
const FORWARDING_SOURCE: &str = r#"
import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = client.get_queue_url(QueueName="SINK_NAME")["QueueUrl"]
    forwarded = 0
    for record in event["Records"]:
        client.send_message(QueueUrl=url, MessageBody=record["Sns"]["Message"])
        forwarded += 1
    return {"forwarded": forwarded}
"#;

/// The template the stack is deployed with: a topic, a sink queue, a function subscribed to
/// the topic over the `lambda` protocol and — when `with_permission` — the
/// `AWS::Lambda::Permission` that grants Amazon SNS the invocation.
pub(crate) fn template(stack: &str, with_permission: bool) -> String {
    let sink = format!("{stack}-sink");
    let mut resources = json!({
        "Sink": {
            "Type": "AWS::SQS::Queue",
            "Properties": { "QueueName": sink },
        },
        "Topic": {
            "Type": "AWS::SNS::Topic",
            "Properties": { "TopicName": format!("{stack}-topic") },
        },
        "Forwarder": {
            "Type": "AWS::Lambda::Function",
            "Properties": {
                "FunctionName": format!("{stack}-forwarder"),
                "Role": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}"),
                "Runtime": "python3.12",
                "Handler": "index.lambda_handler",
                "Timeout": 10,
                "Code": { "ZipFile": FORWARDING_SOURCE.replace("SINK_NAME", &sink) },
            },
        },
        "Subscription": {
            "Type": "AWS::SNS::Subscription",
            "Properties": {
                "TopicArn": { "Ref": "Topic" },
                "Protocol": "lambda",
                "Endpoint": { "Fn::GetAtt": ["Forwarder", "Arn"] },
            },
        },
    });
    if with_permission {
        resources["InvokePermission"] = json!({
            "Type": "AWS::Lambda::Permission",
            "Properties": {
                "FunctionName": { "Fn::GetAtt": ["Forwarder", "Arn"] },
                "Action": "lambda:InvokeFunction",
                "Principal": "sns.amazonaws.com",
                "SourceArn": { "Ref": "Topic" },
            },
        });
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": {
            "TopicArn": { "Value": { "Ref": "Topic" } },
            "QueueUrl": { "Value": { "Ref": "Sink" } },
        },
    })
    .to_string()
}

/// Every message body waiting on the queue at `url`.
pub(crate) async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Wait for `marker` to appear on the queue at `url`, and report every body seen.
pub(crate) async fn wait_for(router: &MultiServiceRouter, url: &str, marker: &str) -> Vec<String> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        seen.extend(drain(router, url).await);
        if seen.iter().any(|body| body.contains(marker)) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Wait out [`QUIET_BUDGET`] and report every body the queue at `url` held in it.
pub(crate) async fn stayed_quiet(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let deadline = Instant::now() + QUIET_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        seen.extend(drain(router, url).await);
        if !seen.is_empty() {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    seen
}

/// Both halves of the boundary, over one stack: without the grant nothing is invoked, and with
/// it the published message reaches the function and what the function did is observable on
/// the queue.
#[tokio::test]
async fn a_stack_deployed_sns_lambda_subscription_invokes_the_function_only_when_permitted() {
    let router = build_router();
    let stack = "sns-lambda-boundary";

    // The execution role the template names, created where that service holds it.
    execution_role(&router).await;

    // The stack WITHOUT the grant: the subscription is wired, and nothing may invoke the
    // function.
    cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(stack, false)),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "the stack without the grant must deploy, got {status} with {described}"
    );
    let outputs = outputs_of(&described);
    let topic = outputs
        .get("TopicArn")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must publish its TopicArn, got {described}"));
    let queue_url = outputs
        .get("QueueUrl")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must publish its QueueUrl, got {described}"));

    let broken_marker = "marker-broken";
    let published = sns(
        &router,
        "Publish",
        &[("TopicArn", &topic), ("Message", broken_marker)],
    )
    .await;
    assert!(
        !element(&published, "MessageId").is_empty(),
        "the publish must be accepted whether or not the delivery is permitted, got {published}"
    );
    let quiet = stayed_quiet(&router, &queue_url).await;
    assert!(
        quiet.is_empty(),
        "case \"no AWS::Lambda::Permission\": expected the function never to be invoked, so \
         nothing reaches the sink queue, got {quiet:?}"
    );

    // The SAME stack, updated to declare the grant: the only thing that changed between the
    // two halves.
    cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(stack, true)),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, stack, UPDATE_COMPLETE).await;
    assert_eq!(
        status, UPDATE_COMPLETE,
        "the stack with the grant must deploy, got {status} with {described}"
    );

    let fixed_marker = "marker-fixed";
    sns(
        &router,
        "Publish",
        &[("TopicArn", &topic), ("Message", fixed_marker)],
    )
    .await;
    let delivered = wait_for(&router, &queue_url, fixed_marker).await;
    assert!(
        delivered.iter().any(|body| body.contains(fixed_marker)),
        "case \"with the AWS::Lambda::Permission\": expected the subscribed function to be \
         invoked and to forward {fixed_marker:?} to the sink queue, got {delivered:?}"
    );
    assert!(
        !delivered.iter().any(|body| body.contains(broken_marker)),
        "case \"with the AWS::Lambda::Permission\": expected only the message published after \
         the grant, got {delivered:?}"
    );
}
