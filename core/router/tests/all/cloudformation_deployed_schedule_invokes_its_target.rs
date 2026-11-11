// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed `AWS::Scheduler::Schedule` FIRES: the whole deploy ->
//! fire -> deliver -> stack-delete round trip, over the wire through the real
//! `MultiServiceRouter` with CloudFormation, EventBridge Scheduler, IAM and Amazon
//! SQS mounted on ONE engine.
//!
//! RULE: a schedule in `State=ENABLED` invokes its target at each occurrence its
//! `ScheduleExpression` names, acting as `Target.RoleArn` and handing
//! `Target.Input` to the API that target's service is invoked by — and that holds
//! however the schedule came to exist, because a stack that declares one creates it
//! through the very `CreateSchedule` a client calls. Deleting the stack deletes the
//! schedule, which stops the series.
//!
//! Four crates have to agree for that to hold (CloudFormation makes the create and
//! the delete; EventBridge Scheduler evaluates the occurrence and makes the
//! invocation; IAM holds the execution role it assumes; Amazon SQS holds the
//! message), so the suite lives here rather than in any one of them. The suites in
//! `services/service-com-amazonaws-scheduler-awschronosservice/tests/all/schedule_invocation.rs`
//! are the ones that span the forms a `ScheduleExpression` takes; this one is about
//! an IaC-owned schedule being no different from an API-owned one.
//!
//! The occurrence is performed at the instant the template's `at(...)` expression
//! names, so the template states an instant seconds ahead of the deploy — far enough
//! that nothing has fired by the time the stack reports CREATE_COMPLETE, and close
//! enough for one client's poll loop to watch it.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, outputs_of};

/// The Region the stack is deployed in, and the one every ARN it creates carries.
const REGION: &str = "eu-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// The group a schedule that names none lands in.
const DEFAULT_GROUP: &str = "default";

/// How far ahead of the deploy the template's one-time instant falls: far enough
/// ahead that nothing could have fired by the time the stack completes, and — since
/// the occurrence is performed at the instant itself — close enough for one client's
/// poll loop to watch it.
const FIRE_TIME_AHEAD: Duration = Duration::from_secs(6);

/// The body the schedule hands its target, which is the one assertion the delivery
/// is judged by: `Target.Input` travels verbatim.
const MESSAGE_BODY: &str = r#"{"marker":"cfn-deployed-schedule"}"#;

/// Long enough for the stack's own clock-owed span to come due, so a deploy reports
/// its terminal status and a delete has finished.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// How long the delivery is waited for: the template's instant is [`FIRE_TIME_AHEAD`]
/// ahead, and the rest is room for the clock's tick and the invocation.
const PATIENCE: Duration = Duration::from_secs(20);

/// How long one poll of the target queue sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The four services this flow spans, on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in
/// [`REGION`], which is what routes the request. The signature is never checked:
/// this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One EventBridge Scheduler operation over its `restJson1` wire.
async fn scheduler(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("scheduler.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("scheduler"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// One Amazon SQS operation over its `awsJson1_0` wire.
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
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The error code a `restJson1` or `awsJson` envelope carries in `__type`, without
/// its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `at` as an `at(...)` expression's body states it: "at(yyyy-mm-ddThh:mm:ss)", a
/// local date and time with no offset, which with no `ScheduleExpressionTimezone`
/// is read in UTC.
fn at_expression_body(at: SystemTime) -> String {
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders as ISO-8601");
    rendered.trim_end_matches('Z').to_string()
}

/// A whole number of seconds ahead of now, which is the resolution an `at(...)`
/// expression states an instant at.
fn whole_seconds_ahead(ahead: Duration) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(now) + ahead
}

/// The template the flow deploys: the queue the schedule targets, the role
/// EventBridge Scheduler assumes to reach it, and the one-time schedule itself.
fn template(queue: &str, role: &str, schedule: &str, fire_time: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "TargetQueue": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": queue }
            },
            "SchedulerRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "scheduler.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "Policies": [{
                        "PolicyName": "send-to-target",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sqs:SendMessage",
                                "Resource": { "Fn::GetAtt": ["TargetQueue", "Arn"] }
                            }]
                        }
                    }]
                }
            },
            "MySchedule": {
                "Type": "AWS::Scheduler::Schedule",
                "Properties": {
                    "Name": schedule,
                    "ScheduleExpression": format!("at({fire_time})"),
                    "FlexibleTimeWindow": { "Mode": "OFF" },
                    "Target": {
                        "Arn": { "Fn::GetAtt": ["TargetQueue", "Arn"] },
                        "RoleArn": { "Fn::GetAtt": ["SchedulerRole", "Arn"] },
                        "Input": MESSAGE_BODY
                    }
                }
            }
        },
        "Outputs": {
            "QueueUrl": { "Value": { "Ref": "TargetQueue" } },
            "ScheduleRef": { "Value": { "Ref": "MySchedule" } },
            "ScheduleArn": { "Value": { "Fn::GetAtt": ["MySchedule", "Arn"] } }
        }
    })
    .to_string()
}

