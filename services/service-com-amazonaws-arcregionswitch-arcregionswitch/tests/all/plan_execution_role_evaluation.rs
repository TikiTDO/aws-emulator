// SPDX-License-Identifier: CC0-1.0

//! What a plan's EXECUTION ROLE does to its evaluation.
//!
//! THE RULE UNDER TEST: Region switch performs a plan's workflows as the plan's own
//! `executionRole`, so a plan whose role does not resolve to a role of the account cannot
//! execute at all. `GetPlanEvaluationStatus` reports that as an active `ResourceWarning`
//! naming the role's ARN, which makes the plan's `evaluationState` `actionRequired`; a
//! plan whose role the account does hold raises no such warning and, with nothing else
//! wrong with its configuration, evaluates `passed`. The verdict is derived from the
//! record on every read, so creating the role clears it and deleting the role raises it
//! again — and a role ARN of ANOTHER account shows nothing, because this account does not
//! hold it.
//!
//! The role is read through the IAM service's OWN store, in the same shared engine, so
//! what the evaluation sees is what `iam:CreateRole` wrote.
//!
//! The table spans what the rule generalises over: whether the account holds the role,
//! whose account the ARN names, and the path the role sits under.

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{
    EVALUATION_ACTION_REQUIRED, EVALUATION_PASSED, WARNING_STATUS_ACTIVE,
};

use crate::switch_support::{
    approval_step, create_plan, given_no_role, given_role, plan_request, role_arn, workflow_of,
    Emulator,
};

/// One row of the execution-role table.
struct Row {
    case: &'static str,
    /// The plan's name, so each row is its own plan.
    name: &'static str,
    /// Whether the account holds a role under the ARN the plan names.
    held: bool,
    /// The `executionRole` the plan carries.
    execution_role: fn() -> String,
    /// The evaluation state the plan then reports.
    state: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a role of this account the IAM store holds no role for",
        name: "role-absent",
        held: false,
        execution_role: || role_arn("ArcRegionSwitchRole"),
        state: EVALUATION_ACTION_REQUIRED,
    },
    Row {
        case: "the same role, created in IAM first",
        name: "role-present",
        held: true,
        execution_role: || role_arn("ArcRegionSwitchRole"),
        state: EVALUATION_PASSED,
    },
    Row {
        case: "another role of this account, created in IAM first",
        name: "role-other-present",
        held: true,
        execution_role: || role_arn("RegionSwitchOperator"),
        state: EVALUATION_PASSED,
    },
    Row {
        case: "a role of this account under a path, held",
        name: "role-path-present",
        held: true,
        execution_role: || role_arn("service-role/RegionSwitcher"),
        state: EVALUATION_PASSED,
    },
    Row {
        case: "a role of this account under a path, not held",
        name: "role-path-absent",
        held: false,
        execution_role: || role_arn("service-role/RegionSwitcher"),
        state: EVALUATION_ACTION_REQUIRED,
    },
    Row {
        case: "a role of ANOTHER account, which this one does not hold",
        name: "role-foreign",
        held: false,
        execution_role: || "arn:aws:iam::123456789012:role/ArcRegionSwitchRole".to_string(),
        state: EVALUATION_PASSED,
    },
];

/// The part of a role ARN after `:role/` — the path and name IAM holds the role under,
/// which is what [`given_role`] records it as.
fn role_name(role_arn: &str) -> &str {
    role_arn
        .split_once(":role/")
        .map(|(_, name)| name)
        .unwrap_or(role_arn)
}

