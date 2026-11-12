// SPDX-License-Identifier: CC0-1.0

//! How `DescribeTaskSets` NAMES a task set the service no longer holds — the batch read's
//! failure channel for a task set that was created, deleted and then drained away.
//!
//! The rule under test: a batch read reports every reference it cannot resolve as its OWN
//! `MISSING` failure, in the order the request named them, naming that task set by the id
//! the reference carried alone under the `task-set` resource type
//! (`arn:aws:ecs:<region>:<account>:task-set/<id>`). The cluster and service segments of a
//! stored task set's ARN say which service the task set exists IN, which one the service
//! holds no more is held by none of, so neither segment is inserted into the ARN a failure
//! reports. That is what lets a client correlate each entry back to the identifier it
//! submitted, however many it submitted at once and however it spelled each one.
//!
//! [`crate::task_set_reports`] asserts the same rule for ids nothing was ever created
//! under, and [`crate::task_set_deletion`] asserts it for ONE deleted task set beside the
//! drain that takes it away. The table here varies what the rule generalises over on the
//! path a client actually takes to it — how many drained-away task sets one request names,
//! in which order, in which of the member's two forms ("The ID or full Amazon Resource
//! Name (ARN) of task sets to describe"), and whether a task set the service does still
//! hold or an id nothing ever created is named beside them.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, list, ok, register_capacity, register_definition, service, string, ACCOUNT,
    SIGNED_REGION,
};
use crate::task_sets::{create_external_service, describe_task_sets};

/// The task definition family every task set below runs.
const FAMILY: &str = "tsfailurefamily";

/// The `Failure$reason` for an identifier that names nothing, from the API failure
/// reasons: "MISSING — the described resource could not be found".
const MISSING: &str = "MISSING";

/// Long enough for one clock-owed transition (`state::TRANSITION_SPAN` is 500 ms) — here
/// the drain a task-set delete starts — to fall due, with margin for a loaded machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// The ARN of a task set under the resource path `path`, in the signed region and the
/// emulator's account.
fn task_set_arn(path: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-set/{path}")
}

/// The `(arn, reason)` pair of every failure a describe reports, in order.
fn failures(body: &Value, case: &str) -> Vec<(String, String)> {
    list(body, "failures", case)
        .iter()
        .map(|failure| (string(failure, "arn", case), string(failure, "reason", case)))
        .collect()
}

/// The ids of the task sets a describe reports, in order.
fn described_ids(body: &Value, case: &str) -> Vec<String> {
    list(body, "taskSets", case)
        .iter()
        .map(|task_set| string(task_set, "id", case))
        .collect()
}

/// One row of the failure-channel table.
struct Row {
    /// What the row is, for the failure message.
    case: &'static str,
    /// What the request names in its `taskSets` member, in order.
    named: Vec<String>,
    /// The ARNs of the failures expected back, IN THE REQUEST'S ORDER.
    expected: Vec<String>,
    /// The ids of the task sets the same answer reports, in order.
    described: Vec<String>,
}

/// Create one task set of `service_name` and return its `(id, taskSetArn)`.
async fn create_task_set(
    router: &MultiServiceRouter,
    cluster: &str,
    service_name: &str,
    external_id: &str,
    case: &str,
) -> (String, String) {
    let created = ok(
        router,
        "CreateTaskSet",
        json!({
            "cluster": cluster,
            "service": service_name,
            "taskDefinition": FAMILY,
            "launchType": "EC2",
            "externalId": external_id,
            "scale": { "value": 0, "unit": "PERCENT" },
        }),
        case,
    )
    .await;
    let created = &created["taskSet"];
    (
        string(created, "id", case),
        string(created, "taskSetArn", case),
    )
}

