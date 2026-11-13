// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the LOCK a budget action takes while the work this service
//! started on it has not settled yet, driven over the wire through the same
//! `MultiServiceRouter` the production router serves from.
//!
//! The rule: an action whose status is one of the `*_IN_PROGRESS` ones is the locked
//! resource `ResourceLockedException` names ("the server rejected that particular method
//! for the requested resource"), so EVERY method that mutates the action — the
//! execution, the update of its members and its deletion, which are exactly the three
//! operations that model the error — is refused while that work runs, and the stored
//! action is left as the in-flight work found it. Once the clock settles the work, the
//! same call is accepted.
//!
//! Each test asserts that over a SET of inputs: the whole modelled `ActionStatus`
//! catalog, every member `UpdateBudgetAction` accepts, and each of the three mutating
//! methods.
//!
//! The harness (`router_with_engine`, `call`, `is_error`, `caller_account`, `same`) and
//! the action setup helpers are the sibling suites', so every suite in this target
//! drives the service the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `ActionStatus` catalog is not re-typed here: this is the model-derived constant
/// the scaffold generated into `src/ops/types/shared_types.rs`, so a table built from it
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::ACTION_STATUS_VALUES;

use super::budget_action_execution::{
    describe_action, email_subscriber, execute_action, make_settle_due, seed_action, seed_budget,
    seed_pending_action, seed_status, sns_subscriber,
};
use super::budget_action_history::read_history;
use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};
use super::recorded_response_members::as_action_subscribers;

/// Whether `status` is one of the `*_IN_PROGRESS` statuses — work this service started
/// and has not settled yet. Written as the shape of the status name rather than as a
/// list, so every entry of the model's `ActionStatus` catalog is classified, including
/// the ones no request in this emulator reaches.
fn is_in_progress(status: &str) -> bool {
    status.ends_with("_IN_PROGRESS")
}

/// A second `ExecutionRoleArn`, satisfying `RoleArn`'s `@length(32, 618)` and its
/// `arn:aws:iam::<12 digits>:role/<name>` pattern — so an accepted update is visible and
/// a refused one is too. It names a role in the account the action lives in, which is the
/// only account this service serves: "roles and actions must be in the same account"
/// (`Action$ExecutionRoleArn`).
fn other_execution_role() -> String {
    format!(
        "arn:aws:iam::{}:role/budget-action-replacement",
        caller_account()
    )
}

/// A second `Definition.IamActionDefinition`, which is the format the action type these
/// rows are created with (`APPLY_IAM_POLICY`) names.
fn other_iam_definition() -> Value {
    json!({
        "IamActionDefinition": {
            "PolicyArn": "arn:aws:iam::111122223333:policy/DenyEverythingElse",
            "Groups": ["finance-admins"]
        }
    })
}

/// One `UpdateBudgetAction` request: the three members that name the action, plus the
/// members `patch` replaces.
fn update_body(account_id: &str, budget_name: &str, action_id: &str, patch: &Value) -> Value {
    let mut request = json!({
        "AccountId": account_id,
        "BudgetName": budget_name,
        "ActionId": action_id,
    });
    let members = request.as_object_mut().expect("the request is an object");
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        members.insert(member, value);
    }
    request
}

/// Every member `UpdateBudgetAction` accepts, one row each plus a row that names all of
/// them and a row that names none — `ActionId`, `BudgetName`, `ActionType` and the
/// service-owned `Status` are not among its inputs
/// (`smithy_query(query="operation",operation="UpdateBudgetAction")`).
fn update_patches() -> Vec<(&'static str, Value)> {
    vec![
        ("definition", json!({"Definition": other_iam_definition()})),
        (
            "execution-role",
            json!({"ExecutionRoleArn": other_execution_role()}),
        ),
        (
            "subscribers",
            json!({"Subscribers": [sns_subscriber(), email_subscriber(1), email_subscriber(2)]}),
        ),
        (
            "notification-type",
            json!({"NotificationType": "FORECASTED"}),
        ),
        ("approval-model", json!({"ApprovalModel": "AUTOMATIC"})),
        (
            "threshold",
            json!({"ActionThreshold": {
                "ActionThresholdValue": 42.5,
                "ActionThresholdType": "ABSOLUTE_VALUE"
            }}),
        ),
        (
            "every-member",
            json!({
                "Definition": other_iam_definition(),
                "ExecutionRoleArn": other_execution_role(),
                "Subscribers": [email_subscriber(7)],
                "NotificationType": "FORECASTED",
                "ApprovalModel": "AUTOMATIC",
                "ActionThreshold": {
                    "ActionThresholdValue": 1.0,
                    "ActionThresholdType": "PERCENTAGE"
                }
            }),
        ),
        ("no-member", json!({})),
    ]
}

