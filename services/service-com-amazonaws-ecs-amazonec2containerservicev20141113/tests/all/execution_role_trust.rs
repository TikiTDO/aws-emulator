// SPDX-License-Identifier: CC0-1.0

//! The IAM roles Amazon ECS has to assume before it launches a task — the task EXECUTION
//! role it acts as and the TASK role it hands the containers — judged against the trust
//! policy each carries in the account's IAM records, driven end to end over the wire, with IAM
//! and CloudWatch Logs mounted beside Amazon ECS on one engine so every role, trust document
//! and log group is created through the OWNING service's own API rather than written into a
//! store by hand.
//!
//! RULE: Amazon ECS reaches a role a launch resolves by ASSUMING it, which a role admits only
//! to the principals its own trust policy names — so a launch whose role IAM holds and whose
//! trust policy does not let `ecs-tasks.amazonaws.com` perform `sts:AssumeRole` launches no
//! container: the task never reports `RUNNING`, it reaches `STOPPED` with `Task$stopCode`
//! `TaskFailedToStart` and the `stoppedReason` Amazon ECS words such a stop with, naming that
//! role, it gets no `startedAt`, no image is fetched for it (so no pull instants), and nothing
//! is delivered to the log group its `awslogs` configuration names.
//!
//! Only what IAM ANSWERED is refused: a launch naming no role, one naming a role this
//! account's IAM holds no record for, and one whose trust document admits the principal are
//! each launched and run.
//!
//! The rows differ along what the rule generalises over: WHICH role the launch names (the
//! revision's execution role, its task role, both, the launch's own `overrides` execution
//! role, which stands in place of the revision's), which principal the trust document names
//! (this service, another service, several at once, an IAM principal rather than a service),
//! the effect it names (`Allow`, `Deny`), the action it names (`sts:AssumeRole`, a wildcard
//! covering it, another STS action that does not), the case the principal and the action are
//! spelled in, and whether IAM holds the role at all — plus the second test, which drives the
//! same rule through the SCHEDULER's placement (`CreateService`) rather than through a
//! client's own `RunTask`.
//!
//! Nothing here reaches into either state struct: every value asserted is read back through
//! `DescribeTasks`, `DescribeServices` and CloudWatch Logs' own `GetLogEvents`. The
//! `stoppedReason` is spelled out here rather than imported, so a wrong wording in the
//! implementation cannot make a test pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, iam, list, ok, string, ACCOUNT, SIGNED_REGION};

/// The log group every container below is configured to deliver to, created through
/// CloudWatch Logs' own API before any task is launched.
const LOG_GROUP: &str = "/emulator/ecs/execution-role-trust";

/// The `awslogs-stream-prefix` the task definitions below configure.
const STREAM_PREFIX: &str = "trust";

/// The one container of every task definition below.
const CONTAINER: &str = "app";

/// The line that container's program writes, which is what a delivered log event carries —
/// and what a task that never started delivers none of.
const LINE: &str = "the container ran";

/// The image the containers start from: a reference naming a registry this emulator answers
/// nothing for, so the image fetch decides nothing in this suite.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Task$lastStatus` of a task that is running.
const RUNNING: &str = "RUNNING";

/// `Task$lastStatus` of a task that has stopped.
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of at all
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated).
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// How often a poll re-reads the emulator.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: comfortably past the clock-owed
/// transitions a task takes to start or to stop (`state::TRANSITION_SPAN` is 500 ms), with
/// margin for a loaded test machine.
const POLL_BUDGET: Duration = Duration::from_secs(10);

/// How many tasks of the service below have to have stopped before the second test is
/// satisfied: the one `CreateService` placed, and at least one the SCHEDULER placed in its
/// stead — a launch no call was there to decide, held to the same rule.
const REPLACED_AT_LEAST: usize = 2;

/// A router carrying Amazon ECS, IAM — which holds the role whose trust policy decides the
/// assume — and CloudWatch Logs, which a container's output would be delivered to, over ONE
/// shared engine, mounted the way the production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// One CloudWatch Logs call, the way a client makes it.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{SIGNED_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /logs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// Create the log group every container of this suite is configured to deliver to.
async fn create_log_group(router: &MultiServiceRouter, case: &str) {
    let (status, body) = logs(
        router,
        "CreateLogGroup",
        json!({ "logGroupName": LOG_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLogGroup to answer 200, got {status} with {body}"
    );
}

/// The messages CloudWatch Logs holds for one task's container, oldest first. An unknown
/// stream reads as no messages, which is what a stream nothing was delivered to holds.
async fn delivered(router: &MultiServiceRouter, task_arn: &str) -> Vec<String> {
    let task_id = task_arn.rsplit('/').next().unwrap_or_default();
    let (status, body) = logs(
        router,
        "GetLogEvents",
        json!({
            "logGroupName": LOG_GROUP,
            // `<prefix>/<container name>/<task id>`, the naming the `awslogs` driver's own
            // `awslogs-stream-prefix` documentation states.
            "logStreamName": format!("{STREAM_PREFIX}/{CONTAINER}/{task_id}"),
            "startFromHead": true,
        }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A trust policy with one statement: `effect` on `actions` for `principals`.
fn trust(effect: &str, principals: &[&str], actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principals },
            "Action": actions,
        }],
    })
    .to_string()
}

