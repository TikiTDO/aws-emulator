// SPDX-License-Identifier: CC0-1.0

//! A load-balanced Fargate service CONVERGES on the task count it was asked for.
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A service whose deployment controller is the ECS one ends up holding exactly
//! > `desiredCount` tasks in the `RUNNING` state — for ANY count the caller asked for, and
//! > whichever call asked for it — with `pendingCount` back to zero and the primary
//! > deployment's `rolloutState` moved from `IN_PROGRESS` to `COMPLETED`; each of those
//! > tasks is allocated its OWN elastic network interface, whose attachment reports the
//! > interface id, the MAC address, the subnet id and a private IPv4 address no other task
//! > of the service holds; and every one of them is a target of each group the service's
//! > `loadBalancers` bind, at that address on the binding's `containerPort`.
//!
//! `CreateService`'s own doc is the count half — it "runs and maintains your desired number
//! of tasks from a specified task definition", and `Service$runningCount` is "The number of
//! tasks in the cluster that are in the RUNNING state"; `Deployment$rolloutState` is the
//! rollout half ("When a service deployment is started, it begins in an IN_PROGRESS state.
//! When the service reaches a steady state, the deployment transitions to a COMPLETED
//! state"). The interface is `RegisterTaskDefinition$networkMode`'s ("If the network mode is
//! `awsvpc`, the task is allocated an elastic network interface") with the four values
//! `Attachment$details` lists "for elastic network interfaces ... the network interface ID,
//! the MAC address, the subnet ID, and the private IPv4 address", and the address is what a
//! target of such a service is reached at, since "tasks that use the `awsvpc` network mode
//! are associated with an elastic network interface, not an Amazon EC2 instance"
//! (`LoadBalancer$targetGroupArn`).
//!
//! The table spans the one thing the rule generalises over: HOW MANY tasks the service wants
//! — none, one, three, and one RAISED to two through `UpdateService`, so the other call that
//! sets the count reaches the same rule. A recording of a single `desiredCount=1` service is
//! one row of it.
//!
//! Nothing here reaches into a state struct: the target group, its load balancer and its
//! listener are created through the Elastic Load Balancing v2 API on the one shared engine,
//! the services through the ECS front door, and every value asserted is read back through
//! `DescribeServices`, `ListTasks`, `DescribeTasks` and `DescribeTargetHealth`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, AWSVPC_START, AWSVPC_SUBNET};
use crate::load_balancer_target_groups::{all_texts, create_target_group, elb_call, emulator};
use crate::task_lifecycle::container;

/// The cluster, the task-definition family and the container every row runs.
const CLUSTER: &str = "converge";
const CONTAINER: &str = "web";

/// The port the container declares in its `portMappings`, and the one every binding
/// attaches: a binding may only name a port the revision has.
const CONTAINER_PORT: i64 = 80;

/// How often a poll re-reads a surface it is waiting on, and how long it keeps trying: the
/// whole start of an `awsvpc` task plus the rollout's own steady state, with margin for a
/// loaded test machine.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_BUDGET: Duration = AWSVPC_START.saturating_add(Duration::from_secs(30));

/// One row: how many tasks the service is asked for, and the names its own resources take.
struct Row {
    case: &'static str,
    /// The `desiredCount` the service is created with.
    desired: i64,
    /// The count the service is RAISED to through `UpdateService` right after the create,
    /// for the row whose count the scheduler is given by that call instead — and `None` for
    /// a row that keeps the count it was created with.
    update_to: Option<i64>,
    /// The target group this row's service binds, and the service's own name.
    group_name: &'static str,
    service_name: &'static str,
}

impl Row {
    /// The count the scheduler is finally asked for: the update's where the row makes one,
    /// the create's otherwise.
    fn wanted(&self) -> i64 {
        self.update_to.unwrap_or(self.desired)
    }
}

const ROWS: [Row; 4] = [
    Row {
        case: "a service that wants no task at all",
        desired: 0,
        update_to: None,
        group_name: "converge-none",
        service_name: "none",
    },
    Row {
        case: "a service that wants one task",
        desired: 1,
        update_to: None,
        group_name: "converge-one",
        service_name: "one",
    },
    Row {
        case: "a service that wants three tasks",
        desired: 3,
        update_to: None,
        group_name: "converge-three",
        service_name: "three",
    },
    Row {
        case: "a service whose count UpdateService raises from one to two",
        desired: 1,
        update_to: Some(2),
        group_name: "converge-raised",
        service_name: "raised",
    },
];

/// Register the one revision every row's service runs: an `awsvpc`, Fargate-compatible
/// revision whose single container declares the port the bindings attach.
async fn register_definition(router: &MultiServiceRouter, case: &str) {
    let mut web = container(CONTAINER, "public.ecr.aws/nginx/nginx:latest");
    web["portMappings"] = json!([{ "containerPort": CONTAINER_PORT, "protocol": "tcp" }]);
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": CLUSTER,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "requiresCompatibilities": ["FARGATE"],
            "containerDefinitions": [web],
        }),
        case,
    )
    .await;
}