/// The `Action` a read reports, failing the test if the read was refused.
async fn read_action(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    action_id: &str,
    case: &str,
) -> Value {
    let (status, body) = describe_action(router, account_id, budget_name, action_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBudgetAction to answer 200, got {status} {body}"
    );
    body.get("Action").cloned().unwrap_or(Value::Null)
}

/// RULE, over the whole modelled `ActionStatus` catalog and every member the operation
/// accepts: `UpdateBudgetAction` is refused with `ResourceLockedException` exactly while
/// the action's work is in flight, and the refusal leaves the stored action — every
/// member of it, and its history — exactly as it was.
#[tokio::test]
async fn update_is_refused_while_the_action_is_in_flight() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "locked-update";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    for status in ACTION_STATUS_VALUES {
        for (member, patch) in update_patches() {
            let case = format!("{status} + {member}");
            seed_status(&engine, &account_id, budget_name, &action_id, status);

            let before = read_action(&router, &account_id, budget_name, &action_id, &case).await;
            let history_before =
                read_history(&router, &account_id, budget_name, &action_id, None, &case).await;

            let (http, body) = call(
                &router,
                "UpdateBudgetAction",
                &update_body(&account_id, budget_name, &action_id, &patch),
            )
            .await;

            if is_in_progress(status) {
                assert_eq!(
                    http, 400,
                    "case {case:?}: expected an update of in-flight work to be refused with \
                     400, got {http} {body}"
                );
                assert!(
                    is_error(&body, "ResourceLockedException"),
                    "case {case:?}: expected ResourceLockedException, got {body}"
                );
                let after =
                    read_action(&router, &account_id, budget_name, &action_id, &case).await;
                assert!(
                    same(&after, &before),
                    "case {case:?}: expected the refusal to leave the action {before}, got \
                     {after}"
                );
                let history_after =
                    read_history(&router, &account_id, budget_name, &action_id, None, &case).await;
                assert_eq!(
                    history_after.len(),
                    history_before.len(),
                    "case {case:?}: expected a refused update to record nothing, got {} records \
                     after {}",
                    history_after.len(),
                    history_before.len()
                );
                continue;
            }

            assert_eq!(
                http, 200,
                "case {case:?}: expected an update of a settled action to answer 200, got \
                 {http} {body}"
            );
            let after = read_action(&router, &account_id, budget_name, &action_id, &case).await;
            for (updated, value) in patch.as_object().cloned().unwrap_or_default() {
                // A replacement subscriber set reads back with the member of each
                // subscriber the SERVICE owns stamped on it
                // (`tests/all/recorded_response_members.rs`).
                let expected = if updated == "Subscribers" {
                    as_action_subscribers(&value)
                } else {
                    value
                };
                assert!(
                    same(&after[&updated], &expected),
                    "case {case:?}: expected {updated} to read back as {expected}, got {}",
                    after[&updated]
                );
            }
            assert_eq!(
                after["Status"].as_str(),
                Some(*status),
                "case {case:?}: expected the update to leave the service-owned Status alone, \
                 got {}",
                after["Status"]
            );
        }
    }
}