/// The bodies of the messages the queue at `url` holds, left on it, once it holds
/// at least one — or whatever it holds after [`PATIENCE`].
async fn delivered(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let began = Instant::now();
    loop {
        let (status, received) = sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "VisibilityTimeout": 0,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ReceiveMessage answered {status} {received}"
        );
        let bodies: Vec<String> = received["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect();
        if !bodies.is_empty() || began.elapsed() >= PATIENCE {
            return bodies;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The whole round trip: the stack completes, the schedule it declared is readable
/// and `ENABLED`, its one occurrence delivers `Target.Input` to the queue the stack
/// created, and deleting the stack leaves neither the schedule nor the queue behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stack_deployed_schedule_invokes_the_queue_the_stack_created() {
    let case = "a one-time schedule deployed by a stack";
    let router = build_router();
    let stack = "scheduler-fires";
    let queue_name = "scheduler-fires-queue";
    let schedule_name = "scheduler-fires-schedule";
    let fire_time = at_expression_body(whole_seconds_ahead(FIRE_TIME_AHEAD));

    let (status, created) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            (
                "TemplateBody",
                &template(
                    queue_name,
                    "scheduler-fires-role",
                    schedule_name,
                    &fire_time,
                ),
            ),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ("OnFailure", "DELETE"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack answered {status} {created}"
    );

    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, described) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks answered {status} {described}"
    );
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
    );
    let outputs = outputs_of(&described);
    let queue_url = outputs
        .get("QueueUrl")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl output in {described}"));

    // The control plane reads the way the client that deployed the stack reads it.
    let (status, read) = scheduler(
        &router,
        "GET",
        &format!("/schedules/{schedule_name}?groupName={DEFAULT_GROUP}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSchedule answered {status} {read}"
    );
    assert_eq!(
        read["State"].as_str(),
        Some("ENABLED"),
        "case {case:?}: expected the deployed schedule to be ENABLED, got {read}"
    );
    let expected_arn =
        format!("arn:aws:scheduler:{REGION}:{ACCOUNT_ID}:schedule/{DEFAULT_GROUP}/{schedule_name}");
    assert_eq!(
        read["Arn"].as_str(),
        Some(expected_arn.as_str()),
        "case {case:?}: expected the schedule ARN {expected_arn:?}, got {read}"
    );

    // The invocation the issue reported as never happening: the one occurrence the
    // template's `at(...)` instant names reaches the queue the stack created,
    // carrying the template's own `Target.Input`.
    let bodies = delivered(&router, case, &queue_url).await;
    assert_eq!(
        bodies,
        vec![MESSAGE_BODY.to_string()],
        "case {case:?}: expected one message carrying the schedule's own Input, got {bodies:?}"
    );

    let (status, deleted) = cloudformation(&router, "DeleteStack", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack answered {status} {deleted}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    // What the stack created it also removes: the schedule and the queue are both
    // gone, each refused by its own service's own refusal.
    let (status, refused) = scheduler(
        &router,
        "GET",
        &format!("/schedules/{schedule_name}?groupName={DEFAULT_GROUP}"),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected GetSchedule to refuse a deleted schedule, got {status} {refused}"
    );
    let (status, refused) = sqs(
        &router,
        "GetQueueUrl",
        &json!({ "QueueName": queue_name }),
    )
    .await;
    assert_eq!(
        error_code(&refused),
        "QueueDoesNotExist",
        "case {case:?}: expected GetQueueUrl to refuse a deleted queue, got {status} {refused}"
    );
}
