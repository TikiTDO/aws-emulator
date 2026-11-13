// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the IDENTITY of a subscriber inside the set a write establishes,
//! driven over the wire through the same `MultiServiceRouter` the production router
//! serves from (the harness is the lifecycle suite's).
//!
//! The RULE every test here asserts: a `Subscriber` carries no id — it is only a
//! subscription type and an address, and every operation that acts on one names it by
//! passing that whole shape back — so the subscribers a write establishes are a SET keyed
//! by that pair: two entries agreeing on both name the SAME subscriber, so the set ends
//! up holding it once, in the position it was first supplied, and the documented per-type
//! capacities ("one SNS subscriber and up to 10 email subscribers", `Notification`) are
//! counted over DISTINCT subscribers.
//!
//! The rule is a property of a subscriber set, not of one operation, so each table is
//! asserted on every write that establishes one from a supplied list:
//! `CreateNotification`, the notifications `CreateBudget` attaches, `CreateBudgetAction`
//! and the replacement set `UpdateBudgetAction` stores.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The subscription-type catalog is not re-typed here: it is the model-derived constant
/// the scaffold generated into `src/ops/types/shared_types.rs`, so a table built from it
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::SUBSCRIPTION_TYPE_VALUES;
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    ComAmazonawsBudgetsAwsbudgetservicegatewayState, MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION,
    MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
};

use super::budget_action_execution::{create_body, definition_for, describe_action, seed_budget};
use super::budget_lifecycle::{caller_account, call, is_error, router_with_engine, same};
use super::recorded_response_members::as_action_subscribers;

/// Every refusal this service renders over awsJson answers this status, whichever
/// modelled error it carries — the sibling suites assert the same one.
const REFUSAL_STATUS: u16 = 400;

/// An email subscriber named by `tag`.
fn email(tag: &str) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("{tag}@example.com")})
}

/// An SNS subscriber named by `tag`: "either an SNS topic or an email".
fn sns(tag: &str) -> Value {
    json!({
        "SubscriptionType": "SNS",
        "Address": format!("arn:aws:sns:us-east-1:{}:{tag}", caller_account())
    })
}

/// `count` email subscribers, each with its own address.
fn distinct_emails(count: usize) -> Vec<Value> {
    (0..count).map(|n| email(&format!("distinct{n}"))).collect()
}

/// `count` copies of the SAME subscriber — one subscriber named `count` times.
fn repeats(subscriber: &Value, count: usize) -> Vec<Value> {
    std::iter::repeat_n(subscriber.clone(), count).collect()
}

/// One row of the collapse table: the list a write supplies, and the set the service must
/// end up holding for it.
struct Row {
    case: String,
    supplied: Vec<Value>,
    distinct: Vec<Value>,
}

