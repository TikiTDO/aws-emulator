// SPDX-License-Identifier: CC0-1.0

//! The capacity an Amazon EC2 instance LAUNCHED INTO A CLUSTER gives that cluster, driven
//! end to end over the shared wire harness in [`crate::ecs_wire`] with the Amazon EC2
//! service mounted beside this one on the same engine — so every instance is launched
//! through EC2's OWN API rather than written into a store by hand, and every value asserted
//! is read back through `ListContainerInstances`, `DescribeContainerInstances`,
//! `DescribeClusters`, `DescribeServices`, `ListTasks` and `DescribeTasks`.
//!
//! # The rule under test
//!
//! > A RUNNING Amazon EC2 instance of the account whose user data configures the container
//! > agent with a cluster of the account is a registered container instance of that cluster
//! > — `ACTIVE`, with its agent connected, reporting its own `ec2InstanceId`, the resources
//! > its instance type offers and the built-in attributes of the machine it runs on — and it
//! > stops being that cluster's capacity when the instance is gone.
//!
//! `RegisterContainerInstance` is the registration itself ("this action is only used by the
//! Amazon ECS agent, and it is not intended for use outside of the agent" — it "registers an
//! EC2 instance into the specified cluster. This instance becomes available to place
//! containers on"), and what makes it capacity is `Cluster$registeredContainerInstancesCount`
//! ("the number of container instances registered into the cluster") plus the launch type
//! that uses it: "the EC2 launch type runs your tasks on Amazon EC2 instances registered to
//! your cluster". The agent runs ON the instance, so the cluster it joins is the one the
//! launch put in the instance's user data (the agent's `ECS_CLUSTER` variable); the
//! deregistration a vanished instance earns is the `INACTIVE` status
//! `DeregisterContainerInstance` documents ("the instance then transitions to an INACTIVE
//! status"), which `ListContainerInstances` excludes by default.
//!
//! # What the rows span
//!
//! The table differs along what the rule generalises over: the FORM the agent's
//! configuration takes in the user data (the documented shell line, base64 as a launch
//! carries it; a bare assignment in plain text; a quoted value), the CLUSTER it names (two
//! different clusters of the account, and one the account does not hold), the INSTANCE TYPE
//! the capacity comes from (two sizes, which is what the reported resources are read off),
//! and user data that configures no cluster at all — including none at all.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    count, create_cluster, ec2, ec2_element, ec2_form, emulator_with_ec2, list, ok, string,
    SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// The cluster the rows that join one mostly join.
const CLUSTER: &str = "capacityfleet";

/// A SECOND cluster of the account, so a row proves the instance joins the cluster its own
/// user data names rather than whichever cluster exists.
const OTHER_CLUSTER: &str = "capacityspare";

/// A cluster name no `CreateCluster` of this suite creates: an agent configured for it has
/// nowhere to register.
const ABSENT_CLUSTER: &str = "capacityabsent";

/// The AMI every launch below boots. Well-formed (`ami-<17 hexadecimal digits>`), which is
/// all a launch needs; which AMI carries the container agent is not a fact this emulator
/// publishes, so it is not what the registration is keyed on.
const IMAGE_ID: &str = "ami-0123456789abcdef0";

/// The two instance types the rows launch: the resources a registration reports are read off
/// the type, so a small one and a large one are what makes that readable.
const SMALL_TYPE: &str = "t3.micro";
const LARGE_TYPE: &str = "m5.4xlarge";

/// How often a poll re-reads a surface it is waiting on, and how long it keeps trying: the
/// agent's registration span plus a task's own start, with generous margin for a loaded test
/// machine.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// `ContainerInstance$status` of a registered instance.
const ACTIVE: &str = "ACTIVE";

/// `ContainerInstance$status` of a deregistered one.
const INACTIVE: &str = "INACTIVE";

/// The built-in attributes a registration reports that these rows read back.
const ATTRIBUTE_INSTANCE_TYPE: &str = "ecs.instance-type";
const ATTRIBUTE_AVAILABILITY_ZONE: &str = "ecs.availability-zone";

