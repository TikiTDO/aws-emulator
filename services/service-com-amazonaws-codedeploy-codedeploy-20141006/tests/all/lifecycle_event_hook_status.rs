// SPDX-License-Identifier: CC0-1.0

//! `PutLifecycleEventHookExecutionStatus`: the result a Lambda validation function
//! reports for one of a deployment's lifecycle event hooks.
//!
//! Driven through the same front door as the rest of the suite — the harness in
//! [`super::codedeploy_lifecycle`] — so the awsJson1_1 envelope and this service's
//! refusal wording are exercised, not bypassed.

use serde_json::{json, Value};

use super::codedeploy_lifecycle::{error_code, s3_revision, seeded_group, Emulator};

/// The clause CodeDeploy answers for a deployment id that is not in the valid
/// format, which is built from the id the request carried.
fn invalid_deployment_id_message(deployment_id: &str) -> String {
    format!("Specified DeploymentId is not in the valid format: {deployment_id}")
}

/// Create an application, a deployment group and a deployment on `platform`,
/// answering the minted deployment id.
async fn seeded_deployment(emulator: &Emulator, application: &str, platform: &str) -> String {
    let group = seeded_group(emulator, application, platform).await;
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;
    created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How many lifecycle event hooks each compute platform runs through a Lambda
/// validation function, from `PutLifecycleEventHookExecutionStatus`'s own
/// documentation (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatus")`):
/// Lambda has BeforeAllowTraffic and AfterAllowTraffic; ECS has BeforeInstall,
/// AfterInstall, AfterAllowTestTraffic, BeforeAllowTraffic and AfterAllowTraffic.
/// `Server` (EC2/on-premises) runs its hooks through the CodeDeploy agent, so it
/// has none.
const HOOK_COUNTS: &[(&str, usize)] = &[("Lambda", 2), ("ECS", 5)];

/// Report `reported` for hook execution `execution_id` of `deployment_id`,
/// answering the execution id the call recorded under.
///
/// `None` means the emulator's own deployment-completion clock
/// (`DEPLOYMENT_COMPLETION_MILLIS`, a span this emulator chose) had already
/// settled the deployment before the report arrived, which a stalled machine can
/// make happen: the caller then asserts the refusal changed nothing instead, so
/// no case here depends on wall-clock timing. Every other answer is a failure.
async fn report_hook_result(
    emulator: &Emulator,
    case: &str,
    deployment_id: &str,
    execution_id: &str,
    reported: &str,
) -> Option<String> {
    let (status, body) = emulator
        .call(
            "PutLifecycleEventHookExecutionStatus",
            json!({
                "deploymentId": deployment_id,
                "lifecycleEventHookExecutionId": execution_id,
                "status": reported,
            }),
        )
        .await;
    if status == 200 {
        let recorded = body["lifecycleEventHookExecutionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            recorded, execution_id,
            "case {case:?}: expected the hook execution id it recorded under, got {body:?}"
        );
        return Some(recorded);
    }
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LifecycleEventAlreadyCompletedException"),
        "case {case:?}: expected the report to be accepted or refused as already completed, \
         got {status} {body:?}"
    );
    None
}

/// The `deploymentInfo` of `deployment_id`.
async fn deployment_info(emulator: &Emulator, deployment_id: &str) -> Value {
    let read = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await;
    read["deploymentInfo"].clone()
}

