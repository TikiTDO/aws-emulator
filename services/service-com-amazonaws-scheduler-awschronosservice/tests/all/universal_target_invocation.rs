// SPDX-License-Identifier: CC0-1.0

//! A schedule whose target names an API OPERATION — a UNIVERSAL target — is invoked:
//! end to end over the real `MultiServiceRouter`, with the services those operations
//! belong to mounted beside EventBridge Scheduler on ONE engine, so the invocation is
//! asserted where a client observes it (the execution Step Functions recorded, the
//! messages the queue holds).
//!
//! RULE: `Target.Arn` of the form `arn:<partition>:scheduler:::aws-sdk:<service>:
//! <operation>` names a CALL rather than a resource — "universal targets that you can
//! customize to invoke over 6,000 API operations across more than 270 services"
//! (`Target`) — where the service field is the SDK SERVICE ID (`sfn` for AWS Step
//! Functions, whose endpoint prefix and signing name are `states` instead) and the
//! operation field is that SDK's own operation name in lowerCamelCase. At each
//! occurrence EventBridge Scheduler acts as `Target.RoleArn` and makes THAT call with
//! `Target.Input` — "the text, or well-formed JSON, passed to the target" — as the
//! operation's own request, so the effect a client then sees is the one that operation
//! has. A target naming no call this emulator makes invokes nothing: it is never
//! reported as delivered, and no other call is made in its place.
//!
//! The rows span the class rather than one recorded case: they differ along the SERVICE
//! the ARN names, along the OPERATION, along the spelling the invoked service's own
//! model gives that operation's members (`stateMachineArn` against `QueueUrl`), and
//! along the shape of the request (a flat document against one carrying a list of
//! entries).
//!
//! Nothing here waits for a fixed span: a row watches for the CONDITION it expects
//! until a generous deadline, and a row that expects NOTHING watches a second schedule
//! whose occurrence is LATER — once that one's effect is there, the clock has run past
//! the instant the row's own schedule named.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::schedule_invocation::{
    at_expression_body, iam, peek, seconds_from_now, sqs, POLL_INTERVAL, REGION,
};
use crate::wire_harness::{account_id, call_in, error_code, error_message};

/// How long a row watches for the condition it expects. Every schedule below names an
/// instant seconds ahead, so this is room for the clock's tick and the call, not a
/// latency expectation of its own.
const PATIENCE: Duration = Duration::from_secs(90);

/// How far ahead a row's own schedule is placed.
const OCCURRENCE_IN: u64 = 4;

/// How far ahead the SENTINEL schedule of a row that expects nothing is placed: later
/// than that row's own occurrence, so its effect proves the clock has already performed
/// — or declined — the occurrence under test.
const SENTINEL_IN: u64 = 9;

/// The scheduler service principal a schedule's execution role must trust.
const SCHEDULER_PRINCIPAL: &str = "scheduler.amazonaws.com";

/// The emulator: EventBridge Scheduler, the services a universal target's call is made
/// to, and IAM, on ONE engine — the shape `core/router/src/lib.rs` mounts them in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        engine,
    ));
    router
}

