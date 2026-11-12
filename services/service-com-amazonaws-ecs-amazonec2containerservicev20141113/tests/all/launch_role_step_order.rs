// SPDX-License-Identifier: CC0-1.0

//! The ORDER the launch steps of a task are taken in: Amazon ECS assumes the roles a launch
//! resolved BEFORE it fetches the images that launch's containers run — driven end to end over
//! the wire with IAM mounted beside Amazon ECS on one engine, so every role and trust document
//! is created through the OWNING service's own API rather than written into a store by hand.
//!
//! RULE: Amazon ECS fetches a task's images AS the task execution role, so the fetch of a
//! launch happens only after the roles THAT launch resolved have been put to the account's IAM
//! records — and a launch a role withholds the assume from is fetched nothing for: it records
//! no image-fetch instants (`Task$pullStartedAt`/`pullStoppedAt`), it gets no `startedAt` and
//! it never reports `RUNNING`, however many other launches are being decided beside it.
//!
//! What makes that an ORDER and not merely a pair of steps is the task the emulator is given
//! WHILE a decision is in flight: the service scheduler places a replacement on any caller's
//! clock pass, so a launch can arrive after the assume's pass has collected its work and
//! before the fetch's has. Such a launch carries no refusal YET, which is not the same thing
//! as having been assumed — so nothing may be fetched for it, and nothing may start it, until
//! its own roles have been put to IAM. The rows are therefore sampled while several callers
//! read at once on a multi-threaded runtime, which is the only way one caller can land inside
//! another's pass.
//!
//! The rows differ along what the rule generalises over: WHO places the task (the service
//! scheduler, over and over as it replaces what it cannot launch, and a client's own
//! `RunTask`), WHICH role the launch resolves (the revision's execution role, its task role,
//! the launch's own `overrides` execution role, none at all), what the role's trust policy
//! says (another service, an explicit `Deny`, an STS action that is not the assume, an action
//! wildcard that covers it, several principals at once) and whether this account's IAM holds
//! the role at all.
//!
//! Nothing here reaches into either state struct: every value asserted is read back through
//! `ListTasks`, `DescribeTasks` and `DescribeServices`. The `stoppedReason` is spelled out
//! here rather than imported, so a wrong wording in the implementation cannot make a test
//! pass.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, count, iam, list, ok, string, ACCOUNT};

/// The one container every revision below runs: an image held in a registry this emulator
/// answers nothing for, and every task here shares the networking of the host that runs it —
/// so the image fetch itself refuses nothing and the role assume decides every row.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Task$lastStatus` of a task that is running.
const RUNNING: &str = "RUNNING";

/// `Task$lastStatus`/`ListTasks$desiredStatus` of a task that has stopped, or is wanted
/// stopped.
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of at all
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated).
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// The service principal Amazon ECS assumes a task's roles as (the `ecs-tasks` service
/// principal of the Amazon ECS task roles, Amazon Elastic Container Service Developer Guide).
const ECS_TASKS: &str = "ecs-tasks.amazonaws.com";

/// How often a poll re-reads the emulator.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long the suite keeps reading for the condition it waits on before it gives up: many
/// times the clock-owed transitions a task takes to be placed, refused, stopped and replaced
/// (`state::TRANSITION_SPAN` is 500 ms), with wide margin for a loaded test machine.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How many callers read AT ONCE while the rows settle, each on its own task of a
/// multi-threaded runtime — more than one, because one caller alone can never land inside the
/// pass another caller is in the middle of.
const SAMPLE_WORKERS: usize = 4;

/// How many tasks of a service whose launch is refused have to have stopped before the suite
/// is satisfied: the one `CreateService` placed, and at least one the SCHEDULER placed in its
/// stead — a launch no call was there to take the steps for.
const REPLACED_AT_LEAST: usize = 2;