/// Evaluate `plan_arn` and answer its state and its warnings.
async fn evaluate(emulator: &Emulator, case: &str, plan_arn: &str) -> (String, Vec<Value>) {
    let (status, body) = emulator
        .call("GetPlanEvaluationStatus", &json!({"planArn": plan_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPlanEvaluationStatus expected 200, got {status} with {body}"
    );
    (
        body["evaluationState"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        body["warnings"].as_array().cloned().unwrap_or_default(),
    )
}

/// Create a plan whose only step is a manual approval — a block that drives nothing
/// regional, so the WORKFLOWS raise no warning of their own and the row's verdict is
/// about the role alone.
async fn plan_performed_as(emulator: &Emulator, case: &str, name: &str, role: &str) -> String {
    create_plan(
        emulator,
        case,
        &plan_request(
            name,
            ["us-east-1", "us-west-2"],
            role,
            json!([workflow_of("activate", vec![approval_step("Approve")])]),
        ),
    )
    .await
}

#[tokio::test]
async fn a_plan_whose_execution_role_does_not_resolve_needs_action() {
    for Row {
        case,
        name,
        held,
        execution_role,
        state,
    } in ROWS
    {
        let emulator = Emulator::with_identity();
        let role = execution_role();
        if *held {
            given_role(&emulator.engine, role_name(&role));
        }
        let plan_arn = plan_performed_as(&emulator, case, name, &role).await;

        let (reported, warnings) = evaluate(&emulator, case, &plan_arn).await;
        assert_eq!(
            reported, *state,
            "case {case:?}: expected the evaluation state {state}, got {reported} with \
             {warnings:?}"
        );
        if *state == EVALUATION_PASSED {
            assert!(
                warnings.is_empty(),
                "case {case:?}: expected no warning, got {warnings:?}"
            );
            continue;
        }
        let about_the_role: Vec<&Value> = warnings
            .iter()
            .filter(|warning| {
                warning["warningMessage"]
                    .as_str()
                    .is_some_and(|message| message.contains(&role))
            })
            .collect();
        assert_eq!(
            about_the_role.len(),
            1,
            "case {case:?}: expected one warning naming the role {role}, got {warnings:?}"
        );
        let warning = about_the_role[0];
        assert_eq!(
            warning["warningStatus"],
            json!(WARNING_STATUS_ACTIVE),
            "case {case:?}: expected an active warning, got {warning}"
        );
        assert_eq!(
            warning["resourceArn"],
            json!(role),
            "case {case:?}: expected the warning to carry the role as its resource, got \
             {warning}"
        );
        assert!(
            warning["warningUpdatedTime"].is_number(),
            "case {case:?}: expected the warning stamped with the version's instant, got \
             {warning}"
        );
    }
}

/// RULE: the verdict is a function of the record and of IAM's store, so creating the role
/// clears the warning and deleting it again raises the same warning — no read changes it.
#[tokio::test]
async fn the_verdict_follows_the_role_the_account_holds() {
    let emulator = Emulator::with_identity();
    let case = "a plan whose role comes and goes";
    let role = role_arn("ArcRegionSwitchRole");
    let plan_arn = plan_performed_as(&emulator, case, "role-lifecycle", &role).await;

    // (what the account holds, the state the plan then reports)
    let stages: [(&str, bool, &str); 4] = [
        ("no role yet", false, EVALUATION_ACTION_REQUIRED),
        ("the role created", true, EVALUATION_PASSED),
        ("the role deleted again", false, EVALUATION_ACTION_REQUIRED),
        ("the role created once more", true, EVALUATION_PASSED),
    ];
    for (stage, held, expected) in stages {
        if held {
            given_role(&emulator.engine, role_name(&role));
        } else {
            given_no_role(&emulator.engine, role_name(&role));
        }
        let (reported, warnings) = evaluate(&emulator, stage, &plan_arn).await;
        assert_eq!(
            reported, expected,
            "case {stage:?}: expected {expected}, got {reported} with {warnings:?}"
        );
        // Reading twice reports the same thing: an evaluation changes nothing.
        let (again, warnings_again) = evaluate(&emulator, stage, &plan_arn).await;
        assert_eq!(
            (again, warnings_again),
            (reported, warnings),
            "case {stage:?}: a second read must report the same evaluation"
        );
    }
}