/// Create the service one row describes, bound to `group` on the container port the revision
/// declares, and assert what the create itself reports: the count the caller asked for, a
/// task set that is not up yet, and a rollout still under way.
async fn create_service(router: &MultiServiceRouter, row: &Row, group: &str) {
    let created = ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "taskDefinition": CLUSTER,
            "desiredCount": row.desired,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [AWSVPC_SUBNET],
                    "securityGroups": ["sg-0123456789abcdef0"],
                }
            },
            "loadBalancers": [{
                "targetGroupArn": group,
                "containerName": CONTAINER,
                "containerPort": CONTAINER_PORT,
            }],
            "healthCheckGracePeriodSeconds": 120,
        }),
        row.case,
    )
    .await;
    let service = &created["service"];
    assert_eq!(
        service["desiredCount"].as_i64(),
        Some(row.desired),
        "case {:?}: the create must keep the count the caller asked for, got {created}",
        row.case
    );
    assert_eq!(
        service["runningCount"].as_i64(),
        Some(0),
        "case {:?}: nothing is running at the instant the service is created, got {created}",
        row.case
    );
    // "When a service deployment is started, it begins in an IN_PROGRESS state": the steady
    // state is owed by the clock, so no create reports a rollout that is already over.
    assert_eq!(
        service["deployments"][0]["rolloutState"].as_str(),
        Some("IN_PROGRESS"),
        "case {:?}: the rollout a create starts is under way, got {created}",
        row.case
    );
    assert_eq!(
        service["deployments"][0]["status"].as_str(),
        Some("PRIMARY"),
        "case {:?}: the deployment a create starts is the primary one, got {created}",
        row.case
    );
}

/// Raise one row's count through `UpdateService`, asserting the stored count is the one that
/// call asked for.
async fn raise_count(router: &MultiServiceRouter, row: &Row, raised: i64) {
    let updated = ok(
        router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": row.service_name,
            "desiredCount": raised,
        }),
        row.case,
    )
    .await;
    assert_eq!(
        updated["service"]["desiredCount"].as_i64(),
        Some(raised),
        "case {:?}: the update must keep the count the caller asked for, got {updated}",
        row.case
    );
}

/// Wait until the service holds the task set it was asked for and its rollout has completed,
/// and answer the service as it is then reported.
async fn converged_service(router: &MultiServiceRouter, row: &Row) -> Value {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let described = ok(
            router,
            "DescribeServices",
            json!({ "cluster": CLUSTER, "services": [row.service_name] }),
            row.case,
        )
        .await;
        assert!(
            list(&described, "failures", row.case).is_empty(),
            "case {:?}: DescribeServices must report no failure, got {described}",
            row.case
        );
        let service = described["services"][0].clone();
        let running = service["runningCount"].as_i64();
        let pending = service["pendingCount"].as_i64();
        let rollout = service["deployments"][0]["rolloutState"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let converged =
            running == Some(row.wanted()) && pending == Some(0) && rollout == "COMPLETED";
        if converged || Instant::now() >= deadline {
            assert!(
                converged,
                "case {:?}: expected runningCount {} with pendingCount 0 and a COMPLETED \
                 rollout, got runningCount {running:?}/pendingCount {pending:?}/{rollout:?} \
                 in {service}",
                row.case,
                row.wanted()
            );
            return service;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The private IPv4 addresses of the RUNNING tasks of one row's service, sorted — with every
/// value an elastic network interface's attachment reports asserted on the way.
async fn running_task_addresses(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let case = row.case;
    // "The default status filter is RUNNING, which shows tasks that Amazon ECS has set the
    // desired status to RUNNING."
    let listed = ok(
        router,
        "ListTasks",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "desiredStatus": "RUNNING",
        }),
        case,
    )
    .await;
    let arns: Vec<Value> = list(&listed, "taskArns", case);
    assert_eq!(
        arns.len() as i64,
        row.wanted(),
        "case {case:?}: expected {} task ARN(s), got {listed}",
        row.wanted()
    );
    // Nothing of this service was asked to stop: a task whose launch failed would be listed
    // here instead of running.
    let stopped = ok(
        router,
        "ListTasks",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "desiredStatus": "STOPPED",
        }),
        case,
    )
    .await;
    assert!(
        list(&stopped, "taskArns", case).is_empty(),
        "case {case:?}: no task of a converged service is on its way out, got {stopped}"
    );
    if arns.is_empty() {
        return Vec::new();
    }

    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    let tasks = list(&described, "tasks", case);
    assert_eq!(
        tasks.len() as i64,
        row.wanted(),
        "case {case:?}: every listed task must be describable, got {described}"
    );
    let mut addresses = Vec::new();
    for task in &tasks {
        assert_eq!(
            task["lastStatus"].as_str(),
            Some("RUNNING"),
            "case {case:?}: a task counted as running reports RUNNING, got {task}"
        );
        assert_eq!(
            task["desiredStatus"].as_str(),
            Some("RUNNING"),
            "case {case:?}: a running task is wanted running, got {task}"
        );
        assert_eq!(
            task["launchType"].as_str(),
            Some("FARGATE"),
            "case {case:?}: the task runs the launch type the service was created with, got \
             {task}"
        );
        // "The Elastic Network Adapter that's associated with the task if the task uses the
        // awsvpc network mode" — exactly one, in place, carrying all four details.
        let attachments = list(task, "attachments", case);
        assert_eq!(
            attachments.len(),
            1,
            "case {case:?}: an awsvpc task holds its one interface, got {task}"
        );
        let attachment = &attachments[0];
        assert_eq!(
            attachment["type"].as_str(),
            Some("ElasticNetworkInterface"),
            "case {case:?}: the attachment is the task's interface, got {attachment}"
        );
        assert_eq!(
            attachment["status"].as_str(),
            Some("ATTACHED"),
            "case {case:?}: a task does not run before its interface is attached, got \
             {attachment}"
        );
        let detail = |name: &str| -> Option<String> {
            list(attachment, "details", case)
                .into_iter()
                .find(|entry| entry["name"].as_str() == Some(name))
                .and_then(|entry| entry["value"].as_str().map(str::to_string))
        };
        for name in ["networkInterfaceId", "macAddress", "privateIPv4Address"] {
            assert!(
                detail(name).is_some_and(|value| !value.is_empty()),
                "case {case:?}: the interface reports its {name}, got {attachment}"
            );
        }
        assert_eq!(
            detail("subnetId").as_deref(),
            Some(AWSVPC_SUBNET),
            "case {case:?}: the interface sits in the subnet the placement named, got \
             {attachment}"
        );
        addresses.push(detail("privateIPv4Address").unwrap_or_default());
    }
    let distinct: BTreeSet<&String> = addresses.iter().collect();
    assert_eq!(
        distinct.len(),
        addresses.len(),
        "case {case:?}: no two tasks share an interface address, got {addresses:?}"
    );
    addresses.sort();
    addresses
}

