// SPDX-License-Identifier: CC0-1.0

//! The window a deployment is IN FLIGHT for, driven over the wire.
//!
//! Two rules, one window. A deployment is not a record that lands already
//! finished: from the moment CodeDeploy dispatches it to its targets until it
//! reaches an outcome it is reported `InProgress`, and so are the targets it
//! dispatched to — a client that polls `GetDeployment`, or reads a target while
//! nothing has reported for it yet, reads that state rather than a success. And
//! while that window is open the deployment group it belongs to is BUSY: a
//! deployment group runs one deployment at a time, so a second `CreateDeployment`
//! against the same group is refused with `DeploymentLimitExceededException` (HTTP
//! 400) until the first is terminal.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness in `codedeploy_lifecycle` — so the awsJson1_1 envelope,
//! the boundary validator and this service's refusal wording are all exercised.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, terminal_deployment, Emulator};

/// The service role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// Create an application and one deployment group on it, carrying `members`.
async fn given_group(
    emulator: &Emulator,
    application: &str,
    platform: &str,
    group: &str,
    members: &Value,
) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": SERVICE_ROLE,
    });
    if let Some(members) = members.as_object() {
        for (name, value) in members {
            request[name] = value.clone();
        }
    }
    emulator.ok("CreateDeploymentGroup", request).await;
}

/// Register `instance` and tag it, so a deployment group selecting that tag
/// resolves it as a target.
async fn given_on_premises_instance(emulator: &Emulator, instance: &str, tag: (&str, &str)) {
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": instance,
                "iamUserArn": format!(
                    "arn:aws:iam::{}:user/{instance}",
                    engine::emulator_account_id()
                ),
            }),
        )
        .await;
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({
                "instanceNames": [instance],
                "tags": [{"Key": tag.0, "Value": tag.1, "Type": "KEY_AND_VALUE"}],
            }),
        )
        .await;
}

