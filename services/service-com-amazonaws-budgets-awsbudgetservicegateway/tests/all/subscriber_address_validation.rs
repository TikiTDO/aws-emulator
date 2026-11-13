// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ADDRESS of a budget subscriber, driven over the wire
//! through the same `MultiServiceRouter` the production router serves from.
//!
//! The RULE every test here asserts: a `Subscriber` "consists of a subscription type and
//! either an Amazon SNS topic or an email address" (`Subscriber`), and its `Address` is
//! "the address that Amazon Web Services sends budget notifications to, either an SNS
//! topic or an email" which, "when you create a subscriber, … can't contain line breaks"
//! (`Subscriber$Address`) — so every write that establishes a subscriber takes an address
//! only in the form that subscriber's own `SubscriptionType` names, and refuses any other
//! with the client error each of those operations models (`InvalidParameterException`,
//! HTTP 400) while persisting nothing. The modelled `@pattern`
//! `^(.*[\n\r\t\f\ ]?)*$` on `SubscriberAddress` admits any text at all, line breaks
//! included, so nothing upstream of the handler refuses these.
//!
//! The tables span the class rather than one request: the malformed rows differ along
//! WHICH way the address fails (no `@`, no domain, a single-label domain, an all-numeric
//! top-level domain, internal whitespace, the other type's form, another vendor's ARN, an
//! ARN naming something beneath a topic, an ARN whose Region field names no Region SNS can
//! be addressed in, an ARN naming a topic of another account, and a line feed or a
//! carriage return under either type), the
//! well-formed rows along the forms each type admits (mailbox shapes and subdomains,
//! partitions, Regions up to the published bound, a topic name with a suffix), and both
//! are driven through EVERY write that establishes a subscriber: `CreateBudget`'s
//! `NotificationsWithSubscribers`, `CreateNotification`, `CreateSubscriber`,
//! `UpdateSubscriber`, `CreateBudgetAction` and `UpdateBudgetAction`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The subscription-type catalog is not re-typed here: it is the model-derived constant
/// the scaffold generated into `src/ops/types/shared_types.rs`, so a table built from it
/// cannot drift from the model.
use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::SUBSCRIPTION_TYPE_VALUES;
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::ComAmazonawsBudgetsAwsbudgetservicegatewayState;

use super::budget_action_execution::{
    definition_for, describe_action, seed_action, seed_budget, simple_create_body,
};
use super::budget_action_subscriber_limits::stored_action_count;
use super::budget_lifecycle::{account, caller_account, call, is_error, router_with_engine, same};
use super::client_error_envelopes::message;
use super::recorded_response_members::as_action_subscribers;

/// The error every write that establishes a subscriber answers a malformed address with:
/// "An error on the client occurred. Typically, the cause is an invalid input value."
pub(crate) const REFUSAL: &str = "InvalidParameterException";

/// One row of the malformed table: the subscriber a write would establish, and the way
/// its address fails the rule.
pub(crate) struct Malformed {
    pub(crate) case: &'static str,
    pub(crate) subscriber: Value,
}

pub(crate) fn subscriber(subscription_type: &str, address: &str) -> Value {
    json!({"SubscriptionType": subscription_type, "Address": address})
}

