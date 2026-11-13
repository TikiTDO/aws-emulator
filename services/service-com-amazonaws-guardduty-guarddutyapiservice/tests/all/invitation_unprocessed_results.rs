// SPDX-License-Identifier: CC0-1.0

//! The per-account REASON the two operations that end a pending invitation report in
//! `UnprocessedAccounts`.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handler are exercised
//! together.
//!
//! The rule under test, asserted over a TABLE of requests rather than one call:
//!
//! An account a request named that the call could not process is reported with the
//! FIXED sentence GuardDuty defines for THAT operation's refusal class — "a reason why
//! the account hasn't been processed" (`smithy_query(shape="UnprocessedAccount")`) —
//! and the two operations do not share one: declining is a TRANSITION of the
//! relationship with the account that invited this one, so an account whose
//! relationship holds no pending invitation to answer is reported as a transition that
//! cannot be made, while the discard enumerates every condition under which an
//! invitation cannot be deleted in one sentence and commits to none of them. Each
//! sentence is the SAME for every account, every count and every Region, and carries
//! nothing of the request or of the store: no account id, no invitation id, no
//! diagnostic of which condition the service found.
//!
//! The accounts whose invitation IS pending are processed, so the same call reports
//! only the others — which is what makes the reason a per-account report rather than a
//! refusal of the request.
//!
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), so a row can hold an
//! invitation from any account id in the space rather than only from the one account
//! id this emulator presents.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, router_over, Router};
use crate::invitation_count::{seeds, Seed};
use crate::invitation_listing::enabled_detector_in;
use crate::negative_class_rejections::seed_invitation;

/// The Region every row below calls through unless it names another.
const REGION: &str = "us-east-1";

/// GuardDuty's fixed `UnprocessedAccount.Result` for an account `DeclineInvitations`
/// had no pending invitation to answer: the relationship-status transition the request
/// asks for cannot be made.
pub(crate) const DECLINE_RESULT: &str = "Relationship status transition is invalid.";

/// GuardDuty's fixed `UnprocessedAccount.Result` for an account `DeleteInvitations`
/// had no invitation to discard: ONE sentence enumerating all three conditions, which
/// is deliberately not disambiguated.
pub(crate) const DELETE_RESULT: &str = "The request is rejected either because no such invitation exists, or the current account is still associated to the given master account, or the current account has not yet declined the invitation from the given master account.";

/// The two operations that answer an invitation without accepting it, each with the
/// path it is bound under (`smithy_query(operation="DeclineInvitations")` and
/// `smithy_query(operation="DeleteInvitations")`) and the fixed reason it reports.
const OPERATIONS: [(&str, &str, &str); 2] = [
    ("DeclineInvitations", "/invitation/decline", DECLINE_RESULT),
    ("DeleteInvitations", "/invitation/delete", DELETE_RESULT),
];

/// Well-formed account ids no seeded inbox holds an invitation from, differing along
/// the digit space: the low and high ends of the 12-digit `AccountId` space
/// (`len 12..12` — `smithy_query(shape="AccountId")`) and two mid-range ids.
const NEVER_INVITED: [&str; 4] = [
    "999999999999",
    "313131313131",
    "555555555555",
    "424242424242",
];

/// One request of the table: the accounts it names and the ones that must come back
/// reported, once each, in request order.
struct Row {
    case: &'static str,
    named: Vec<String>,
    reported: Vec<String>,
}

/// The requests every operation is driven through, against an inbox holding `inbox`.
///
/// The rows differ along what the rule generalises over: the COUNT of accounts named,
/// WHICH of them the inbox holds an invitation from, where the unanswerable ones sit in
/// the list, the ORDER they are named in, whether one is repeated, and whether the
/// request names an account that cannot have invited this one at all (its own id).
fn rows(inbox: &[Seed]) -> Vec<Row> {
    let own = ::engine::emulator_account_id().to_string();
    vec![
        Row {
            case: "one account that sent no invitation",
            named: vec![NEVER_INVITED[0].to_string()],
            reported: vec![NEVER_INVITED[0].to_string()],
        },
        Row {
            case: "the current account's own id, which cannot have invited itself",
            named: vec![own.clone()],
            reported: vec![own.clone()],
        },
        Row {
            case: "an unanswerable account before one the inbox holds",
            named: vec![NEVER_INVITED[1].to_string(), inbox[0].0.clone()],
            reported: vec![NEVER_INVITED[1].to_string()],
        },
        Row {
            case: "an unanswerable account between two the inbox holds",
            named: vec![
                inbox[1].0.clone(),
                NEVER_INVITED[2].to_string(),
                inbox[2].0.clone(),
            ],
            reported: vec![NEVER_INVITED[2].to_string()],
        },
        Row {
            case: "every unanswerable account, one of them named twice",
            named: vec![
                NEVER_INVITED[3].to_string(),
                NEVER_INVITED[0].to_string(),
                NEVER_INVITED[3].to_string(),
                own.clone(),
            ],
            reported: vec![
                NEVER_INVITED[3].to_string(),
                NEVER_INVITED[0].to_string(),
                own,
            ],
        },
        Row {
            case: "every account the inbox holds, so none is reported",
            named: inbox.iter().map(|(id, _, _)| id.clone()).collect(),
            reported: vec![],
        },
    ]
}

