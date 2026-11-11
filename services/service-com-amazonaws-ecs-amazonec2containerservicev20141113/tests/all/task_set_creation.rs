// SPDX-License-Identifier: CC0-1.0

//! What `CreateTaskSet` PERSISTS, and which requests it never persists anything for.
//!
//! Two rules, each asserted over a table of inputs rather than one recorded case:
//!
//! 1. The capacity a task set's tasks run on is RESOLVED once, when the task set is
//!    created, from the request and then from the cluster: the `launchType` the request
//!    names ("If a launchType is specified, the capacityProviderStrategy parameter must be
//!    omitted"), else the `capacityProviderStrategy` it names, else "the
//!    defaultCapacityProviderStrategy for the cluster", and when the cluster has none
//!    either the task set runs under the default launch type — and what was resolved is
//!    what every later read of the task set reports.
//! 2. A request that names a resource this service does not hold, or a parameter value the
//!    operation's contract excludes, is refused with the modelled error for THAT condition
//!    and creates no task set: a client that retries after fixing the request finds the
//!    service exactly as it was.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};

use crate::ecs_wire::{
    awsvpc_network_configuration, count, create_cluster, list, ok, refusal, register_capacity,
    register_definition, service, string,
};
use crate::task_sets::{create_external_service, describe_task_sets, register_fargate_definition};

/// A task definition family placed with a plain (bridge-network) revision.
const PLAIN_FAMILY: &str = "plaintaskset";

/// A task definition family whose revision is Fargate-capable (`awsvpc` plus task-level
/// cpu and memory), so a row may place it under the Fargate launch type or through a
/// capacity provider Amazon ECS manages.
const FARGATE_FAMILY: &str = "fargatetaskset";

/// The capacity provider Amazon ECS manages for every account, from
/// `smithy_query(query="shape",shape="CapacityProviderStrategyItem")`'s
/// `capacityProvider` member ("The short name of the capacity provider"): the Fargate one
/// needs no Auto Scaling group of the caller's, so a row may name it without registering
/// capacity of its own.
const MANAGED_PROVIDER: &str = "FARGATE";

/// The `(capacityProvider, weight, base)` of every entry of a task set's
/// `capacityProviderStrategy`, in the order it is reported.
fn strategy_entries(task_set: &Value, case: &str) -> Vec<(String, i64, i64)> {
    list(task_set, "capacityProviderStrategy", case)
        .iter()
        .map(|item| {
            (
                string(item, "capacityProvider", case),
                count(item, "weight", case),
                count(item, "base", case),
            )
        })
        .collect()
}

