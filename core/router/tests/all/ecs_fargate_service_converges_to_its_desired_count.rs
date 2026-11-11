// SPDX-License-Identifier: CC0-1.0

//! Where a load-balanced Fargate ECS service's convergence on its `desiredCount` STARTS:
//! the instant the create answers, across the three services the rollout spans.
//!
//! RULE: a service whose deployment controller is the ECS one converges on holding exactly
//! `desiredCount` tasks in the `RUNNING` state, and that convergence is owed by the CLOCK
//! rather than done by the call — so at the instant `CreateService` answers, the scheduler
//! has PLACED the task set it was asked for and none of it is up yet:
//! `Service$pendingCount` ("the number of tasks in the cluster that are in the PENDING
//! state", which covers a task Amazon ECS still has a provisioning step to take for) is the
//! count the caller asked for, `runningCount` is zero, the primary `Deployment$rolloutState`
//! is `IN_PROGRESS` ("When a service deployment is started, it begins in an IN_PROGRESS
//! state"), each placed task already carries the elastic network interface its `awsvpc`
//! revision is "allocated" with the four values `Attachment$details` lists for one — "the
//! network interface ID, the MAC address, the subnet ID, and the private IPv4 address" — in
//! an attachment that is NOT attached yet, and the target group the service's
//! `loadBalancers` bind holds no target at all, because Amazon ECS registers a task once it
//! is running.
//!
//! Three services have to agree for that one instant to be reported correctly, which is why
//! this suite lives here: ECS places the task set, the interface is created in a subnet of
//! the account's own Amazon EC2 records, and the group that must still be empty is Elastic
//! Load Balancing v2's. Everything is driven through the real operations on one shared
//! engine, so what is asserted is what each service persisted.
//!
//! The rows differ along what the rule generalises over: how many tasks the service is asked
//! for — none, one (the count a recording shows), and three.
//!
//! WHERE the convergence ARRIVES — `runningCount` reaching the count asked for with a
//! `COMPLETED` rollout, the attached interface, and one target per running task — is pinned
//! by the owning crate's own suite, which can mount both services:
//! `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/tests/all/service_desired_count_convergence.rs`.
//! This suite deliberately waits for no clock-owed transition, so the shared router test
//! binary keeps its wall-clock cost.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{form_encode, members_of, opt_element};

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The API version an awsQuery request naming the ELBv2 model carries.
const ELB_VERSION: &str = "2015-12-01";

/// The ECS cluster, task-definition family and container every row runs.
const CLUSTER: &str = "converge";
const FAMILY: &str = "convergefamily";
const CONTAINER: &str = "app";

/// The port the container declares in its `portMappings`, and the one every binding
/// attaches: a binding may only name a port the revision has.
const CONTAINER_PORT: i64 = 80;

/// The subnets the tasks and the load balancer are placed in, and the VPC holding them.
const SUBNET: &str = "subnet-0123456789abcdef0";
const SECOND_SUBNET: &str = "subnet-0123456789abcdef1";
const VPC: &str = "vpc-0123456789abcdef0";

/// The `Attachment$status` values Amazon ECS takes an interface it is creating and attaching
/// through, none of which is the `ATTACHED` a task that is RUNNING has reached (from
/// `Attachment$status`' own list: "PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING,
/// DETACHED, DELETED, and FAILED").
const ATTACHING_STATUSES: [&str; 3] = ["PRECREATED", "CREATED", "ATTACHING"];

/// The `lastStatus` values a task that has been placed but has not started is in:
/// `PROVISIONING` while Amazon ECS prepares what it needs (here, the interface it is
/// allocated) and `PENDING` once it has been accepted onto its capacity — the two statuses
/// `Service$pendingCount` counts.
const COMING_UP_STATUSES: [&str; 2] = ["PROVISIONING", "PENDING"];

/// One row: how many tasks the service is asked for, and the names its own resources take.
struct Row {
    case: &'static str,
    /// The `desiredCount` the service is created with.
    desired: i64,
    /// The target group this row's service binds, the listener port that attaches it, and
    /// the service's own name.
    group_name: &'static str,
    listener_port: &'static str,
    service_name: &'static str,
}

const ROWS: [Row; 3] = [
    Row {
        case: "a service that wants no task at all",
        desired: 0,
        group_name: "converge-none",
        listener_port: "80",
        service_name: "none",
    },
    Row {
        case: "the recorded count: a service that wants one task",
        desired: 1,
        group_name: "converge-one",
        listener_port: "8080",
        service_name: "one",
    },
    Row {
        case: "a service that wants three tasks",
        desired: 3,
        group_name: "converge-three",
        listener_port: "8090",
        service_name: "three",
    },
];

