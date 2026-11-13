// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for a budget's NOTIFICATIONS: `CreateNotification`, read back
//! through `DescribeNotificationsForBudget` and `DescribeSubscribersForNotification`,
//! driven over the wire through the same `MultiServiceRouter` the production router
//! serves from.
//!
//! Each test states a RULE and asserts it over a table that spans the class the rule
//! generalises over, not one recorded request:
//!
//! 1. A created notification, and the subscribers created with it, are PERSISTED on
//!    the budget that owns it and read back as supplied, with the member the
//!    service owns (`NotificationState`) computed by the service.
//! 2. A notification is identified by what it alerts on — `NotificationType`,
//!    `ComparisonOperator`, `Threshold`, `ThresholdType` — so a repeat is a duplicate
//!    and a difference in any one of them is a different notification.
//! 3. A notification belongs to a budget, so one named for a budget the account does
//!    not hold is refused and nothing is stored.
//! 4. The documented limits — "a budget can have up to ten notifications", "one SNS
//!    subscriber and up to 10 email subscribers, for a total of 11" — are enforced.
//! 5. Reading subscribers is a pure, paginated read.
//!
//! The harness (`router`, `call`, `is_error`, `caller_account`) is the lifecycle
//! suite's, so
//! every suite in this target drives the service exactly the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The enum catalogs are not re-typed here: these are the model-derived constants the
/// scaffold generated into `src/ops/types/shared_types.rs`, so a table built from them
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    COMPARISON_OPERATOR_VALUES, NOTIFICATION_TYPE_VALUES, SUBSCRIPTION_TYPE_VALUES,
    THRESHOLD_TYPE_VALUES,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION, MAX_NOTIFICATIONS_PER_BUDGET,
    MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION, THRESHOLD_TYPE_PERCENTAGE,
};

use super::budget_lifecycle::{caller_account, call, is_error, router, same};

/// A budget that satisfies every modelled constraint and documented cross-member rule,
/// so a notification row varies only the notification.
fn budget(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// Create `name` under `account_id` and fail the test if the setup call did not take.
///
/// `pub(crate)` so a sibling suite seeds a budget exactly the way this one does.
pub(crate) async fn seed_budget(router: &MultiServiceRouter, account_id: &str, name: &str) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": account_id, "Budget": budget(name)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBudget({name}) to answer 200, got {status} {body}"
    );
}

/// An email subscriber. `SubscriptionType` is the model's `EMAIL`
/// (`SUBSCRIPTION_TYPE_VALUES`), and `Address` satisfies `SubscriberAddress`'s
/// `@length(min: 1)` and its "can't contain line breaks" rule.
fn email_subscriber(n: usize) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("ops{n}@example.com")})
}

/// The SNS subscriber: "either an SNS topic or an email".
fn sns_subscriber() -> Value {
    json!({
        "SubscriptionType": "SNS",
        "Address": format!("arn:aws:sns:us-east-1:{}:budget-alerts", caller_account())
    })
}

/// `n` email subscribers.
fn email_subscribers(n: usize) -> Value {
    Value::Array((0..n).map(email_subscriber).collect())
}

/// The notification members a caller supplies, as one JSON object.
fn notification(notification_type: &str, operator: &str, threshold: Value) -> Value {
    json!({
        "NotificationType": notification_type,
        "ComparisonOperator": operator,
        "Threshold": threshold
    })
}

/// The threshold type a notification NAMES: the member when it carries one, and the
/// percentage "thresholds are always" (`Notification$Threshold`) when it does not — the
/// one spelling the service keeps, so a read of a notification created with the default
/// reports no such member (`tests/all/recorded_response_members.rs`).
///
/// `pub(crate)` so every suite that names a notification by what it alerts on reads its
/// threshold type the same way.
pub(crate) fn named_threshold_type(notification: &Value) -> Value {
    match notification.get("ThresholdType") {
        Some(named) if !named.is_null() => named.clone(),
        _ => json!(THRESHOLD_TYPE_PERCENTAGE),
    }
}

