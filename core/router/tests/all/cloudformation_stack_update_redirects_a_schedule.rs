// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack UPDATE that re-declares an `AWS::Scheduler::Schedule`
//! re-arms it: the occurrences that follow reach the target the NEW declaration
//! names, carrying the new `Target.Input`, and the target the re-declaration no
//! longer names receives nothing further.
//!
//! RULE: a write that replaces a schedule's definition — its `ScheduleExpression`,
//! its `State` or any part of its `Target` — replaces the series the schedule is
//! invoked on, so every occurrence after the write is delivered according to the
//! replacement ALONE and the target the replacement dropped is handed nothing more.
//! `UpdateSchedule` is a full replacement ("uses all values, including empty values,
//! specified in the request and overrides the existing schedule"), and the rule holds
//! however the replacement was asked for, because a stack that re-declares a schedule
//! applies it through the very `UpdateSchedule` a client calls.
//!
//! Four crates have to agree for that to hold (CloudFormation makes the create and
//! the update; EventBridge Scheduler re-evaluates the occurrence and makes the
//! invocation; IAM holds the execution role it assumes; Amazon SQS holds the
//! messages), so the suite lives here rather than in any one of them. The
//! sibling suite `cloudformation_deployed_schedule_invokes_its_target.rs` is about the
//! CREATE path of that same flow, and
//! `cloudformation_provisions_scheduler_schedules.rs` about what the control plane
//! reports after an update; this one is about the DATA plane following an update.
//!
//! The rows differ along what the re-declaration changes: the target queue and the
//! input together, the input alone (the target unchanged), the target alone (the
//! input unchanged), the redirection run the other way round, and a re-declaration
//! that disables the schedule, after which neither queue is handed anything.
//!
//! Each occurrence is performed at the instant the declaration's `at(...)`
//! expression names, so every declaration states a FRESH instant seconds ahead of
//! the deploy that writes it: a one-time instant that has already been performed
//! names no further occurrence, however the schedule is re-declared.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, outputs_of};

/// The Region every row deploys in, and the one every ARN it creates carries.
const REGION: &str = "ap-south-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// The group a schedule that names none lands in.
const DEFAULT_GROUP: &str = "default";

/// How far ahead of the deploy each declaration's one-time instant falls: far enough
/// ahead that nothing has fired by the time the stack reports its terminal status,
/// and — since the occurrence is performed at the instant itself — close enough for
/// one client's poll loop to watch it.
const FIRE_TIME_AHEAD: Duration = Duration::from_secs(6);

/// Long enough for a stack's own clock-owed span to come due, so a deploy reports its
/// terminal status.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// How long one delivery is waited for: each declaration's instant is
/// [`FIRE_TIME_AHEAD`] ahead, and the rest is room for the clock's tick and the
/// invocation itself.
const PATIENCE: Duration = Duration::from_secs(20);

/// How long a queue the rule says is handed NOTHING is watched before the claim is
/// believed: longer than the wait before the occurrence an instant
/// [`FIRE_TIME_AHEAD`] ahead names, so an occurrence this emulator would have
/// performed has come and gone.
const QUIET: Duration = Duration::from_secs(10);

/// How long a queue a redirection dropped is watched after the redirected delivery has
/// already been observed: the same window the row with no delivery to wait for is held to
/// ([`QUIET`]), since this is the half of the rule that says the dropped target is handed
/// nothing more.
const QUIET_AFTER_DELIVERY: Duration = QUIET;

/// How long one poll of a queue sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Which of the two queues a declaration targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Queue {
    A,
    B,
}

impl Queue {
    /// The parameter value that selects this queue through the template's condition.
    fn use_queue_b(self) -> &'static str {
        match self {
            Queue::A => "false",
            Queue::B => "true",
        }
    }

    /// The other queue — the one a row that redirects leaves behind.
    fn other(self) -> Queue {
        match self {
            Queue::A => Queue::B,
            Queue::B => Queue::A,
        }
    }

    /// The name the row of `index` gives this queue.
    fn name(self, index: usize) -> String {
        match self {
            Queue::A => format!("schedule-redirect-{index}-a"),
            Queue::B => format!("schedule-redirect-{index}-b"),
        }
    }

    /// The stack output whose value is this queue's URL (`Ref` on an `AWS::SQS::Queue`
    /// "returns the queue URL").
    fn url_output(self) -> &'static str {
        match self {
            Queue::A => "QueueAUrl",
            Queue::B => "QueueBUrl",
        }
    }
}