/// The resources a registration reports.
const RESOURCE_CPU: &str = "CPU";
const RESOURCE_MEMORY: &str = "MEMORY";
const RESOURCE_PORTS: &str = "PORTS";

/// The user data form one row launches with.
#[derive(Debug, Clone, Copy)]
enum Configuration {
    /// The documented shell line that appends the assignment to the agent's configuration
    /// file, base64 as a launch carries it.
    ShellLine(&'static str),
    /// The same line with the assignment QUOTED.
    QuotedShellLine(&'static str),
    /// A bare `ECS_CLUSTER=<cluster>` in plain text, which is what the agent's configuration
    /// file itself holds.
    PlainAssignment(&'static str),
    /// User data that configures no cluster.
    NoCluster,
    /// No user data at all.
    None,
}

impl Configuration {
    /// The `UserData` the launch carries for this form, or `None` for a launch that carries
    /// none.
    fn user_data(self) -> Option<String> {
        use ::base64::Engine as _;

        let text = match self {
            Self::ShellLine(cluster) => format!(
                "#!/bin/bash\necho ECS_CLUSTER={cluster} >> /etc/ecs/ecs.config\n"
            ),
            Self::QuotedShellLine(cluster) => format!(
                "#!/bin/bash\necho \"ECS_CLUSTER={cluster}\" >> /etc/ecs/ecs.config\n"
            ),
            // Plain text, which is what an instance whose launch did not encode its user
            // data carries.
            Self::PlainAssignment(cluster) => return Some(format!("ECS_CLUSTER={cluster}")),
            Self::NoCluster => "#!/bin/bash\nyum update -y\n".to_string(),
            Self::None => return None,
        };
        Some(::base64::engine::general_purpose::STANDARD.encode(text))
    }
}

/// One row: how the launch configures the agent, what it launches, and the cluster the
/// instance must become capacity of (empty for a row that becomes capacity of none).
struct Row {
    case: &'static str,
    configuration: Configuration,
    instance_type: &'static str,
    joins: &'static str,
}

const ROWS: [Row; 6] = [
    Row {
        case: "the documented shell line, base64 as a launch carries it",
        configuration: Configuration::ShellLine(CLUSTER),
        instance_type: SMALL_TYPE,
        joins: CLUSTER,
    },
    Row {
        case: "a bare assignment in plain text, on a larger instance type",
        configuration: Configuration::PlainAssignment(CLUSTER),
        instance_type: LARGE_TYPE,
        joins: CLUSTER,
    },
    Row {
        case: "a quoted value naming the account's other cluster",
        configuration: Configuration::QuotedShellLine(OTHER_CLUSTER),
        instance_type: SMALL_TYPE,
        joins: OTHER_CLUSTER,
    },
    Row {
        case: "a cluster the account does not hold",
        configuration: Configuration::ShellLine(ABSENT_CLUSTER),
        instance_type: SMALL_TYPE,
        joins: "",
    },
    Row {
        case: "user data that configures no cluster",
        configuration: Configuration::NoCluster,
        instance_type: SMALL_TYPE,
        joins: "",
    },
    Row {
        case: "no user data at all",
        configuration: Configuration::None,
        instance_type: SMALL_TYPE,
        joins: "",
    },
];

/// The zone every instance below is launched into, which is the zone its subnet is in.
fn zone() -> String {
    format!("{SIGNED_REGION}a")
}

/// Create the one VPC and subnet the launches below are placed in, through Amazon EC2's own
/// API, and answer the subnet's id.
async fn network(router: &MultiServiceRouter, case: &str) -> String {
    let vpc = ec2_element(
        &ec2_form(router, "CreateVpc", &[("CidrBlock", "10.80.0.0/16")], case).await,
        "vpcId",
        case,
    );
    let subnet = ec2_form(
        router,
        "CreateSubnet",
        &[
            ("VpcId", vpc.as_str()),
            ("CidrBlock", "10.80.0.0/20"),
            ("AvailabilityZone", zone().as_str()),
        ],
        case,
    )
    .await;
    ec2_element(&subnet, "subnetId", case)
}

/// Launch ONE instance of `row` into `subnet` and answer the instance id Amazon EC2 minted.
async fn launch(router: &MultiServiceRouter, row: &Row, subnet: &str) -> String {
    let user_data = row.configuration.user_data();
    let mut params: Vec<(&str, &str)> = vec![
        ("ImageId", IMAGE_ID),
        ("InstanceType", row.instance_type),
        ("SubnetId", subnet),
        ("MinCount", "1"),
        ("MaxCount", "1"),
    ];
    if let Some(user_data) = user_data.as_deref() {
        params.push(("UserData", user_data));
    }
    let launched = ec2_form(router, "RunInstances", &params, row.case).await;
    ec2_element(&launched, "instanceId", row.case)
}

/// The container-instance ARNs `ListContainerInstances` reports for `cluster` — the default
/// filter, which "includes container instances set to all states other than INACTIVE".
async fn listed(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<String> {
    let body = ok(
        router,
        "ListContainerInstances",
        json!({ "cluster": cluster }),
        case,
    )
    .await;
    list(&body, "containerInstanceArns", case)
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// Every container instance of `cluster`, described through `DescribeContainerInstances`.
async fn described(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<Value> {
    let arns = listed(router, cluster, case).await;
    if arns.is_empty() {
        return Vec::new();
    }
    let body = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": cluster, "containerInstances": arns }),
        case,
    )
    .await;
    list(&body, "containerInstances", case)
}

/// The `ec2InstanceId`s `cluster` reports capacity for, in a set so the comparison does not
/// depend on the order the listing walked.
async fn capacity_of(router: &MultiServiceRouter, cluster: &str, case: &str) -> BTreeSet<String> {
    described(router, cluster, case)
        .await
        .iter()
        .filter_map(|instance| instance["ec2InstanceId"].as_str().map(str::to_string))
        .collect()
}

/// Wait until `cluster` reports capacity for exactly `wanted`, answering what it last read.
async fn settled_capacity(
    router: &MultiServiceRouter,
    cluster: &str,
    wanted: &BTreeSet<String>,
    case: &str,
) -> BTreeSet<String> {
    let deadline = Instant::now() + POLL_BUDGET;
    let mut last = capacity_of(router, cluster, case).await;
    while &last != wanted && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        last = capacity_of(router, cluster, case).await;
    }
    last
}

/// The `registeredContainerInstancesCount` `DescribeClusters` reports for `cluster`.
async fn registered_count(router: &MultiServiceRouter, cluster: &str, case: &str) -> i64 {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster] }),
        case,
    )
    .await;
    let clusters = list(&body, "clusters", case);
    let cluster = clusters
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the cluster back, got {body}"));
    count(cluster, "registeredContainerInstancesCount", case)
}