/// A router carrying Amazon ECS and IAM — which holds the roles whose trust policies decide
/// the assumes — over ONE shared engine, mounted the way the production router mounts them.
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
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Where the role a launch resolves is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// `TaskDefinition$executionRoleArn` — the role Amazon ECS itself acts as, and the
    /// principal the image fetch is made as.
    Execution,
    /// `TaskDefinition$taskRoleArn` — the role the task's containers are given.
    Task,
    /// `RunTask$overrides.executionRoleArn` — "the task execution role override for the
    /// task", which stands in place of the revision's value.
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

/// Who places the tasks of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launcher {
    /// The service scheduler, which keeps placing: a task it cannot launch is replaced, and
    /// each replacement is a launch the CLOCK made with no call there to decide it.
    Scheduler,
    /// A client's own `RunTask`, which places exactly one task and carries the `overrides` a
    /// row asks for.
    Client,
}

/// What this account's IAM holds for the role a row names.
#[derive(Debug, Clone, Copy)]
enum Trusted {
    /// IAM holds the role, carrying one trust statement of this effect, for these principals,
    /// on these actions.
    Held {
        effect: &'static str,
        principals: &'static [&'static str],
        actions: &'static [&'static str],
    },
    /// The ARN names a role of this account that IAM holds no record for — these roles are
    /// routinely provisioned out of band, so a store holding none is evidence of nothing.
    AbsentFromIam,
}

/// One row: who places the launch, which role it resolves, what IAM holds for that role, and
/// so whether the launch may be made at all.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The stem this row's own cluster, roles, family and service are named from. Each row
    /// gets its OWN cluster, so every task a row reads is that row's alone.
    name: &'static str,
    launcher: Launcher,
    /// Where the role this row is about sits, and `None` for a revision that names no role.
    slot: Option<Slot>,
    trusted: Trusted,
    /// Whether Amazon ECS may assume what the launch resolved, and so whether its containers
    /// are fetched for and started.
    runs: bool,
}

/// A trust statement admitting Amazon ECS the assume.
const ADMITS: Trusted = Trusted::Held {
    effect: "Allow",
    principals: &[ECS_TASKS],
    actions: &["sts:AssumeRole"],
};

/// The table.
const ROWS: [Row; 9] = [
    Row {
        case: "a scheduler launch whose execution role trusts another service",
        name: "orderexecother",
        launcher: Launcher::Scheduler,
        slot: Some(Slot::Execution),
        trusted: Trusted::Held {
            effect: "Allow",
            principals: &["ec2.amazonaws.com"],
            actions: &["sts:AssumeRole"],
        },
        runs: false,
    },
    Row {
        case: "a scheduler launch whose task role denies this service the assume",
        name: "ordertaskdeny",
        launcher: Launcher::Scheduler,
        slot: Some(Slot::Task),
        trusted: Trusted::Held {
            effect: "Deny",
            principals: &[ECS_TASKS],
            actions: &["sts:AssumeRole"],
        },
        runs: false,
    },
    Row {
        case: "a scheduler launch whose execution role admits another STS action only",
        name: "orderexecaction",
        launcher: Launcher::Scheduler,
        slot: Some(Slot::Execution),
        trusted: Trusted::Held {
            effect: "Allow",
            principals: &[ECS_TASKS],
            actions: &["sts:TagSession"],
        },
        runs: false,
    },
    Row {
        case: "a scheduler launch admitted by a wildcard action, among several principals",
        name: "orderexecwildcard",
        launcher: Launcher::Scheduler,
        slot: Some(Slot::Execution),
        trusted: Trusted::Held {
            effect: "Allow",
            principals: &["ec2.amazonaws.com", ECS_TASKS],
            actions: &["sts:*"],
        },
        runs: true,
    },
    Row {
        case: "a scheduler launch naming a role this account's IAM holds no record for",
        name: "orderexecabsent",
        launcher: Launcher::Scheduler,
        slot: Some(Slot::Execution),
        trusted: Trusted::AbsentFromIam,
        runs: true,
    },
    Row {
        case: "a scheduler launch whose revision names no role at all",
        name: "ordernorole",
        launcher: Launcher::Scheduler,
        slot: None,
        trusted: ADMITS,
        runs: true,
    },
    Row {
        case: "a client launch whose execution role trusts another service",
        name: "orderrunexec",
        launcher: Launcher::Client,
        slot: Some(Slot::Execution),
        trusted: Trusted::Held {
            effect: "Allow",
            principals: &["ec2.amazonaws.com"],
            actions: &["sts:AssumeRole"],
        },
        runs: false,
    },
    Row {
        case: "a client launch whose overrides name a role that withholds the assume",
        name: "orderrunoverride",
        launcher: Launcher::Client,
        slot: Some(Slot::OverrideExecution),
        trusted: Trusted::Held {
            effect: "Allow",
            principals: &["ec2.amazonaws.com"],
            actions: &["sts:AssumeRole"],
        },
        runs: false,
    },
    Row {
        case: "a client launch whose task role admits this service",
        name: "orderruntask",
        launcher: Launcher::Client,
        slot: Some(Slot::Task),
        trusted: ADMITS,
        runs: true,
    },
];