/// One emulator carrying Elastic Load Balancing v2 and ECS over ONE shared engine, exactly as
/// the production router builds them, with the network the placements name held in the
/// account's EC2 store — which both services resolve a subnet against.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::ec2_network::register_network(
        &engine,
        VPC,
        &[SUBNET, SECOND_SUBNET],
        &format!("{REGION}a"),
    );
    // The placements below are given a public address on a subnet that routes to an internet
    // gateway, which is the path ECS reads off these records for the image their containers
    // run — so no task of these services is stopped before it starts.
    crate::ec2_network::register_internet_route(&engine, VPC);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to the ELBv2 model, answered with its XML body.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={ELB_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("elasticloadbalancingv2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action}: expected Elastic Load Balancing to answer 200, got {status} with {body}"
    );
    body
}

/// One awsJson1_1 call to ECS that must succeed, answered with the parsed body.
async fn ecs(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{REGION}\
                 /ecs/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    assert_eq!(
        status, 200,
        "case {operation}: expected ECS to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// Create the `ip` target group one row describes, attach it to `load_balancer` with a
/// listener forwarding to it, and answer its ARN.
async fn target_group(router: &MultiServiceRouter, row: &Row, load_balancer: &str) -> String {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", row.group_name),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("TargetType", "ip"),
            ("VpcId", VPC),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", "/"),
        ],
    )
    .await;
    let arn = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("case {:?}: no TargetGroupArn in {created}", row.case));
    // A target group no listener forwards to cannot be bound to a service at all.
    elbv2(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", load_balancer),
            ("Protocol", "HTTP"),
            ("Port", row.listener_port),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", arn.as_str()),
        ],
    )
    .await;
    arn
}

/// How many `TargetHealthDescription`s the group reports.
async fn registered_target_count(router: &MultiServiceRouter, group: &str) -> usize {
    let body = elbv2(router, "DescribeTargetHealth", &[("TargetGroupArn", group)]).await;
    members_of(&body).len()
}

/// The tasks `ListTasks` reports for one row's service, described through `DescribeTasks`.
///
/// Both calls are the ones a client polls with, so what is asserted below is what a client
/// reading straight after the create sees.
async fn placed_tasks(router: &MultiServiceRouter, row: &Row) -> Vec<Value> {
    let listed = ecs(
        router,
        "ListTasks",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service_name,
            "desiredStatus": "RUNNING",
        }),
    )
    .await;
    let arns: Vec<&str> = listed["taskArns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        arns.len() as i64,
        row.desired,
        "case {:?}: the scheduler must have placed the task set the service was asked for, \
         got {listed}",
        row.case
    );
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ecs(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
    )
    .await;
    described["tasks"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {:?}: DescribeTasks answers its tasks", row.case))
}