/// The malformed table. Every row is a value the modelled `@pattern` and `@length` admit,
/// so each one reaches the handler.
pub(crate) fn malformed_rows() -> Vec<Malformed> {
    let rows: Vec<(&'static str, &'static str, &'static str)> = vec![
        ("EMAIL with no @ at all", "EMAIL", "not-an-email"),
        ("EMAIL with an empty local part", "EMAIL", "@example.com"),
        ("EMAIL with no domain after the @", "EMAIL", "ops@"),
        ("EMAIL with a single-label domain", "EMAIL", "ops@localhost"),
        ("EMAIL with an empty domain label", "EMAIL", "ops@example..com"),
        (
            "EMAIL with an all-numeric top-level domain",
            "EMAIL",
            "ops@example.123",
        ),
        (
            "EMAIL carrying whitespace inside the local part",
            "EMAIL",
            "ops team@example.com",
        ),
        (
            "EMAIL carrying a second @",
            "EMAIL",
            "ops@team@example.com",
        ),
        (
            "EMAIL whose address is an SNS topic ARN",
            "EMAIL",
            "arn:aws:sns:us-east-1:111122223333:budget-alerts",
        ),
        ("SNS whose address is an email address", "SNS", "team@example.com"),
        ("SNS whose address is not an ARN at all", "SNS", "budget-alerts"),
        (
            "SNS naming another vendor's resource",
            "SNS",
            "arn:aws:sqs:us-east-1:111122223333:budget-alerts",
        ),
        (
            "SNS with no resource field at all",
            "SNS",
            "arn:aws:sns:us-east-1:111122223333:",
        ),
        (
            "SNS naming a topic no account owns",
            "SNS",
            "arn:aws:sns:us-east-1::budget-alerts",
        ),
        (
            "SNS naming a subscription beneath a topic rather than the topic",
            "SNS",
            "arn:aws:sns:us-east-1:111122223333:budget-alerts:9f2b1c44-0000-4a0e-9c66-7f3f1a2b3c4d",
        ),
        // Amazon SNS is a REGIONAL service, so a topic lives in a Region and a value whose
        // Region field names none names no topic. The grammar the rows below step outside
        // of is the one `service_common::arn::region_label` reads — non-empty, at most 20
        // characters, `[a-z0-9-]` only — which is the Region field of every `*Arn`
        // `@pattern` in the AWS models.
        (
            "SNS whose Region field is empty, so it names a partition rather than a Region",
            "SNS",
            "arn:aws:sns::111122223333:budget-alerts",
        ),
        (
            "SNS whose Region field is in upper case",
            "SNS",
            "arn:aws:sns:US-EAST-1:111122223333:budget-alerts",
        ),
        (
            "SNS whose Region field is one character past the published bound",
            "SNS",
            "arn:aws:sns:us-east-1-us-east-1xy:111122223333:budget-alerts",
        ),
        (
            "SNS whose Region field carries a host label after a path separator",
            "SNS",
            "arn:aws:sns:us-east-1/evil.example.com:111122223333:budget-alerts",
        ),
        (
            "SNS carrying whitespace in the topic name",
            "SNS",
            "arn:aws:sns:us-east-1:111122223333:budget alerts",
        ),
        (
            "EMAIL carrying a line feed",
            "EMAIL",
            "ops@example.com\nbcc@example.com",
        ),
        (
            "EMAIL carrying a carriage return",
            "EMAIL",
            "ops@example.com\r",
        ),
        (
            "SNS carrying a line feed",
            "SNS",
            "arn:aws:sns:us-east-1:111122223333:budget-alerts\n",
        ),
        (
            "SNS carrying a carriage return",
            "SNS",
            "arn:aws:sns:us-east-1:111122223333:budget\r-alerts",
        ),
    ];
    rows.into_iter()
        .map(|(case, subscription_type, address)| Malformed {
            case,
            subscriber: subscriber(subscription_type, address),
        })
        // An `SNS` address also has to name a topic of the account the subscriber is
        // established under — "the address that Amazon Web Services sends budget
        // notifications to" is a topic of the account that owns the budget — so a
        // well-formed topic ARN of any other account is an address no notification of
        // this account's budgets is sent to. Built from the account helper rather than
        // written out, so the row cannot accidentally name the account this emulator
        // answers for. `tests/all/subscriber_topic_account.rs` spans that rule.
        .chain(::std::iter::once(Malformed {
            case: "SNS naming a topic of an account other than the caller's",
            subscriber: subscriber(
                "SNS",
                &format!(
                    "arn:aws:sns:us-east-1:{}:budget-alerts",
                    account(111_122_223_333)
                ),
            ),
        }))
        .collect()
}