/// Create the IAM role `name` carrying `document` as its trust policy — through IAM's own
/// API — and answer the role's ARN.
async fn role(router: &MultiServiceRouter, name: &str, document: &str, case: &str) -> String {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", document.to_string()),
        ],
        case,
    )
    .await;
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The one container definition every revision below carries: it RUNS a program that writes
/// [`LINE`], delivering its output to [`LOG_GROUP`] through the `awslogs` driver — so a task
/// that launches delivers that line and a task that never launches delivers nothing.
fn container() -> Value {
    json!({
        "name": CONTAINER,
        "image": IMAGE,
        "memory": 128,
        "command": ["sh", "-c", format!("echo {LINE}")],
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": LOG_GROUP,
                "awslogs-region": SIGNED_REGION,
                "awslogs-stream-prefix": STREAM_PREFIX,
            },
        },
    })
}

/// Register one revision of `family` whose `executionRoleArn` and `taskRoleArn` are the roles
/// `slots` puts under those members (each absent when the row names none).
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    slots: &[(Slot, String)],
    case: &str,
) {
    let mut definition = json!({
        "family": family,
        "containerDefinitions": [container()],
    });
    for (slot, role_arn) in slots {
        match slot {
            Slot::Execution => definition["executionRoleArn"] = json!(role_arn),
            Slot::Task => definition["taskRoleArn"] = json!(role_arn),
            // An override is the launch's, not the revision's.
            Slot::OverrideExecution => {}
        }
    }
    ok(router, "RegisterTaskDefinition", definition, case).await;
}

/// Launch one standalone task of `family` in `cluster`, carrying the `overrides` the row's
/// slots ask for, and answer its ARN.
async fn run_task(
    router: &MultiServiceRouter,
    cluster: &str,
    family: &str,
    slots: &[(Slot, String)],
    case: &str,
) -> String {
    let mut request =
        json!({ "cluster": cluster, "taskDefinition": family, "launchType": "EC2" });
    for (slot, role_arn) in slots {
        if matches!(slot, Slot::OverrideExecution) {
            request["overrides"] = json!({ "executionRoleArn": role_arn });
        }
    }
    let body = ok(router, "RunTask", request, case).await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected RunTask to place one task, got {body}"
    );
    string(&tasks[0], "taskArn", case)
}

/// Read ONE task back through `DescribeTasks` — the stored record, not the answer of the call
/// that launched it.
async fn describe(router: &MultiServiceRouter, cluster: &str, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected DescribeTasks to describe {task_arn}, got {body}"
    );
    tasks[0].clone()
}

/// Poll one task until its `lastStatus` is `wanted`, answering that record and EVERY status
/// the poll observed on the way — which is how a row says a task never reported `RUNNING`.
async fn poll_to(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    wanted: &str,
    case: &str,
) -> (Value, Vec<String>) {
    let deadline = Instant::now() + POLL_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let task = describe(router, cluster, task_arn, case).await;
        let status = string(&task, "lastStatus", case);
        if !seen.contains(&status) {
            seen.push(status.clone());
        }
        if status == wanted {
            return (task, seen);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to reach {wanted}, it was still {status:?} after \
             {seen:?}: {task}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `stoppedReason` Amazon ECS words the stop of a task whose role it could not assume
/// with: fixed text carrying the role the launch resolved.
fn unable_to_assume(role_arn: &str) -> String {
    format!(
        "ECS was unable to assume the configured role '{role_arn}'. Please verify that the ECS \
         service principal is allowed to assume this role."
    )
}

/// Where a row puts a role: the member of the revision that names it, or the launch's own
/// override, which stands in place of the revision's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `TaskDefinition$executionRoleArn` — the role Amazon ECS itself acts as.
    Execution,
    /// `TaskDefinition$taskRoleArn` — the role the task's containers are given.
    Task,
    /// `RunTask$overrides.executionRoleArn` — "the task execution role override for the task".
    OverrideExecution,
}

impl Slot {
    /// A suffix for the IAM role name, so each slot of each row holds its own role.
    fn suffix(self) -> &'static str {
        match self {
            Self::Execution => "exec",
            Self::Task => "task",
            Self::OverrideExecution => "override",
        }
    }
}