/// The collapse table. Every row stays inside the modelled `@length(1, 11)` of
/// `Subscribers`, so every one of them reaches a handler, and the rows differ along what
/// the rule generalises over: the subscription type the repeat is of, how many copies are
/// supplied (two, the email maximum, the whole modelled list), where the repeats sit in
/// the list (adjacent, interleaved, several addresses repeated at once), whether anything
/// distinct is supplied beside them, and the case where nothing may be collapsed at all.
fn collapse_rows() -> Vec<Row> {
    let ops = email("ops");
    let billing = email("billing");
    let finance = email("finance");
    let topic = sns("alerts");
    // A subscriber's address has to be the form its own `SubscriptionType` names ("either
    // an Amazon SNS topic or an email address", `Subscriber`), so one subscriber of each
    // type the model lists is a set with nothing to collapse: the type is half of the
    // identity and the address the other half, and both differ here.
    let one_of_each_type: Vec<Value> = SUBSCRIPTION_TYPE_VALUES
        .iter()
        .map(|subscription_type| match *subscription_type {
            "SNS" => sns("shared"),
            _ => email("shared"),
        })
        .collect();

    let mut interleaved = vec![
        billing.clone(),
        ops.clone(),
        billing.clone(),
        finance.clone(),
        ops.clone(),
    ];
    interleaved.push(finance.clone());

    let mut email_max_plus_a_repeat = distinct_emails(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION);
    let first_distinct = email_max_plus_a_repeat[0].clone();
    email_max_plus_a_repeat.push(first_distinct);

    let mut topic_twice_and_emails = repeats(&topic, 2);
    topic_twice_and_emails.extend(distinct_emails(
        MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION - 1,
    ));
    let mut topic_twice_and_emails_distinct = vec![topic.clone()];
    topic_twice_and_emails_distinct.extend(distinct_emails(
        MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION - 1,
    ));

    let mut whole_distinct_set = vec![topic.clone()];
    whole_distinct_set.extend(distinct_emails(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION));

    vec![
        Row {
            case: "one email address supplied twice".to_string(),
            supplied: repeats(&ops, 2),
            distinct: vec![ops.clone()],
        },
        Row {
            case: "one SNS topic supplied twice".to_string(),
            supplied: repeats(&topic, 2),
            distinct: vec![topic.clone()],
        },
        Row {
            case: format!(
                "one email address supplied {} times: the whole modelled list",
                MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION
            ),
            supplied: repeats(
                &ops,
                MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION,
            ),
            distinct: vec![ops.clone()],
        },
        Row {
            case: "two addresses, each supplied twice, adjacent".to_string(),
            supplied: vec![ops.clone(), ops.clone(), billing.clone(), billing.clone()],
            distinct: vec![ops.clone(), billing.clone()],
        },
        Row {
            case: "three addresses repeated out of order".to_string(),
            supplied: interleaved,
            distinct: vec![billing.clone(), ops.clone(), finance.clone()],
        },
        Row {
            case: format!(
                "{} distinct email addresses and one of them again",
                MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION
            ),
            supplied: email_max_plus_a_repeat,
            distinct: distinct_emails(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
        },
        Row {
            case: "one SNS topic twice beside distinct email addresses".to_string(),
            supplied: topic_twice_and_emails,
            distinct: topic_twice_and_emails_distinct,
        },
        Row {
            case: "one subscriber of each subscription type the model lists".to_string(),
            supplied: one_of_each_type.clone(),
            distinct: one_of_each_type,
        },
        Row {
            case: "a set with nothing to collapse: the documented maximum of each type"
                .to_string(),
            supplied: whole_distinct_set.clone(),
            distinct: whole_distinct_set,
        },
    ]
}

/// What the service must do with a supplied list whose per-type counts depend on whether
/// repeats are counted.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    /// Within both documented halves once repeats are collapsed.
    Accepted,
    /// Past a documented half on DISTINCT subscribers: the subscriber limit the service
    /// enforces itself.
    PerTypeLimit,
    /// Outside the modelled `@length(1, 11)` of `Subscribers`, which bounds the list as
    /// SUPPLIED and is refused at request validation before any handler collapses
    /// anything.
    OutsideModelledTotal,
}

/// One row of the limit table: the list supplied, what the service must do with it, and
/// the set it must end up holding when it accepts it.
struct LimitRow {
    case: String,
    supplied: Vec<Value>,
    verdict: Verdict,
    distinct: Vec<Value>,
}