/// The notification a read reported, found by what it alerts on, or `Value::Null`.
fn read_notification(body: &Value, wanted: &Value) -> Value {
    body["Notifications"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|found| {
            ["NotificationType", "ComparisonOperator", "Threshold"]
                .iter()
                .all(|member| same(&found[member], &wanted[member]))
                && same(&named_threshold_type(found), &named_threshold_type(wanted))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// One row of the persistence table: the notification and the subscribers to create it
/// with, and the `NotificationState` the read must report.
struct Row {
    case: &'static str,
    notification: Value,
    subscribers: Value,
    /// `NotificationState` as the read reports it: the state the SERVICE computes for the
    /// notification, which is `"OK"` for every row here (nothing spends behind a budget
    /// in this emulator, so no `GREATER_THAN` threshold is passed).
    state: &'static str,
}

/// The persistence table. Rows differ along what rule 1 generalises over: the
/// notification type, the comparison operator, the threshold (both published `@range`
/// bounds and a fractional value), whether `ThresholdType` is set and to which value,
/// whether `NotificationState` was supplied, and the number and types of subscribers
/// (up to the modelled `@length` maximum of 11).
///
/// Bounds and value sets come from the model
/// (`smithy_query(query="shape",shape="Notification,NotificationThreshold,Subscribers")`):
/// `NotificationThreshold` is `@range(min: 0, max: 15000000000000)`, `Subscribers` is
/// `@length(min: 1, max: 11)`.
fn persistence_rows() -> Vec<Row> {
    vec![
        Row {
            case: "actual spend over a percentage threshold, one email subscriber",
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            }),
            subscribers: json!([email_subscriber(0)]),
            state: "OK",
        },
        Row {
            case: "forecasted spend over an absolute threshold, one SNS subscriber",
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 160,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            subscribers: json!([sns_subscriber()]),
            state: "OK",
        },
        Row {
            case: "threshold at the published minimum",
            notification: notification("ACTUAL", "LESS_THAN", json!(0)),
            subscribers: json!([email_subscriber(1)]),
            state: "OK",
        },
        Row {
            case: "threshold at the published maximum",
            notification: notification("ACTUAL", "EQUAL_TO", json!(15_000_000_000_000_i64)),
            subscribers: json!([email_subscriber(2)]),
            state: "OK",
        },
        Row {
            case: "a fractional threshold",
            notification: notification("FORECASTED", "LESS_THAN", json!(12.5)),
            subscribers: json!([email_subscriber(3)]),
            state: "OK",
        },
        Row {
            case: "no threshold type at all",
            notification: notification("FORECASTED", "EQUAL_TO", json!(50)),
            subscribers: json!([email_subscriber(4), sns_subscriber()]),
            state: "OK",
        },
        Row {
            case: "a caller that reports the notification already in alarm",
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 100,
                "ThresholdType": "PERCENTAGE",
                "NotificationState": "ALARM"
            }),
            subscribers: json!([email_subscriber(5)]),
            // The alarm state is the SERVICE's, not the caller's: nothing has been spent
            // over this budget's limit, so the read reports the state it computed.
            // `tests/all/notification_alarm_state.rs` asserts that rule over the class.
            state: "OK",
        },
        Row {
            case: "the full subscriber set: one SNS and ten email, the length maximum",
            notification: notification("ACTUAL", "GREATER_THAN", json!(90)),
            subscribers: {
                let mut all = vec![sns_subscriber()];
                all.extend((0..MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION).map(email_subscriber));
                Value::Array(all)
            },
            state: "OK",
        },
    ]
}