/// One declaration of the schedule: what the template's parameters state about it.
struct Declaration {
    /// The queue its `Target.Arn` resolves to.
    queue: Queue,
    /// Its `Target.Input`, which is what a delivery is judged by: `Input` travels
    /// verbatim.
    input: &'static str,
    /// Its `State`.
    state: &'static str,
}

/// One row: the declaration a stack is created with, and the one it is updated to.
struct Row {
    case: &'static str,
    before: Declaration,
    after: Declaration,
}

/// The rows differ along what the re-declaration replaces — the target and the input
/// together, the input alone, the target alone, the direction of the redirection, and
/// the `State` that stops the series.
const ROWS: &[Row] = &[
    Row {
        case: "the target queue and the input are both re-declared",
        before: Declaration {
            queue: Queue::A,
            input: r#"{"marker":"before-redirect"}"#,
            state: "ENABLED",
        },
        after: Declaration {
            queue: Queue::B,
            input: r#"{"marker":"after-redirect"}"#,
            state: "ENABLED",
        },
    },
    Row {
        case: "only the input is re-declared, on the same queue",
        // The instant the re-declaration states is a fresh one: the occurrence the
        // first declaration named has already been performed, and a one-time instant
        // names no second occurrence. What the row pins is that the NEXT occurrence
        // carries the input the declaration now states, on the queue it keeps.
        before: Declaration {
            queue: Queue::A,
            input: "first-body",
            state: "ENABLED",
        },
        after: Declaration {
            queue: Queue::A,
            input: "second-body",
            state: "ENABLED",
        },
    },
    Row {
        case: "only the target queue is re-declared, the input unchanged",
        before: Declaration {
            queue: Queue::A,
            input: "one-body-two-queues",
            state: "ENABLED",
        },
        after: Declaration {
            queue: Queue::B,
            input: "one-body-two-queues",
            state: "ENABLED",
        },
    },
    Row {
        case: "the redirection runs the other way round",
        before: Declaration {
            queue: Queue::B,
            input: "from-b",
            state: "ENABLED",
        },
        after: Declaration {
            queue: Queue::A,
            input: "to-a",
            state: "ENABLED",
        },
    },
    Row {
        case: "the re-declaration disables the schedule as it re-points it",
        before: Declaration {
            queue: Queue::A,
            input: "last-delivery",
            state: "ENABLED",
        },
        after: Declaration {
            queue: Queue::B,
            input: "never-delivered",
            state: "DISABLED",
        },
    },
];

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
/// [`REGION`], which is what routes the request. The signature is never checked: this
/// router runs with request authentication off.
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
    members: &[(String, String)],
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

/// `at` as an `at(...)` expression's body states it ("at(yyyy-mm-ddThh:mm:ss)"), a
/// local date and time with no offset, read in the zone the declaration names.
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

