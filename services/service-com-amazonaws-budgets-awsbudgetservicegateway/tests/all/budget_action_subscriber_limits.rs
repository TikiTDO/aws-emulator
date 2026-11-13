// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SUBSCRIBER SET a budget action holds, driven over the wire
//! through the same `MultiServiceRouter` the production router serves from.
//!
//! The RULE both tests assert: a subscriber set this service stores holds at most one
//! `SNS` subscriber and at most ten `EMAIL` subscribers — "a notification can have one
//! SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers"
//! (`Notification`), the split the one modelled `Subscribers` shape's `@length(1..11)`
//! total is made of. A write that would leave a budget action holding more of either
//! type is refused and stores nothing: `CreateBudgetAction` answers the subscriber limit
//! it models (`CreationLimitExceededException`), and `UpdateBudgetAction`, which models
//! no such error, answers the client-error it does model (`InvalidParameterException`).
//! The total and the at-least-one are the modelled constraint itself, refused at request
//! validation (`ValidationException`) before a handler runs.
//!
//! Each table spans the class rather than one request: its rows sit on each published
//! per-type bound and one past it, mix the types inside the modelled total, and reach
//! past the total in both directions — and the create table walks the `ActionType`
//! catalog as it goes, so the rule is not keyed to one action shape.

use serde_json::{json, Value};
use std::sync::Arc;

/// The subscription-type catalog is not re-typed here: it is the model-derived constant
/// the scaffold generated into `src/ops/types/shared_types.rs`, so a table built from it
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    ACTION_TYPE_VALUES, SUBSCRIPTION_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION,
    MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_action_execution::{
    create_body, definition_for, describe_action, email_subscriber, is_action_id,
    simple_create_body, sns_subscriber,
};
use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};
use super::recorded_response_members::as_action_subscribers;

/// A second (and third, …) SNS subscriber, each with its own topic, so a row that is
/// past the SNS limit is past it on the subscription TYPE and not on a repeated address.
fn extra_sns_subscriber(n: usize) -> Value {
    json!({
        "SubscriptionType": "SNS",
        "Address": format!(
            "arn:aws:sns:us-east-1:{}:budget-action-alerts-{n}",
            caller_account()
        )
    })
}

/// A set of `sns` SNS subscribers followed by `email` email subscribers.
fn subscribers(sns: usize, email: usize) -> Value {
    let mut set: Vec<Value> = Vec::new();
    for n in 0..sns {
        set.push(if n == 0 {
            sns_subscriber()
        } else {
            extra_sns_subscriber(n)
        });
    }
    set.extend((0..email).map(email_subscriber));
    Value::Array(set)
}

/// What the service must do with one row's subscriber set.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    /// Inside both per-type halves and the modelled total: stored as supplied.
    Accepted,
    /// Inside the modelled total but past a per-type half: the subscriber limit the
    /// service itself enforces.
    PerTypeLimit,
    /// Outside the modelled `@length(1, 11)` on `Subscribers`: refused at request
    /// validation, before any handler runs.
    OutsideModelledTotal,
}

/// One row of the subscriber-set table.
struct Row {
    case: String,
    subscribers: Value,
    verdict: Verdict,
}

/// The subscriber-set table: each published per-type bound, one past it, a mix of the
/// two types that stays inside the modelled total while breaking a half, and the
/// modelled total's own bounds from both sides.
fn subscriber_rows() -> Vec<Row> {
    let sns_max = MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION;
    let email_max = MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION;
    let rows: Vec<(String, Value, Verdict)> = vec![
        (
            "one EMAIL: the modelled minimum".to_string(),
            subscribers(0, 1),
            Verdict::Accepted,
        ),
        (
            format!("{email_max} EMAIL: the documented email maximum"),
            subscribers(0, email_max),
            Verdict::Accepted,
        ),
        (
            format!("{sns_max} SNS: the documented SNS maximum"),
            subscribers(sns_max, 0),
            Verdict::Accepted,
        ),
        (
            format!("{sns_max} SNS and {email_max} EMAIL: the modelled total"),
            subscribers(sns_max, email_max),
            Verdict::Accepted,
        ),
        (
            format!("{} SNS: one past the SNS maximum", sns_max + 1),
            subscribers(sns_max + 1, 0),
            Verdict::PerTypeLimit,
        ),
        (
            format!(
                "{} SNS and {} EMAIL: past the SNS maximum inside the modelled total",
                sns_max + 1,
                email_max - 1
            ),
            subscribers(sns_max + 1, email_max - 1),
            Verdict::PerTypeLimit,
        ),
        (
            format!("{} EMAIL: one past the email maximum", email_max + 1),
            subscribers(0, email_max + 1),
            Verdict::PerTypeLimit,
        ),
        (
            format!(
                "{} SNS and {} EMAIL: past both maxima inside the modelled total",
                sns_max + 2,
                email_max - 2
            ),
            subscribers(sns_max + 2, email_max - 2),
            Verdict::PerTypeLimit,
        ),
        (
            format!("{} EMAIL: one past the modelled total", email_max + 2),
            subscribers(0, email_max + 2),
            Verdict::OutsideModelledTotal,
        ),
        (
            "no subscribers at all: under the modelled minimum".to_string(),
            subscribers(0, 0),
            Verdict::OutsideModelledTotal,
        ),
    ];
    rows.into_iter()
        .map(|(case, subscribers, verdict)| Row {
            case,
            subscribers,
            verdict,
        })
        .collect()
}