/// The `stoppedReason` Amazon ECS words the stop of a task whose role it could not assume
/// with: fixed text carrying the role the launch resolved.
fn unable_to_assume(role_arn: &str) -> String {
    format!(
        "ECS was unable to assume the configured role '{role_arn}'. Please verify that the ECS \
         service principal is allowed to assume this role."
    )
}

/// A trust policy with one statement.
fn trust_document(effect: &str, principals: &[&str], actions: &[&str]) -> String {
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

/// Create the IAM role `name` carrying `document` — through IAM's own API — and answer its ARN.
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

/// What one row's set-up left behind: its cluster, the service that keeps placing tasks in it
/// (for a scheduler row), and the role ARN whose trust policy decides the row.
#[derive(Debug, Clone)]
struct Placed {
    row: Row,
    cluster: String,
    service: Option<String>,
    /// The role the launch RESOLVED in the row's slot — the one a refusal names — and `None`
    /// for a row whose revision names no role.
    role_arn: Option<String>,
}

/// Build everything one row needs and place its first task.
async fn set_up(router: &MultiServiceRouter, row: Row) -> Placed {
    let case = row.case;
    let cluster = format!("{}cluster", row.name);
    cluster_with_capacity(router, &cluster, case).await;

    // The role the row is about, in the slot the row names.
    let role_arn = match row.slot {
        None => None,
        Some(slot) => Some(match row.trusted {
            Trusted::Held {
                effect,
                principals,
                actions,
            } => {
                let name = format!("{}{}", row.name, slot.suffix());
                let document = trust_document(effect, principals, actions);
                role(router, &name, &document, case).await
            }
            Trusted::AbsentFromIam => {
                format!("arn:aws:iam::{ACCOUNT}:role/{}-never-made", row.name)
            }
        }),
    };

    // A row whose OVERRIDE carries the judged role also gives the revision an execution role
    // Amazon ECS may assume, so what is judged is the role the launch RESOLVED rather than the
    // one the revision fixed.
    let revision_execution_role = match row.slot {
        Some(Slot::OverrideExecution) => {
            let name = format!("{}revision", row.name);
            let document = trust_document("Allow", &[ECS_TASKS], &["sts:AssumeRole"]);
            Some(role(router, &name, &document, case).await)
        }
        _ => None,
    };

    let family = format!("{}family", row.name);
    let mut definition = json!({
        "family": family,
        "containerDefinitions": [{
            "name": "app",
            "image": IMAGE,
            "memory": 128,
        }],
    });
    match (row.slot, role_arn.as_ref()) {
        (Some(Slot::Execution), Some(arn)) => definition["executionRoleArn"] = json!(arn),
        (Some(Slot::Task), Some(arn)) => definition["taskRoleArn"] = json!(arn),
        _ => {}
    }
    if let Some(arn) = revision_execution_role.as_ref() {
        definition["executionRoleArn"] = json!(arn);
    }
    ok(router, "RegisterTaskDefinition", definition, case).await;

    let service = match row.launcher {
        Launcher::Scheduler => {
            let service = format!("{}service", row.name);
            ok(
                router,
                "CreateService",
                json!({
                    "cluster": cluster,
                    "serviceName": service,
                    "taskDefinition": family,
                    "desiredCount": 1,
                }),
                case,
            )
            .await;
            Some(service)
        }
        Launcher::Client => {
            let mut request = json!({
                "cluster": cluster,
                "taskDefinition": family,
                "launchType": "EC2",
            });
            if row.slot == Some(Slot::OverrideExecution) {
                let arn = role_arn
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {case:?}: this row names an override role"));
                request["overrides"] = json!({ "executionRoleArn": arn });
            }
            let body = ok(router, "RunTask", request, case).await;
            let tasks = list(&body, "tasks", case);
            assert_eq!(
                tasks.len(),
                1,
                "case {case:?}: expected RunTask to place one task, got {body}"
            );
            None
        }
    };

    Placed {
        row,
        cluster,
        service,
        role_arn,
    }
}

/// Every task of `placed`'s cluster, as `DescribeTasks` reports it: the ones the scheduler
/// still wants running and the ones it has given up on.
async fn described_tasks(router: &MultiServiceRouter, placed: &Placed) -> Vec<Value> {
    let case = placed.row.case;
    let mut arns: Vec<String> = Vec::new();
    for desired in [RUNNING, STOPPED] {
        let listed = ok(
            router,
            "ListTasks",
            json!({ "cluster": placed.cluster, "desiredStatus": desired }),
            case,
        )
        .await;
        for arn in list(&listed, "taskArns", case) {
            if let Some(arn) = arn.as_str() {
                arns.push(arn.to_string());
            }
        }
    }
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": placed.cluster, "tasks": arns }),
        case,
    )
    .await;
    list(&described, "tasks", case)
}