/// The ARN Amazon SQS gives the queue of `name` in [`REGION`].
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// The template every row deploys: the two candidate target queues, the role
/// EventBridge Scheduler assumes to reach either of them, and the one-time schedule
/// whose instant, state, target and input are all stated by parameters — so an update
/// changes the declaration exactly as the client that owns the schedule as IaC does.
fn template(index: usize, role: &str, schedule: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "UseQueueB": {
                "Type": "String",
                "AllowedValues": ["true", "false"],
                "Default": "false"
            },
            "FireTime": { "Type": "String" },
            "MessageBody": { "Type": "String" },
            "ScheduleState": {
                "Type": "String",
                "AllowedValues": ["ENABLED", "DISABLED"],
                "Default": "ENABLED"
            }
        },
        "Conditions": {
            "IsQueueB": { "Fn::Equals": [{ "Ref": "UseQueueB" }, "true"] }
        },
        "Resources": {
            "QueueA": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": Queue::A.name(index) }
            },
            "QueueB": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": Queue::B.name(index) }
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
                        "PolicyName": "send-to-either-queue",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sqs:SendMessage",
                                "Resource": [
                                    { "Fn::GetAtt": ["QueueA", "Arn"] },
                                    { "Fn::GetAtt": ["QueueB", "Arn"] }
                                ]
                            }]
                        }
                    }]
                }
            },
            "MySchedule": {
                "Type": "AWS::Scheduler::Schedule",
                "Properties": {
                    "Name": schedule,
                    "ScheduleExpression": { "Fn::Sub": "at(${FireTime})" },
                    "ScheduleExpressionTimezone": "UTC",
                    "FlexibleTimeWindow": { "Mode": "OFF" },
                    "State": { "Ref": "ScheduleState" },
                    "Target": {
                        "Arn": {
                            "Fn::If": [
                                "IsQueueB",
                                { "Fn::GetAtt": ["QueueB", "Arn"] },
                                { "Fn::GetAtt": ["QueueA", "Arn"] }
                            ]
                        },
                        "RoleArn": { "Fn::GetAtt": ["SchedulerRole", "Arn"] },
                        "Input": { "Ref": "MessageBody" }
                    }
                }
            }
        },
        "Outputs": {
            "QueueAUrl": { "Value": { "Ref": "QueueA" } },
            "QueueBUrl": { "Value": { "Ref": "QueueB" } }
        }
    })
    .to_string()
}