/// The integer a named resource of a registration reports, or `None` when it reports no such
/// resource.
fn integer_resource(instance: &Value, name: &str) -> Option<i64> {
    instance["registeredResources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|resource| resource["name"].as_str() == Some(name))
        .and_then(|resource| resource["integerValue"].as_i64())
}

/// The value a named attribute of a registration reports.
fn attribute_value(instance: &Value, name: &str) -> Option<String> {
    instance["attributes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|attribute| attribute["name"].as_str() == Some(name))
        .and_then(|attribute| attribute["value"].as_str())
        .map(str::to_string)
}

#[tokio::test]
async fn an_instance_launched_into_a_cluster_becomes_that_cluster_s_registered_capacity() {
    let case = "launched capacity";
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, case).await;
    create_cluster(&router, OTHER_CLUSTER, case).await;
    let subnet = network(&router, case).await;

    // Every row is launched first and the clusters are polled afterwards: each registration
    // falls due on its own clock, so the whole table costs one span rather than one per row.
    let mut launched: Vec<(&Row, String)> = Vec::new();
    for row in &ROWS {
        let instance_id = launch(&router, row, &subnet).await;
        launched.push((row, instance_id));
    }

    let expected = |cluster: &str| -> BTreeSet<String> {
        launched
            .iter()
            .filter(|(row, _)| row.joins == cluster)
            .map(|(_, instance_id)| instance_id.clone())
            .collect()
    };
    for cluster in [CLUSTER, OTHER_CLUSTER] {
        let wanted = expected(cluster);
        let held = settled_capacity(&router, cluster, &wanted, case).await;
        assert_eq!(
            held, wanted,
            "case {case:?}: expected {cluster} to report capacity for exactly the instances \
             launched into it, {wanted:?}, got {held:?}"
        );
        let counted = registered_count(&router, cluster, case).await;
        assert_eq!(
            counted,
            wanted.len() as i64,
            "case {case:?}: expected {cluster} to count {} registered container instance(s), \
             got {counted}",
            wanted.len()
        );
    }

    // No row that configures no cluster of the account is capacity of either cluster: the
    // registration is the agent's, and an agent configured for nothing registers nowhere.
    let all_capacity: BTreeSet<String> = capacity_of(&router, CLUSTER, case)
        .await
        .into_iter()
        .chain(capacity_of(&router, OTHER_CLUSTER, case).await)
        .collect();
    for (row, instance_id) in &launched {
        let is_capacity = all_capacity.contains(instance_id);
        assert_eq!(
            is_capacity,
            !row.joins.is_empty(),
            "case {:?}: expected the instance {instance_id} to be capacity of a cluster: \
             {}, got {is_capacity} with {all_capacity:?}",
            row.case,
            !row.joins.is_empty()
        );
    }

    // What each registration REPORTS, read back through the describe.
    let mut cpu_by_type: Vec<(&str, i64)> = Vec::new();
    let mut memory_by_type: Vec<(&str, i64)> = Vec::new();
    for cluster in [CLUSTER, OTHER_CLUSTER] {
        for instance in described(&router, cluster, case).await {
            let instance_id = string(&instance, "ec2InstanceId", case);
            let row = launched
                .iter()
                .find(|(_, launched_id)| *launched_id == instance_id)
                .map(|(row, _)| *row)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {cluster} reports capacity nothing launched: {instance}")
                });
            let row_case = row.case;
            assert_eq!(
                instance["status"].as_str(),
                Some(ACTIVE),
                "case {row_case:?}: expected a registered instance to be {ACTIVE}, got {instance}"
            );
            assert_eq!(
                instance["agentConnected"].as_bool(),
                Some(true),
                "case {row_case:?}: expected the agent that registered to be connected, got \
                 {instance}"
            );
            assert!(
                instance["registeredAt"].is_number(),
                "case {row_case:?}: expected the instant the instance registered, got {instance}"
            );
            // The resources are the instance TYPE's, so each is a positive number and the
            // ports the agent holds are a non-empty set.
            let cpu = integer_resource(&instance, RESOURCE_CPU).unwrap_or_default();
            let memory = integer_resource(&instance, RESOURCE_MEMORY).unwrap_or_default();
            assert!(
                cpu > 0,
                "case {row_case:?}: expected the registration to report the instance's {RESOURCE_CPU}, \
                 got {instance}"
            );
            assert!(
                memory > 0,
                "case {row_case:?}: expected the registration to report the instance's \
                 {RESOURCE_MEMORY}, got {instance}"
            );
            let ports = instance["registeredResources"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|resource| resource["name"].as_str() == Some(RESOURCE_PORTS))
                .and_then(|resource| resource["stringSetValue"].as_array().cloned())
                .unwrap_or_default();
            assert!(
                !ports.is_empty(),
                "case {row_case:?}: expected the ports the agent holds, got {instance}"
            );
            cpu_by_type.push((row.instance_type, cpu));
            memory_by_type.push((row.instance_type, memory));
            // "remainingResources": what is not already allocated to tasks, which on an
            // instance running none is everything it registered.
            assert_eq!(
                instance["remainingResources"], instance["registeredResources"],
                "case {row_case:?}: expected an instance running no task to have all it \
                 registered remaining, got {instance}"
            );
            assert_eq!(
                attribute_value(&instance, ATTRIBUTE_INSTANCE_TYPE).as_deref(),
                Some(row.instance_type),
                "case {row_case:?}: expected the built-in {ATTRIBUTE_INSTANCE_TYPE} attribute to \
                 name the type the instance was launched as, got {instance}"
            );
            assert_eq!(
                attribute_value(&instance, ATTRIBUTE_AVAILABILITY_ZONE),
                Some(zone()),
                "case {row_case:?}: expected the built-in {ATTRIBUTE_AVAILABILITY_ZONE} attribute \
                 to name the zone the instance runs in, got {instance}"
            );
        }
    }

    // The resources come from the instance TYPE: the larger type offers strictly more of
    // both than the smaller one.
    let largest = |values: &[(&str, i64)], instance_type: &str| -> i64 {
        values
            .iter()
            .filter(|(reported_type, _)| *reported_type == instance_type)
            .map(|(_, value)| *value)
            .max()
            .unwrap_or_default()
    };
    assert!(
        largest(&cpu_by_type, LARGE_TYPE) > largest(&cpu_by_type, SMALL_TYPE),
        "case {case:?}: expected {LARGE_TYPE} to register more {RESOURCE_CPU} than {SMALL_TYPE}, \
         got {cpu_by_type:?}"
    );
    assert!(
        largest(&memory_by_type, LARGE_TYPE) > largest(&memory_by_type, SMALL_TYPE),
        "case {case:?}: expected {LARGE_TYPE} to register more {RESOURCE_MEMORY} than \
         {SMALL_TYPE}, got {memory_by_type:?}"
    );

    // And the other end of the rule: an instance that is GONE is no longer capacity. The
    // registration it made is deregistered, which is the INACTIVE status the listing's
    // default filter excludes.
    let (_, terminated_id) = launched
        .iter()
        .find(|(row, _)| row.joins == CLUSTER)
        .expect("one row joins the cluster");
    // The ARN of that instance's registration, read while it is still listed: `INACTIVE` is
    // not a value `ListContainerInstances$status` takes ("the default is to include container
    // instances set to all states other than INACTIVE"), so a deregistered instance is
    // reached by the ARN it was registered under.
    let terminated_arn = described(&router, CLUSTER, case)
        .await
        .into_iter()
        .find(|instance| instance["ec2InstanceId"].as_str() == Some(terminated_id.as_str()))
        .map(|instance| string(&instance, "containerInstanceArn", case))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the instance {terminated_id} to be listed capacity")
        });
    ec2_form(
        &router,
        "TerminateInstances",
        &[("InstanceId.1", terminated_id.as_str())],
        case,
    )
    .await;
    let mut remaining = expected(CLUSTER);
    remaining.remove(terminated_id);
    let held = settled_capacity(&router, CLUSTER, &remaining, case).await;
    assert_eq!(
        held, remaining,
        "case {case:?}: expected a terminated instance to stop being capacity, leaving \
         {remaining:?}, got {held:?}"
    );
    let counted = registered_count(&router, CLUSTER, case).await;
    assert_eq!(
        counted,
        remaining.len() as i64,
        "case {case:?}: expected the cluster to count {} registered container instance(s) \
         once one was terminated, got {counted}",
        remaining.len()
    );
    // The record itself is still there, deregistered: a describe that names its ARN reports
    // the status and the agent that is no longer connected.
    let described_gone = ok(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [terminated_arn] }),
        case,
    )
    .await;
    let gone = list(&described_gone, "containerInstances", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the terminated instance's registration back, got \
                 {described_gone}"
            )
        });
    assert_eq!(
        gone["status"].as_str(),
        Some(INACTIVE),
        "case {case:?}: expected a terminated instance's registration to be {INACTIVE}, got \
         {gone}"
    );
    assert_eq!(
        gone["agentConnected"].as_bool(),
        Some(false),
        "case {case:?}: expected a terminated instance to run no agent, got {gone}"
    );
}