/// `Service$runningCount` of a scheduler row's service — "the number of tasks in the cluster
/// that are in the RUNNING state" — and `None` for a row that has no service.
async fn running_count(router: &MultiServiceRouter, placed: &Placed) -> Option<i64> {
    let case = placed.row.case;
    let service = placed.service.as_ref()?;
    let described = ok(
        router,
        "DescribeServices",
        json!({ "cluster": placed.cluster, "services": [service] }),
        case,
    )
    .await;
    let services = list(&described, "services", case);
    let reported = services
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {described}"));
    Some(count(&reported, "runningCount", case))
}

/// The RULE, asserted over every task `placed` holds right now. Every read here is also a
/// clock pass, so a sample that finds a task running whose launch nothing had decided is a
/// sample that caught the fetch running ahead of the assume.
async fn assert_rule(router: &MultiServiceRouter, placed: &Placed) {
    let case = placed.row.case;
    let tasks = described_tasks(router, placed).await;
    if placed.row.runs {
        for task in &tasks {
            assert_ne!(
                task["stopCode"].as_str(),
                Some(TASK_FAILED_TO_START),
                "case {case:?}: expected no failed launch for a role Amazon ECS may assume, \
                 got {task}"
            );
            if task["lastStatus"].as_str() == Some(RUNNING) {
                // The fetch precedes the start, so a task that is running was fetched for.
                assert!(
                    !task["pullStartedAt"].is_null() && !task["startedAt"].is_null(),
                    "case {case:?}: expected a running task to carry the instants of the \
                     fetch its start waited for, got {task}"
                );
            }
        }
        return;
    }
    let role_arn = placed
        .role_arn
        .as_deref()
        .unwrap_or_else(|| panic!("case {case:?}: a refused row names the role it is refused by"));
    for task in &tasks {
        let status = string(task, "lastStatus", case);
        assert_ne!(
            status, RUNNING,
            "case {case:?}: expected no task of a launch Amazon ECS may not make to run, \
             got {task}"
        );
        assert!(
            task["startedAt"].is_null(),
            "case {case:?}: expected no startedAt on a task that was never launched, got {task}"
        );
        // Amazon ECS fetches an image AS the execution role, so a launch whose role it could
        // not assume reached no registry: the fetch was never attempted for it.
        for member in ["pullStartedAt", "pullStoppedAt"] {
            assert!(
                task[member].is_null(),
                "case {case:?}: expected no {member} on a task whose roles Amazon ECS never \
                 assumed, got {task}"
            );
        }
        if status == STOPPED {
            assert_eq!(
                string(task, "stopCode", case),
                TASK_FAILED_TO_START,
                "case {case:?}: expected the stop code of a task that never started, got {task}"
            );
            assert_eq!(
                string(task, "stoppedReason", case),
                unable_to_assume(role_arn),
                "case {case:?}: expected the failure to assume the role the launch resolved as \
                 the stoppedReason, got {task}"
            );
        }
    }
    if let Some(running) = running_count(router, placed).await {
        assert_eq!(
            running, 0,
            "case {case:?}: expected a service whose tasks cannot be launched to run none, got \
             {running}"
        );
    }
}