/// The well-formed table: the address forms each subscription type admits. Every `SNS`
/// row names a topic of the caller's own account, the only account whose topics a
/// notification of its budgets is published to
/// (`tests/all/subscriber_topic_account.rs`), so the rows differ along the address FORM
/// alone.
pub(crate) fn well_formed_rows() -> Vec<(&'static str, Value)> {
    let owner = caller_account();
    vec![
        (
            "an email address",
            subscriber("EMAIL", "ops@example.com"),
        ),
        (
            "an email address with a tag and a subdomain chain",
            subscriber("EMAIL", "budget-ops+monthly@team.example.co.uk"),
        ),
        (
            "the shortest email address the form admits",
            subscriber("EMAIL", "a@b.co"),
        ),
        (
            "an email address under an internationalised top-level domain",
            subscriber("EMAIL", "ops@example.xn--p1ai"),
        ),
        (
            "an SNS topic ARN",
            subscriber("SNS", &format!("arn:aws:sns:us-east-1:{owner}:budget-alerts")),
        ),
        (
            "an SNS topic ARN in another partition and Region",
            subscriber(
                "SNS",
                &format!("arn:aws-cn:sns:cn-north-1:{owner}:budget-alerts"),
            ),
        ),
        (
            "an SNS topic ARN whose Region field is exactly at the published bound",
            subscriber(
                "SNS",
                &format!("arn:aws:sns:us-east-1-us-east-1x:{owner}:budget-alerts"),
            ),
        ),
        (
            "an SNS topic ARN whose topic name carries a suffix",
            subscriber(
                "SNS",
                &format!("arn:aws:sns:us-east-1:{owner}:budget-alerts.fifo"),
            ),
        ),
    ]
}

/// The notification at `position` of a table: what a notification alerts on identifies
/// it, so each row gets its own threshold and so its own notification.
pub(crate) fn notification(position: usize) -> Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "GREATER_THAN",
        "Threshold": position as f64 + 1.0
    })
}

/// The subscriber every seeded notification and action starts with — `Subscribers` is
/// `@length(min: 1)`, so neither can be established without one.
pub(crate) fn seed_subscriber() -> Value {
    subscriber("EMAIL", "seed@example.com")
}