/// RULE, over the SET of methods that mutate an action: the lock refuses each of them —
/// the three operations the model gives `ResourceLockedException` to — from each
/// `*_IN_PROGRESS` status, and the action survives every one of those refusals intact.
#[tokio::test]
async fn the_lock_covers_every_mutating_budget_action_method() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "locked-methods";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;

    let locked: Vec<&&str> = ACTION_STATUS_VALUES
        .iter()
        .filter(|status| is_in_progress(status))
        .collect();
    assert!(
        !locked.is_empty(),
        "expected the model's ActionStatus catalog to carry in-progress statuses, got \
         {ACTION_STATUS_VALUES:?}"
    );

    for status in locked {
        seed_status(&engine, &account_id, budget_name, &action_id, status);
        let before = read_action(&router, &account_id, budget_name, &action_id, status).await;

        for method in ["ExecuteBudgetAction", "UpdateBudgetAction", "DeleteBudgetAction"] {
            let case = format!("{status} + {method}");
            let (http, body) = match method {
                "ExecuteBudgetAction" => {
                    execute_action(
                        &router,
                        &account_id,
                        budget_name,
                        &action_id,
                        "APPROVE_BUDGET_ACTION",
                    )
                    .await
                }
                "UpdateBudgetAction" => {
                    call(
                        &router,
                        "UpdateBudgetAction",
                        &update_body(
                            &account_id,
                            budget_name,
                            &action_id,
                            &json!({"ExecutionRoleArn": other_execution_role()}),
                        ),
                    )
                    .await
                }
                _ => {
                    call(
                        &router,
                        "DeleteBudgetAction",
                        &json!({
                            "AccountId": &account_id,
                            "BudgetName": budget_name,
                            "ActionId": &action_id
                        }),
                    )
                    .await
                }
            };

            assert_eq!(
                http, 400,
                "case {case:?}: expected the locked resource to refuse the method with 400, \
                 got {http} {body}"
            );
            assert!(
                is_error(&body, "ResourceLockedException"),
                "case {case:?}: expected ResourceLockedException, got {body}"
            );
            let after = read_action(&router, &account_id, budget_name, &action_id, &case).await;
            assert!(
                same(&after, &before),
                "case {case:?}: expected the refusal to leave the action {before}, got {after}"
            );
        }
    }
}

/// RULE: the lock is the in-flight work's, so it is the CLOCK that releases it — the
/// update an execution refuses is accepted once the settle it owed has fallen due, and
/// the accepted update reads back through `DescribeBudgetAction`.
#[tokio::test]
async fn the_clock_releases_the_lock_an_execution_took() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "locked-until-settled";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_pending_action(&router, &account_id, budget_name).await;
    let created = read_action(&router, &account_id, budget_name, &action_id, "created").await;

    let (http, body) = execute_action(
        &router,
        &account_id,
        budget_name,
        &action_id,
        "APPROVE_BUDGET_ACTION",
    )
    .await;
    assert_eq!(
        http, 200,
        "expected the approval to be accepted with 200, got {http} {body}"
    );

    // The caller acting straight after the execution still sees the work in flight, so
    // its update meets the lock.
    let patch = json!({
        "Definition": other_iam_definition(),
        "ExecutionRoleArn": other_execution_role(),
        "Subscribers": [email_subscriber(4)]
    });
    let request = update_body(&account_id, budget_name, &action_id, &patch);
    let (http, body) = call(&router, "UpdateBudgetAction", &request).await;
    assert_eq!(
        http, 400,
        "expected the update of in-flight work to be refused with 400, got {http} {body}"
    );
    assert!(
        is_error(&body, "ResourceLockedException"),
        "expected ResourceLockedException while the execution runs, got {body}"
    );
    let in_flight = read_action(&router, &account_id, budget_name, &action_id, "in-flight").await;
    for member in ["Definition", "ExecutionRoleArn", "Subscribers"] {
        assert!(
            same(&in_flight[member], &created[member]),
            "case {member:?}: expected the in-flight action to keep the member it was started \
             with ({}), got {}",
            created[member],
            in_flight[member]
        );
    }

    // The settle is owed to the clock; once it falls due the action is no longer locked.
    make_settle_due(&engine, &account_id, budget_name, &action_id);
    let (http, body) = call(&router, "UpdateBudgetAction", &request).await;
    assert_eq!(
        http, 200,
        "expected the same update to be accepted once the work settled, got {http} {body}"
    );
    let settled = read_action(&router, &account_id, budget_name, &action_id, "settled").await;
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        let expected = if member == "Subscribers" {
            as_action_subscribers(&value)
        } else {
            value
        };
        assert!(
            same(&settled[&member], &expected),
            "case {member:?}: expected the accepted update to store {expected}, got {}",
            settled[&member]
        );
    }
    assert_eq!(
        settled["Status"].as_str(),
        Some("EXECUTION_SUCCESS"),
        "expected the settled status the execution owed, got {}",
        settled["Status"]
    );
}