/// One stack operation (a create or an update) on a row's stack, waited past the
/// stack's clock-owed span and answered with the `DescribeStacks` body.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    stack: &str,
    body: &str,
    declaration: &Declaration,
    instant: &str,
) -> String {
    let parameters = [
        ("UseQueueB", declaration.queue.use_queue_b().to_string()),
        ("FireTime", instant.to_string()),
        ("MessageBody", declaration.input.to_string()),
        ("ScheduleState", declaration.state.to_string()),
    ];
    let mut members = vec![
        ("StackName".to_string(), stack.to_string()),
        ("TemplateBody".to_string(), body.to_string()),
        // Every row's template NAMES the role it declares, which is the
        // acknowledgement CloudFormation asks for separately.
        (
            "Capabilities.member.1".to_string(),
            "CAPABILITY_NAMED_IAM".to_string(),
        ),
    ];
    for (position, (key, value)) in parameters.iter().enumerate() {
        let member = position + 1;
        members.push((
            format!("Parameters.member.{member}.ParameterKey"),
            (*key).to_string(),
        ));
        members.push((
            format!("Parameters.member.{member}.ParameterValue"),
            value.clone(),
        ));
    }
    let (status, answered) = cloudformation(router, action, &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {answered}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, described) = cloudformation(
        router,
        "DescribeStacks",
        &[("StackName".to_string(), stack.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {described}"
    );
    described
}

/// Take every message the queue at `url` holds, DELETING each one, so a later read of
/// the same queue reports only what arrived after this call.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
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
        "case {case:?}: expected HTTP 200 from ReceiveMessage, got {status} with body {received}"
    );
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut bodies = Vec::new();
    for message in &messages {
        bodies.push(message["Body"].as_str().unwrap_or_default().to_string());
        let (status, deleted) = sqs(
            router,
            "DeleteMessage",
            &json!({
                "QueueUrl": url,
                "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteMessage, got {status} with body {deleted}"
        );
    }
    bodies
}

/// The bodies the queue at `url` is handed, once it holds at least one — or whatever
/// it holds after [`PATIENCE`], which is what a missing delivery reports as.
async fn delivered(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let began = Instant::now();
    loop {
        let bodies = drain(router, case, url).await;
        if !bodies.is_empty() || began.elapsed() >= PATIENCE {
            return bodies;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert the queue at `url` is handed NOTHING for `quiet`.
async fn stays_empty(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    url: &str,
    quiet: Duration,
) {
    let began = Instant::now();
    loop {
        let bodies = drain(router, case, url).await;
        assert!(
            bodies.is_empty(),
            "case {case:?}: expected {what} to be handed nothing, got {bodies:?}"
        );
        if began.elapsed() >= quiet {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read the schedule back through EventBridge Scheduler's own `GetSchedule` and assert
/// it reports the declaration `expected` states, addressed by the pair that service
/// keys a schedule by.
async fn assert_control_plane_reports(
    router: &MultiServiceRouter,
    case: &str,
    when: &str,
    schedule: &str,
    expected: &Declaration,
    index: usize,
) {
    let (status, read) = scheduler(
        router,
        "GET",
        &format!("/schedules/{schedule}?groupName={DEFAULT_GROUP}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetSchedule {when}, got {status} with body {read}"
    );
    let arn = queue_arn(&expected.queue.name(index));
    assert_eq!(
        (
            read["Target"]["Arn"].as_str(),
            read["Target"]["Input"].as_str(),
            read["State"].as_str()
        ),
        (Some(arn.as_str()), Some(expected.input), Some(expected.state)),
        "case {case:?}: expected GetSchedule {when} to report the target {arn:?}, the input \
         {:?} and the state {:?}, got {read}",
        expected.input,
        expected.state
    );
}

/// Every row: the schedule a stack deploys delivers to the queue the declaration
/// targets, and after the stack is updated the NEXT occurrence is delivered to the
/// queue the NEW declaration targets, carrying the new input — while the queue the
/// re-declaration dropped is handed nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stack_update_re_arms_the_schedule_it_re_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("schedule-redirect-{index}");
        let role = format!("schedule-redirect-{index}-role");
        let schedule = format!("schedule-redirect-{index}-schedule");
        let body = template(index, &role, &schedule);
        let instant = at_expression_body(whole_seconds_ahead(FIRE_TIME_AHEAD));

        let described = deploy(
            &router,
            case,
            "CreateStack",
            &stack,
            &body,
            &row.before,
            &instant,
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );
        let outputs = outputs_of(&described);
        let url_of = |queue: Queue| {
            outputs
                .get(queue.url_output())
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no {} output in {outputs:?}", queue.url_output()))
        };
        assert_control_plane_reports(
            &router,
            case,
            "after the create",
            &schedule,
            &row.before,
            index,
        )
        .await;

        // The first declaration's own occurrence, delivered to the queue the stack
        // published the URL of.
        let bodies = delivered(&router, case, &url_of(row.before.queue)).await;
        assert_eq!(
            bodies,
            vec![row.before.input.to_string()],
            "case {case:?}: expected one message carrying the declared Input on the queue the \
             first declaration targets, got {bodies:?}"
        );
        let untargeted = drain(&router, case, &url_of(row.before.queue.other())).await;
        assert!(
            untargeted.is_empty(),
            "case {case:?}: expected the queue no declaration targets yet to be handed nothing, \
             got {untargeted:?}"
        );

        // The re-declaration states a FRESH instant: the occurrence the declaration
        // before it named has been performed, and a one-time instant names no
        // second occurrence.
        let after_instant = at_expression_body(whole_seconds_ahead(FIRE_TIME_AHEAD));
        let described = deploy(
            &router,
            case,
            "UpdateStack",
            &stack,
            &body,
            &row.after,
            &after_instant,
        )
        .await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to reach UPDATE_COMPLETE, got {described}"
        );
        assert_control_plane_reports(
            &router,
            case,
            "after the update",
            &schedule,
            &row.after,
            index,
        )
        .await;

        if row.after.state == "ENABLED" {
            let bodies = delivered(&router, case, &url_of(row.after.queue)).await;
            assert_eq!(
                bodies,
                vec![row.after.input.to_string()],
                "case {case:?}: expected one message carrying the re-declared Input on the queue \
                 the update targets, got {bodies:?}"
            );
            if row.after.queue != row.before.queue {
                stays_empty(
                    &router,
                    case,
                    "the queue the update stopped targeting",
                    &url_of(row.before.queue),
                    QUIET_AFTER_DELIVERY,
                )
                .await;
            }
        } else {
            // A re-declaration that disables the schedule stops the series outright:
            // neither the queue it dropped nor the one it now names is handed
            // anything.
            stays_empty(
                &router,
                case,
                "the queue a disabled schedule names",
                &url_of(row.after.queue),
                QUIET,
            )
            .await;
            let dropped = drain(&router, case, &url_of(row.before.queue)).await;
            assert!(
                dropped.is_empty(),
                "case {case:?}: expected the queue the disabled schedule stopped targeting to be \
                 handed nothing, got {dropped:?}"
            );
        }
    }
}