/// The limit table: the per-type capacities counted over distinct subscribers. Rows sit
/// on each documented half from both sides, built once out of repeats (which must not
/// consume capacity) and once out of distinct addresses (which must).
fn limit_rows() -> Vec<LimitRow> {
    let ops = email("ops");
    let topic = sns("alerts");
    let email_max = MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION;
    let sns_max = MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION;

    let mut sns_repeated_with_email_max = repeats(&topic, 2);
    sns_repeated_with_email_max.extend(distinct_emails(email_max - 1));

    let mut two_topics = vec![topic.clone()];
    two_topics.push(sns("other-alerts"));

    let mut email_max_repeated_once_over = distinct_emails(email_max);
    let first = email_max_repeated_once_over[0].clone();
    email_max_repeated_once_over.push(first);

    let mut over_the_modelled_total = repeats(&topic, 2);
    over_the_modelled_total.extend(distinct_emails(email_max));

    let mut sns_and_emails_distinct = vec![topic.clone()];
    sns_and_emails_distinct.extend(distinct_emails(email_max - 1));

    vec![
        LimitRow {
            case: format!("{} copies of one email address", email_max + 1),
            supplied: repeats(&ops, email_max + 1),
            verdict: Verdict::Accepted,
            distinct: vec![ops.clone()],
        },
        LimitRow {
            case: format!("{} copies of one SNS topic", sns_max + 1),
            supplied: repeats(&topic, sns_max + 1),
            verdict: Verdict::Accepted,
            distinct: vec![topic.clone()],
        },
        LimitRow {
            case: format!("{email_max} distinct email addresses and a repeat of one"),
            supplied: email_max_repeated_once_over,
            verdict: Verdict::Accepted,
            distinct: distinct_emails(email_max),
        },
        LimitRow {
            case: format!("one SNS topic twice and {} distinct emails", email_max - 1),
            supplied: sns_repeated_with_email_max,
            verdict: Verdict::Accepted,
            distinct: sns_and_emails_distinct,
        },
        LimitRow {
            case: format!("{} distinct email addresses", email_max + 1),
            supplied: distinct_emails(email_max + 1),
            verdict: Verdict::PerTypeLimit,
            distinct: Vec::new(),
        },
        LimitRow {
            case: format!("{} distinct SNS topics", sns_max + 1),
            supplied: two_topics,
            verdict: Verdict::PerTypeLimit,
            distinct: Vec::new(),
        },
        LimitRow {
            case: format!(
                "{} entries: past the modelled total as supplied, though only {} are distinct",
                email_max + 2,
                email_max + 1
            ),
            supplied: over_the_modelled_total,
            verdict: Verdict::OutsideModelledTotal,
            distinct: Vec::new(),
        },
    ]
}

/// The notification row `index` creates, so no two rows of a table share a notification
/// identity inside one budget.
fn notification(index: usize) -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": index as f64 + 1.0,
        "ThresholdType": "PERCENTAGE"
    })
}

/// The subscribers `DescribeSubscribersForNotification` reports for one notification, in
/// order. `MaxResults` is the modelled total, so one page carries the whole set.
async fn reported_subscribers(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
) -> (u16, Value) {
    let (status, body) = call(
        router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification,
            "MaxResults": 100
        }),
    )
    .await;
    (status, body["Subscribers"].clone())
}

/// The subscribers the STORE holds for one notification of a budget, as
/// `(SubscriptionType, Address)` in order — read from the state struct directly, so what
/// the write persisted is asserted independently of any read operation's projection.
fn stored_subscribers(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
    position: usize,
) -> Vec<(String, String)> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .expect("the budget under test is stored")
        .notifications[position]
        .subscribers
        .iter()
        .map(|subscriber| {
            (
                subscriber.subscription_type.clone(),
                subscriber.address.clone(),
            )
        })
        .collect()
}