/// RULE: a reported hook result is PERSISTED against the deployment the hook
/// belongs to — the call answers the execution id it recorded under, a second
/// report under that same id is refused as already completed, and a `Failed`
/// result fails the deployment, which a later read of that deployment reports.
#[tokio::test]
async fn a_reported_hook_result_is_persisted_on_its_deployment() {
    // Rows differ along the compute platform whose hooks a validation function
    // runs, and along the result it reports.
    let cases: &[(&str, &str)] = &[
        ("Lambda", "Succeeded"),
        ("Lambda", "Failed"),
        ("ECS", "Succeeded"),
        ("ECS", "Failed"),
    ];

    for (platform, reported) in cases {
        let case = format!("{platform}/{reported}");
        let emulator = Emulator::new();
        let application = format!("hooked-{}", platform.to_lowercase());
        let deployment_id = seeded_deployment(&emulator, &application, platform).await;
        let execution_id = format!("hook-execution-of-{deployment_id}");

        let recorded =
            report_hook_result(&emulator, &case, &deployment_id, &execution_id, reported).await;
        if recorded.is_none() {
            // The chosen completion clock settled the deployment first; the
            // refused report wrote nothing, so the deployment is still the
            // success the clock owed it.
            let info = deployment_info(&emulator, &deployment_id).await;
            assert_eq!(
                info["status"].as_str(),
                Some("Succeeded"),
                "case {case:?}: expected a refused report to leave the clock's own terminal \
                 status, got {info:?}"
            );
            assert!(
                info["errorInformation"].is_null(),
                "case {case:?}: expected a refused report to write no error information, \
                 got {info:?}"
            );
            continue;
        }

        // The same hook execution cannot report a second result.
        let (status, body) = emulator
            .call(
                "PutLifecycleEventHookExecutionStatus",
                json!({
                    "deploymentId": deployment_id,
                    "lifecycleEventHookExecutionId": execution_id,
                    "status": reported,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "LifecycleEventAlreadyCompletedException"),
            "case {case:?}: expected a second report of one hook execution to be refused, \
             got {status} {body:?}"
        );

        let info = deployment_info(&emulator, &deployment_id).await;
        if *reported == "Failed" {
            assert_eq!(
                info["status"].as_str(),
                Some("Failed"),
                "case {case:?}: expected the deployment to have failed with its hook, got {info:?}"
            );
            assert_eq!(
                info["errorInformation"]["code"].as_str(),
                Some("HOOK_EXECUTION_FAILURE"),
                "case {case:?}: expected the hook-execution-failure error code, got {info:?}"
            );
            assert!(
                info["completeTime"].as_f64().is_some(),
                "case {case:?}: expected a completeTime on the failed deployment, got {info:?}"
            );
            // The clock owed this deployment a success; the hook's failure is
            // terminal, so no later transition overwrites it.
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            let later = deployment_info(&emulator, &deployment_id).await;
            assert_eq!(
                later["status"].as_str(),
                Some("Failed"),
                "case {case:?}: expected the failure to be terminal, got {later:?}"
            );
        } else {
            // A succeeded hook leaves the deployment on the clock's own path:
            // every value here is a member of the model's `DeploymentStatus`
            // (`smithy_query(query="shape",shape="DeploymentStatus")`) this
            // emulator's deployments move through.
            let reached = info["status"].as_str().unwrap_or_default().to_string();
            assert!(
                ["Created", "InProgress", "Succeeded"].contains(&reached.as_str()),
                "case {case:?}: expected a succeeded hook to leave the deployment running or \
                 succeeded, got {info:?}"
            );
            assert!(
                info["errorInformation"].is_null(),
                "case {case:?}: expected no error information, got {info:?}"
            );
        }
    }
}

/// RULE: a deployment id that is not in CodeDeploy's `d-` + uppercase
/// alphanumeric format is refused with `InvalidDeploymentIdException`, naming the
/// value the request carried, BEFORE the deployment is looked up; a well-formed
/// id that names no deployment is refused with `DeploymentDoesNotExistException`;
/// and an absent or blank one with `DeploymentIdRequiredException`.
#[tokio::test]
async fn a_malformed_deployment_id_is_refused_before_existence() {
    let emulator = Emulator::new();
    // A deployment that DOES exist, so "unknown" rows cannot pass by accident.
    let existing = seeded_deployment(&emulator, "addressed", "Lambda").await;

    // Rows differ along the shape of the id: absent, blank, outside the format
    // (prefix, alphabet, nothing after the prefix, whitespace) and well-formed
    // but unknown — including a well-formed id of another length.
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("absent", None, "DeploymentIdRequiredException"),
        ("blank", Some("   "), "DeploymentIdRequiredException"),
        (
            "prose",
            Some("not a valid deployment id !!"),
            "InvalidDeploymentIdException",
        ),
        (
            "wrong prefix",
            Some("x-ABCDEFGHI"),
            "InvalidDeploymentIdException",
        ),
        (
            "lowercase",
            Some("d-abcdefghi"),
            "InvalidDeploymentIdException",
        ),
        ("prefix only", Some("d-"), "InvalidDeploymentIdException"),
        (
            "punctuation",
            Some("d-ABCDEFGH!"),
            "InvalidDeploymentIdException",
        ),
        (
            "unknown, nine characters",
            Some("d-ABCDEFGHI"),
            "DeploymentDoesNotExistException",
        ),
        (
            // The format is the one the service MINTS: `d-` and nine characters of
            // its uppercase-alphanumeric alphabet, so a shorter suffix is not an
            // id at all rather than an id nobody has.
            "four characters",
            Some("d-ZZZZ"),
            "InvalidDeploymentIdException",
        ),
        (
            "unknown, digits",
            Some("d-000000000"),
            "DeploymentDoesNotExistException",
        ),
    ];

    for (case, deployment_id, expected) in cases {
        let mut request = json!({
            "lifecycleEventHookExecutionId": "hook-execution",
            "status": "Succeeded",
        });
        if let Some(id) = deployment_id {
            request["deploymentId"] = json!(id);
        }
        let (status, body) = emulator
            .call("PutLifecycleEventHookExecutionStatus", request)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected {expected}, got {status} {body:?}"
        );
        if *expected == "InvalidDeploymentIdException" {
            let id = deployment_id.unwrap_or_default();
            assert_eq!(
                body["message"].as_str(),
                Some(invalid_deployment_id_message(id).as_str()),
                "case {case:?}: expected the clause naming the id, got {body:?}"
            );
        }
    }

    // The existing deployment is still untouched by every refusal above: it
    // accepts a report of its own.
    let _accepted = report_hook_result(
        &emulator,
        "existing deployment",
        &existing,
        "hook-execution",
        "Succeeded",
    )
    .await;
}

