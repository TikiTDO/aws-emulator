// SPDX-License-Identifier: CC0-1.0

//! The refusals a plan execution's own arguments and its own state produce, and the exact
//! sentence each carries.
//!
//! THE RULES UNDER TEST:
//!
//! 1. **The execution holding a plan.** `DeletePlan` is refused `IllegalStateException`
//!    (400) while an execution of the plan is still running, and the refusal names the
//!    plan the request addressed and that execution as a `<region>: <id>` pair — the
//!    Region once, and the BARE execution id, which is the part an `ExecutionId` carries
//!    after its `<region>/` prefix, so a client can feed the pair straight back to
//!    `GetPlanExecution`.
//! 2. **A post-recovery execution's `recoveryExecutionId`.** `postRecovery` follows a
//!    recovery execution named by `recoveryExecutionId`, which `RecoveryExecutionId`
//!    spells `region/executionId`; a `postRecovery` start that names none, or names a
//!    value of another form, is refused `IllegalArgumentException` (400) with the one
//!    sentence that names the plan addressed and the form to supply. The other actions
//!    need no such id and are served without one.
//!
//! The tables span what each rule generalises over: the Region the execution runs in and
//! the plan it belongs to for the first, and the form of the value supplied (absent, and
//! every way the documented form can be broken) for the second.

use serde_json::json;
use service_com_amazonaws_arcregionswitch_arcregionswitch::state;

use crate::switch_support::{
    approval_step, error_code, error_message, plan_request, poll_until_state, role_arn,
    start_execution, workflow_of, Emulator,
};

/// RULE 1: the active-execution refusal renders the Region and the bare execution id once
/// each, whatever Region the execution runs in.
#[tokio::test]
async fn the_active_execution_refusal_names_the_region_and_the_bare_id() {
    // The rows differ along what the rule generalises over: which of the plan's two
    // Regions the execution targets, and which plan it is.
    let rows = [
        ("the second of the plan's Regions", "us-west-2", "held-west"),
        ("the first of the plan's Regions", "us-east-1", "held-east"),
    ];
    for (case, target_region, name) in rows {
        let emulator = Emulator::new();
        let plan_arn = crate::switch_support::plan_with_steps(
            &emulator,
            case,
            name,
            vec![approval_step("Approve")],
        )
        .await;
        let execution_id = start_execution(&emulator, case, &plan_arn, target_region).await;
        poll_until_state(
            &emulator,
            case,
            &plan_arn,
            &execution_id,
            &["pendingManualApproval"],
        )
        .await;

        // The id on the wire carries the Region, and the refusal states that Region once:
        // the pair is `<region>: <bare id>`.
        let (region, bare_id) = execution_id
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case:?}: {execution_id:?} is not region/id"));
        assert_eq!(
            region, target_region,
            "case {case:?}: expected the execution id to carry {target_region}, got \
             {execution_id}"
        );

        let (status, refused) = emulator.call("DeletePlan", &json!({"arn": plan_arn})).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "IllegalStateException"),
            "case {case:?}: expected 400 IllegalStateException, got {status} with {refused}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message,
            format!("Active executions found for {plan_arn}: {region}: {bare_id}"),
            "case {case:?}: expected the Region and the bare id once each, got {message:?}"
        );
        assert_eq!(
            message.matches(target_region).count(),
            1,
            "case {case:?}: expected the Region named once, got {message:?}"
        );

        // The plan is still there, with that execution still holding it.
        let (status, plan) = emulator.call("GetPlan", &json!({"arn": plan_arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the plan must survive the refusal, got {status} with {plan}"
        );
    }
}

/// One row of the `recoveryExecutionId` table: the value a `postRecovery` start carries,
/// and whether the documented form admits it.
struct Supplied {
    case: &'static str,
    /// The value the request carries; `None` carries the member not at all.
    value: Option<&'static str>,
    /// Whether `RecoveryExecutionId`'s own pattern admits the value — the boundary
    /// validator refuses one it does not before the handler runs, so only the values it
    /// admits reach the operation's own check.
    well_formed: bool,
}

/// Values drawn from `RecoveryExecutionId`'s pattern
/// `^[a-z]{2}(-[a-z]+)+-[0-9]+/[0-9a-fA-F]{16}$`
/// (`smithy_query(query="shape", shape="RecoveryExecutionId", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`),
/// and every way of breaking it: no value at all, no Region part, no id part, an id of the
/// wrong length, an id outside hexadecimal, a Region of another shape.
const SUPPLIED: &[Supplied] = &[
    Supplied {
        case: "no recoveryExecutionId at all",
        value: None,
        well_formed: false,
    },
    Supplied {
        case: "a bare execution id, with no Region part",
        value: Some("0123456789abcdef"),
        well_formed: false,
    },
    Supplied {
        case: "a Region with no id after it",
        value: Some("us-west-2/"),
        well_formed: false,
    },
    Supplied {
        case: "an id one digit short",
        value: Some("us-west-2/0123456789abcde"),
        well_formed: false,
    },
    Supplied {
        case: "an id outside hexadecimal",
        value: Some("us-west-2/0123456789abcdeg"),
        well_formed: false,
    },
    Supplied {
        case: "a Region of another shape",
        value: Some("usw2/0123456789abcdef"),
        well_formed: false,
    },
    Supplied {
        case: "the documented form",
        value: Some("us-west-2/0123456789ABCDEF"),
        well_formed: true,
    },
    Supplied {
        case: "the documented form, a multi-word Region",
        value: Some("ap-southeast-4/00000000000000ff"),
        well_formed: true,
    },
];