/// Every subscriber the STORE holds for a budget, as `(SubscriptionType, Address)` per
/// notification in notification order — read from the state struct directly, so what a
/// refused write did to the store is asserted independently of any read's projection.
pub(crate) fn stored_subscribers(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> Vec<Vec<(String, String)>> {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .get(&(account_id.to_string(), budget_name.to_string()))
        .map(|stored| {
            stored
                .notifications
                .iter()
                .map(|held| {
                    held.subscribers
                        .iter()
                        .map(|subscriber| {
                            (
                                subscriber.subscription_type.clone(),
                                subscriber.address.clone(),
                            )
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the store holds a budget under `(account_id, budget_name)` at all.
fn budget_is_stored(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    budget_name: &str,
) -> bool {
    let state = engine.state::<ComAmazonawsBudgetsAwsbudgetservicegatewayState>();
    let store = state.lock();
    store
        .budgets
        .contains_key(&(account_id.to_string(), budget_name.to_string()))
}

/// The subscribers `DescribeSubscribersForNotification` reports for one notification.
pub(crate) async fn reported_subscribers(
    router: &MultiServiceRouter,
    account_id: &str,
    budget_name: &str,
    notification: &Value,
) -> Vec<Value> {
    let (status, body) = call(
        router,
        "DescribeSubscribersForNotification",
        &json!({
            "AccountId": account_id,
            "BudgetName": budget_name,
            "Notification": notification
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the subscribers of {budget_name:?} back: expected 200, got {status} {body}"
    );
    body["Subscribers"].as_array().cloned().unwrap_or_default()
}

/// Assert one refused write: HTTP 400, the modelled client error, and a message that does
/// not echo the address — `Subscriber$Address` is `@sensitive`, so a refusal that repeated
/// it would put a caller's mailbox or topic in every error body and log.
pub(crate) fn assert_refused(case: &str, address: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the write to be refused at 400, got {status} {body}"
    );
    assert!(
        is_error(body, REFUSAL),
        "case {case:?}: expected {REFUSAL}, got {body}"
    );
    let reported = message(body);
    assert!(
        !reported.contains(address),
        "case {case:?}: expected the refusal not to echo the sensitive address, got \
         {reported:?}"
    );
}

/// The subscription types these tables span are the model's own catalog, so a catalog
/// that grew a third type fails the suite instead of going silently untested.
pub(crate) fn assert_catalog_spanned() {
    assert_eq!(
        SUBSCRIPTION_TYPE_VALUES,
        &["EMAIL", "SNS"],
        "the subscription-type catalog this suite spans changed: {SUBSCRIPTION_TYPE_VALUES:?}"
    );
}

/// RULE, on the three notification write paths: `CreateNotification`, `CreateSubscriber`
/// and `CreateBudget`'s `NotificationsWithSubscribers` each refuse an address that is not
/// what its subscriber's `SubscriptionType` names, and each leaves the store exactly as it
/// was.
#[tokio::test]
async fn a_notification_write_refuses_an_address_its_type_does_not_name() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "address-rules";
    seed_budget(&router, &account_id, budget_name).await;

    // One notification to grow with `CreateSubscriber`, holding one well-formed
    // subscriber.
    let seeded = notification(0);
    let (status, body) = call(
        &router,
        "CreateNotification",
        &json!({
            "AccountId": &account_id,
            "BudgetName": budget_name,
            "Notification": &seeded,
            "Subscribers": [seed_subscriber()]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateNotification to answer 200, got {status} {body}"
    );
    let untouched = stored_subscribers(&engine, &account_id, budget_name);

    for (index, row) in malformed_rows().into_iter().enumerate() {
        let case = row.case;
        let address = row.subscriber["Address"].as_str().unwrap_or_default().to_string();

        // `CreateNotification`: the whole notification is refused, so the budget keeps
        // the one notification it held.
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": notification(index + 1),
                "Subscribers": [&row.subscriber]
            }),
        )
        .await;
        assert_refused(case, &address, status, &body);

        // `CreateSubscriber`: the notification keeps the subscribers it had.
        let (status, body) = call(
            &router,
            "CreateSubscriber",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": &seeded,
                "Subscriber": &row.subscriber
            }),
        )
        .await;
        assert_refused(case, &address, status, &body);

        // `UpdateSubscriber`, which establishes the subscriber it puts in the old one's
        // place: the notification keeps the subscriber it held.
        let (status, body) = call(
            &router,
            "UpdateSubscriber",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": &seeded,
                "OldSubscriber": seed_subscriber(),
                "NewSubscriber": &row.subscriber
            }),
        )
        .await;
        assert_refused(case, &address, status, &body);

        let held = stored_subscribers(&engine, &account_id, budget_name);
        assert_eq!(
            held, untouched,
            "case {case:?}: expected the refused writes to leave the store at \
             {untouched:?}, got {held:?}"
        );
        let reported =
            reported_subscribers(&router, &account_id, budget_name, &seeded).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the read to report only the seeded subscriber, \
             got {reported:?}"
        );

        // `CreateBudget`, which "creates a budget and, if included, notifications and
        // subscribers": the budget is created whole or not at all, so nothing is stored
        // under its name.
        let created = format!("address-create-{index}");
        let (status, body) = call(
            &router,
            "CreateBudget",
            &json!({
                "AccountId": &account_id,
                "Budget": {
                    "BudgetName": &created,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "100", "Unit": "USD"}
                },
                "NotificationsWithSubscribers": [{
                    "Notification": notification(index + 1),
                    "Subscribers": [&row.subscriber]
                }]
            }),
        )
        .await;
        assert_refused(case, &address, status, &body);
        assert!(
            !budget_is_stored(&engine, &account_id, &created),
            "case {case:?}: expected a refused CreateBudget to store no budget"
        );
    }
}

/// RULE, on the two budget-action write paths: a budget action's subscriber set is the
/// same modelled `Subscribers` shape, so `CreateBudgetAction` and `UpdateBudgetAction`
/// hold every address in it to the same rule — and a refused write mints no action and
/// leaves the stored one's set alone.
#[tokio::test]
async fn a_budget_action_write_refuses_an_address_its_type_does_not_name() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "address-rules-action";
    seed_budget(&router, &account_id, budget_name).await;
    let action_id = seed_action(&router, &account_id, budget_name).await;
    let held = describe_action(&router, &account_id, budget_name, &action_id)
        .await
        .1["Action"]["Subscribers"]
        .clone();

    for row in malformed_rows() {
        let case = row.case;
        let address = row.subscriber["Address"].as_str().unwrap_or_default().to_string();
        let before = stored_action_count(&engine, &account_id, budget_name);

        let (status, body) = call(
            &router,
            "CreateBudgetAction",
            &super::budget_action_execution::create_body(
                &account_id,
                budget_name,
                "APPLY_SCP_POLICY",
                &definition_for("APPLY_SCP_POLICY"),
                "AUTOMATIC",
                "FORECASTED",
                &json!({"ActionThresholdValue": 50.0, "ActionThresholdType": "PERCENTAGE"}),
                &json!([&row.subscriber]),
                None,
            ),
        )
        .await;
        assert_refused(case, &address, status, &body);
        assert_eq!(
            stored_action_count(&engine, &account_id, budget_name),
            before,
            "case {case:?}: expected a refused CreateBudgetAction to mint no action"
        );

        let (status, body) = call(
            &router,
            "UpdateBudgetAction",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "ActionId": &action_id,
                "Subscribers": [&row.subscriber]
            }),
        )
        .await;
        assert_refused(case, &address, status, &body);
        let (status, body) = describe_action(&router, &account_id, budget_name, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the action to still read back, got {status} {body}"
        );
        assert!(
            same(&body["Action"]["Subscribers"], &held),
            "case {case:?}: expected the stored set to stay {held}, got {}",
            body["Action"]["Subscribers"]
        );
    }
}