/// RULE 1: a created notification is persisted on the budget that owns it and read
/// back exactly as supplied — with `NotificationState`, the one member the service
/// owns, reported as the service computes it rather than as the caller sent it — and so
/// are its subscribers, in the order they were supplied.
#[tokio::test]
async fn a_created_notification_and_its_subscribers_read_back() {
    let router = router();
    let account_id = caller_account();

    for (index, row) in persistence_rows().into_iter().enumerate() {
        let Row {
            case,
            notification,
            subscribers,
            state,
        } = row;
        // One budget per row, so each row's notification is the only one on it.
        let name = format!("notified-{index}");
        seed_budget(&router, &account_id, &name).await;

        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": &name,
                "Notification": &notification,
                "Subscribers": &subscribers
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateNotification to answer 200, got {status} {body}"
        );

        // Read back through the budget's notification list.
        let (status, body) = call(
            &router,
            "DescribeNotificationsForBudget",
            &json!({"AccountId": &account_id, "BudgetName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNotificationsForBudget to answer 200, \
             got {status} {body}"
        );
        assert_eq!(
            body["Notifications"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the budget to hold exactly one notification, got {body}"
        );
        let found = read_notification(&body, &notification);
        for member in ["NotificationType", "ComparisonOperator", "Threshold"] {
            assert!(
                same(&found[member], &notification[member]),
                "case {case:?}: expected {member} to read back as {:?}, got {:?}",
                notification[member],
                found[member]
            );
        }
        assert!(
            same(
                &named_threshold_type(&found),
                &named_threshold_type(&notification)
            ),
            "case {case:?}: expected the notification to read back as the threshold type \
             it names ({:?}), got {:?}",
            named_threshold_type(&notification),
            found["ThresholdType"]
        );
        assert_eq!(
            found["NotificationState"].as_str(),
            Some(state),
            "case {case:?}: expected NotificationState {state:?}, got {found:?}"
        );

        // Read back the subscribers, addressing the notification BOTH as it was sent
        // and as the read reported it: `NotificationState` is not part of a
        // notification's identity, so either addresses the same one.
        for (addressed, wanted) in [("as sent", &notification), ("as read back", &found)] {
            let (status, body) = call(
                &router,
                "DescribeSubscribersForNotification",
                &json!({
                    "AccountId": &account_id,
                    "BudgetName": &name,
                    "Notification": wanted
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} addressed {addressed}: expected \
                 DescribeSubscribersForNotification to answer 200, got {status} {body}"
            );
            assert!(
                same(&body["Subscribers"], &subscribers),
                "case {case:?} addressed {addressed}: expected the subscribers \
                 {subscribers} to read back in order, got {body}"
            );
        }
    }
}

/// RULE 1, over the full value sets: every combination of the model's
/// `NotificationType`, `ComparisonOperator` and `ThresholdType` catalogs is accepted
/// and reads back, and all of them can live on ONE budget up to its documented limit.
///
/// The catalogs are the generated model-derived constants, so this covers values the
/// rows above do not cite.
#[tokio::test]
async fn every_modelled_notification_combination_reads_back() {
    let router = router();
    let account_id = caller_account();

    // The catalogs are small, and a budget holds at most ten notifications, so each
    // combination gets its own budget keyed by the combination itself.
    for notification_type in NOTIFICATION_TYPE_VALUES {
        for operator in COMPARISON_OPERATOR_VALUES {
            for threshold_type in THRESHOLD_TYPE_VALUES {
                let case = format!("{notification_type}/{operator}/{threshold_type}");
                let name = format!("combo-{notification_type}-{operator}-{threshold_type}");
                seed_budget(&router, &account_id, &name).await;
                let notification = json!({
                    "NotificationType": notification_type,
                    "ComparisonOperator": operator,
                    "Threshold": 75,
                    "ThresholdType": threshold_type
                });
                let (status, body) = call(
                    &router,
                    "CreateNotification",
                    &json!({
                        "AccountId": &account_id,
                        "BudgetName": &name,
                        "Notification": &notification,
                        "Subscribers": [email_subscriber(0)]
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected CreateNotification to answer 200, got {status} {body}"
                );
                let (_, body) = call(
                    &router,
                    "DescribeNotificationsForBudget",
                    &json!({"AccountId": &account_id, "BudgetName": &name}),
                )
                .await;
                let found = read_notification(&body, &notification);
                assert_eq!(
                    named_threshold_type(&found),
                    json!(threshold_type),
                    "case {case:?}: expected the notification to read back, got {body}"
                );
            }
        }
    }
}

/// One row of the identity table: the member that differs from the stored
/// notification, and the notification that differs in it.
struct Distinct {
    case: &'static str,
    notification: Value,
}

/// RULE 2: a notification is identified by what it alerts on, so creating one the
/// budget already holds is refused as a `DuplicateRecordException` and leaves the
/// stored subscribers alone, while a notification differing in ANY of those members is
/// a notification of its own.
///
/// The rows differ along which identifying member differs: the notification type, the
/// comparison operator, the threshold and the threshold type. An ABSENT
/// `ThresholdType` is NOT a difference — "thresholds are always a percentage"
/// (`Notification$Threshold`), so it names the same notification as an explicit
/// `PERCENTAGE` (`crate::state::THRESHOLD_TYPE_PERCENTAGE`), and so is a repeat; so is
/// a `NotificationState` the stored one does not carry, since the alarm state is the
/// service's report and not part of the identity.
#[tokio::test]
async fn what_a_notification_alerts_on_is_its_identity() {
    let router = router();
    let account_id = caller_account();
    let name = "identity";
    seed_budget(&router, &account_id, name).await;

    let stored = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": 80,
        "ThresholdType": "PERCENTAGE"
    });
    let (status, body) = call(
        &router,
        "CreateNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &stored,
            "Subscribers": [email_subscriber(0)]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateNotification to answer 200, got {status} {body}"
    );

    // A repeat is the duplicate record, whatever subscribers it carries, and the ones
    // already stored are not replaced. The rows differ along what the repeat varies
    // that is NOT part of the identity: the subscribers, the alarm state, and an
    // absent `ThresholdType` standing for the percentage the stored one names.
    let percentage = json!(THRESHOLD_TYPE_PERCENTAGE);
    for (case, repeat, subscribers) in [
        (
            "the same notification and the same subscribers",
            stored.clone(),
            json!([email_subscriber(0)]),
        ),
        (
            "the same notification with different subscribers",
            stored.clone(),
            json!([sns_subscriber()]),
        ),
        (
            "a notification state the first create did not carry",
            json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "ThresholdType": &percentage,
                "NotificationState": "ALARM"
            }),
            json!([email_subscriber(9)]),
        ),
        (
            "no threshold type, which is the percentage the stored one names",
            notification("ACTUAL", "GREATER_THAN", json!(80)),
            json!([email_subscriber(8)]),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": &repeat,
                "Subscribers": &subscribers
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a repeat to be refused at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "DuplicateRecordException"),
            "case {case:?}: expected DuplicateRecordException, got {body}"
        );
    }
    let (_, body) = call(
        &router,
        "DescribeSubscribersForNotification",
        &json!({"AccountId": &account_id, "BudgetName": name, "Notification": &stored}),
    )
    .await;
    assert!(
        same(&body["Subscribers"], &json!([email_subscriber(0)])),
        "expected a refused repeat to leave the stored subscribers alone, got {body}"
    );

    // A difference in any identifying member is a different notification.
    let rows = vec![
        Distinct {
            case: "a different notification type",
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            }),
        },
        Distinct {
            case: "a different comparison operator",
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80,
                "ThresholdType": "PERCENTAGE"
            }),
        },
        Distinct {
            case: "a different threshold",
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 81,
                "ThresholdType": "PERCENTAGE"
            }),
        },
        Distinct {
            case: "a different threshold type",
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
        },
        Distinct {
            case: "an absolute threshold type where the stored one has none at all",
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 42,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
        },
        Distinct {
            case: "the same alert with no threshold type, so the percentage one",
            notification: notification("FORECASTED", "EQUAL_TO", json!(42)),
        },
    ];
    let mut held = 1;
    for row in rows {
        let Distinct { case, notification } = row;
        let subscribers = json!([email_subscriber(held)]);
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": &notification,
                "Subscribers": &subscribers
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a distinct notification to be accepted, got {status} {body}"
        );
        held += 1;
        let (_, body) = call(
            &router,
            "DescribeNotificationsForBudget",
            &json!({"AccountId": &account_id, "BudgetName": name}),
        )
        .await;
        assert_eq!(
            body["Notifications"].as_array().map(Vec::len),
            Some(held),
            "case {case:?}: expected the budget to hold {held} notifications, got {body}"
        );
        // And it is addressable on its own: its subscribers, not the stored one's.
        let (status, body) = call(
            &router,
            "DescribeSubscribersForNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": &notification
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected its subscribers to be readable, got {status} {body}"
        );
        assert!(
            same(&body["Subscribers"], &subscribers),
            "case {case:?}: expected {subscribers}, got {body}"
        );
    }
}

