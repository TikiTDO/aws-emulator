// SPDX-License-Identifier: CC0-1.0

//! The admission rule both mutating operations are admitted by, driven end-to-end
//! through each of them.
//!
//! `UpdateScalingPlan` and `DeleteScalingPlan` resolve the plan they are about to write
//! the same way: the name and version are looked up first, so a pair that names no
//! stored plan is refused as not found, and a plan that IS stored but still has a
//! lifecycle transition in flight is refused as a concurrent update — "You cannot update
//! a scaling plan if it is in the process of being created, updated, or deleted". Each
//! operation answers the not-found refusal in the shape it models, naming the coordinate
//! the request asked about; the overlapping-mutation refusal is one fixed answer both
//! share (see [`crate::concurrent_update_refusal`] for its own table).
//!
//! Every row below is driven through BOTH operations, so the one rule is exercised from
//! each of the two call sites, including the ranking between its two refusals and the
//! `UpdateScalingPlan`-only application-source rule that sits under them.
//!
//! The harness (router, dispatch, error-body readers, the due-instant controls) is
//! reused from [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::wire_refusals::{
    CONCURRENT_UPDATE_MESSAGE, CONCURRENT_UPDATE_STATUS,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    ONLY_VERSION, STATUS_CREATION_IN_PROGRESS, STATUS_DELETION_IN_PROGRESS,
    STATUS_UPDATE_IN_PROGRESS,
};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The operations the rule admits.
///
/// Source: `smithy_query(query="operation",
/// operation="UpdateScalingPlan,DeleteScalingPlan")` — both bind
/// `ObjectNotFoundException` (httpError 400) and `ConcurrentUpdateException`.
const MUTATING_OPERATIONS: [&str; 2] = ["UpdateScalingPlan", "DeleteScalingPlan"];

/// The not-found sentence the rule requires for a requested name and version.
fn expected_not_found(name: &str, version: i64) -> String {
    format!("No scaling plan found for scaling plan name: {name}, version: {version}")
}

/// The identifying members both operations carry.
fn identifier(name: &str, version: i64) -> Value {
    json!({ "ScalingPlanName": name, "ScalingPlanVersion": version })
}

