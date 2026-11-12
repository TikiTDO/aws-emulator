// SPDX-License-Identifier: CC0-1.0

//! What the START of a task waits for: the launch steps Amazon ECS takes before any container
//! of it runs — assuming the roles the launch resolved, judged against the trust policy each
//! carries in the account's IAM records — driven end to end over the wire with IAM mounted
//! beside Amazon ECS on one engine, so every role and trust document is created through the
//! OWNING service's own API rather than written into a store by hand.
//!
//! RULE: a task reports `RUNNING` only once Amazon ECS has taken the launch steps its start
//! depends on, so NO operation reports a task running before those steps have been taken for
//! it — whichever operation's clock pass is the one that materialises the start, and whoever
//! placed the task: a client's own call, or the service scheduler, which replaces a task its
//! service lost with no call there at all. Every such pass takes the steps itself, so the
//! start of a launch nothing blocks is withheld rather than dropped: it happens as soon as
//! the steps have been taken.
//!
//! The rows differ along what the rule generalises over: WHICH role the launch resolves (the
//! revision's execution role, its task role), what the role's own trust policy says about
//! this service (another service's principal, an explicit `Deny`, an `Allow`, an action
//! wildcard that covers the assume), and so whether the launch may be made at all — and,
//! crossing them, WHICH operation's clock pass is the one that could start the task, sampled
//! over and over while the scheduler keeps replacing the tasks it cannot launch, so the tasks
//! the samples fall on are the ones the CLOCK placed rather than only the one `CreateService`
//! did.
//!
//! Nothing here reaches into either state struct: every value asserted is read back through
//! `DescribeServices`, `ListTasks`, `DescribeTasks` and `DescribeClusters`. The
//! `stoppedReason` is spelled out here rather than imported, so a wrong wording in the
//! implementation cannot make a test pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{count, create_cluster, iam, list, ok, register_capacity, string, ACCOUNT};

/// The one container of every revision below: it runs an image held in a registry this
/// emulator answers nothing for, and the task shares the networking of the host that runs it,
/// so the image fetch decides nothing in this suite and the role assume decides everything.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Task$lastStatus` of a task that is running.
const RUNNING: &str = "RUNNING";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of at all
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated).
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// The service principal Amazon ECS assumes a task's roles as (the `ecs-tasks` service
/// principal of the Amazon ECS task roles, Amazon Elastic Container Service Developer Guide).
const ECS_TASKS: &str = "ecs-tasks.amazonaws.com";

/// How long the sampling loop keeps reading before it asks whether it has seen enough: long
/// enough for the scheduler to have placed, failed and replaced a task more than once (the
/// transitions of one such cycle are two `state::TRANSITION_SPAN` apart).
const SAMPLE_BUDGET: Duration = Duration::from_secs(3);

/// How long one sample waits before the next, so the samples fall at every phase of those
/// cycles rather than at one.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(5);

/// The hard cap on the sampling loop: comfortably past [`SAMPLE_BUDGET`] plus the clock-owed
/// transitions it waits for, with margin for a loaded test machine.
const SAMPLE_DEADLINE: Duration = Duration::from_secs(25);

/// How many tasks of a service whose role Amazon ECS may not assume have to have stopped
/// before the loop is satisfied: the one `CreateService` placed, and at least one the
/// SCHEDULER placed in its stead — the launch no call was there to take the steps for.
const REPLACED_AT_LEAST: usize = 2;

/// How often a poll re-reads the emulator, and how long it keeps trying.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLL_BUDGET: Duration = Duration::from_secs(15);

/// A router carrying Amazon ECS and IAM — which holds the role whose trust policy decides the
/// assume — over ONE shared engine, mounted the way the production router mounts them.
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

/// Where a revision declares the role a launch of it resolves.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// `TaskDefinition$executionRoleArn`, which Amazon ECS itself acts as.
    Execution,
    /// `TaskDefinition$taskRoleArn`, which it hands the task's containers.
    Task,
}