#[tokio::test]
async fn a_load_balanced_fargate_service_places_its_task_set_with_nothing_running_yet() {
    let router = emulator();
    ecs(&router, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    ecs(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": CONTAINER,
                "image": "public.ecr.aws/nginx/nginx:latest",
                "essential": true,
                "portMappings": [{ "containerPort": CONTAINER_PORT, "protocol": "tcp" }],
            }],
        }),
    )
    .await;
    let load_balancer = opt_element(
        &elbv2(
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "converge-alb"),
                ("Scheme", "internet-facing"),
                ("Subnets.member.1", SUBNET),
                ("Subnets.member.2", SECOND_SUBNET),
            ],
        )
        .await,
        "LoadBalancerArn",
    )
    .expect("the create answers the load balancer's ARN");

    for row in &ROWS {
        let case = row.case;
        let group = target_group(&router, row, &load_balancer).await;
        let created = ecs(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": row.service_name,
                "taskDefinition": FAMILY,
                "desiredCount": row.desired,
                "launchType": "FARGATE",
                "networkConfiguration": {
                    "awsvpcConfiguration": {
                        "subnets": [SUBNET, SECOND_SUBNET],
                        "securityGroups": ["sg-0123456789abcdef0"],
                        // The task's own public address, which is what makes the internet
                        // gateway its subnet routes through a path to the image's registry.
                        "assignPublicIp": "ENABLED",
                    }
                },
                "loadBalancers": [{
                    "targetGroupArn": group,
                    "containerName": CONTAINER,
                    "containerPort": CONTAINER_PORT,
                }],
                "healthCheckGracePeriodSeconds": 120,
            }),
        )
        .await;

        // What the create itself answers: the count the caller asked for, placed and on its
        // way up, with a rollout that has not reached its steady state.
        let service = &created["service"];
        assert_eq!(
            service["desiredCount"].as_i64(),
            Some(row.desired),
            "case {case:?}: the create must keep the count the caller asked for, got {created}"
        );
        assert_eq!(
            service["runningCount"].as_i64(),
            Some(0),
            "case {case:?}: nothing is running at the instant the service is created, got \
             {created}"
        );
        assert_eq!(
            service["pendingCount"].as_i64(),
            Some(row.desired),
            "case {case:?}: the scheduler places the task set the create asked for, and \
             every task of it is on its way up, got {created}"
        );
        assert_eq!(
            service["deployments"][0]["status"].as_str(),
            Some("PRIMARY"),
            "case {case:?}: the deployment a create starts is the primary one, got {created}"
        );
        assert_eq!(
            service["deployments"][0]["rolloutState"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: the rollout a create starts is under way, got {created}"
        );
        assert_eq!(
            service["deployments"][0]["pendingCount"].as_i64(),
            Some(row.desired),
            "case {case:?}: the primary deployment holds the tasks it placed, got {created}"
        );

        // The tasks it placed, as a client polling straight after the create reads them.
        let tasks = placed_tasks(&router, row).await;
        let mut addresses = Vec::new();
        for task in &tasks {
            let last_status = task["lastStatus"].as_str().unwrap_or_default();
            assert!(
                COMING_UP_STATUSES.contains(&last_status),
                "case {case:?}: a task the create placed is still coming up, got \
                 {last_status:?} in {task}"
            );
            assert_eq!(
                task["startedAt"].as_str(),
                None,
                "case {case:?}: a task that has not started carries no startedAt, got {task}"
            );
            // "If the network mode is awsvpc, the task is allocated an elastic network
            // interface": the interface exists from the moment the task is placed, carrying
            // its own values, and the ATTACHMENT is what the clock still has to move.
            let attachments: Vec<&Value> =
                task["attachments"].as_array().into_iter().flatten().collect();
            assert_eq!(
                attachments.len(),
                1,
                "case {case:?}: an awsvpc task holds its one interface, got {task}"
            );
            let attachment = attachments[0];
            assert_eq!(
                attachment["type"].as_str(),
                Some("ElasticNetworkInterface"),
                "case {case:?}: the attachment is the task's interface, got {attachment}"
            );
            let status = attachment["status"].as_str().unwrap_or_default();
            assert!(
                ATTACHING_STATUSES.contains(&status),
                "case {case:?}: the interface of a task that has not started is not attached \
                 yet, got {status:?} in {attachment}"
            );
            let detail = |name: &str| -> Option<String> {
                attachment["details"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|entry| entry["name"].as_str() == Some(name))
                    .and_then(|entry| entry["value"].as_str())
                    .map(str::to_string)
            };
            // `Attachment$details`: "For elastic network interfaces, this includes the
            // network interface ID, the MAC address, the subnet ID, and the private IPv4
            // address."
            for name in ["networkInterfaceId", "macAddress", "privateIPv4Address"] {
                assert!(
                    detail(name).is_some_and(|value| !value.is_empty()),
                    "case {case:?}: the interface reports its {name}, got {attachment}"
                );
            }
            let subnet = detail("subnetId").unwrap_or_default();
            assert!(
                subnet == SUBNET || subnet == SECOND_SUBNET,
                "case {case:?}: the interface is created in a subnet the placement named, \
                 got {subnet:?}"
            );
            addresses.push(detail("privateIPv4Address").unwrap_or_default());
        }
        let distinct: BTreeSet<&String> = addresses.iter().collect();
        assert_eq!(
            distinct.len(),
            addresses.len(),
            "case {case:?}: each task is allocated its own interface, so no two share an \
             address, got {addresses:?}"
        );

        // And nothing is a target of the bound group yet: Amazon ECS registers a task once
        // it is running, which none of these is.
        let held = registered_target_count(&router, &group).await;
        assert_eq!(
            held, 0,
            "case {case:?}: a task that is not running is a target of nothing, got {held} \
             target description(s)"
        );
    }
}