/// RULE, from the other side: an address that IS what its subscription type names is
/// accepted and read back as supplied — through a notification
/// (`DescribeSubscribersForNotification`) and through a budget action
/// (`DescribeBudgetAction`), so the rule refuses malformed addresses without narrowing
/// the forms each type admits.
#[tokio::test]
async fn a_well_formed_address_of_either_type_is_stored_and_read_back() {
    assert_catalog_spanned();
    let (router, engine) = router_with_engine();
    let account_id = caller_account();
    let budget_name = "address-accepted";
    seed_budget(&router, &account_id, budget_name).await;

    for (index, (case, row)) in well_formed_rows().into_iter().enumerate() {
        let alerts_on = notification(index);
        let (status, body) = call(
            &router,
            "CreateNotification",
            &json!({
                "AccountId": &account_id,
                "BudgetName": budget_name,
                "Notification": &alerts_on,
                "Subscribers": [&row]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateNotification to answer 200, got {status} {body}"
        );
        let reported =
            reported_subscribers(&router, &account_id, budget_name, &alerts_on).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the read to report one subscriber, got {reported:?}"
        );
        assert!(
            same(&reported[0], &row),
            "case {case:?}: expected the subscriber to read back as supplied ({row}), \
             got {}",
            reported[0]
        );
        let stored = stored_subscribers(&engine, &account_id, budget_name);
        let expected = (
            row["SubscriptionType"].as_str().unwrap_or_default().to_string(),
            row["Address"].as_str().unwrap_or_default().to_string(),
        );
        assert_eq!(
            stored[index],
            vec![expected.clone()],
            "case {case:?}: expected the store to hold {expected:?}, got {:?}",
            stored[index]
        );

        // The same address through a budget action, whose subscriber set is the same
        // modelled shape.
        let action_budget = format!("address-accepted-action-{index}");
        seed_budget(&router, &account_id, &action_budget).await;
        let mut create = simple_create_body(&account_id, &action_budget);
        create["Subscribers"] = json!([&row]);
        let (status, body) = call(&router, "CreateBudgetAction", &create).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudgetAction to answer 200, got {status} {body}"
        );
        let action_id = body["ActionId"].as_str().unwrap_or_default().to_string();
        let (status, body) = describe_action(&router, &account_id, &action_budget, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the action to read back, got {status} {body}"
        );
        let expected_subscribers = as_action_subscribers(&json!([&row]));
        assert!(
            same(&body["Action"]["Subscribers"], &expected_subscribers),
            "case {case:?}: expected the action to hold the subscriber as supplied \
             ({expected_subscribers}), got {}",
            body["Action"]["Subscribers"]
        );
    }
}