/// The family and container the service below runs.
const FAMILY: &str = "capacityfamily";
const SERVICE: &str = "capacitysvc";

#[tokio::test]
async fn an_ec2_launch_type_service_runs_its_task_on_the_capacity_launched_into_its_cluster() {
    let case = "ec2 launch type service";
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, case).await;
    let subnet = network(&router, case).await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;

    // "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
    // cluster": the capacity is launched into the cluster the documented way, and the
    // service is created with a count to maintain.
    let row = Row {
        case,
        configuration: Configuration::ShellLine(CLUSTER),
        instance_type: SMALL_TYPE,
        joins: CLUSTER,
    };
    let instance_id = launch(&router, &row, &subnet).await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": FAMILY,
            "launchType": "EC2",
            "desiredCount": 1,
        }),
        case,
    )
    .await;

    // The service converges on the count it was asked for once the capacity its tasks run on
    // has registered — the whole point of the capacity: a client polls the service, as this
    // does, and nothing else.
    let deadline = Instant::now() + POLL_BUDGET;
    let mut service = describe_service(&router, case).await;
    while service["runningCount"].as_i64() != Some(1) && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        service = describe_service(&router, case).await;
    }
    assert_eq!(
        service["runningCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the service to run the one task it wants on the capacity \
         launched into its cluster ({instance_id}), got {service}"
    );
    assert_eq!(
        service["desiredCount"].as_i64(),
        Some(1),
        "case {case:?}: expected the count the caller asked for to stand, got {service}"
    );

    // And the task itself, read back through the listing and the describe a client polls.
    let listed = ok(
        &router,
        "ListTasks",
        json!({ "cluster": CLUSTER, "serviceName": SERVICE }),
        case,
    )
    .await;
    let arns = list(&listed, "taskArns", case);
    assert_eq!(
        arns.len(),
        1,
        "case {case:?}: expected the scheduler to hold one task of the service, got {listed}"
    );
    let described = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    let task = list(&described, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {described}"));
    assert_eq!(
        task["lastStatus"].as_str(),
        Some("RUNNING"),
        "case {case:?}: expected the placed task to be running, got {task}"
    );
}

/// The service under test, read back through `DescribeServices`.
async fn describe_service(router: &MultiServiceRouter, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [SERVICE] }),
        case,
    )
    .await;
    list(&body, "services", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {body}"))
}