// ---------------------------------------------------------------------------
// The rule: one fixed reason per operation, for every unprocessable account.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_operation_reports_its_own_fixed_reason_for_every_account_it_cannot_process() {
    let inbox = seeds(3);

    for (operation, path, expected_result) in OPERATIONS {
        for row in rows(&inbox) {
            let case = format!("{operation}: {}", row.case);
            // One store per row, so one row's answers cannot explain another's.
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = router_over(Arc::clone(&engine));
            let detector_id = enabled_detector_in(&router, REGION).await;
            for (account_id, invitation_id, millis) in &inbox {
                seed_invitation(
                    &engine,
                    REGION,
                    &detector_id,
                    account_id,
                    invitation_id,
                    *millis,
                );
            }

            let unprocessed = answer_ok(&router, REGION, path, &row.named, &case).await;
            assert_eq!(
                reported_ids(&unprocessed),
                row.reported,
                "case {case:?}: expected exactly the accounts with nothing to answer reported, \
                 once each in request order, got {unprocessed:?}"
            );
            for entry in &unprocessed {
                assert_eq!(
                    entry["result"],
                    json!(expected_result),
                    "case {case:?}: expected this operation's fixed reason, got {entry:?}"
                );
            }
            for account_id in &row.reported {
                let result = result_of(&unprocessed, account_id);
                assert!(
                    !result.contains(account_id.as_str()),
                    "case {case:?}: the reason must carry nothing of the request, got {result:?}"
                );
            }
        }

        // The reason does not depend on WHY there was nothing to answer: an account
        // whose invitation this very operation already ended, and an endpoint that
        // holds no inbox at all, are reported with the same fixed sentence.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;
        for (account_id, invitation_id, millis) in &inbox {
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }
        let named = vec![inbox[0].0.clone()];
        let case = format!("{operation}: the first answer of a pending invitation");
        assert!(
            answer_ok(&router, REGION, path, &named, &case)
                .await
                .is_empty(),
            "case {case:?}: expected the pending invitation answered, so nothing reported"
        );
        for round in 2..=4 {
            let case = format!("{operation}: round {round} on an invitation already answered");
            let unprocessed = answer_ok(&router, REGION, path, &named, &case).await;
            assert_eq!(
                reported_ids(&unprocessed),
                named,
                "case {case:?}: expected the account reported once, got {unprocessed:?}"
            );
            assert_eq!(
                result_of(&unprocessed, &named[0]),
                expected_result,
                "case {case:?}: expected this operation's fixed reason, got {unprocessed:?}"
            );
        }

        // Another Region's endpoint serves no detector in this store, so its inbox
        // holds nothing and every named account is reported — with the same sentence,
        // not a Region-specific one.
        let case = format!("{operation}: an endpoint holding no inbox");
        let every: Vec<String> = inbox.iter().map(|(id, _, _)| id.clone()).collect();
        let unprocessed = answer_ok(&router, "ap-southeast-2", path, &every, &case).await;
        assert_eq!(
            reported_ids(&unprocessed),
            every,
            "case {case:?}: expected every named account reported, got {unprocessed:?}"
        );
        for entry in &unprocessed {
            assert_eq!(
                entry["result"],
                json!(expected_result),
                "case {case:?}: expected this operation's fixed reason, got {entry:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_two_operations_report_different_reasons_for_the_same_request() {
    let inbox = seeds(2);
    let named = vec![
        NEVER_INVITED[0].to_string(),
        inbox[0].0.clone(),
        NEVER_INVITED[1].to_string(),
    ];

    let mut answers: Vec<(&str, Vec<String>, String)> = Vec::new();
    for (operation, path, _) in OPERATIONS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, REGION).await;
        for (account_id, invitation_id, millis) in &inbox {
            seed_invitation(
                &engine,
                REGION,
                &detector_id,
                account_id,
                invitation_id,
                *millis,
            );
        }
        let case = format!("{operation}: the shared request");
        let unprocessed = answer_ok(&router, REGION, path, &named, &case).await;
        answers.push((
            operation,
            reported_ids(&unprocessed),
            result_of(&unprocessed, &NEVER_INVITED[0].to_string()),
        ));
    }

    let (first_operation, first_ids, first_result) = &answers[0];
    let (second_operation, second_ids, second_result) = &answers[1];
    assert_eq!(
        first_ids, second_ids,
        "the two operations act on the same accounts, so they must report the same ones: \
         {first_operation} reported {first_ids:?}, {second_operation} reported {second_ids:?}"
    );
    assert_ne!(
        first_result, second_result,
        "case {first_operation}/{second_operation}: the two refusal classes are different \
         sentences, got {first_result:?} and {second_result:?}"
    );
    assert_eq!(
        first_result, DECLINE_RESULT,
        "case {first_operation}: expected the transition reason, got {first_result:?}"
    );
    assert_eq!(
        second_result, DELETE_RESULT,
        "case {second_operation}: expected the three-condition reason, got {second_result:?}"
    );
}

// ---------------------------------------------------------------------------
// The calls and the helpers.
// ---------------------------------------------------------------------------

/// One call to `path` with `accountIds`, returning the `unprocessedAccounts` it
/// reported. The member is required on both operations, so an absent list is a defect
/// whatever the call answered.
async fn answer_ok(
    router: &Router,
    region: &str,
    path: &str,
    account_ids: &[String],
    case: &str,
) -> Vec<Value> {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        path,
        Some(json!({"accountIds": account_ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    body.get("unprocessedAccounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an unprocessedAccounts array, got {body:?}")
        })
}

/// The account ids reported, in the order the answer lists them.
fn reported_ids(unprocessed: &[Value]) -> Vec<String> {
    unprocessed
        .iter()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `result` reported for `account_id`.
fn result_of(unprocessed: &[Value], account_id: &String) -> String {
    unprocessed
        .iter()
        .find(|entry| entry["accountId"].as_str() == Some(account_id))
        .map(|entry| entry["result"].as_str().unwrap_or_default().to_string())
        .unwrap_or_else(|| panic!("expected {account_id} reported, got {unprocessed:?}"))
}