/// One row of the capacity-resolution table.
struct CapacityCase {
    /// What the row is, for the failure message.
    name: &'static str,
    /// The cluster the task set is created in — one with a default capacity provider
    /// strategy, or one without.
    cluster: &'static str,
    /// The task definition family the task set runs.
    family: &'static str,
    /// The `launchType` the request names, if any.
    launch_type: Option<&'static str>,
    /// The `capacityProviderStrategy` the request names, if any.
    strategy: Option<Vec<(&'static str, i64)>>,
    /// The `launchType` the stored task set must report.
    stored_launch_type: Option<&'static str>,
    /// The `capacityProviderStrategy` the stored task set must report.
    stored_strategy: Vec<(&'static str, i64, i64)>,
}

#[tokio::test]
async fn create_task_set_resolves_its_capacity_from_the_request_then_from_the_cluster() {
    let router = service();
    let case = "setup";
    // One cluster carries a default capacity provider strategy and the other carries
    // none, so the rows differ along what the cluster contributes as well as along what
    // the request names.
    let with_default = "tsdefaulted";
    let without_default = "tsundefaulted";
    for cluster in [with_default, without_default] {
        create_cluster(&router, cluster, case).await;
        register_capacity(&router, cluster, case).await;
    }
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": with_default,
            "capacityProviders": [MANAGED_PROVIDER],
            "defaultCapacityProviderStrategy": [
                { "capacityProvider": MANAGED_PROVIDER, "weight": 2 },
            ],
        }),
        case,
    )
    .await;
    // The other cluster holds the same provider WITHOUT making it its default: a placement
    // uses the providers its cluster holds, and these rows differ along what the request
    // and the cluster's default contribute rather than along which providers are available.
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": without_default,
            "capacityProviders": [MANAGED_PROVIDER],
            "defaultCapacityProviderStrategy": [],
        }),
        case,
    )
    .await;
    register_definition(&router, PLAIN_FAMILY, case).await;
    register_fargate_definition(&router, FARGATE_FAMILY, case).await;

    let cases = [
        CapacityCase {
            name: "a launch type on the registered capacity",
            cluster: without_default,
            family: PLAIN_FAMILY,
            launch_type: Some("EC2"),
            strategy: None,
            stored_launch_type: Some("EC2"),
            stored_strategy: Vec::new(),
        },
        CapacityCase {
            name: "a launch type Amazon ECS runs the capacity for",
            cluster: without_default,
            family: FARGATE_FAMILY,
            launch_type: Some("FARGATE"),
            strategy: None,
            stored_launch_type: Some("FARGATE"),
            stored_strategy: Vec::new(),
        },
        CapacityCase {
            name: "a capacity provider strategy the request names",
            cluster: without_default,
            family: FARGATE_FAMILY,
            launch_type: None,
            strategy: Some(vec![(MANAGED_PROVIDER, 1)]),
            stored_launch_type: None,
            stored_strategy: vec![(MANAGED_PROVIDER, 1, 0)],
        },
        CapacityCase {
            name: "neither, in a cluster that has a default strategy",
            cluster: with_default,
            family: FARGATE_FAMILY,
            launch_type: None,
            strategy: None,
            stored_launch_type: None,
            // The cluster's own default strategy, weight and all.
            stored_strategy: vec![(MANAGED_PROVIDER, 2, 0)],
        },
        CapacityCase {
            name: "neither, in a cluster that has no default strategy",
            cluster: without_default,
            family: PLAIN_FAMILY,
            launch_type: None,
            strategy: None,
            stored_launch_type: Some("EC2"),
            stored_strategy: Vec::new(),
        },
    ];

    for (index, test) in cases.iter().enumerate() {
        let case = test.name;
        // One service per row, so no row can see another row's task sets.
        let service_name = format!("capext{index}");
        create_external_service(&router, test.cluster, &service_name, 1, case).await;

        let mut request = json!({
            "cluster": test.cluster,
            "service": service_name,
            "taskDefinition": test.family,
        });
        if test.family == FARGATE_FAMILY {
            request["networkConfiguration"] = awsvpc_network_configuration();
        }
        if let Some(launch_type) = test.launch_type {
            request["launchType"] = json!(launch_type);
        }
        if let Some(strategy) = test.strategy.as_ref() {
            request["capacityProviderStrategy"] = json!(strategy
                .iter()
                .map(|(provider, weight)| json!({
                    "capacityProvider": provider,
                    "weight": weight,
                }))
                .collect::<Vec<Value>>());
        }

        let created = ok(&router, "CreateTaskSet", request, case).await;
        let created = created["taskSet"].clone();
        let id = string(&created, "id", case);

        // ROUND TRIP: the resolved capacity is read back through the operation a client
        // reads a task set with, so it was PERSISTED rather than echoed.
        let read = describe_task_sets(&router, test.cluster, &service_name, None, false, case).await;
        let task_sets = list(&read, "taskSets", case);
        assert_eq!(
            task_sets.len(),
            1,
            "case {case:?}: expected exactly the created task set back, got {read}"
        );
        let stored = task_sets[0].clone();
        assert_eq!(
            string(&stored, "id", case),
            id,
            "case {case:?}: describe must report the created task set, got {read}"
        );

        for (source, view) in [("the create", &created), ("the read-back", &stored)] {
            let actual_launch_type = view["launchType"].as_str();
            assert_eq!(
                actual_launch_type,
                test.stored_launch_type,
                "case {case:?}: expected {source} to report launchType {:?}, got {view}",
                test.stored_launch_type
            );
            let actual_strategy = strategy_entries(view, case);
            let expected_strategy: Vec<(String, i64, i64)> = test
                .stored_strategy
                .iter()
                .map(|(provider, weight, base)| (provider.to_string(), *weight, *base))
                .collect();
            assert_eq!(
                actual_strategy, expected_strategy,
                "case {case:?}: expected {source} to report capacityProviderStrategy \
                 {expected_strategy:?}, got {actual_strategy:?} in {view}"
            );
            // "A platform version is specified only for tasks using the Fargate launch
            // type": the capacity resolved above is what decides whether one is reported.
            let fargate = test.stored_launch_type == Some("FARGATE")
                || test
                    .stored_strategy
                    .iter()
                    .any(|(provider, _, _)| *provider == MANAGED_PROVIDER);
            let platform_version = view["platformVersion"].as_str();
            assert_eq!(
                platform_version.is_some(),
                fargate,
                "case {case:?}: expected {source} to report a platformVersion only for \
                 Fargate capacity (fargate={fargate}), got {platform_version:?} in {view}"
            );
        }
    }
}