/// A launch into a subnet the account does not hold is refused by Amazon EC2 itself, so no
/// instance of it could ever be capacity — the one guard this suite needs from the service
/// that owns instances, asserted here so the rows above cannot pass against a store written
/// by hand.
#[tokio::test]
async fn capacity_comes_from_instances_amazon_ec2_itself_holds() {
    let case = "capacity is amazon ec2's";
    let router = emulator_with_ec2();
    create_cluster(&router, CLUSTER, case).await;
    let subnet = network(&router, case).await;
    let instance_id = launch(
        &router,
        &Row {
            case,
            configuration: Configuration::ShellLine(CLUSTER),
            instance_type: SMALL_TYPE,
            joins: CLUSTER,
        },
        &subnet,
    )
    .await;
    let wanted: BTreeSet<String> = [instance_id.clone()].into_iter().collect();
    let held = settled_capacity(&router, CLUSTER, &wanted, case).await;
    assert_eq!(
        held, wanted,
        "case {case:?}: expected the launched instance to be the cluster's capacity, got \
         {held:?}"
    );
    // Amazon EC2's own describe resolves the same instance id, which is what makes the
    // capacity one machine rather than two records.
    let described = ec2(
        &router,
        "DescribeInstances",
        &[("InstanceId.1", instance_id.as_str())],
        case,
    )
    .await;
    assert_eq!(
        ec2_element(&described, "instanceId", case),
        instance_id,
        "case {case:?}: expected Amazon EC2 to hold the instance the cluster reports capacity \
         for, got {described}"
    );
}