/// One row: the role a launch resolves, where the revision declares it, what the role's own
/// trust policy says about Amazon ECS, and so whether the service runs its task at all.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The stem this row's own cluster, role, family and service are named from. Each row
    /// gets its OWN cluster, so a count a cluster reports is this row's alone.
    name: &'static str,
    slot: Slot,
    effect: &'static str,
    principal: &'static str,
    action: &'static str,
    /// Whether Amazon ECS may assume the role, and so whether the service's task starts.
    runs: bool,
}

/// The table: two roles that withhold the assume and two that admit it, over both role slots.
const ROWS: [Row; 4] = [
    Row {
        case: "the revision's execution role trusts another service",
        name: "stepsexecother",
        slot: Slot::Execution,
        effect: "Allow",
        principal: "ec2.amazonaws.com",
        action: "sts:AssumeRole",
        runs: false,
    },
    Row {
        case: "the revision's task role denies this service the assume",
        name: "stepstaskdeny",
        slot: Slot::Task,
        effect: "Deny",
        principal: ECS_TASKS,
        action: "sts:AssumeRole",
        runs: false,
    },
    Row {
        case: "the revision's execution role admits this service through an action wildcard",
        name: "stepsexecwildcard",
        slot: Slot::Execution,
        effect: "Allow",
        principal: ECS_TASKS,
        action: "sts:*",
        runs: true,
    },
    Row {
        case: "the revision's task role admits this service",
        name: "stepstaskallow",
        slot: Slot::Task,
        effect: "Allow",
        principal: ECS_TASKS,
        action: "sts:AssumeRole",
        runs: true,
    },
];

/// What one row's set-up left behind: the cluster its service runs in, the service itself,
/// the container instance registered into that cluster, and the role ARN the launch resolves.
#[derive(Debug, Clone)]
struct Placed {
    row: Row,
    cluster: String,
    service: String,
    instance_arn: String,
    role_arn: String,
}

/// The `stoppedReason` Amazon ECS words the stop of a task whose role it could not assume
/// with, spelled out here rather than imported.
fn unable_to_assume(role_arn: &str) -> String {
    format!(
        "ECS was unable to assume the configured role '{role_arn}'. Please verify that the ECS \
         service principal is allowed to assume this role."
    )
}

/// A trust policy with one statement: `effect` on `action` for `principal`.
fn trust(row: &Row) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": row.effect,
            "Principal": { "Service": [row.principal] },
            "Action": [row.action],
        }],
    })
    .to_string()
}

/// Build everything one row needs — its cluster with capacity, its role, a revision declaring
/// that role in the row's slot, and a service that wants one task of it — and answer what the
/// assertions read back through.
async fn set_up(router: &MultiServiceRouter, row: Row) -> Placed {
    let case = row.case;
    let cluster = format!("{}cluster", row.name);
    create_cluster(router, &cluster, case).await;
    let instance_arn = register_capacity(router, &cluster, case).await;

    let role = format!("{}role", row.name);
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role.clone()),
            ("AssumeRolePolicyDocument", trust(&row)),
        ],
        case,
    )
    .await;
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role}");

    let family = format!("{}family", row.name);
    let mut definition = json!({
        "family": family,
        "containerDefinitions": [{
            "name": "app",
            "image": IMAGE,
            "memory": 128,
        }],
    });
    match row.slot {
        Slot::Execution => definition["executionRoleArn"] = json!(role_arn),
        Slot::Task => definition["taskRoleArn"] = json!(role_arn),
    }
    ok(router, "RegisterTaskDefinition", definition, case).await;

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

    Placed {
        row,
        cluster,
        service,
        instance_arn,
        role_arn,
    }
}

/// The operations whose clock pass could be the one that starts a task: each of them
/// materialises what the clock owes before it answers, so each is a pass the rule has to hold
/// for.
#[derive(Debug, Clone, Copy)]
enum Reader {
    /// `Service$runningCount`: "The number of tasks in the cluster that are in the RUNNING
    /// state."
    DescribeServices,
    /// The tasks themselves: `ListTasks` for what the scheduler still wants running, and
    /// `Task$lastStatus` for where each of them stands.
    DescribeTasks,
    /// `Cluster$runningTasksCount`: "The number of tasks in the cluster that are in the
    /// RUNNING state."
    DescribeClusters,
    /// A pass that reports no count of a service's tasks — a task the scheduler placed is on
    /// no named container instance — and only materialises what the clock owes.
    DescribeContainerInstances,
    /// The same, through a listing.
    ListServices,
}