/// Create one plan over its own application source and leave it settled and `Active`.
async fn active_plan(emulator: &Emulator, name: &str, source: usize) {
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
    emulator.make_all_due();
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
async fn a_plan_with_a_transition_in_flight_refuses_every_mutation_in_awss_words() {
    let emulator = Emulator::new();

    // Rows: every transient status a plan can be caught in, paired with how the plan got
    // there, and with names that differ in length and character content (`ScalingPlanName`
    // is len 1..128, pattern ^[\p{Print}&&[^|:/]]+$) because the name is substituted into
    // the refusal.
    let long_name = "d".repeat(128);
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "being created",
            "creating plan".to_string(),
            STATUS_CREATION_IN_PROGRESS,
        ),
        (
            "being updated",
            "updating plan #2 (staging)".to_string(),
            STATUS_UPDATE_IN_PROGRESS,
        ),
        ("being deleted", long_name, STATUS_DELETION_IN_PROGRESS),
    ];

    for (source, (label, name, status)) in rows.into_iter().enumerate() {
        // Put the plan into `status` and hold the owed transition open, so the transient
        // window is deterministic rather than raced against the span.
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(source + 1),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        if status != STATUS_CREATION_IN_PROGRESS {
            emulator.make_all_due();
            let starter = if status == STATUS_UPDATE_IN_PROGRESS {
                "UpdateScalingPlan"
            } else {
                "DeleteScalingPlan"
            };
            emulator.ok(starter, identifier(&name, ONLY_VERSION)).await;
        }
        emulator.hold_pending(&name);

        for operation in MUTATING_OPERATIONS {
            let (http_status, body) = emulator
                .call(operation, &identifier(&name, ONLY_VERSION))
                .await;
            assert_eq!(
                http_status, CONCURRENT_UPDATE_STATUS,
                "case {operation}/{label}: expected HTTP {CONCURRENT_UPDATE_STATUS}, got \
                 {http_status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConcurrentUpdateException",
                "case {operation}/{label}: expected a ConcurrentUpdateException, got {body}"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, CONCURRENT_UPDATE_MESSAGE,
                "case {operation}/{label}: expected {CONCURRENT_UPDATE_MESSAGE:?}, got \
                 {actual:?}"
            );
        }

        // A refused mutation wrote nothing: the plan is still in the status it was caught
        // in, with its transition still owed.
        let reported = reported_status(&emulator, &name).await;
        assert_eq!(
            reported.as_deref(),
            Some(status),
            "case {label}: a refused mutation must leave the status alone, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_name_and_version_that_name_no_plan_refuse_every_mutation_in_awss_words() {
    let emulator = Emulator::new();
    active_plan(&emulator, "stored plan", 1).await;

    // Rows probe both halves of the lookup: an absent NAME (plain, one character, with
    // punctuation, at the 128-character bound) and a stored name at a VERSION nothing is
    // stored under (`ScalingPlanVersion` is a long, so magnitude and sign vary too).
    let name_at_bound = "n".repeat(128);
    let rows: Vec<(&str, String, i64)> = vec![
        ("absent plain name", "absent plan".to_string(), ONLY_VERSION),
        ("absent one-character name", "x".to_string(), ONLY_VERSION),
        (
            "absent name with punctuation",
            "absent plan #7 (eu)".to_string(),
            ONLY_VERSION,
        ),
        (
            "absent name at the 128-character bound",
            name_at_bound,
            ONLY_VERSION,
        ),
        ("stored name, next version", "stored plan".to_string(), 2),
        (
            "stored name, very large version",
            "stored plan".to_string(),
            9_007_199_254_740_993,
        ),
        ("stored name, version zero", "stored plan".to_string(), 0),
        ("stored name, negative version", "stored plan".to_string(), -1),
    ];

    for (label, name, version) in rows {
        for operation in MUTATING_OPERATIONS {
            let (http_status, body) = emulator.call(operation, &identifier(&name, version)).await;
            assert_eq!(
                http_status, 400,
                "case {operation}/{label}: expected HTTP 400, got {http_status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ObjectNotFoundException",
                "case {operation}/{label}: expected an ObjectNotFoundException, got {body}"
            );
            let expected = expected_not_found(&name, version);
            let actual = error_message(&body);
            assert_eq!(
                actual, expected,
                "case {operation}/{label}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The control: the stored plan at the version it has is admitted by the same rule,
    // so the refusals above answer the lookup and not the operations themselves.
    let reported = reported_status(&emulator, "stored plan").await;
    assert_eq!(
        reported.as_deref(),
        Some("Active"),
        "case control: the stored plan must still be Active, got {reported:?}"
    );
    emulator
        .ok(
            "UpdateScalingPlan",
            identifier("stored plan", ONLY_VERSION),
        )
        .await;
    emulator.make_all_due();
    emulator
        .ok(
            "DeleteScalingPlan",
            identifier("stored plan", ONLY_VERSION),
        )
        .await;
}

#[tokio::test]
async fn the_admission_refusals_outrank_the_application_source_rule() {
    let emulator = Emulator::new();
    active_plan(&emulator, "claimant", 1).await;
    active_plan(&emulator, "in flight", 2).await;
    emulator
        .ok("UpdateScalingPlan", identifier("in flight", ONLY_VERSION))
        .await;
    emulator.hold_pending("in flight");

    // `UpdateScalingPlan` also refuses a request that claims an application source
    // another plan covers, but only AFTER the plan the request addresses has been
    // resolved: a request that breaks both rules is answered about its own plan.
    let covered = stack_source(1);
    let cases: Vec<(&str, &str, i64, u16, &str, String)> = vec![
        (
            "absent plan, covered source",
            "absent plan",
            ONLY_VERSION,
            400,
            "ObjectNotFoundException",
            expected_not_found("absent plan", ONLY_VERSION),
        ),
        (
            "stored plan, unmodelled version, covered source",
            "in flight",
            2,
            400,
            "ObjectNotFoundException",
            expected_not_found("in flight", 2),
        ),
        (
            "plan in flight, covered source",
            "in flight",
            ONLY_VERSION,
            CONCURRENT_UPDATE_STATUS,
            "ConcurrentUpdateException",
            CONCURRENT_UPDATE_MESSAGE.to_string(),
        ),
    ];

    for (label, name, version, expected_status, expected_code, expected_message) in cases {
        let mut request = identifier(name, version);
        request["ApplicationSource"] = covered.clone();
        let (http_status, body) = emulator.call("UpdateScalingPlan", &request).await;
        assert_eq!(
            http_status, expected_status,
            "case {label}: expected HTTP {expected_status}, got {http_status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {label}: expected error code {expected_code}, got {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected_message,
            "case {label}: expected {expected_message:?}, got {actual:?}"
        );
    }

    // Both stored plans came through untouched: the covered source was never claimed.
    for (name, status) in [
        ("claimant", "Active"),
        ("in flight", STATUS_UPDATE_IN_PROGRESS),
    ] {
        let reported = reported_status(&emulator, name).await;
        assert_eq!(
            reported.as_deref(),
            Some(status),
            "case {name}: a refused update must leave the plan alone, got {reported:?}"
        );
    }
}
