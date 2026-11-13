// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER an enumerated member's refusal lists the admitted
//! values in, over the one enumerated type of this service whose recorded refusals pin
//! that order on a set small enough to spell out: `ActionType`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`] and the envelope and clause
//! builders from [`super::constraint_refusal_messages`], this crate's ONE statement of
//! them.
//!
//! RULE: a value outside an enumerated member's set is refused with the standard
//! constraint report over that member's WHOLE admitted set, and the ORDER the set is
//! listed in belongs to the enumerated TYPE: the order this front door walks its own hash
//! table over the admitted values, which is neither the model's declaration order nor
//! lexicographic. The order is therefore the same whatever value was rejected, and two
//! members of one request each state their own type's order.
//!
//! The table varies what the rule generalises over: the shape of the rejected value (a
//! near-miss of an admitted value, the same value in another case, a value with the
//! separator changed, a long one, a single character), and a row that breaks TWO
//! enumerated members at once so the two orders are seen side by side.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    ACTION_TYPE_VALUES, SCHEDULE_AT_VALUES,
};
use service_common::validation_message::hashed_enum_order;

use super::constraint_refusal_messages::{clause, report};
use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// The order `ActionType`'s value set is published in, as the refusals of this service
/// state it. Spelled out here rather than derived, so a change to either the rule or the
/// values fails this suite — the model declares the type
/// `[SERVICE_SOFTWARE_UPDATE, JVM_HEAP_SIZE_TUNING, JVM_YOUNG_GEN_TUNING]`
/// (`smithy_query(query="shape",shape="ActionType")`) and codegen hands the walk them
/// sorted, so this is neither of those two orders.
const PUBLISHED_ACTION_TYPES: [&str; 3] = [
    "JVM_YOUNG_GEN_TUNING",
    "SERVICE_SOFTWARE_UPDATE",
    "JVM_HEAP_SIZE_TUNING",
];

/// A domain name that satisfies `DomainName`'s own constraints, so the only clauses a
/// refusal states are the ones the row is about.
const WELL_FORMED_DOMAIN: &str = "scheduled-order";

/// An `ActionID` the request carries: any string, since the member is a plain `String`
/// with no constraint of its own.
const SOME_ACTION_ID: &str = "action-1";

/// `Member must satisfy enum value set: [<values>]`, the framework's own requirement
/// sentence for an enumerated member.
fn value_set(stated: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", stated.join(", "))
}

/// `UpdateScheduledAction`, the one operation whose input declares an `ActionType`
/// member (`smithy_query(query="operation",operation="UpdateScheduledAction")`).
async fn update(emulator: &Emulator, action_type: &str, schedule_at: &str) -> Answer {
    emulator
        .call(
            "PUT",
            &format!(
                "{PREFIX}/opensearch/domain/{WELL_FORMED_DOMAIN}/scheduledAction/update"
            ),
            Some(json!({
                "ActionID": SOME_ACTION_ID,
                "ActionType": action_type,
                "ScheduleAt": schedule_at,
            })),
        )
        .await
}

/// The refusal message of an answer, with the case named on failure.
fn message(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a refusal message, got {:?}", answer.body))
        .to_owned()
}

/// The values a bracketed set lists, in the order it lists them, read back out of
/// `message` after `member`'s clause.
fn stated_set(message: &str, case: &str) -> Vec<String> {
    let opened = message
        .split_once("Member must satisfy enum value set: [")
        .unwrap_or_else(|| panic!("case {case:?}: expected an enum value set, got {message:?}"))
        .1;
    let set = opened
        .split_once(']')
        .unwrap_or_else(|| panic!("case {case:?}: expected a closed value set, got {message:?}"))
        .0;
    set.split(", ").map(str::to_owned).collect()
}

#[tokio::test]
async fn a_refused_action_type_lists_the_admitted_set_in_the_order_the_service_publishes_it() {
    let emulator = Emulator::new();

    // One row per shape of rejected value: a near-miss of an admitted value, the same
    // value in another case, the separator changed, a long value, and one character.
    let rejected = [
        "NOT_A_REAL_ACTION_TYPE",
        "SERVICE_SOFTWARE_UPDATES",
        "service_software_update",
        "JVM-HEAP-SIZE-TUNING",
        "JVM_YOUNG_GEN_TUNING_AND_THEN_SOME_MORE_WORDS_NOBODY_DECLARED",
        "x",
    ];

    for value in rejected {
        let answer = update(&emulator, value, "NOW").await;
        let said = message(&answer, value);
        let expected = report(&[clause(
            value,
            "actionType",
            &value_set(&PUBLISHED_ACTION_TYPES),
        )]);
        assert_eq!(
            said, expected,
            "case {value:?}: expected the refusal {expected:?}, got {said:?}"
        );

        // The set stated is the member's WHOLE admitted set — the model's values, no
        // more and no less — whatever order it is listed in.
        let mut stated = stated_set(&said, value);
        stated.sort_unstable();
        let mut admitted: Vec<String> = ACTION_TYPE_VALUES.iter().map(|v| (*v).to_owned()).collect();
        admitted.sort_unstable();
        assert_eq!(
            stated, admitted,
            "case {value:?}: expected the whole declared set, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn each_enumerated_member_of_one_request_states_its_own_types_order() {
    let emulator = Emulator::new();

    // Both enumerated members of one request are outside their sets, and each states its
    // OWN type's order: `ActionType` as spelled out above, `ScheduleAt` as the same rule
    // lists its own three values.
    let case = "both enumerated members outside their sets";
    let answer = update(&emulator, "NOT_A_REAL_ACTION_TYPE", "SOMETIME_SOON").await;
    let said = message(&answer, case);

    let action_clause = clause(
        "NOT_A_REAL_ACTION_TYPE",
        "actionType",
        &value_set(&PUBLISHED_ACTION_TYPES),
    );
    let published_schedule = hashed_enum_order(SCHEDULE_AT_VALUES);
    let schedule_clause = clause(
        "SOMETIME_SOON",
        "scheduleAt",
        &value_set(&published_schedule),
    );
    assert!(
        said.contains(&action_clause),
        "case {case:?}: expected the clause {action_clause:?}, got {said:?}"
    );
    assert!(
        said.contains(&schedule_clause),
        "case {case:?}: expected the clause {schedule_clause:?}, got {said:?}"
    );
    assert!(
        said.starts_with("2 validation errors detected: "),
        "case {case:?}: expected a two-clause report, got {said:?}"
    );
}

#[tokio::test]
async fn an_admitted_action_type_states_no_value_set_at_all() {
    let emulator = Emulator::new();

    // Every value the type admits passes the boundary, so what the caller is answered is
    // about the domain the request names and never about the member's value set. The
    // domain does not exist, which is what such a request is refused for.
    for admitted in ACTION_TYPE_VALUES {
        let answer = update(&emulator, admitted, "NOW").await;
        let body: &Value = &answer.body;
        let said = body["message"].as_str().unwrap_or_default();
        assert!(
            !said.contains("Member must satisfy enum value set"),
            "case {admitted:?}: an admitted value states no value set, got {body:?}"
        );
        assert_ne!(
            error_code(&answer),
            "ValidationException",
            "case {admitted:?}: an admitted value is not a constraint refusal, got {body:?}"
        );
    }
}