/// Create a deployment on `application`/`group`, answering its minted id.
async fn deploy(emulator: &Emulator, application: &str, group: &str) -> String {
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": s3_revision("artifacts", "app.zip"),
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Poll `deployment_id` until the clock has STARTED it, answering its
/// `deploymentInfo`.
///
/// A deployment CodeDeploy has not dispatched yet has deployed to nothing, so an
/// operation over its targets is refused with `DeploymentNotStartedException` —
/// which is what a client waits out before reading a target, and what this does.
async fn started_deployment(emulator: &Emulator, deployment_id: &str, case: &str) -> Value {
    let deadline_at = Instant::now() + Duration::from_secs(5);
    loop {
        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await["deploymentInfo"]
            .clone();
        if info["status"].as_str() != Some("Created") {
            return info;
        }
        assert!(
            Instant::now() < deadline_at,
            "case {case:?}: expected the deployment to be dispatched, got {info:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The `status` each target of `deployment_id` is reported with, by both target
/// readers — the listing's ids fetched one by one, which have to agree.
async fn target_statuses(emulator: &Emulator, deployment_id: &str, case: &str) -> Vec<String> {
    let listed = emulator
        .ok(
            "ListDeploymentTargets",
            json!({"deploymentId": deployment_id}),
        )
        .await;
    let ids: Vec<String> = listed["targetIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect();
    assert!(
        !ids.is_empty(),
        "case {case:?}: expected the deployment to have enumerated its targets, got {listed:?}"
    );
    let mut statuses = Vec::new();
    for id in &ids {
        let read = emulator
            .ok(
                "GetDeploymentTarget",
                json!({"deploymentId": deployment_id, "targetId": id}),
            )
            .await;
        let target = &read["deploymentTarget"];
        let status = target["instanceTarget"]["status"]
            .as_str()
            .or_else(|| target["lambdaTarget"]["status"].as_str())
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the target {id} to report a status, got {read:?}")
            })
            .to_string();
        statuses.push(status);
    }
    statuses
}

/// RULE: a deployment and every target it dispatched to are reported `InProgress`
/// from the moment it is dispatched until it reaches an outcome — no agent, and
/// nothing else on the target, has reported anything by then — and `Succeeded`
/// only once it has.
#[tokio::test]
async fn a_dispatched_deployment_and_its_targets_are_in_progress_until_it_settles() {
    // Rows differ along the fleet the deployment group selects: one on-premises
    // instance, several of them, and a group whose style gives its targets an
    // environment label.
    let cases: &[(&str, usize, Value)] = &[
        ("one on-premises target", 1, json!({})),
        ("three on-premises targets", 3, json!({})),
        (
            "a blue/green group's original environment",
            2,
            json!({
                "deploymentStyle": {
                    "deploymentType": "BLUE_GREEN",
                    "deploymentOption": "WITH_TRAFFIC_CONTROL",
                },
            }),
        ),
    ];

    for (case, fleet, members) in cases {
        // One emulator per row: the on-premises registry is shared state, and each
        // row is about the instances IT registered.
        let emulator = Emulator::new();
        let application = "in-flight";
        let group = "in-flight-dg";
        let mut selectors = json!({
            "onPremisesInstanceTagFilters": [
                {"Key": "Fleet", "Value": application, "Type": "KEY_AND_VALUE"},
            ],
        });
        if let Some(members) = members.as_object() {
            for (name, value) in members {
                selectors[name] = value.clone();
            }
        }
        given_group(&emulator, application, "Server", group, &selectors).await;
        for index in 0..*fleet {
            given_on_premises_instance(
                &emulator,
                &format!("host-{index}"),
                ("Fleet", application),
            )
            .await;
        }

        let deployment_id = deploy(&emulator, application, group).await;
        // The deployment is dispatched, and nothing has reported for any target:
        // every one of them is being deployed to.
        let polled = started_deployment(&emulator, &deployment_id, case).await;
        assert_eq!(
            polled["status"].as_str(),
            Some("InProgress"),
            "case {case:?}: expected the dispatched deployment to be InProgress, got {polled:?}"
        );
        assert!(
            polled["completeTime"].is_null(),
            "case {case:?}: expected no completeTime while it is in flight, got {polled:?}"
        );
        let dispatched = target_statuses(&emulator, &deployment_id, case).await;
        assert_eq!(
            dispatched,
            vec!["InProgress".to_string(); *fleet],
            "case {case:?}: expected every dispatched target to be InProgress, got {dispatched:?}"
        );

        // And once the deployment has reached its outcome, so have its targets.
        let info = terminal_deployment(&emulator, &deployment_id, case).await;
        assert_eq!(
            info["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: expected the deployment to succeed, got {info:?}"
        );
        let settled = target_statuses(&emulator, &deployment_id, case).await;
        assert_eq!(
            settled,
            vec!["Succeeded".to_string(); *fleet],
            "case {case:?}: expected every target to have succeeded with it, got {settled:?}"
        );
    }
}

/// RULE: a deployment group runs ONE deployment at a time — while a deployment of
/// it has not reached an outcome a second `CreateDeployment` against that group is
/// refused with `DeploymentLimitExceededException`, another group is unaffected,
/// and once the first deployment is terminal the group accepts a new one.
#[tokio::test]
async fn a_deployment_group_takes_one_deployment_at_a_time() {
    let emulator = Emulator::new();
    let application = "serialized";
    given_group(
        &emulator,
        application,
        "Server",
        "first-dg",
        &json!({
            "onPremisesInstanceTagFilters": [
                {"Key": "Fleet", "Value": application, "Type": "KEY_AND_VALUE"},
            ],
        }),
    )
    .await;
    // A second group of the SAME application, which the refusal must not reach.
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": "second-dg",
                "serviceRoleArn": SERVICE_ROLE,
            }),
        )
        .await;
    given_on_premises_instance(&emulator, "host-one", ("Fleet", application)).await;
    // The id CodeDeploy minted for the busy group, which its refusal names.
    let group_id = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": "first-dg"}),
        )
        .await["deploymentGroupInfo"]["deploymentGroupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let active = deploy(&emulator, application, "first-dg").await;

    // Rows differ along which group is deployed to while `first-dg` is busy, and
    // along how many times in a row the same group is asked.
    let cases: &[(&str, &str, bool)] = &[
        ("the busy group", "first-dg", false),
        ("the busy group again", "first-dg", false),
        ("another group of the same application", "second-dg", true),
    ];
    for (case, group, accepted) in cases {
        let (status, body) = emulator
            .call(
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "revision": s3_revision("artifacts", "app.zip"),
                }),
            )
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the deployment to be created, got {status} {body:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DeploymentLimitExceededException"),
            "case {case:?}: expected the second deployment to be refused, got {status} {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!(
                    "The Deployment Group 'first-dg (id={group_id})' already has an active \
                     Deployment '{active}'"
                )
                .as_str()
            ),
            "case {case:?}: expected the clause naming the group and the active deployment, got \
             {body:?}"
        );
        assert!(
            body["deploymentId"].is_null(),
            "case {case:?}: a refused deployment must not be created, got {body:?}"
        );
    }

    // Once the active deployment is terminal the group takes a new one again.
    terminal_deployment(&emulator, &active, "the active deployment").await;
    let next = deploy(&emulator, application, "first-dg").await;
    assert_ne!(
        next, active,
        "case the group is free again: expected a new deployment, got {next:?}"
    );
}