const READERS: [Reader; 5] = [
    Reader::DescribeServices,
    Reader::DescribeTasks,
    Reader::DescribeClusters,
    Reader::DescribeContainerInstances,
    Reader::ListServices,
];

/// Every task of `placed`'s cluster that reports `RUNNING`, by ARN — read through `ListTasks`
/// and `DescribeTasks`, never off a count.
async fn running_tasks(router: &MultiServiceRouter, placed: &Placed) -> Vec<String> {
    let case = placed.row.case;
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": placed.cluster }),
        case,
    )
    .await;
    let arns: Vec<String> = list(&listed, "taskArns", case)
        .iter()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect();
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
        .iter()
        .filter(|task| task["lastStatus"].as_str() == Some(RUNNING))
        .map(|task| string(task, "taskArn", case))
        .collect()
}

/// How many tasks of `placed` this reader reports RUNNING, and `None` for a reader that
/// reports no count of its own — which still materialises what the clock owes, which is the
/// whole reason it is in the table.
async fn running_as(
    router: &MultiServiceRouter,
    reader: Reader,
    placed: &Placed,
) -> Option<i64> {
    let case = placed.row.case;
    match reader {
        Reader::DescribeServices => {
            let described = ok(
                router,
                "DescribeServices",
                json!({ "cluster": placed.cluster, "services": [placed.service] }),
                case,
            )
            .await;
            let services = list(&described, "services", case);
            let service = services
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {described}"));
            Some(count(&service, "runningCount", case))
        }
        Reader::DescribeTasks => {
            Some(i64::try_from(running_tasks(router, placed).await.len()).unwrap_or(i64::MAX))
        }
        Reader::DescribeClusters => {
            let described = ok(
                router,
                "DescribeClusters",
                json!({ "clusters": [placed.cluster] }),
                case,
            )
            .await;
            let clusters = list(&described, "clusters", case);
            let cluster = clusters
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected the cluster back, got {described}"));
            Some(count(&cluster, "runningTasksCount", case))
        }
        Reader::DescribeContainerInstances => {
            ok(
                router,
                "DescribeContainerInstances",
                json!({
                    "cluster": placed.cluster,
                    "containerInstances": [placed.instance_arn],
                }),
                case,
            )
            .await;
            None
        }
        Reader::ListServices => {
            ok(
                router,
                "ListServices",
                json!({ "cluster": placed.cluster }),
                case,
            )
            .await;
            None
        }
    }
}