/// A subscriber as `(SubscriptionType, Address)`, for comparing a stored set against the
/// one a row expects.
fn pair(subscriber: &Value) -> (String, String) {
    (
        subscriber["SubscriptionType"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        subscriber["Address"].as_str().unwrap_or_default().to_string(),
    )
}

/// Create the budget every table seeds its notifications on.
async fn seed_cost_budget(router: &MultiServiceRouter, account_id: &str, budget_name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({
            "AccountId": account_id,
            "Budget": {
                "BudgetName": budget_name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {budget_name:?}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// RULE on `CreateNotification`: the set the notification ends up holding carries each
/// `(SubscriptionType, Address)` pair once, in the position it was first supplied — both
/// as the store holds it and as `DescribeSubscribersForNotification` reports it — and a
/// `CreateSubscriber` naming a collapsed subscriber afterwards is the duplicate the
/// single-subscriber write path models, which is only true if the set really holds it.
#[tokio::test]
async fn create_notification_holds_each_subscriber_once() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "notification-subscriber-set";
    seed_cost_budget(&router, &account_id, budget_name).await;

    for (index, row) in collapse_rows().into_iter().enumerate() {
        let case = &row.case;
        let notification = notification(index);
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": &notification,
                "Subscribers": Value::Array(row.supplied.clone())
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the supplied set to be accepted, got {status} {body}"
        );

        let expected: Vec<(String, String)> = row.distinct.iter().map(pair).collect();
        let held = stored_subscribers(&engine, &account_id, budget_name, index);
        assert_eq!(
            held, expected,
            "case {case:?}: expected the stored set to be {expected:?}, got {held:?}"
        );

        let (status, reported) =
            reported_subscribers(&router, &account_id, budget_name, &notification).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the subscribers to read back, got {status} {reported}"
        );
        let distinct = Value::Array(row.distinct.clone());
        assert!(
            same(&reported, &distinct),
            "case {case:?}: expected {distinct} to be reported, got {reported}"
        );

        // Every entry the set holds is one a later single-subscriber write finds already
        // there: a collapsed repeat left the subscriber it names addressable, not gone.
        for subscriber in &row.distinct {
            let (status, body) = call(
                &router,
                "CreateSubscriber",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": budget_name,
                    "Notification": &notification,
                    "Subscriber": subscriber
                }),
            )
            .await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected a subscriber the set holds to be refused, \
                 got {status} {body}"
            );
            assert!(
                is_error(&body, "DuplicateRecordException"),
                "case {case:?}: expected DuplicateRecordException, got {body}"
            );
        }
    }
}

/// RULE on `CreateBudget`: a notification attached with the budget establishes its
/// subscriber set the same way `CreateNotification` does, so the same table collapses the
/// same way when the set arrives with the budget.
#[tokio::test]
async fn create_budget_holds_each_attached_subscriber_once() {
    let (router, engine) = router_with_engine();
    let account_id = caller_account();

    for (index, row) in collapse_rows().into_iter().enumerate() {
        let case = &row.case;
        // One budget per row, so each row's notification sits at position 0 of its own
        // budget and no row can see another's set.
        let budget_name = format!("attached-subscriber-set-{index}");
        let notification = notification(index);
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": &budget_name,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "100", "Unit": "USD"}
                },
                "NotificationsWithSubscribers": [{
                    "Notification": &notification,
                    "Subscribers": Value::Array(row.supplied.clone())
                }]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the budget and its notification to be created, \
             got {status} {body}"
        );

        let expected: Vec<(String, String)> = row.distinct.iter().map(pair).collect();
        let held = stored_subscribers(&engine, &account_id, &budget_name, 0);
        assert_eq!(
            held, expected,
            "case {case:?}: expected the stored set to be {expected:?}, got {held:?}"
        );

        let (status, reported) =
            reported_subscribers(&router, &account_id, &budget_name, &notification).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the subscribers to read back, got {status} {reported}"
        );
        let distinct = Value::Array(row.distinct.clone());
        assert!(
            same(&reported, &distinct),
            "case {case:?}: expected {distinct} to be reported, got {reported}"
        );
    }
}

