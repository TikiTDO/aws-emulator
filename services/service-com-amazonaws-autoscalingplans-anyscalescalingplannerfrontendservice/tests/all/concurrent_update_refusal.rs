// SPDX-License-Identifier: CC0-1.0

//! The refusal a mutation that overlaps a plan's in-flight transition is answered with,
//! driven end-to-end through every operation that can raise it.
//!
//! One rule is pinned here: a mutating operation that addresses a plan which still has a
//! lifecycle transition in flight is refused as a CLIENT error — HTTP 400, error code
//! `ConcurrentUpdateException`, and AWS's fixed sentence for that error class, which
//! interpolates nothing from the request or from the stored plan.
//!
//! The status is recorded behaviour, not modelled: the model classifies
//! `ConcurrentUpdateException` as a server fault with `httpError` 500, and the live
//! service answers the refusal at 400 instead, so the recording decides. The rows below
//! therefore span what the rule generalises over — WHICH operation was called, WHICH
//! transient status the plan was caught in, and the plan's own name and stored status,
//! neither of which may reach the answer — rather than the single recorded pair of
//! overlapping updates.
//!
//! The harness (router, dispatch, error-body readers, the due-instant controls) is
//! reused from [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::wire_refusals::{
    CONCURRENT_UPDATE_CODE, CONCURRENT_UPDATE_MESSAGE, CONCURRENT_UPDATE_STATUS,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    ONLY_VERSION, STATUS_ACTIVE, STATUS_CREATION_IN_PROGRESS, STATUS_DELETION_IN_PROGRESS,
    STATUS_UPDATE_IN_PROGRESS,
};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// Every operation that can raise this refusal, with the request that reaches it.
///
/// Source: `smithy_query(query="operation",operation="UpdateScalingPlan,
/// DeleteScalingPlan")` — the two mutating operations that address one stored plan by
/// `ScalingPlanName` + `ScalingPlanVersion`, and the only two that bind this error and can
/// find the plan busy. `UpdateScalingPlan` appears twice, because what else the request
/// carries must not change the refusal.
fn mutations(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "UpdateScalingPlan",
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": name,
                "ScalingPlanVersion": ONLY_VERSION,
                "ScalingInstructions": simple_instructions(),
            }),
        ),
        (
            "DeleteScalingPlan",
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
        ),
    ]
}

/// The transient statuses a plan can be caught in, each with the call that puts it there.
///
/// Source: `smithy_query(query="shape",shape="ScalingPlanStatusCode")` — the three
/// in-progress values, mirrored in `src/ops/types/shared_types.rs` as
/// `SCALING_PLAN_STATUS_CODE_VALUES`. `CreationInProgress` is reached by the create
/// itself, so it carries no starter.
const TRANSIENT_STATUSES: [(&str, Option<&str>); 3] = [
    (STATUS_CREATION_IN_PROGRESS, None),
    (STATUS_UPDATE_IN_PROGRESS, Some("UpdateScalingPlan")),
    (STATUS_DELETION_IN_PROGRESS, Some("DeleteScalingPlan")),
];

/// Plan names that differ in length and character content (`ScalingPlanName` is
/// len 1..128, pattern `^[\p{Print}&&[^|:/]]+$`), so a name that reached the answer would
/// show up as a difference between rows.
fn names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "busy plan".to_string()),
        ("one character", "x".to_string()),
        ("spaces and punctuation", "plan #2 (staging)".to_string()),
        ("the longest name allowed", "d".repeat(128)),
    ]
}

/// Store one plan, put it into `status`, and hold the owed transition open so the
/// transient window is deterministic rather than raced against the span.
async fn plan_caught_in(emulator: &Emulator, name: &str, source: usize, status: &str) {
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": name,
                "ApplicationSource": stack_source(source),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    let starter = TRANSIENT_STATUSES
        .iter()
        .find(|(value, _)| *value == status)
        .unwrap_or_else(|| panic!("no transient status named {status:?}"))
        .1;
    if let Some(starter) = starter {
        emulator.make_all_due();
        emulator
            .ok(
                starter,
                json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
            )
            .await;
    }
    emulator.hold_pending(name);
}

/// The `StatusCode` the emulator reports for one plan, or `None` when it holds no such
/// plan.
async fn reported_status(emulator: &Emulator, name: &str) -> Option<String> {
    let described = emulator
        .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [name] }))
        .await;
    described["ScalingPlans"]
        .as_array()
        .and_then(|plans| plans.first())
        .and_then(|plan| plan["StatusCode"].as_str())
        .map(ToString::to_string)
}