/// Every task of `placed`'s cluster that has STOPPED, as `DescribeTasks` reports it.
async fn stopped_tasks(router: &MultiServiceRouter, placed: &Placed) -> Vec<Value> {
    let case = placed.row.case;
    let listed = ok(
        router,
        "ListTasks",
        json!({ "cluster": placed.cluster, "desiredStatus": "STOPPED" }),
        case,
    )
    .await;
    let arns: Vec<String> = list(&listed, "taskArns", case)
        .iter()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect();
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

#[tokio::test]
async fn no_read_reports_a_task_running_before_its_launch_has_been_decided() {
    let router = emulator();
    let mut placements = Vec::new();
    for row in ROWS {
        placements.push(set_up(&router, row).await);
    }

    // Sample every reader against every row, over and over, while the scheduler keeps
    // replacing the tasks it cannot launch: a read that materialised the start of a task
    // whose launch nothing had decided yet would report it RUNNING here.
    let mut started = vec![false; placements.len()];
    let mut replaced = vec![0usize; placements.len()];
    let began = Instant::now();
    let deadline = began + SAMPLE_DEADLINE;
    let mut sample = 0usize;
    loop {
        let reader = READERS[sample % READERS.len()];
        sample += 1;
        for (placed, ran) in placements.iter().zip(started.iter_mut()) {
            let case = placed.row.case;
            let Some(running) = running_as(&router, reader, placed).await else {
                continue;
            };
            if placed.row.runs {
                assert!(
                    running <= 1,
                    "case {case:?}: expected the one task the service wants, {reader:?} \
                     reported {running} running"
                );
                if running == 1 {
                    *ran = true;
                } else {
                    assert!(
                        !*ran,
                        "case {case:?}: expected a task that has started to stay running, \
                         {reader:?} reported {running} running"
                    );
                }
            } else {
                assert_eq!(
                    running, 0,
                    "case {case:?}: expected no task of a service whose role Amazon ECS may \
                     not assume to be reported running, {reader:?} reported {running}"
                );
            }
        }
        if began.elapsed() >= SAMPLE_BUDGET {
            for (placed, failed) in placements.iter().zip(replaced.iter_mut()) {
                if !placed.row.runs {
                    *failed = stopped_tasks(&router, placed).await.len();
                }
            }
            let enough = placements.iter().zip(started.iter()).zip(replaced.iter()).all(
                |((placed, ran), failed)| match placed.row.runs {
                    true => *ran,
                    false => *failed >= REPLACED_AT_LEAST,
                },
            );
            if enough {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "expected every service whose role Amazon ECS may assume to have started its \
             task and every service whose role it may not to have had its task replaced at \
             least {REPLACED_AT_LEAST} times over, got started={started:?} stopped={replaced:?} \
             after {sample} samples"
        );
        tokio::time::sleep(SAMPLE_INTERVAL).await;
    }

    // And every one of those stops is the stop of a launch that was never made: the tasks the
    // scheduler placed were judged exactly as the one `CreateService` placed was.
    for placed in &placements {
        let case = placed.row.case;
        if placed.row.runs {
            continue;
        }
        for task in stopped_tasks(&router, placed).await {
            assert_eq!(
                string(&task, "stopCode", case),
                TASK_FAILED_TO_START,
                "case {case:?}: expected the stop code of a task that never started, got {task}"
            );
            assert_eq!(
                string(&task, "stoppedReason", case),
                unable_to_assume(&placed.role_arn),
                "case {case:?}: expected the failure to assume the role as the stoppedReason, \
                 got {task}"
            );
            assert!(
                task["startedAt"].is_null(),
                "case {case:?}: expected no startedAt on a task that never ran, got {task}"
            );
        }
    }
}

/// Poll until exactly one task of `placed` is RUNNING and answer its ARN.
async fn poll_to_running(router: &MultiServiceRouter, placed: &Placed) -> String {
    let case = placed.row.case;
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let running = running_tasks(router, placed).await;
        if let Some(task_arn) = running.first() {
            return task_arn.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the service to run the task it wants, none was running"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_task_the_scheduler_places_in_a_stopped_tasks_stead_starts_too() {
    let router = emulator();
    // The rows whose role Amazon ECS may assume: the start of such a task is WITHHELD until
    // the launch steps have been taken, never dropped — including for the replacement, which
    // the CLOCK places and no call is there to take the steps for.
    for row in ROWS.into_iter().filter(|row| row.runs) {
        let case = row.case;
        let placed = set_up(&router, row).await;
        let first = poll_to_running(&router, &placed).await;

        ok(
            &router,
            "StopTask",
            json!({
                "cluster": placed.cluster,
                "task": first,
                "reason": "making room for the scheduler's replacement",
            }),
            case,
        )
        .await;

        let replacement = poll_to_running(&router, &placed).await;
        assert_ne!(
            replacement, first,
            "case {case:?}: expected the task the scheduler placed in the stopped task's \
             stead, got the stopped task back"
        );
    }
}

/// How many readers hammer the emulator AT ONCE in the concurrent row below, each on its own
/// task of a multi-threaded runtime — more than one, because one caller alone can never
/// observe the pass another caller is in the middle of.
const CONCURRENT_READERS: usize = 4;

/// How long NOTHING runs for in the starved row below: longer than the start the scheduler's
/// replacement is owed (`state::TRANSITION_SPAN`), so that start falls due while nothing has
/// taken the launch's steps.
const STARVED: Duration = Duration::from_millis(1_200);

/// The rule, asserted on a read that arrives after NOTHING has run for a while — the state a
/// loaded machine leaves an emulator in, reproduced here rather than waited for.
///
/// The scheduler's replacement for the task it could not launch is placed by the clock, with
/// no call there to take its launch steps, and its start falls due one span later. Blocking
/// the one runtime thread is what keeps anything in-process from taking those steps in the
/// meantime, so the read that arrives next IS the pass that would start a task nothing has
/// decided the launch of — and it must report no running task all the same.
#[tokio::test]
async fn a_read_that_arrives_after_nothing_has_run_starts_no_undecided_launch() {
    let router = emulator();
    let row = ROWS[0];
    let case = row.case;
    let placed = set_up(&router, row).await;

    // The first task has failed its launch, so the scheduler has placed its replacement: the
    // launch this row is about is now one the CLOCK made.
    let deadline = Instant::now() + POLL_BUDGET;
    while stopped_tasks(&router, &placed).await.is_empty() {
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task whose role Amazon ECS may not assume to stop"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Nothing runs at all for longer than the replacement's start is owed. A blocking wait,
    // deliberately: an `.await` would hand the runtime back, which is exactly what this row
    // is about not happening.
    std::thread::sleep(STARVED);

    for reader in READERS {
        let Some(running) = running_as(&router, reader, &placed).await else {
            continue;
        };
        assert_eq!(
            running, 0,
            "case {case:?}: expected the first read after nothing ran to report no running \
             task, {reader:?} reported {running}"
        );
    }
}

/// The rule, asserted while SEVERAL callers read at once: the emulator serves concurrent
/// requests, so the pass that takes a launch's steps and the pass that would materialise its
/// start can be two different callers' — and the second of them must still not report a task
/// running whose launch the first has not finished deciding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_read_starts_a_task_while_another_pass_is_still_deciding_its_launch() {
    let router = Arc::new(emulator());
    // Only the rows whose role Amazon ECS may NOT assume: a task of one of those must never
    // be reported running, so any sample that reports one caught a start that was
    // materialised before the launch had been decided.
    let mut placements = Vec::new();
    for row in ROWS.into_iter().filter(|row| !row.runs) {
        placements.push(set_up(&router, row).await);
    }

    let mut readers = Vec::new();
    for worker in 0..CONCURRENT_READERS {
        let router = Arc::clone(&router);
        let placements = placements.clone();
        readers.push(tokio::spawn(async move {
            let deadline = Instant::now() + SAMPLE_BUDGET;
            // Each worker starts at a different reader, so the readers are spread over the
            // passes rather than all making the same call at the same instant.
            let mut sample = worker;
            while Instant::now() < deadline {
                let reader = READERS[sample % READERS.len()];
                sample += 1;
                for placed in &placements {
                    let case = placed.row.case;
                    let Some(running) = running_as(&router, reader, placed).await else {
                        continue;
                    };
                    assert_eq!(
                        running, 0,
                        "case {case:?}: expected no task of a service whose role Amazon ECS \
                         may not assume to be reported running, {reader:?} reported {running} \
                         on worker {worker}"
                    );
                }
                // Yield rather than sleep: the samples have to land inside the passes the
                // other workers and the clock are making, not between them.
                tokio::task::yield_now().await;
            }
        }));
    }
    for (worker, reader) in readers.into_iter().enumerate() {
        reader
            .await
            .unwrap_or_else(|error| panic!("reader {worker} failed: {error}"));
    }

    // The sampling was over the tasks the SCHEDULER placed, not only the one `CreateService`
    // did: by now each service has lost and replaced its task.
    for placed in &placements {
        let case = placed.row.case;
        let deadline = Instant::now() + POLL_BUDGET;
        loop {
            let stopped = stopped_tasks(&router, placed).await.len();
            if stopped >= REPLACED_AT_LEAST {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the scheduler to have replaced the task it cannot \
                 launch at least {} times over, got {stopped}",
                REPLACED_AT_LEAST - 1
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}