#[tokio::test]
async fn every_task_set_a_request_cannot_resolve_is_named_by_its_own_id() {
    let router = service();
    let case = "setup";
    let cluster = "tsgone";
    let service_name = "tsgoneext";
    create_cluster(&router, cluster, case).await;
    register_capacity(&router, cluster, case).await;
    register_definition(&router, FAMILY, case).await;
    create_external_service(&router, cluster, service_name, 1, case).await;

    // Two task sets the caller deletes, and one it keeps — so a row may name a reference
    // that resolves beside ones that do not.
    let (first_id, first_arn) =
        create_task_set(&router, cluster, service_name, "firstgone", case).await;
    let (second_id, second_arn) =
        create_task_set(&router, cluster, service_name, "secondgone", case).await;
    let (kept_id, _) = create_task_set(&router, cluster, service_name, "kept", case).await;
    // The ARN a stored task set carries names the service it exists in, which is what a
    // failure about one the service holds no more must NOT report.
    assert_eq!(
        first_arn,
        task_set_arn(&format!("{cluster}/{service_name}/{first_id}")),
        "a stored task set is named under the service it exists in, got {first_arn:?}"
    );

    for reference in [&first_id, &second_id] {
        ok(
            &router,
            "DeleteTaskSet",
            json!({
                "cluster": cluster,
                "service": service_name,
                "taskSet": reference,
            }),
            case,
        )
        .await;
    }
    // The CLOCK finishes the drain each delete started; from there the service holds
    // neither task set, so a request that names one is answered through the failure
    // channel.
    tokio::time::sleep(ONE_SPAN).await;

    // An id nothing was ever created under, in the minted form (`TaskSet$id`): `ecs-svc/`
    // followed by 19 digits.
    let never = "ecs-svc/0000000000000009999";

    // Rows: the case, what the request names, the failure ARNs expected back IN THE
    // REQUEST'S ORDER, and the ids of the task sets the same answer reports.
    let rows = [
        Row {
            case: "both drained-away task sets, by their ids in creation order",
            named: vec![first_id.clone(), second_id.clone()],
            expected: vec![task_set_arn(&first_id), task_set_arn(&second_id)],
            described: Vec::new(),
        },
        Row {
            case: "the same two, named the other way round",
            named: vec![second_id.clone(), first_id.clone()],
            expected: vec![task_set_arn(&second_id), task_set_arn(&first_id)],
            described: Vec::new(),
        },
        Row {
            case: "both named by the service-qualified ARNs their creates answered with",
            named: vec![first_arn.clone(), second_arn.clone()],
            expected: vec![task_set_arn(&first_id), task_set_arn(&second_id)],
            described: Vec::new(),
        },
        Row {
            case: "one by its id and the other by its ARN",
            named: vec![first_id.clone(), second_arn.clone()],
            expected: vec![task_set_arn(&first_id), task_set_arn(&second_id)],
            described: Vec::new(),
        },
        Row {
            case: "the same drained-away task set named in both forms",
            named: vec![first_id.clone(), first_arn.clone()],
            expected: vec![task_set_arn(&first_id)],
            described: Vec::new(),
        },
        Row {
            case: "a drained-away task set beside the one the service still holds",
            named: vec![first_id.clone(), kept_id.clone()],
            expected: vec![task_set_arn(&first_id)],
            described: vec![kept_id.clone()],
        },
        Row {
            case: "the surviving task set, a drained-away one and an id nothing ever created",
            named: vec![kept_id.clone(), second_id.clone(), never.to_string()],
            expected: vec![task_set_arn(&second_id), task_set_arn(never)],
            described: vec![kept_id.clone()],
        },
        Row {
            case: "only the task set the service still holds",
            named: vec![kept_id.clone()],
            expected: Vec::new(),
            described: vec![kept_id.clone()],
        },
    ];

    for Row {
        case,
        named,
        expected,
        described,
    } in rows
    {
        let body = describe_task_sets(
            &router,
            cluster,
            service_name,
            Some(named.clone()),
            false,
            case,
        )
        .await;
        let reported = failures(&body, case);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|arn| (arn, MISSING.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the failures {expected:?} for {named:?}, got \
             {reported:?} in {body}"
        );
        // Spelled out: neither the cluster the request named nor the service it looked in
        // appears anywhere in a failure's ARN.
        for (arn, _) in &reported {
            let qualified = arn.contains(&format!("/{cluster}/"))
                || arn.contains(&format!("/{service_name}/"));
            assert!(
                !qualified,
                "case {case:?}: expected no cluster or service segment in the failure ARN \
                 {arn:?}, got {body}"
            );
        }
        let reported = described_ids(&body, case);
        assert_eq!(
            reported, described,
            "case {case:?}: expected the task sets {described:?} reported beside the \
             failures, got {reported:?} in {body}"
        );
    }

    // The failure channel is the only thing the drain changed: the task set the caller
    // kept still reads back through a describe that names nothing else.
    let read =
        describe_task_sets(&router, cluster, service_name, None, false, "the survivor").await;
    assert_eq!(
        described_ids(&read, "the survivor"),
        vec![kept_id],
        "the service must still hold the task set nothing deleted, got {read}"
    );
    assert!(
        failures(&read, "the survivor").is_empty(),
        "a request that names no task set reports no failure, got {read}"
    );
}