/// One row of the refusal table.
struct RefusalCase {
    /// What the row is, for the failure message.
    name: &'static str,
    /// The request body, built fresh per row.
    request: Value,
    /// The modelled error code the condition is refused with.
    code: &'static str,
    /// The message that refusal carries.
    message: String,
}

#[tokio::test]
async fn create_task_set_refuses_each_condition_it_models_and_creates_nothing() {
    let router = service();
    let case = "setup";
    let cluster = "tsrefusals";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, PLAIN_FAMILY, case).await;
    register_fargate_definition(&router, FARGATE_FAMILY, case).await;
    // A family whose only revision was deregistered, so a reference to it resolves to a
    // revision this service HOLDS in a status no new work may use.
    let retired = "retiredtaskset";
    register_definition(&router, retired, case).await;
    ok(
        &router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{retired}:1") }),
        case,
    )
    .await;
    // The service every row that gets past the resource lookups acts on, and one whose
    // own scheduler places its tasks.
    let external = "extrefused";
    create_external_service(&router, cluster, external, 1, case).await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "ecsmanaged",
            "taskDefinition": PLAIN_FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        case,
    )
    .await;

    // A `taskDefinition` value that is none of the three forms the member documents (a
    // family, a `family:revision` or the resource's own ARN) — here the ARN of another
    // resource entirely.
    let foreign_arn = format!("arn:aws:ecs:us-west-2:000000000000:service/{cluster}/{external}");
    // 37 characters: one past the 36 the `clientToken` member documents.
    let long_token = "t".repeat(37);

    let cases = vec![
        RefusalCase {
            name: "a cluster this service does not hold",
            request: json!({
                "cluster": "ghosttaskset",
                "service": external,
                "taskDefinition": PLAIN_FAMILY,
                "launchType": "EC2",
            }),
            code: "ClusterNotFoundException",
            message: "Cluster not found.".to_string(),
        },
        RefusalCase {
            name: "a service the cluster does not hold",
            request: json!({
                "cluster": cluster,
                "service": "ghostservice",
                "taskDefinition": PLAIN_FAMILY,
                "launchType": "EC2",
            }),
            code: "ServiceNotFoundException",
            message: "Service not found.".to_string(),
        },
        RefusalCase {
            name: "a service whose own scheduler places its tasks",
            request: json!({
                "cluster": cluster,
                "service": "ecsmanaged",
                "taskDefinition": PLAIN_FAMILY,
                "launchType": "EC2",
            }),
            code: "InvalidParameterException",
            message: "A task set can only be created in a service that uses the EXTERNAL \
                      or CODE_DEPLOY deployment controller."
                .to_string(),
        },
        RefusalCase {
            name: "a launch type beside a capacity provider strategy",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": FARGATE_FAMILY,
                "networkConfiguration": awsvpc_network_configuration(),
                "launchType": "FARGATE",
                "capacityProviderStrategy": [
                    { "capacityProvider": MANAGED_PROVIDER, "weight": 1 },
                ],
            }),
            code: "InvalidParameterException",
            message: "Specifying both a launch type and capacity provider strategy is not \
                      supported. Remove one and try again."
                .to_string(),
        },
        RefusalCase {
            name: "a platform version that does not exist",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": FARGATE_FAMILY,
                "networkConfiguration": awsvpc_network_configuration(),
                "launchType": "FARGATE",
                "platformVersion": "9.9.9",
            }),
            code: "PlatformUnknownException",
            message: "The specified platform does not exist.".to_string(),
        },
        RefusalCase {
            name: "a task definition family that was never registered",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": "ghostfamily",
                "launchType": "EC2",
            }),
            code: "ClientException",
            message: "TaskDefinition not found.".to_string(),
        },
        RefusalCase {
            name: "a task definition revision that was deregistered",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": format!("{retired}:1"),
                "launchType": "EC2",
            }),
            code: "InvalidParameterException",
            message: "TaskDefinition is inactive".to_string(),
        },
        RefusalCase {
            name: "a taskDefinition that is not a task definition reference at all",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": foreign_arn,
                "launchType": "EC2",
            }),
            code: "ClientException",
            message: "Family contains invalid characters.".to_string(),
        },
        RefusalCase {
            name: "a client token one character past the documented length",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": PLAIN_FAMILY,
                "launchType": "EC2",
                "clientToken": long_token,
            }),
            code: "InvalidParameterException",
            message: format!(
                "clientToken must be up to 36 ASCII characters in the range 33-126; got \
                 {long_token:?}"
            ),
        },
        RefusalCase {
            name: "a client token carrying a character outside the documented range",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": PLAIN_FAMILY,
                "launchType": "EC2",
                "clientToken": "token with spaces",
            }),
            code: "InvalidParameterException",
            message: "clientToken must be up to 36 ASCII characters in the range 33-126; \
                      got \"token with spaces\""
                .to_string(),
        },
        RefusalCase {
            name: "an awsvpc revision with no network configuration to take",
            request: json!({
                "cluster": cluster,
                "service": external,
                "taskDefinition": FARGATE_FAMILY,
                "launchType": "FARGATE",
            }),
            code: "InvalidParameterException",
            message: "Network Configuration must be provided when networkMode 'awsvpc' is \
                      specified."
                .to_string(),
        },
    ];

    for test in &cases {
        let case = test.name;
        let (code, message) = refusal(&router, "CreateTaskSet", test.request.clone(), case).await;
        assert_eq!(
            code, test.code,
            "case {case:?}: expected a {} refusal, got {code} ({message:?})",
            test.code
        );
        assert_eq!(
            message, test.message,
            "case {case:?}: expected the message {:?}, got {message:?}",
            test.message
        );
        // A refused create persists nothing: the service a client would retry against
        // still holds no task set.
        let read = describe_task_sets(&router, cluster, external, None, false, case).await;
        assert!(
            list(&read, "taskSets", case).is_empty(),
            "case {case:?}: a refused create must leave the service without a task set, \
             got {read}"
        );
    }

    // The control: the same service accepts the request once it names a revision and the
    // capacity it needs, and the created task set reads back.
    let case = "the request every row above varies from";
    let created = ok(
        &router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": external,
            "taskDefinition": PLAIN_FAMILY,
            "launchType": "EC2",
            "externalId": "the-caller-tag",
        }),
        case,
    )
    .await;
    let id = string(&created["taskSet"], "id", case);
    let read = describe_task_sets(&router, cluster, external, None, false, case).await;
    let task_sets = list(&read, "taskSets", case);
    assert_eq!(
        task_sets.len(),
        1,
        "case {case:?}: expected exactly one task set after one accepted create, got {read}"
    );
    assert_eq!(
        string(&task_sets[0], "id", case),
        id,
        "case {case:?}: describe must report the created task set, got {read}"
    );
    assert_eq!(
        string(&task_sets[0], "externalId", case),
        "the-caller-tag",
        "case {case:?}: the stored task set keeps the externalId it was created with, got {read}"
    );
}