#[tokio::test]
async fn an_overlapping_mutation_is_refused_as_a_client_error_in_awss_words() {
    // Every answer observed, so the rule's "one fixed refusal" half is checked across
    // rows and not only row by row.
    let mut answers: Vec<(String, u16, String, String)> = Vec::new();

    for (status, _) in TRANSIENT_STATUSES {
        for (name_label, name) in names() {
            // A fresh emulator per row: the plan is held mid-transition, so a row must not
            // inherit another row's held plan or its claimed application source.
            let emulator = Emulator::new();
            plan_caught_in(&emulator, &name, 1, status).await;

            for (operation, request) in mutations(&name) {
                let case = format!("{operation}/{status}/{name_label}");
                let (http_status, body) = emulator.call(operation, &request).await;
                assert_eq!(
                    http_status, CONCURRENT_UPDATE_STATUS,
                    "case {case}: expected HTTP {CONCURRENT_UPDATE_STATUS}, got {http_status} \
                     with {body}"
                );
                assert!(
                    (400..500).contains(&http_status),
                    "case {case}: the refusal must be in the client-error class, got \
                     {http_status}"
                );
                assert_eq!(
                    error_code(&body),
                    CONCURRENT_UPDATE_CODE,
                    "case {case}: expected error code {CONCURRENT_UPDATE_CODE}, got {body}"
                );
                let message = error_message(&body);
                assert_eq!(
                    message, CONCURRENT_UPDATE_MESSAGE,
                    "case {case}: expected {CONCURRENT_UPDATE_MESSAGE:?}, got {message:?}"
                );
                // The sentence is fixed text: nothing of the request or of the stored
                // record may appear in it.
                assert!(
                    !message.contains(name.as_str()) && !message.contains(status),
                    "case {case}: the refusal must interpolate neither the plan name nor \
                     its status, got {message:?}"
                );
                answers.push((
                    case,
                    http_status,
                    error_code(&body).to_string(),
                    message.to_string(),
                ));
            }

            // A refused mutation wrote nothing: the plan is still in the status it was
            // caught in, with its transition still owed.
            let reported = reported_status(&emulator, &name).await;
            assert_eq!(
                reported.as_deref(),
                Some(status),
                "case {status}/{name_label}: a refused mutation must leave the status \
                 alone, got {reported:?}"
            );
        }
    }

    let (first_case, first_status, first_code, first_message) =
        answers.first().cloned().expect("at least one row");
    for (case, http_status, code, message) in &answers {
        assert_eq!(
            (*http_status, code.as_str(), message.as_str()),
            (first_status, first_code.as_str(), first_message.as_str()),
            "case {case}: one error class is one answer; it must match case {first_case}"
        );
    }
}

#[tokio::test]
async fn the_plan_a_refused_mutation_addressed_is_still_writable_once_it_settles() {
    let emulator = Emulator::new();
    let name = "contended plan";
    plan_caught_in(&emulator, name, 1, STATUS_UPDATE_IN_PROGRESS).await;

    // The control: the very call that was refused succeeds once the owed transition has
    // been made due, so the refusal is about the overlap and not about the operation.
    let (refused_status, refused_body) = emulator
        .call(
            "UpdateScalingPlan",
            &json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
        )
        .await;
    assert_eq!(
        (refused_status, error_code(&refused_body)),
        (CONCURRENT_UPDATE_STATUS, CONCURRENT_UPDATE_CODE),
        "case overlapping update: expected the concurrent-update refusal, got \
         {refused_status} with {refused_body}"
    );

    emulator.make_all_due();
    emulator
        .ok(
            "UpdateScalingPlan",
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": ONLY_VERSION }),
        )
        .await;
    emulator.make_all_due();

    // Read the plan back: the refusal damaged nothing — it settles `Active` at the one
    // modelled version, still carrying the instruction it was created with.
    let described = emulator
        .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [name] }))
        .await;
    let plan = &described["ScalingPlans"][0];
    assert_eq!(
        plan["StatusCode"].as_str(),
        Some(STATUS_ACTIVE),
        "case settled plan: expected {STATUS_ACTIVE}, got {described}"
    );
    assert_eq!(
        plan["ScalingPlanVersion"].as_i64(),
        Some(ONLY_VERSION),
        "case settled plan: expected version {ONLY_VERSION}, got {described}"
    );
    assert_eq!(
        plan["ScalingInstructions"].as_array().map(Vec::len),
        Some(simple_instructions().len()),
        "case settled plan: the instruction in force must survive the refusal, got \
         {described}"
    );
}