/// How many actions the STORE holds for a budget — read from the state struct directly,
/// so what a refused write did to the store is asserted independently of any read
/// operation's own projection.
///
/// `pub(crate)` so the sibling suites in this test target observe a refused write the
/// same way rather than copying the state access.
pub(crate) fn stored_action_count(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> usize {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the budget under test is stored")
        .actions
        .len()
}

/// The subscription types these tables span are the model's own catalog, so a catalog
/// that grew a third type fails the suite instead of silently going untested.
fn assert_catalog_spanned() {
    assert_eq!(
        SUBSCRIPTION_TYPE_VALUES,
        &["EMAIL", "SNS"],
        "the subscription-type catalog these tables span changed: {SUBSCRIPTION_TYPE_VALUES:?}"
    );
    assert_eq!(
        MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION, 1,
        "the documented SNS limit these tables span changed"
    );
    assert_eq!(
        MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION, 10,
        "the documented email limit these tables span changed"
    );
}

/// RULE, on the create path: `CreateBudgetAction` accepts a subscriber set inside both
/// documented per-type halves and reads it back whole; a set past either half is refused
/// with `CreationLimitExceededException` and no action is minted; a set outside the
/// modelled total is refused at request validation.
#[tokio::test]
async fn creating_an_action_holds_the_documented_subscribers_of_each_type() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "subscribed-action";
    super::budget_action_execution::seed_budget(&router, &account_id, budget_name).await;

    for (index, row) in subscriber_rows().into_iter().enumerate() {
        let case = &row.case;
        // The rows walk the `ActionType` catalog (and with it the `Definition` member
        // that type names) and the threshold, so the rule is asserted across action
        // shapes rather than on one.
        let action_type = ACTION_TYPE_VALUES[index % ACTION_TYPE_VALUES.len()];
        let create = create_body(
            &account_id,
            budget_name,
            action_type,
            &definition_for(action_type),
            if index.is_multiple_of(2) {
                "AUTOMATIC"
            } else {
                "MANUAL"
            },
            if index.is_multiple_of(3) {
                "FORECASTED"
            } else {
                "ACTUAL"
            },
            &json!({
                "ActionThresholdValue": 10.0 * (index as f64 + 1.0),
                "ActionThresholdType": "PERCENTAGE"
            }),
            &row.subscribers,
            None,
        );
        let before = stored_action_count(&engine, &account_id, budget_name);
        let (status, body) = call(&router, "CreateBudgetAction", &create).await;
        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the set to be accepted, got {status} {body}"
                );
                let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
                assert!(
                    is_action_id(&action_id),
                    "case {case:?}: expected a minted UUID ActionId, got {action_id:?}"
                );
                let (status, body) =
                    describe_action(&router, &account_id, budget_name, &action_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the created action to read back, got {status} {body}"
                );
                assert!(
                    same(
                        &body["Action"]["Subscribers"],
                        &as_action_subscribers(&row.subscribers)
                    ),
                    "case {case:?}: expected the accepted set to read back as supplied, \
                     got {}",
                    body["Action"]["Subscribers"]
                );
                assert_eq!(
                    stored_action_count(&engine, &account_id, budget_name),
                    before + 1,
                    "case {case:?}: expected the accepted create to store one action"
                );
            }
            refused => {
                let expected = match refused {
                    Verdict::PerTypeLimit => "CreationLimitExceededException",
                    _ => "ValidationException",
                };
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the refusal at 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, expected),
                    "case {case:?}: expected {expected}, got {body}"
                );
                assert_eq!(
                    stored_action_count(&engine, &account_id, budget_name),
                    before,
                    "case {case:?}: expected a refused create to store nothing"
                );
            }
        }
    }
}

/// RULE, on the update path: a replacement `Subscribers` set is held to the same
/// per-type split, so `UpdateBudgetAction` cannot leave an action holding a set a create
/// would have been refused. A set inside both halves replaces the stored one and reads
/// back; a set past a half is refused with the client-error this operation models
/// (`InvalidParameterException`) and the stored action keeps the set it had; a set
/// outside the modelled total is refused at request validation.
#[tokio::test]
async fn updating_an_action_holds_the_documented_subscribers_of_each_type() {
    assert_catalog_spanned();
    let (router, _engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "updated-action";
    super::budget_action_execution::seed_budget(&router, &account_id, budget_name).await;

    let (status, body) = call(
        &router,
        "CreateBudgetAction",
        &simple_create_body(&account_id, budget_name),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudgetAction to answer 200, got {status} {body}"
    );
    let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();

    // The set the action holds when a row starts, which a refused update must leave
    // alone.
    let mut held = describe_action(&router, &account_id, budget_name, &action_id)
        .await
        .1["Action"]["Subscribers"]
        .clone();

    for row in subscriber_rows() {
        let case = &row.case;
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "Subscribers": &row.subscribers
            }),
        )
        .await;
        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the replacement to be accepted, got {status} {body}"
                );
                assert!(
                    same(&body["OldAction"]["Subscribers"], &held),
                    "case {case:?}: expected OldAction to carry the set it replaced, got {}",
                    body["OldAction"]["Subscribers"]
                );
                assert!(
                    same(
                        &body["NewAction"]["Subscribers"],
                        &as_action_subscribers(&row.subscribers)
                    ),
                    "case {case:?}: expected NewAction to carry the replacement, got {}",
                    body["NewAction"]["Subscribers"]
                );
                held = as_action_subscribers(&row.subscribers);
            }
            refused => {
                let expected = match refused {
                    Verdict::PerTypeLimit => "InvalidParameterException",
                    _ => "ValidationException",
                };
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the refusal at 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, expected),
                    "case {case:?}: expected {expected}, got {body}"
                );
            }
        }
        // Whatever the verdict, the stored action now holds exactly `held`: the
        // replacement when it was accepted, the set it already had when it was refused.
        let (status, body) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the action to still read back, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["Subscribers"], &held),
            "case {case:?}: expected the stored set to be {held}, got {}",
            body["Action"]["Subscribers"]
        );
    }
}