#[tokio::test]
async fn a_create_past_the_per_service_task_set_quota_is_refused_and_stores_nothing() {
    let router = service();
    let case = "setup";
    let cluster = "tsquota";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, PLAIN_FAMILY, case).await;
    let service_name = "extquota";
    // A service that wants no tasks, so the rows below exercise the quota on the task set
    // RECORDS rather than the scheduler's placement.
    create_external_service(&router, cluster, service_name, 0, case).await;

    let request = json!({
        "cluster": cluster,
        "service": service_name,
        "taskDefinition": PLAIN_FAMILY,
        "launchType": "EC2",
    });

    // The quota Amazon ECS documents for task sets per service; each accepted create is
    // one more stored task set, so the count is the evidence the records are real.
    let quota = 5;
    for index in 0..quota {
        let case = &format!("task set {} of the quota", index + 1);
        ok(&router, "CreateTaskSet", request.clone(), case).await;
        let read = describe_task_sets(&router, cluster, service_name, None, false, case).await;
        assert_eq!(
            list(&read, "taskSets", case).len(),
            index + 1,
            "case {case:?}: expected {} stored task sets, got {read}",
            index + 1
        );
    }

    let case = "one past the quota";
    let (code, message) = refusal(&router, "CreateTaskSet", request, case).await;
    assert_eq!(
        code, "LimitExceededException",
        "case {case:?}: a create past the quota is the modelled limit failure, got {code} \
         ({message:?})"
    );
    assert!(
        message.contains(&quota.to_string()),
        "case {case:?}: expected the refusal to state the quota {quota}, got {message:?}"
    );
    let read = describe_task_sets(&router, cluster, service_name, None, false, case).await;
    assert_eq!(
        list(&read, "taskSets", case).len(),
        quota,
        "case {case:?}: the refused create must store nothing, got {read}"
    );
}