/// One awsJson1_0 call against Step Functions (`AWSStepFunctions.<Operation>`), which
/// is the service whose SDK id (`sfn`) and endpoint prefix (`states`) differ.
async fn step_functions(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/states/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A state machine whose single state needs nothing of its own, and its ARN.
async fn given_state_machine(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let definition = json!({
        "StartAt": "Done",
        "States": { "Done": { "Type": "Succeed" } },
    });
    let (status, created) = step_functions(
        router,
        "CreateStateMachine",
        &json!({
            "name": name,
            "definition": definition.to_string(),
            "roleArn": format!("arn:aws:iam::{}:role/StepFunctionsRole", account_id()),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStateMachine answered {status} {created}"
    );
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no stateMachineArn in {created}"))
        .to_string()
}

/// The names of the executions Step Functions holds for the state machine `machine`.
async fn execution_names(router: &MultiServiceRouter, case: &str, machine: &str) -> Vec<String> {
    let (status, listed) = step_functions(
        router,
        "ListExecutions",
        &json!({ "stateMachineArn": machine }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListExecutions answered {status} {listed}"
    );
    listed["executions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|execution| execution["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A queue called `name`: its url and the ARN a policy names it by.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let (status, created) = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {attributes}"
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// A role EventBridge Scheduler may assume, granted each `(action, resource)` pair —
/// the pair AWS documents for an execution role: a trust policy for the scheduler
/// service principal and the permission the schedule's own call needs.
async fn given_execution_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    grants: &[(&str, &str)],
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SCHEDULER_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    let arn = format!("arn:aws:iam::{}:role/{name}", account_id());
    if grants.is_empty() {
        // A role carrying no permission at all: there is no statement to write, and
        // what it withholds is what the invocation is then refused by.
        return arn;
    }
    let statements: Vec<Value> = grants
        .iter()
        .map(|(action, resource)| {
            json!({ "Effect": "Allow", "Action": action, "Resource": resource })
        })
        .collect();
    let policy = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
    let (status, written) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "invoke-the-target"),
            ("PolicyDocument", &policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status}: {written}"
    );
    arn
}

/// A schedule called `name` whose one occurrence is `in_seconds` ahead, pointed at the
/// universal target `target` with `input` as its request.
async fn given_schedule(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    in_seconds: u64,
    target: &str,
    role: &str,
    input: &str,
) {
    let (status, created) = call_in(
        router,
        "POST",
        &format!("/schedules/{name}"),
        REGION,
        Some(json!({
            "ScheduleExpression": format!("at({})", at_expression_body(seconds_from_now(in_seconds))),
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "State": "ENABLED",
            "Target": { "Arn": target, "RoleArn": role, "Input": input },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSchedule answered {status} {created}"
    );
    // The universal target is echoed as the schedule holds it, so what is invoked below
    // is the target a client reads back.
    let (status, read) = call_in(
        router,
        "GET",
        &format!("/schedules/{name}?groupName=default"),
        REGION,
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetSchedule answered {status} {read}"
    );
    assert_eq!(
        read["Target"]["Arn"].as_str(),
        Some(target),
        "case {case:?}: GetSchedule reports another Target.Arn: {read}"
    );
}

/// One row of the delivery table: the universal target, the request it carries and the
/// effect that call has.
struct Row {
    case: &'static str,
    /// The SDK service id and the operation the target's ARN names.
    service: &'static str,
    operation: &'static str,
    /// What the call must be able to do, as the IAM action the invoked operation
    /// publishes.
    action: &'static str,
    /// Which effect the row watches for.
    effect: Effect,
}

/// What a row's call does, as a client sees it.
enum Effect {
    /// An execution of the row's own state machine, named by the request.
    Execution,
    /// One message on the row's own queue, carrying the body the request named.
    Message(&'static str),
    /// One message per entry of a request that carries a LIST of them.
    Batch(&'static [&'static str]),
}

const ROWS: &[Row] = &[
    Row {
        // The service whose SDK id (`sfn`) is not its endpoint prefix (`states`), and
        // whose model spells an operation's members in lowerCamelCase while the
        // request names them in the API's own PascalCase.
        case: "a state machine execution started through the universal target",
        service: "sfn",
        operation: "startExecution",
        action: "states:StartExecution",
        effect: Effect::Execution,
    },
    Row {
        // Another service, whose model spells the same operation's members in
        // PascalCase.
        case: "a message sent to a queue through the universal target",
        service: "sqs",
        operation: "sendMessage",
        action: "sqs:SendMessage",
        effect: Effect::Message("{\"marker\":\"one\"}"),
    },
    Row {
        // Another operation of that service, whose request carries a LIST of entries
        // rather than one flat document.
        case: "a batch of messages sent to a queue through the universal target",
        service: "sqs",
        operation: "sendMessageBatch",
        action: "sqs:SendMessage",
        effect: Effect::Batch(&["{\"marker\":\"first\"}", "{\"marker\":\"second\"}"]),
    },
];

/// Every row: the occurrence makes the call the universal target's ARN names, and the
/// effect that call has is there afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_universal_target_makes_the_call_its_arn_names() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let target = format!("arn:aws:scheduler:::aws-sdk:{}:{}", row.service, row.operation);
        let machine = given_state_machine(&emulator, case, &format!("machine-{index}")).await;
        let (url, queue_arn) = given_queue(&emulator, case, &format!("target-{index}")).await;
        let execution = format!("run-{index}");
        let (input, resource) = match row.effect {
            Effect::Execution => (
                json!({
                    "StateMachineArn": machine,
                    "Name": execution,
                    "Input": "{}",
                })
                .to_string(),
                machine.clone(),
            ),
            Effect::Message(body) => (
                json!({ "QueueUrl": url, "MessageBody": body }).to_string(),
                queue_arn.clone(),
            ),
            Effect::Batch(bodies) => (
                json!({
                    "QueueUrl": url,
                    "Entries": bodies
                        .iter()
                        .enumerate()
                        .map(|(at, body)| json!({ "Id": format!("e{at}"), "MessageBody": body }))
                        .collect::<Vec<Value>>(),
                })
                .to_string(),
                queue_arn.clone(),
            ),
        };
        let role = given_execution_role(
            &emulator,
            case,
            &format!("role-{index}"),
            &[(row.action, resource.as_str())],
        )
        .await;
        given_schedule(
            &emulator,
            case,
            &format!("schedule-{index}"),
            OCCURRENCE_IN,
            &target,
            &role,
            &input,
        )
        .await;

        // What the row's call must leave behind, as a client reads it.
        let mut expected: Vec<String> = match row.effect {
            Effect::Execution => vec![execution.clone()],
            Effect::Message(body) => vec![body.to_string()],
            Effect::Batch(bodies) => bodies.iter().map(ToString::to_string).collect(),
        };
        expected.sort();

        let began = Instant::now();
        loop {
            let mut held: Vec<String> = match row.effect {
                Effect::Execution => execution_names(&emulator, case, &machine).await,
                Effect::Message(_) | Effect::Batch(_) => peek(&emulator, &url)
                    .await
                    .iter()
                    .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                    .collect(),
            };
            if held.len() >= expected.len() {
                held.sort();
                assert_eq!(
                    held, expected,
                    "case {case:?}: expected what the request named ({expected:?}), got {held:?}"
                );
                break;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: the universal target {target:?} made no call: expected \
                 {expected:?}, got {held:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// One row of the declined table: a universal target whose call this emulator makes
/// NOT, and why.
struct Declined {
    case: &'static str,
    /// The service field of the target's ARN.
    service: &'static str,
    /// Its operation field.
    operation: &'static str,
    /// Whether the schedule's role is granted `states:StartExecution` on the state
    /// machine at all.
    granted: bool,
    /// Whether `Target.Input` is the well-formed JSON the operation's request is made
    /// of.
    json_input: bool,
}

const DECLINED: &[Declined] = &[
    Declined {
        case: "an operation that service does not publish",
        service: "sfn",
        operation: "startEverything",
        granted: true,
        json_input: true,
    },
    Declined {
        case: "an operation that service does not publish",
        service: "sfn",
        operation: "startEverything",
        granted: true,
        json_input: true,
    },
    Declined {
        case: "a role whose permissions do not allow the call",
        service: "sfn",
        operation: "startExecution",
        granted: false,
        json_input: true,
    },
    Declined {
        case: "an Input that is not the JSON the request is made of",
        service: "sfn",
        operation: "startExecution",
        granted: true,
        json_input: false,
    },
];

/// Every row: no execution is recorded for a universal target whose call is not made,
/// and the clock is proven to have run past the occurrence by a SENTINEL schedule whose
/// own occurrence is later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_universal_target_whose_call_is_not_made_starts_nothing() {
    for (index, row) in DECLINED.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let target = format!("arn:aws:scheduler:::aws-sdk:{}:{}", row.service, row.operation);
        let machine = given_state_machine(&emulator, case, &format!("machine-{index}")).await;
        let (url, queue_arn) = given_queue(&emulator, case, &format!("sentinel-{index}")).await;
        let input = match row.json_input {
            true => json!({
                "StateMachineArn": machine,
                "Name": format!("never-{index}"),
                "Input": "{}",
            })
            .to_string(),
            false => format!("start the machine {machine}"),
        };
        let grants: Vec<(&str, &str)> = match row.granted {
            true => vec![("states:StartExecution", machine.as_str())],
            false => Vec::new(),
        };
        let role = given_execution_role(&emulator, case, &format!("role-{index}"), &grants).await;
        given_schedule(
            &emulator,
            case,
            &format!("declined-{index}"),
            OCCURRENCE_IN,
            &target,
            &role,
            &input,
        )
        .await;
        // The sentinel: a universal target whose call this emulator DOES make, one
        // whose occurrence is later than the row's own.
        let sentinel_role = given_execution_role(
            &emulator,
            case,
            &format!("sentinel-role-{index}"),
            &[("sqs:SendMessage", queue_arn.as_str())],
        )
        .await;
        given_schedule(
            &emulator,
            case,
            &format!("sentinel-{index}"),
            SENTINEL_IN,
            "arn:aws:scheduler:::aws-sdk:sqs:sendMessage",
            &sentinel_role,
            &json!({ "QueueUrl": url, "MessageBody": "{\"sentinel\":\"fired\"}" }).to_string(),
        )
        .await;

        let began = Instant::now();
        loop {
            if !peek(&emulator, &url).await.is_empty() {
                break;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: the sentinel schedule was never invoked, so nothing is known \
                 about the occurrence under test"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let held = execution_names(&emulator, case, &machine).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected the universal target {target:?} to start no execution, \
             got {held:?}"
        );
    }
}

/// One row of the refusal table: how a universal target's two fields are written, and
/// whether the schedule may be written at all.
struct Written {
    case: &'static str,
    /// The `Target.Arn` the request carries.
    target: &'static str,
    /// Whether the service refuses the write.
    refused: bool,
}

const WRITTEN: &[Written] = &[    Written {
        case: "the SDK service id and a lowerCamelCase operation",
        target: "arn:aws:scheduler:::aws-sdk:sfn:startExecution",
        refused: false,
    },
    Written {
        // An SDK service id of a service this emulator makes no call for is still an
        // SDK service id, so the schedule is written.
        case: "an SDK service id this emulator makes no call for",
        target: "arn:aws:scheduler:::aws-sdk:cloudfront:createInvalidation",
        refused: false,
    },
    Written {
        // `states` is Step Functions' endpoint prefix and signing name, and no AWS SDK
        // names the service by it — the distinction a universal target's service field
        // draws.
        case: "the endpoint prefix of a service instead of its SDK service id",
        target: "arn:aws:scheduler:::aws-sdk:states:startExecution",
        refused: true,
    },
    Written {
        case: "the ARN namespace of a service instead of its SDK service id",
        target: "arn:aws:scheduler:::aws-sdk:events:putEvents",
        refused: true,
    },
    Written {
        case: "the capitalised form of the operation",
        target: "arn:aws:scheduler:::aws-sdk:sqs:SendMessage",
        refused: true,
    },
];

/// Every row: a universal target whose service is not an SDK service id, or whose
/// operation is not the SDK's lowerCamelCase name, is refused when the schedule is
/// WRITTEN — 400 `ValidationException` reporting the submitted value — and nothing is
/// stored for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_universal_target_that_names_no_call_is_refused_when_written() {
    let emulator = emulator();
    let role = given_execution_role(&emulator, "the write table", "writer-role", &[(
        "states:StartExecution",
        "*",
    )])
    .await;
    for (index, row) in WRITTEN.iter().enumerate() {
        let case = row.case;
        let name = format!("written-{index}");
        let (status, answered) = call_in(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            REGION,
            Some(json!({
                // A year-long-ahead instant, so no row's schedule invokes anything
                // while the next row is being written.
                "ScheduleExpression": "at(2099-01-01T00:00:00)",
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "State": "ENABLED",
                "Target": { "Arn": row.target, "RoleArn": role, "Input": "{}" },
            })),
        )
        .await;
        if !row.refused {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the schedule to be written, got {status} {answered}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answered}"
        );
        // The refusal names the pair the submitted value states.
        let (service, api) = row
            .target
            .rsplit_once(':')
            .map(|(head, api)| (head.rsplit(':').next().unwrap_or_default(), api))
            .unwrap_or_default();
        assert_eq!(
            error_message(&answered),
            format!(
                "Invalid RequestJson provided. Reason The api {api} is not valid for the \
                 service aws-sdk:{service}."
            ),
            "case {case:?}: expected the refusal to name the pair the Target.Arn states, got \
             {answered}"
        );
        // Nothing is stored for a refused write.
        let (status, read) = call_in(
            &emulator,
            "GET",
            &format!("/schedules/{name}?groupName=default"),
            REGION,
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected no schedule to have been stored, got {status} {read}"
        );
    }
}