/// RULE 3: a notification belongs to a budget — "you must create the budget before you
/// create the associated notification" — so a call naming a budget the account does not
/// hold is refused with `NotFoundException` and stores nothing. Reading the subscribers of
/// a notification THAT budget does not hold is not such a call: the notification holds no
/// subscribers, which the read reports as the empty set.
///
/// The rows differ along WHAT is missing: the budget name, and the notification itself.
#[tokio::test]
async fn a_notification_needs_the_budget_that_owns_it() {
    let router = router();
    let account_id = caller_account();
    let name = "owned";
    seed_budget(&router, &account_id, name).await;
    let present = notification("ACTUAL", "GREATER_THAN", json!(80));
    let absent = notification("FORECASTED", "LESS_THAN", json!(20));
    let (status, _) = call(
        &router,
        "CreateNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &present,
            "Subscribers": [email_subscriber(0)]
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: expected CreateNotification to answer 200");

    for (case, operation, request) in [
        (
            "create for a budget name the account does not hold",
            "CreateNotification",
            json!({
                "AccountId": &account_id,
                "BudgetName": "no-such-budget",
                "Notification": &present,
                "Subscribers": [email_subscriber(0)]
            }),
        ),
        (
            "create for a budget name whose case does not match the stored one",
            "CreateNotification",
            json!({
                "AccountId": &account_id,
                "BudgetName": name.to_uppercase(),
                "Notification": &present,
                "Subscribers": [email_subscriber(0)]
            }),
        ),
        (
            "read subscribers for a budget that does not exist",
            "DescribeSubscribersForNotification",
            json!({
                "AccountId": &account_id,
                "BudgetName": "no-such-budget",
                "Notification": &present
            }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to be refused at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "NotFoundException"),
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The notification the budget does not hold is the OTHER half of the rule: the budget
    // exists, so there is a subscriber set to report for the notification addressed, and
    // that set is empty — `Subscribers` is `@length(min: 1)`, so the empty set is the
    // member left out.
    let (status, body) = call(
        &router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &absent
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"read subscribers of a notification the budget does not hold\": expected 200, \
         got {status} {body}"
    );
    assert!(
        body.get("Subscribers").is_none_or(Value::is_null),
        "case \"read subscribers of a notification the budget does not hold\": expected no \
         Subscribers member, got {body}"
    );

    // Nothing the refusals named was created: the budget still holds exactly the one
    // notification, and no budget was brought into being by a refused call.
    let (_, body) = call(
        &router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": &account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        body["Notifications"].as_array().map(Vec::len),
        Some(1),
        "expected the refusals to store nothing, got {body}"
    );
    let (_, body) = call(
        &router,
        "DescribeBudgets",
        &json!({"AccountId": &account_id}),
    )
    .await;
    assert_eq!(
        body["Budgets"].as_array().map(Vec::len),
        Some(1),
        "expected the account to still hold only the one seeded budget, got {body}"
    );
}

/// RULE 4, the notification limit: "a budget can have up to ten notifications", so the
/// tenth is accepted and the eleventh is refused with `CreationLimitExceededException`
/// — and the refusal leaves the ten alone.
#[tokio::test]
async fn a_budget_holds_at_most_the_documented_number_of_notifications() {
    let router = router();
    let account_id = caller_account();
    let name = "crowded";
    seed_budget(&router, &account_id, name).await;

    // Each notification differs in its threshold, so each is a notification of its own
    // rather than a repeat.
    for index in 0..MAX_NOTIFICATIONS_PER_BUDGET {
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": notification("ACTUAL", "GREATER_THAN", json!(index + 1)),
                "Subscribers": [email_subscriber(index)]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case notification {index}: expected 200 below the limit of {MAX_NOTIFICATIONS_PER_BUDGET}, \
             got {status} {body}"
        );
    }
    let over = notification(
        "ACTUAL",
        "GREATER_THAN",
        json!(MAX_NOTIFICATIONS_PER_BUDGET + 1),
    );
    let (status, body) = call(
        &router,
        "CreateNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &over,
            "Subscribers": [email_subscriber(99)]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the limit: expected the refusal at 400, got {status} {body}"
    );
    assert!(
        is_error(&body, "CreationLimitExceededException"),
        "case one past the limit: expected CreationLimitExceededException, got {body}"
    );
    let (_, body) = call(
        &router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": &account_id, "BudgetName": name, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        body["Notifications"].as_array().map(Vec::len),
        Some(MAX_NOTIFICATIONS_PER_BUDGET),
        "expected the refusal to leave the stored notifications alone, got {body}"
    );
}

/// RULE 4, the subscriber limits: "a notification can have one SNS subscriber and up
/// to 10 email subscribers, for a total of 11 subscribers". The per-type halves are
/// refused by the service with `CreationLimitExceededException`; the total and the
/// at-least-one are the modelled `@length(1, 11)` on `Subscribers`, refused at request
/// validation.
///
/// The rows sit on each published bound and one past it, per subscription type.
#[tokio::test]
async fn a_notification_holds_at_most_the_documented_subscribers_of_each_type() {
    let router = router();
    let account_id = caller_account();
    let name = "subscribed";
    seed_budget(&router, &account_id, name).await;

    // The subscription types this table spans are the model's own catalog.
    assert_eq!(
        SUBSCRIPTION_TYPE_VALUES,
        &["EMAIL", "SNS"],
        "the subscription-type catalog this table spans changed: {SUBSCRIPTION_TYPE_VALUES:?}"
    );

    let mut sns_and_email = vec![sns_subscriber()];
    sns_and_email.extend((0..MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION).map(email_subscriber));
    let mut two_sns = vec![sns_subscriber()];
    two_sns.push(json!({
        "SubscriptionType": "SNS",
        "Address": format!("arn:aws:sns:us-east-1:{}:other-alerts", caller_account())
    }));

    // (case, subscribers, the error code expected, or None for "accepted")
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "one email, the published minimum",
            email_subscribers(1),
            None,
        ),
        (
            "the email maximum",
            email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION),
            None,
        ),
        (
            "one SNS, its maximum",
            json!([sns_subscriber()]),
            None,
        ),
        (
            "one SNS and the email maximum: the total of 11",
            Value::Array(sns_and_email),
            None,
        ),
        (
            "one past the SNS maximum",
            Value::Array(two_sns),
            Some("CreationLimitExceededException"),
        ),
        (
            "one past the email maximum",
            email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + 1),
            Some("CreationLimitExceededException"),
        ),
        (
            "one past the modelled total",
            email_subscribers(MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION + 2),
            Some("ValidationException"),
        ),
        (
            "fewer than the modelled minimum",
            json!([]),
            Some("ValidationException"),
        ),
    ];

    for (index, (case, subscribers, refusal)) in rows.into_iter().enumerate() {
        // A threshold per row, so no row is refused as a repeat of another.
        let notification = notification("FORECASTED", "GREATER_THAN", json!(index + 1));
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": &notification,
                "Subscribers": &subscribers
            }),
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the set to be accepted, got {status} {body}"
                );
                let (_, body) = call(
                    &router,
                    "DescribeSubscribersForNotification",
                    &json!({
                        "AccountId": &account_id,
                        "BudgetName": name,
                        "Notification": &notification
                    }),
                )
                .await;
                assert!(
                    same(&body["Subscribers"], &subscribers),
                    "case {case:?}: expected the accepted set to read back, got {body}"
                );
            }
            Some(exception) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the refusal at 400, got {status} {body}"
                );
                assert!(
                    is_error(&body, exception),
                    "case {case:?}: expected {exception}, got {body}"
                );
                // A refused create stores nothing: the notification is not there, so the
                // read of its subscribers reports the empty set (`Subscribers` is
                // `@length(min: 1)`, so the empty set is the member left out).
                let (status, body) = call(
                    &router,
                    "DescribeSubscribersForNotification",
                    &json!({
                        "AccountId": &account_id,
                        "BudgetName": name,
                        "Notification": &notification
                    }),
                )
                .await;
                assert!(
                    status == 200 && body.get("Subscribers").is_none_or(Value::is_null),
                    "case {case:?}: expected a refused create to store nothing, \
                     got {status} {body}"
                );
            }
        }
    }
    assert_eq!(
        MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION, 1,
        "the documented SNS limit this table spans changed"
    );
}