/// RULE 2 (the handler's own check): the one sentence covers a `postRecovery` start whose
/// `recoveryExecutionId` is missing AND one whose value is of another form, and no other
/// action is asked for one.
///
/// Asserted against the crate's own reading of the rule as well as over the wire, because
/// `RecoveryExecutionId` carries a pattern: the framework's constraint walk refuses a
/// value the pattern does not admit before any handler runs, so the malformed rows cannot
/// reach the operation from a client.
#[test]
fn the_recovery_execution_id_rule_covers_missing_and_malformed() {
    let plan_arn = format!("arn:aws:arc-region-switch::{}:plan/post:aa12bb", "000000000000");
    let expected = format!(
        "The recoveryExecutionId is missing or incorrectly formatted for the {plan_arn}. \
         Please provide the recoveryExecutionId in the format 'region/executionId' for Post \
         Recovery execution."
    );
    for Supplied {
        case,
        value,
        well_formed,
    } in SUPPLIED
    {
        let rejection = state::recovery_execution_id_rejection(&plan_arn, "postRecovery", *value);
        if *well_formed {
            assert!(
                rejection.is_none(),
                "case {case:?}: expected the documented form admitted, got {rejection:?}"
            );
        } else {
            assert_eq!(
                rejection.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the service's one sentence, got {rejection:?}"
            );
        }
        // The other actions shift traffic and follow no recovery execution, so they are
        // never asked for one.
        for action in ["activate", "deactivate"] {
            let asked = state::recovery_execution_id_rejection(&plan_arn, action, *value);
            assert!(
                asked.is_none(),
                "case {case:?}: {action} needs no recoveryExecutionId, got {asked:?}"
            );
        }
    }
}

/// RULE 2 over the wire: a `postRecovery` start carrying no `recoveryExecutionId` is the
/// modelled `IllegalArgumentException` naming the plan and the form, one carrying a value
/// of the documented form is served, and `activate` needs none.
#[tokio::test]
async fn a_post_recovery_start_without_a_recovery_execution_id_is_refused() {
    let emulator = Emulator::new();
    let case = "a plan with an activate and a postRecovery workflow";
    let plan_arn = crate::switch_support::create_plan(
        &emulator,
        case,
        &plan_request(
            "post-recovery",
            ["us-east-1", "us-west-2"],
            &role_arn("ArcRegionSwitchRole"),
            json!([
                workflow_of("activate", vec![approval_step("Approve")]),
                workflow_of("postRecovery", vec![approval_step("Confirm")]),
            ]),
        ),
    )
    .await;

    let (status, refused) = emulator
        .call(
            "StartPlanExecution",
            &json!({
                "planArn": plan_arn,
                "targetRegion": "us-west-2",
                "action": "postRecovery",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "IllegalArgumentException"),
        "case {case:?}: expected 400 IllegalArgumentException, got {status} with {refused}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "The recoveryExecutionId is missing or incorrectly formatted for the {plan_arn}. \
             Please provide the recoveryExecutionId in the format 'region/executionId' for \
             Post Recovery execution."
        ),
        "case {case:?}: expected the service's own sentence, got {refused}"
    );

    // Nothing was started by the refused call.
    let (status, page) = emulator
        .call("ListPlanExecutions", &json!({"planArn": plan_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPlanExecutions expected 200, got {status} with {page}"
    );
    assert_eq!(
        page["items"],
        json!([]),
        "case {case:?}: a refused start must record no execution, got {}",
        page["items"]
    );

    // The same request carrying an id of the documented form is served, and so is an
    // `activate` start, which is asked for no id at all.
    let (status, started) = emulator
        .call(
            "StartPlanExecution",
            &json!({
                "planArn": plan_arn,
                "targetRegion": "us-west-2",
                "action": "postRecovery",
                "recoveryExecutionId": "us-east-1/0123456789abcdef",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a well-formed postRecovery start expected 200, got {status} with \
         {started}"
    );
    let execution_id = started["executionId"].as_str().unwrap_or_default().to_string();
    let execution = poll_until_state(
        &emulator,
        case,
        &plan_arn,
        &execution_id,
        &["pendingManualApproval", "failed", "completed"],
    )
    .await;
    assert_eq!(
        execution["recoveryExecutionId"],
        json!("us-east-1/0123456789abcdef"),
        "case {case:?}: expected the execution to carry the id it was started with, got \
         {execution}"
    );
}

/// The crate's fixed duplicate-region text is a constant of the service, not a template:
/// it is the same sentence whichever Region a request names twice.
#[test]
fn the_duplicate_region_message_is_fixed_text() {
    // The sentence itself, spelled out here, so what the service carries is anchored to
    // the text and not to itself — trailing space included.
    assert_eq!(
        state::DUPLICATE_REGIONS_MESSAGE,
        "There are duplicate regions defined in the plan. Please remove the duplicated \
         regions. ",
        "expected the service's duplicate-region sentence, got {:?}",
        state::DUPLICATE_REGIONS_MESSAGE
    );
    // Regions drawn from `Region`'s own pattern `^[a-z]{2}-[a-z-]+-\d+$`.
    for region in ["us-west-2", "eu-central-1", "ap-southeast-4", "us-gov-west-1"] {
        let violation = state::region_pair_violation(&[region.to_string(), region.to_string()]);
        assert_eq!(
            violation.as_deref(),
            Some(state::DUPLICATE_REGIONS_MESSAGE),
            "case {region:?}: expected the service's fixed text, got {violation:?}"
        );
        let pair = state::region_pair_violation(&[region.to_string(), "us-east-1".to_string()]);
        assert!(
            region == "us-east-1" || pair.is_none(),
            "case {region:?}: two different Regions are a legal pair, got {pair:?}"
        );
    }
}