/// `(target id, target port)` for every registration `group` reports, sorted.
///
/// One `TargetHealthDescription` carries exactly one `Target.Id` and one `Target.Port` — the
/// group's own `HealthCheckPort` is a differently named element — so the two leaf sequences
/// pair up, which the length check asserts rather than assumes.
async fn registered_targets(
    router: &MultiServiceRouter,
    group: &str,
    case: &str,
) -> Vec<(String, String)> {
    let (status, body) =
        elb_call(router, "DescribeTargetHealth", &[("TargetGroupArn", group)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTargetHealth must succeed, got {status} with {body}"
    );
    let ids = all_texts(&body, "Id");
    let ports = all_texts(&body, "Port");
    assert_eq!(
        ids.len(),
        ports.len(),
        "case {case:?}: expected one port per described target, got {ids:?} and {ports:?} \
         from {body}"
    );
    let mut targets: Vec<(String, String)> = ids.into_iter().zip(ports).collect();
    targets.sort();
    targets
}

/// Wait until `group` holds exactly `addresses` on the binding's container port, and answer
/// what it holds.
///
/// The registration is the clock's work too — Amazon ECS registers a task once it is running
/// — so a row looks until the set settles rather than once.
async fn registered_targets_reaching(
    router: &MultiServiceRouter,
    group: &str,
    case: &str,
    addresses: &[String],
) -> Vec<(String, String)> {
    let wanted: Vec<(String, String)> = addresses
        .iter()
        .map(|address| (address.clone(), CONTAINER_PORT.to_string()))
        .collect();
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let held = registered_targets(router, group, case).await;
        if held == wanted || Instant::now() >= deadline {
            assert_eq!(
                held, wanted,
                "case {case:?}: expected the group to hold one target per running task, got \
                 {held:?}"
            );
            return held;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_fargate_service_runs_the_count_it_was_asked_for_and_registers_every_task() {
    let router = emulator();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        "the cluster every row's service runs in",
    )
    .await;
    register_definition(&router, "the revision every row's service runs").await;

    // Every row's service is created up front, so the clock-owed work of all of them falls
    // due at once rather than one row's budget after another's.
    let mut groups: Vec<String> = Vec::new();
    for row in &ROWS {
        let group = create_target_group(&router, row.group_name, "ip", row.case).await;
        create_service(&router, row, &group).await;
        if let Some(raised) = row.update_to {
            raise_count(&router, row, raised).await;
        }
        groups.push(group);
    }

    for (group, row) in groups.iter().zip(&ROWS) {
        let service = converged_service(&router, row).await;
        // The primary deployment's own counts are the service's: the rollout that completed
        // is the one holding those tasks.
        let deployment = &service["deployments"][0];
        assert_eq!(
            deployment["runningCount"].as_i64(),
            Some(row.wanted()),
            "case {:?}: the completed rollout holds the tasks the service runs, got \
             {deployment}",
            row.case
        );
        assert_eq!(
            deployment["pendingCount"].as_i64(),
            Some(0),
            "case {:?}: a completed rollout has nothing still coming up, got {deployment}",
            row.case
        );

        let addresses = running_task_addresses(&router, row).await;
        registered_targets_reaching(&router, group, row.case, &addresses).await;
    }
}