/// RULE 5: reading a notification's subscribers is a pure, paginated read — every page
/// is at most `MaxResults` long, the pages concatenate to the stored order with no
/// repeats and no gaps, the last page carries no token, a token this service never
/// minted is `InvalidNextTokenException`, and reading changes nothing a later caller
/// sees.
///
/// The rows differ along the page size: 1, a size that divides the list evenly, one
/// that does not, the whole list, more than the whole list, and no `MaxResults` at all.
#[tokio::test]
async fn reading_subscribers_pages_without_changing_anything() {
    let router = router();
    let account_id = caller_account();
    let name = "paged";
    seed_budget(&router, &account_id, name).await;

    // The full 11: one SNS and ten email, the modelled maximum.
    let mut all = vec![sns_subscriber()];
    all.extend((0..MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION).map(email_subscriber));
    let subscribers = Value::Array(all);
    let notification = notification("ACTUAL", "GREATER_THAN", json!(80));
    let (status, body) = call(
        &router,
        "CreateNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &notification,
            "Subscribers": &subscribers
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateNotification to answer 200, got {status} {body}"
    );
    let total = subscribers.as_array().expect("a list").len();

    for page_size in [Some(1), Some(11), Some(4), Some(12), Some(100), None] {
        let case = format!("MaxResults {page_size:?}");
        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": &notification
            });
            if let Some(max) = page_size {
                request["MaxResults"] = json!(max);
            }
            if let Some(next) = &token {
                request["NextToken"] = json!(next);
            }
            let (status, body) = call(&router, "DescribeSubscribersForNotification", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            let page = body["Subscribers"].as_array().cloned().unwrap_or_default();
            assert!(
                !page.is_empty(),
                "case {case:?}: expected a non-empty page, got {body}"
            );
            if let Some(max) = page_size {
                assert!(
                    page.len() <= max,
                    "case {case:?}: expected at most {max} per page, got {}",
                    page.len()
                );
            }
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: expected pagination to terminate, got {pages} pages"
            );
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            same(&Value::Array(collected.clone()), &subscribers),
            "case {case:?}: expected the pages to concatenate to the stored order, \
             got {collected:?}"
        );
    }

    // A token this service never minted is refused, and tells the two token errors
    // apart: it is invalid, not expired.
    for bogus in ["not-a-token", "6e6f7065"] {
        let (status, body) = call(
            &router,
            "DescribeSubscribersForNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": name,
                "Notification": &notification,
                "NextToken": bogus
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case token {bogus:?}: expected the refusal at 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidNextTokenException"),
            "case token {bogus:?}: expected InvalidNextTokenException, got {body}"
        );
    }

    // The read changed nothing: the same call answers the same thing, and the budget
    // still holds exactly the one notification.
    let (_, first) = call(
        &router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &notification
        }),
    )
    .await;
    let (_, again) = call(
        &router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": name,
            "Notification": &notification
        }),
    )
    .await;
    assert!(
        same(&first, &again),
        "expected a repeated read to answer the same thing, got {first} then {again}"
    );
    let (_, body) = call(
        &router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": &account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        body["Notifications"].as_array().map(Vec::len),
        Some(1),
        "expected reading subscribers to leave the notification list alone, got {body}"
    );
}