/// RULE on the budget-action write paths: an action's `Subscribers` is the same modelled
/// set shape, so `CreateBudgetAction` and the replacement set `UpdateBudgetAction` stores
/// collapse the same repeats, and `DescribeBudgetAction` reports each subscriber once.
#[tokio::test]
async fn budget_action_writes_hold_each_subscriber_once() {
    let (router, _engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "action-subscriber-set";
    seed_budget(&router, &account_id, budget_name).await;

    for (index, row) in collapse_rows().into_iter().enumerate() {
        let case = &row.case;
        let action_type = "APPLY_IAM_POLICY";
        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &create_body(
                &account_id,
                budget_name,
                action_type,
                &definition_for(action_type),
                "MANUAL",
                "ACTUAL",
                &json!({
                    "ActionThresholdValue": 10.0 * (index as f64 + 1.0),
                    "ActionThresholdType": "PERCENTAGE"
                }),
                &Value::Array(row.supplied.clone()),
                None,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the action to be created, got {status} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();

        let distinct = as_action_subscribers(&Value::Array(row.distinct.clone()));
        let (status, body) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created action to read back, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["Subscribers"], &distinct),
            "case {case:?}: expected the created action to hold {distinct}, got {}",
            body["Action"]["Subscribers"]
        );

        // The same list supplied as a REPLACEMENT set collapses the same way, so an
        // update cannot leave an action holding a subscriber twice either.
        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "Subscribers": Value::Array(row.supplied.clone())
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the replacement set to be accepted, got {status} {body}"
        );
        assert!(
            same(&body["NewAction"]["Subscribers"], &distinct),
            "case {case:?}: expected NewAction to hold {distinct}, got {}",
            body["NewAction"]["Subscribers"]
        );
        let (status, body) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated action to read back, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["Subscribers"], &distinct),
            "case {case:?}: expected the updated action to hold {distinct}, got {}",
            body["Action"]["Subscribers"]
        );
    }
}

/// RULE on the documented per-type capacities: they bound DISTINCT subscribers, so a
/// list whose repeats push it past a half is accepted and one with that many distinct
/// addresses is refused with the subscriber limit — while the modelled `@length(1, 11)`
/// bounds the list as SUPPLIED and is still refused at request validation.
#[tokio::test]
async fn the_per_type_limits_count_distinct_subscribers() {
    assert_eq!(
        SUBSCRIPTION_TYPE_VALUES,
        &["EMAIL", "SNS"],
        "the subscription-type catalog this table spans changed: {SUBSCRIPTION_TYPE_VALUES:?}"
    );
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "distinct-subscriber-limits";
    seed_cost_budget(&router, &account_id, budget_name).await;

    let mut stored = 0_usize;
    for (index, row) in limit_rows().into_iter().enumerate() {
        let case = &row.case;
        let notification = notification(index);
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": &notification,
                "Subscribers": Value::Array(row.supplied.clone())
            }),
        )
        .await;
        match row.verdict {
            Verdict::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the set to be accepted once repeats collapse, \
                     got {status} {body}"
                );
                let expected: Vec<(String, String)> = row.distinct.iter().map(pair).collect();
                let held = stored_subscribers(&engine, &account_id, budget_name, stored);
                assert_eq!(
                    held, expected,
                    "case {case:?}: expected the stored set to be {expected:?}, got {held:?}"
                );
                stored += 1;
            }
            refused => {
                let expected = match refused {
                    Verdict::PerTypeLimit => "CreationLimitExceededException",
                    _ => "ValidationException",
                };
                assert_eq!(
                    status, REFUSAL_STATUS,
                    "case {case:?}: expected the refusal at {REFUSAL_STATUS}, got {status} {body}"
                );
                assert!(
                    is_error(&body, expected),
                    "case {case:?}: expected {expected}, got {body}"
                );
                // A refused create stored no notification, so the read of that
                // notification's subscribers reports the empty set: `Subscribers` is
                // `@length(min: 1)`, so the empty set is the member left out.
                let (status, reported) =
                    reported_subscribers(&router, &account_id, budget_name, &notification).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the read of a notification no refused create \
                     stored to answer 200, got {status} {reported}"
                );
                assert!(
                    reported.is_null(),
                    "case {case:?}: expected a refused create to store no notification, \
                     got {reported}"
                );
            }
        }
    }
}