/// RULE: only the two terminal `LifecycleEventStatus` values a validation
/// function returns can be reported; every other value of the model's enum is
/// refused with `InvalidLifecycleEventHookExecutionStatusException`.
#[tokio::test]
async fn only_a_validation_result_can_be_reported() {
    let emulator = Emulator::new();
    let deployment_id = seeded_deployment(&emulator, "status-checked", "Lambda").await;

    // Every member of `LifecycleEventStatus`
    // (`smithy_query(query="shape",shape="LifecycleEventStatus")`) that a
    // validation function cannot report, plus an absent status. `Succeeded` and
    // `Failed` — the two the operation's own documentation says can be passed
    // successfully — are exercised below, each on its own deployment.
    let cases: &[(&str, Option<&str>)] = &[
        ("Pending", Some("Pending")),
        ("InProgress", Some("InProgress")),
        ("Skipped", Some("Skipped")),
        ("Unknown", Some("Unknown")),
        ("absent", None),
    ];

    for (case, reported) in cases {
        // Each row reports against its own hook execution, so a row is never
        // refused for being a second report of one hook execution.
        let mut request = json!({
            "deploymentId": deployment_id,
            "lifecycleEventHookExecutionId": format!("hook-{case}"),
        });
        if let Some(status) = reported {
            request["status"] = json!(status);
        }
        let (status, body) = emulator
            .call("PutLifecycleEventHookExecutionStatus", request)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLifecycleEventHookExecutionStatusException"),
            "case {case:?}: expected the status to be refused, got {status} {body:?}"
        );
    }

    // And the two reportable results are accepted, each on its own deployment.
    for reported in ["Succeeded", "Failed"] {
        let own = seeded_deployment(
            &emulator,
            &format!("status-{}", reported.to_lowercase()),
            "Lambda",
        )
        .await;
        let _accepted =
            report_hook_result(&emulator, reported, &own, "hook-execution", reported).await;
    }
}