/// Whether `placed` has settled far enough for the suite to stop: a refused scheduler row has
/// had its task replaced at least [`REPLACED_AT_LEAST`] times over (so the launches the rule
/// was asserted over include the ones the CLOCK placed), a refused client row's one task has
/// stopped, and an admitted row is running the task it was placed for.
async fn settled(router: &MultiServiceRouter, placed: &Placed) -> bool {
    let tasks = described_tasks(router, placed).await;
    let stopped = tasks
        .iter()
        .filter(|task| task["lastStatus"].as_str() == Some(STOPPED))
        .count();
    let running = tasks
        .iter()
        .filter(|task| task["lastStatus"].as_str() == Some(RUNNING))
        .count();
    match (placed.row.runs, placed.row.launcher) {
        (true, _) => running >= 1,
        (false, Launcher::Scheduler) => stopped >= REPLACED_AT_LEAST,
        (false, Launcher::Client) => stopped >= 1,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_launch_is_fetched_for_before_its_roles_are_assumed() {
    let router = Arc::new(emulator());
    let mut placements = Vec::new();
    for row in ROWS {
        placements.push(set_up(&router, row).await);
    }

    // Several callers read at once while the rows settle: each read is a clock pass, so the
    // pass that takes a launch's steps and the pass that would fetch for and start another
    // launch are different callers' — which is the window the rule is about.
    let done = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for worker in 0..SAMPLE_WORKERS {
        let router = Arc::clone(&router);
        let placements = placements.clone();
        let done = Arc::clone(&done);
        workers.push(tokio::spawn(async move {
            // Each worker starts at a different row, so the readers are spread over the rows
            // rather than all reading the same one at the same instant.
            let mut sample = worker;
            while !done.load(Ordering::Relaxed) {
                let placed = &placements[sample % placements.len()];
                sample += 1;
                assert_rule(&router, placed).await;
                // Yield rather than sleep: the samples have to land inside the passes the
                // other workers and the clock are making, not between them.
                tokio::task::yield_now().await;
            }
        }));
    }

    let deadline = Instant::now() + SETTLE_DEADLINE;
    let outcome = loop {
        let mut outstanding = Vec::new();
        for placed in &placements {
            if !settled(&router, placed).await {
                outstanding.push(placed.row.case);
            }
        }
        if outstanding.is_empty() {
            break Ok(());
        }
        if Instant::now() >= deadline {
            break Err(outstanding);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    done.store(true, Ordering::Relaxed);
    for (worker, handle) in workers.into_iter().enumerate() {
        handle
            .await
            .unwrap_or_else(|error| panic!("reader {worker} failed: {error}"));
    }
    if let Err(outstanding) = outcome {
        panic!(
            "expected every row to settle — an admitted launch running, a refused scheduler \
             launch replaced {REPLACED_AT_LEAST} times over, a refused client launch stopped — \
             still waiting on {outstanding:?}"
        );
    }

    // And the rule holds on one last read, after the sampling has stopped: the refused rows
    // were fetched nothing for and ran nothing, whichever pass arrived last.
    for placed in &placements {
        assert_rule(&router, placed).await;
    }
}