/// What a row says about one of the roles it names.
enum Role {
    /// This account's IAM holds the role, carrying this trust document.
    Held(String),
    /// The ARN names a role of this account that IAM holds nothing under.
    AbsentFromIam,
}

/// What a row expects of the task it launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Amazon ECS assumed every role the launch named (or had none to assume), so the task
    /// launched and ran.
    Launched,
    /// IAM answered that the role in this slot withholds the assume, so nothing was launched
    /// and the stop names THAT role.
    FailedToStart(Slot),
}

/// One row: the roles the launch names, where each sits, and what the launch must do.
struct Row {
    case: &'static str,
    roles: Vec<(Slot, Role)>,
    expected: Outcome,
}

/// One launched task, waiting to be read back, with the ARN each of its slots resolved to.
struct Launched {
    case: &'static str,
    roles: Vec<(Slot, String)>,
    task_arn: String,
    expected: Outcome,
}

#[tokio::test]
async fn a_task_launches_only_under_roles_ecs_may_assume() {
    let router = emulator();
    let cluster = "trustcluster";
    let case = "the suite's cluster";
    cluster_with_capacity(&router, cluster, case).await;
    create_log_group(&router, case).await;

    // The trust documents the rows are built from: one statement each, differing along the
    // effect, the principals and the actions it names.
    let admits = trust("Allow", &["ecs-tasks.amazonaws.com"], &["sts:AssumeRole"]);
    let admits_ec2_only = trust("Allow", &["ec2.amazonaws.com"], &["sts:AssumeRole"]);
    let rows = vec![
        Row {
            case: "an execution role whose trust policy admits this service",
            roles: vec![(Slot::Execution, Role::Held(admits.clone()))],
            expected: Outcome::Launched,
        },
        Row {
            // The recorded misconfiguration: a role that trusts Amazon EC2 and nothing else.
            case: "an execution role admitting another service only",
            roles: vec![(Slot::Execution, Role::Held(admits_ec2_only.clone()))],
            expected: Outcome::FailedToStart(Slot::Execution),
        },
        Row {
            case: "an execution role admitting several services, this one among them",
            roles: vec![(
                Slot::Execution,
                Role::Held(trust(
                    "Allow",
                    &["ec2.amazonaws.com", "ecs-tasks.amazonaws.com"],
                    &["sts:AssumeRole"],
                )),
            )],
            expected: Outcome::Launched,
        },
        Row {
            case: "an execution role whose trust policy DENIES this service the assume",
            roles: vec![(
                Slot::Execution,
                Role::Held(trust(
                    "Deny",
                    &["ecs-tasks.amazonaws.com"],
                    &["sts:AssumeRole"],
                )),
            )],
            expected: Outcome::FailedToStart(Slot::Execution),
        },
        Row {
            case: "a wildcard action covering the assume",
            roles: vec![(
                Slot::Execution,
                Role::Held(trust("Allow", &["ecs-tasks.amazonaws.com"], &["sts:*"])),
            )],
            expected: Outcome::Launched,
        },
        Row {
            case: "another STS action, which is not the assume",
            roles: vec![(
                Slot::Execution,
                Role::Held(trust(
                    "Allow",
                    &["ecs-tasks.amazonaws.com"],
                    &["sts:TagSession"],
                )),
            )],
            expected: Outcome::FailedToStart(Slot::Execution),
        },
        Row {
            // IAM compares the principal and the action case-insensitively, so a document
            // spelled in another case grants exactly the same assume.
            case: "the principal and the action spelled in another case",
            roles: vec![(
                Slot::Execution,
                Role::Held(trust(
                    "Allow",
                    &["ECS-Tasks.AmazonAWS.com"],
                    &["STS:AssumeRole"],
                )),
            )],
            expected: Outcome::Launched,
        },
        Row {
            // A trust policy may name principal kinds other than a service: a role trusted
            // by an IAM principal admits that principal and not this service.
            case: "a trust policy admitting an IAM principal rather than a service",
            roles: vec![(
                Slot::Execution,
                Role::Held(
                    json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                            "Action": "sts:AssumeRole",
                        }],
                    })
                    .to_string(),
                ),
            )],
            expected: Outcome::FailedToStart(Slot::Execution),
        },
        Row {
            // Nothing is KNOWN about such a role: these roles are routinely provisioned out
            // of band, so IAM holding no record of one is evidence about this store.
            case: "a role this account's IAM holds no record for",
            roles: vec![(Slot::Execution, Role::AbsentFromIam)],
            expected: Outcome::Launched,
        },
        Row {
            case: "a revision naming no role at all",
            roles: Vec::new(),
            expected: Outcome::Launched,
        },
        Row {
            // The TASK role is assumed for the same launch, by the same principal, so the
            // same trust policy decides it.
            case: "a task role admitting another service only",
            roles: vec![(Slot::Task, Role::Held(admits_ec2_only.clone()))],
            expected: Outcome::FailedToStart(Slot::Task),
        },
        Row {
            case: "a task role admitting this service, beside an execution role that does",
            roles: vec![
                (Slot::Execution, Role::Held(admits.clone())),
                (Slot::Task, Role::Held(admits.clone())),
            ],
            expected: Outcome::Launched,
        },
        Row {
            // Amazon ECS acts as the execution role before the containers are given the task
            // role, so a launch that can assume neither stops with the execution role.
            case: "neither role assumable, which stops with the execution role",
            roles: vec![
                (Slot::Execution, Role::Held(admits_ec2_only.clone())),
                (Slot::Task, Role::Held(admits_ec2_only.clone())),
            ],
            expected: Outcome::FailedToStart(Slot::Execution),
        },
        Row {
            // An override stands in place of the revision's value, so it is the resolved role
            // that is judged — not the one the revision fixed.
            case: "an override naming a role that withholds the assume",
            roles: vec![
                (Slot::Execution, Role::Held(admits.clone())),
                (Slot::OverrideExecution, Role::Held(admits_ec2_only.clone())),
            ],
            expected: Outcome::FailedToStart(Slot::OverrideExecution),
        },
        Row {
            case: "an override naming a role that admits it, over a revision role that does \
                   not",
            roles: vec![
                (Slot::Execution, Role::Held(admits_ec2_only.clone())),
                (Slot::OverrideExecution, Role::Held(admits.clone())),
            ],
            expected: Outcome::Launched,
        },
    ];

    // Every task is launched first and read back afterwards: each settles on its own clock,
    // so the whole table costs one set of spans rather than one per row.
    let mut launched = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut resolved: Vec<(Slot, String)> = Vec::new();
        for (slot, held) in &row.roles {
            let name = format!("trustrole{index}{}", slot.suffix());
            let role_arn = match held {
                Role::Held(document) => role(&router, &name, document, row.case).await,
                Role::AbsentFromIam => format!("arn:aws:iam::{ACCOUNT}:role/{name}-never-made"),
            };
            resolved.push((*slot, role_arn));
        }
        let family = format!("trustfamily{index}");
        register(&router, &family, &resolved, row.case).await;
        let task_arn = run_task(&router, cluster, &family, &resolved, row.case).await;
        launched.push(Launched {
            case: row.case,
            roles: resolved,
            task_arn,
            expected: row.expected,
        });
    }

    for Launched {
        case,
        roles,
        task_arn,
        expected,
    } in launched
    {
        match expected {
            Outcome::Launched => {
                let (task, _) = poll_to(&router, cluster, &task_arn, RUNNING, case).await;
                assert!(
                    !task["startedAt"].is_null(),
                    "case {case:?}: expected a startedAt on a task that launched, got {task}"
                );
                assert!(
                    task["stopCode"].is_null(),
                    "case {case:?}: expected no stopCode on a task that launched, got {task}"
                );
                // The container's own output is what says it RAN: the line it writes reaches
                // the log group the revision configured.
                let deadline = Instant::now() + POLL_BUDGET;
                loop {
                    let messages = delivered(&router, &task_arn).await;
                    if messages.iter().any(|message| message == LINE) {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "case {case:?}: expected the container's line {LINE:?} in the log group, \
                         got {messages:?}"
                    );
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
            Outcome::FailedToStart(slot) => {
                let (task, seen) = poll_to(&router, cluster, &task_arn, STOPPED, case).await;
                assert!(
                    !seen.iter().any(|status| status == RUNNING),
                    "case {case:?}: expected a task whose role cannot be assumed never to \
                     report {RUNNING}, observed {seen:?}: {task}"
                );
                assert_eq!(
                    string(&task, "stopCode", case),
                    TASK_FAILED_TO_START,
                    "case {case:?}: expected the stop code of a task that never started, got \
                     {task}"
                );
                let role_arn = roles
                    .iter()
                    .find(|(candidate, _)| *candidate == slot)
                    .map(|(_, role_arn)| role_arn.as_str())
                    .unwrap_or_else(|| panic!("case {case:?}: this row names a {slot:?} role"));
                assert_eq!(
                    string(&task, "stoppedReason", case),
                    unable_to_assume(role_arn),
                    "case {case:?}: expected the failure to assume the {slot:?} role as the \
                     stoppedReason, got {task}"
                );
                assert!(
                    task["startedAt"].is_null(),
                    "case {case:?}: expected no startedAt on a task that never ran, got {task}"
                );
                // Amazon ECS pulls an image AS the execution role, so a task whose role it
                // could not assume never reached a registry: no pull was attempted.
                for member in ["pullStartedAt", "pullStoppedAt"] {
                    assert!(
                        task[member].is_null(),
                        "case {case:?}: expected no {member} on a task that was never launched, \
                         got {task}"
                    );
                }
                let messages = delivered(&router, &task_arn).await;
                assert!(
                    messages.is_empty(),
                    "case {case:?}: expected nothing delivered for a container that never ran, \
                     got {messages:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_scheduler_launches_no_task_under_a_role_ecs_may_not_assume() {
    let router = emulator();
    let cluster = "trustservicecluster";
    let case = "a service whose revision's execution role trusts another service";
    cluster_with_capacity(&router, cluster, case).await;
    create_log_group(&router, case).await;

    let role_arn = role(
        &router,
        "trustserviceroleec2",
        &trust("Allow", &["ec2.amazonaws.com"], &["sts:AssumeRole"]),
        case,
    )
    .await;
    register(
        &router,
        "trustservicefamily",
        &[(Slot::Execution, role_arn.clone())],
        case,
    )
    .await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "trustservice",
            "taskDefinition": "trustservicefamily",
            "desiredCount": 1,
        }),
        case,
    )
    .await;

    // The scheduler's placement is launched the same way a client's own is, so the task it
    // placed stops with the same stop code — and the service holds no running task.
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let listed = ok(
            &router,
            "ListTasks",
            json!({ "cluster": cluster, "desiredStatus": STOPPED }),
            case,
        )
        .await;
        let arns = list(&listed, "taskArns", case);
        if let Some(task_arn) = arns.first().and_then(Value::as_str) {
            let task = describe(&router, cluster, task_arn, case).await;
            assert_eq!(
                string(&task, "stopCode", case),
                TASK_FAILED_TO_START,
                "case {case:?}: expected the scheduler's task to stop with \
                 {TASK_FAILED_TO_START}, got {task}"
            );
            assert_eq!(
                string(&task, "stoppedReason", case),
                unable_to_assume(&role_arn),
                "case {case:?}: expected the role-assumption failure as the stoppedReason, got \
                 {task}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the scheduler's task to stop, none had after {listed}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // And it stays that way: nothing the scheduler replaces the failed task with reaches
    // `RUNNING` either, because the role it would run under is the same one. Waited for by
    // the SIGNAL the scheduler raises — another stopped task, which is the replacement having
    // been placed, judged and stopped in its turn — rather than for a stretch of the clock,
    // with the count asserted on every read along the way.
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let described = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": cluster, "services": ["trustservice"] }),
            case,
        )
        .await;
        let service = described["services"]
            .as_array()
            .and_then(|services| services.first().cloned())
            .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {described}"));
        assert_eq!(
            service["runningCount"].as_i64().unwrap_or_default(),
            0,
            "case {case:?}: expected a service whose tasks cannot be launched to run none, got \
             {service}"
        );
        let running = ok(
            &router,
            "ListTasks",
            json!({ "cluster": cluster, "desiredStatus": RUNNING }),
            case,
        )
        .await;
        let still_wanted = list(&running, "taskArns", case);
        for task_arn in &still_wanted {
            let task_arn = task_arn
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {running}"));
            let task = describe(&router, cluster, task_arn, case).await;
            assert_ne!(
                string(&task, "lastStatus", case),
                RUNNING,
                "case {case:?}: expected no task of the service to be running, got {task}"
            );
        }
        // The replacements themselves: once the scheduler has lost more than the one task
        // `CreateService` placed, the launches this has held to the rule include the ones the
        // CLOCK placed, which is what makes the "stays that way" an observation rather than a
        // wait.
        let stopped = ok(
            &router,
            "ListTasks",
            json!({ "cluster": cluster, "desiredStatus": STOPPED }),
            case,
        )
        .await;
        if list(&stopped, "taskArns", case).len() >= REPLACED_AT_LEAST {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the scheduler to have replaced the task it cannot launch \
             at least {} times over, got {stopped}",
            REPLACED_AT_LEAST - 1
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