/// RULE: a hook result is reported against a hook execution OF the deployment
/// named, so a `lifecycleEventHookExecutionId` that names none of its hook
/// executions is refused with `InvalidLifecycleEventHookExecutionIdException` —
/// an id past the number of hooks its compute platform runs, none at all, and
/// any id at all against a deployment whose platform runs its lifecycle events
/// through the CodeDeploy agent and so issues no hook execution id.
#[tokio::test]
async fn a_hook_execution_belongs_to_a_deployment_that_has_one() {
    let emulator = Emulator::new();

    // EC2/on-premises lifecycle events are run by the CodeDeploy agent, so such
    // a deployment issues no hook execution id and no id names a hook execution
    // of it.
    let server = seeded_deployment(&emulator, "agent-driven", "Server").await;
    let (status, body) = emulator
        .call(
            "PutLifecycleEventHookExecutionStatus",
            json!({
                "deploymentId": server,
                "lifecycleEventHookExecutionId": "hook-execution",
                "status": "Succeeded",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidLifecycleEventHookExecutionIdException"),
        "case Server: expected the hook execution id to be refused, got {status} {body:?}"
    );

    for (platform, hooks) in HOOK_COUNTS {
        let deployment_id = seeded_deployment(
            &emulator,
            &format!("bounded-{}", platform.to_lowercase()),
            platform,
        )
        .await;

        // An execution id is the hook execution's identity, so a request without
        // one names no hook execution.
        let (status, body) = emulator
            .call(
                "PutLifecycleEventHookExecutionStatus",
                json!({"deploymentId": deployment_id, "status": "Succeeded"}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLifecycleEventHookExecutionIdException"),
            "case {platform:?} without an execution id: expected it to be refused, \
             got {status} {body:?}"
        );

        // Every hook of the platform may report once. A report the emulator's
        // own completion clock beat answers `None`; that deployment can report
        // nothing more, so the row starts over on a fresh one rather than
        // asserting anything about wall-clock timing.
        let mut reported_all = deployment_id.clone();
        for attempt in 0..5 {
            let mut accepted = true;
            for hook in 0..*hooks {
                if report_hook_result(
                    &emulator,
                    &format!("{platform} hook {hook}"),
                    &reported_all,
                    &format!("hook-{hook}"),
                    "Succeeded",
                )
                .await
                .is_none()
                {
                    accepted = false;
                    break;
                }
            }
            if accepted {
                break;
            }
            assert!(
                attempt < 4,
                "case {platform:?}: expected one deployment to accept a report for each of its \
                 {hooks} hooks within five attempts"
            );
            reported_all = seeded_deployment(
                &emulator,
                &format!("bounded-{}-{attempt}", platform.to_lowercase()),
                platform,
            )
            .await;
        }

        // One past the platform's hook count names no hook execution of it. The
        // id is judged before the deployment's own progress, so this holds
        // however long the reports above took.
        let (status, body) = emulator
            .call(
                "PutLifecycleEventHookExecutionStatus",
                json!({
                    "deploymentId": reported_all,
                    "lifecycleEventHookExecutionId": format!("hook-{hooks}"),
                    "status": "Succeeded",
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidLifecycleEventHookExecutionIdException"),
            "case {platform:?} one hook too many: expected it to be refused, got {status} {body:?}"
        );
    }
}

/// RULE: a deployment that already reached a terminal status has no lifecycle
/// event left to report on, so a report against it is refused as already
/// completed.
#[tokio::test]
async fn a_completed_deployment_reports_no_further_hook_result() {
    let emulator = Emulator::new();
    let deployment_id = seeded_deployment(&emulator, "already-done", "Lambda").await;

    // The transition is owed by the clock, not by the poll; wait for it rather
    // than forcing it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let polled: Value = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await;
        if polled["deploymentInfo"]["status"].as_str() == Some("Succeeded") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case terminal deployment: expected the deployment to complete, got {polled:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let (status, body) = emulator
        .call(
            "PutLifecycleEventHookExecutionStatus",
            json!({
                "deploymentId": deployment_id,
                "lifecycleEventHookExecutionId": "hook-execution",
                "status": "Failed",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LifecycleEventAlreadyCompletedException"),
        "case terminal deployment: expected the report to be refused, got {status} {body:?}"
    );

    // And the refusal wrote nothing: the deployment is still the success the
    // clock owed it.
    let info = emulator
        .ok("GetDeployment", json!({"deploymentId": deployment_id}))
        .await;
    assert_eq!(
        info["deploymentInfo"]["status"].as_str(),
        Some("Succeeded"),
        "case terminal deployment: expected a refused report to change nothing, got {info:?}"
    );
}
