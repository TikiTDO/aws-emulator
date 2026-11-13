// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ACCOUNT an `SNS` subscriber's topic belongs to: the write
//! that establishes such a subscriber and the alert published to it read the address the
//! same way, so an address a write accepts is one the notification's alert actually
//! reaches.
//!
//! The RULE asserted here: `Subscriber$Address` is "the address that Amazon Web Services
//! sends budget notifications to, either an SNS topic or an email", and a budget's
//! notifications are sent to the topics of the account that owns the budget — the
//! `AccountId` every operation names, which is the only account a budgets caller reaches.
//! An address whose ARN names a topic of any other account is therefore an address no
//! notification of this budget is sent to, however well-formed that ARN is: every write
//! that establishes a subscriber refuses it with the client error each of them models
//! (`InvalidParameterException`, HTTP 400) and stores nothing, and every `SNS` address
//! those writes DO accept is one the alert of a notification in alarm is published to.
//! It is the same shape as the rule the other caller-supplied ARN this service holds is
//! checked against before it is stored (`Action$ExecutionRoleArn`: "roles and actions
//! must be in the same account", `tests/all/budget_action_role_account_scope.rs`).
//!
//! That pairing is the point: the acceptance and the delivery halves are asserted
//! together, over one table, so neither half can admit an address the other drops. The
//! rows differ along the ACCOUNT field of the address — the caller's own account in two
//! Regions, against another twelve-digit account, the caller's account off by its last
//! digit, by one digit too many, by one too few, carrying a character before or after it,
//! a field that is no account id at all, and an empty field.
//!
//! `tests/all/subscriber_sns_topic_region.rs` is the same pairing over the Region field;
//! `tests/all/subscriber_address_validation.rs` spans every way an address can fail its
//! type's form over all six writes that establish a subscriber;
//! `tests/all/notification_alert_delivery.rs` spans what an alert says and which writes
//! owe one.

use serde_json::json;
use std::time::SystemTime;

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    Budget, Notification, Spend, Subscriber,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    alerts, budget_as_read, subscriber_address_violation, THRESHOLD_TYPE_PERCENTAGE,
};

use super::budget_lifecycle::{account, caller_account, call, is_error, same};
use super::client_error_envelopes::message;
use super::notification_alert_delivery::{
    account_id, alert, cost_budget, delivered, emulator, given_topic, sns_subscriber, topic_arn,
};
use super::subscriber_address_validation::REFUSAL;

/// The topic name every row's address names, so the rows differ in their account field
/// alone.
const TOPIC: &str = "budget-alerts";

/// The Region every row's topic lives in unless the row names another.
const REGION: &str = "us-east-1";

/// A second Region, for the row that proves the account rule is read off the account
/// field alone: a topic of the caller's own account is reached in any Region.
const OTHER_REGION: &str = "eu-west-2";

/// The sentence a refusal carries when the address names no SNS topic AT ALL — the half
/// of `Subscriber`'s rule its type names ("either an Amazon SNS topic or an email
/// address"), stated about the member it was violated on.
const NO_TOPIC_REFUSAL: &str = "The Address of an SNS subscriber must be the ARN of an Amazon SNS topic.";

/// The sentence a refusal carries when the address names a well-formed topic of ANOTHER
/// account: the same rule read on the account a budget notification is sent in, naming
/// the account the REQUEST supplied and nothing from the `@sensitive` address.
fn other_account_refusal() -> String {
    format!(
        "The Address of an SNS subscriber must be the ARN of an Amazon SNS topic in account {}.",
        caller_account()
    )
}

/// What a row's address is: an address an alert is published to, or one of the two ways it
/// fails — each carrying its own sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Stored, read back, and published to.
    Published,
    /// Refused: the value names no SNS topic at all.
    NamesNoTopic,
    /// Refused: the value names a topic of an account other than the caller's.
    NamesAnotherAccount,
}

impl Verdict {
    fn accepted(self) -> bool {
        self == Verdict::Published
    }

    /// The `Message` the refusal carries, or `None` for an accepted address.
    fn refusal(self) -> Option<String> {
        match self {
            Verdict::Published => None,
            Verdict::NamesNoTopic => Some(NO_TOPIC_REFUSAL.to_string()),
            Verdict::NamesAnotherAccount => Some(other_account_refusal()),
        }
    }
}

/// One row: the account field an `SNS` subscriber's address carries, the Region its topic
/// is in, and what that address is.
struct Row {
    case: &'static str,
    account_field: String,
    region: &'static str,
    verdict: Verdict,
}

/// The table. The caller's account is the one this emulator answers for
/// (`budget_lifecycle::caller_account`), never a literal, so no row depends on which
/// twelve digits that is; the accounts that are NOT the caller's are built from the same
/// account helper (`AccountId`: `@length(12, 12)`, `^\d{12}$`).
fn rows() -> Vec<Row> {
    let owner = caller_account();
    vec![
        Row {
            case: "the caller's own account",
            account_field: owner.clone(),
            region: REGION,
            verdict: Verdict::Published,
        },
        Row {
            case: "the caller's own account, on a topic in another Region",
            account_field: owner.clone(),
            region: OTHER_REGION,
            verdict: Verdict::Published,
        },
        Row {
            case: "another twelve-digit account",
            account_field: account(111_122_223_333),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "an account differing from the caller's in its last digit",
            account_field: last_digit_changed(&owner),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "the caller's account with a thirteenth digit",
            account_field: format!("{owner}7"),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "the caller's account with its last digit cut off",
            account_field: owner[..owner.len() - 1].to_string(),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "the caller's account with a character after it",
            account_field: format!("{owner}x"),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "the caller's account with a character before it",
            account_field: format!("x{owner}"),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "an account field that is no account id at all",
            account_field: "every-account".to_string(),
            region: REGION,
            verdict: Verdict::NamesAnotherAccount,
        },
        Row {
            case: "an empty account field, which names no account to own a topic",
            account_field: String::new(),
            region: REGION,
            verdict: Verdict::NamesNoTopic,
        },
    ]
}

/// `owner` with its last digit replaced by another one, so the row differs from the
/// caller's account in exactly one position.
fn last_digit_changed(owner: &str) -> String {
    let mut digits: Vec<char> = owner.chars().collect();
    if let Some(last) = digits.last_mut() {
        *last = if *last == '7' { '3' } else { '7' };
    }
    digits.into_iter().collect()
}

/// The address a row names: the topic [`TOPIC`] in the row's Region, owned by the row's
/// account field.
fn address(row: &Row) -> String {
    format!("arn:aws:sns:{}:{}:{TOPIC}", row.region, row.account_field)
}

/// The notification every budget below holds: nothing has been spent, so a limit of 100
/// USD read against "less than 80%" is one the service holds in `ALARM` and therefore owes
/// its `SNS` subscribers an alert.
fn alerting_notification() -> serde_json::Value {
    json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "LESS_THAN",
        "Threshold": 80
    })
}

/// The alert a budget of `budget_name` publishes once its notification is in alarm.
fn expected_alert(budget_name: &str) -> serde_json::Value {
    alert(
        budget_name,
        "COST",
        "MONTHLY",
        Some(("100", "USD")),
        "ACTUAL",
        "LESS_THAN",
        80.0,
        // Named by neither the notification nor the budget: "thresholds are always a
        // percentage", so a notification naming no `ThresholdType` alerts as the
        // percentage it means.
        THRESHOLD_TYPE_PERCENTAGE,
    )
}

/// The seed subscriber a notification is created with: `Subscribers` is `@length(min: 1)`,
/// and an email mailbox is an address this emulator hosts no transport for, so it adds no
/// alert of its own.
fn seed_subscriber() -> serde_json::Value {
    json!({"SubscriptionType": "EMAIL", "Address": "ops@example.com"})
}

/// RULE, on the two halves at once: the address gate accepts exactly the addresses naming
/// a topic of the budget's own account, and the delivery path owes an alert for exactly
/// those — so no address is accepted by one half and dropped by the other.
///
/// Asserted where each half DECIDES (`state::subscriber_address_violation` and
/// `state::alerts::owed`), so every row is covered whether or not a topic in its account
/// could be stood up for a publish to be observed on: every topic this emulator holds is
/// minted under the one account it answers for.
#[test]
fn the_address_gate_and_the_alert_path_agree_on_which_accounts_own_a_reachable_topic() {
    let budget = Budget {
        budget_name: "topic-account".to_string(),
        budget_type: "COST".to_string(),
        time_unit: "MONTHLY".to_string(),
        budget_limit: Some(Spend {
            amount: "100".to_string(),
            unit: "USD".to_string(),
        }),
        ..Budget::default()
    };
    let source = alerts::source(&caller_account(), &budget_as_read(&budget, SystemTime::now()));
    let notification = Notification {
        notification_type: "ACTUAL".to_string(),
        comparison_operator: "LESS_THAN".to_string(),
        threshold: 80.0,
        threshold_type: None,
        notification_state: None,
        consecutive_breach_count: None,
    };

    for row in rows() {
        let address = address(&row);
        let subscribers = vec![Subscriber {
            subscription_type: "SNS".to_string(),
            address: address.clone(),
            verification_status: None,
        }];
        let refused = subscriber_address_violation(&caller_account(), &subscribers).err();
        assert_eq!(
            refused,
            row.verdict.refusal(),
            "case {:?}: expected the address gate on {address} to answer {:?}, got {refused:?}",
            row.case,
            row.verdict.refusal()
        );
        let owed = alerts::owed(&source, &notification, &subscribers);
        assert_eq!(
            owed.len() == 1,
            row.verdict.accepted(),
            "case {:?}: expected the notification in alarm to owe an alert on {address}: {}, \
             got {owed:?}",
            row.case,
            row.verdict.accepted()
        );
        if row.verdict.accepted() {
            assert_eq!(
                owed[0].topic_arn, address,
                "case {:?}: expected the alert on {address}, got it on {}",
                row.case, owed[0].topic_arn
            );
        }
    }
}

/// RULE, over the wire through `CreateBudget` — the write that "creates a budget and, if
/// included, notifications and subscribers": an address naming a topic of the budget's own
/// account is stored, read back and PUBLISHED to; an address naming any other account is
/// refused at 400 with the modelled client error and its own sentence, leaves no budget
/// behind (the budget is created whole or not at all), and publishes nothing.
#[tokio::test]
async fn create_budget_publishes_to_every_sns_address_it_accepts() {
    for (index, row) in rows().iter().enumerate() {
        let emulator = emulator();
        let budget_name = format!("topic-account-{index}");
        let address = address(row);

        // The topic a row that names one is published to has to exist for SNS to hold the
        // delivery: "the address that Amazon Web Services sends budget notifications to"
        // is a topic inside this emulator, which answers that it does not exist otherwise.
        if row.verdict.accepted() {
            given_topic(&emulator, row.region, TOPIC).await;
            assert_eq!(
                topic_arn(row.region, TOPIC),
                address,
                "case {:?}: the topic SNS minted is not the address the row names",
                row.case
            );
        }

        let mut budget = cost_budget();
        budget["BudgetName"] = json!(&budget_name);
        let (status, body) = call(
            &emulator.router,
            "CreateBudget",
            &json!({
                "AccountId": account_id(),
                "Budget": budget,
                "NotificationsWithSubscribers": [{
                    "Notification": alerting_notification(),
                    "Subscribers": [sns_subscriber(&address)]
                }]
            }),
        )
        .await;
        let found = delivered(&emulator);

        if row.verdict.accepted() {
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateBudget to accept {address}, got {status} {body}",
                row.case
            );
            let (read_status, read) = call(
                &emulator.router,
                "DescribeSubscribersForNotification",
                &json!({
                    "AccountId": account_id(),
                    "BudgetName": &budget_name,
                    "Notification": alerting_notification()
                }),
            )
            .await;
            assert_eq!(
                read_status, 200,
                "case {:?}: reading the subscribers back answered {read_status} {read}",
                row.case
            );
            let held = read["Subscribers"].as_array().cloned().unwrap_or_default();
            assert!(
                held.iter()
                    .any(|subscriber| same(subscriber, &sns_subscriber(&address))),
                "case {:?}: expected the stored set to hold the SNS subscriber, got {held:?}",
                row.case
            );
            let expected = expected_alert(&budget_name);
            assert_eq!(
                found.len(),
                1,
                "case {:?}: expected one alert on {address}, got {found:?}",
                row.case
            );
            assert_eq!(
                found[0].0, address,
                "case {:?}: expected the alert on {address}, got it on {}",
                row.case, found[0].0
            );
            assert!(
                same(&found[0].1, &expected),
                "case {:?}: expected the alert {expected}, got {}",
                row.case,
                found[0].1
            );
        } else {
            let expected = row.verdict.refusal().unwrap_or_default();
            assert_eq!(
                status, 400,
                "case {:?}: expected CreateBudget to refuse {address}, got {status} {body}",
                row.case
            );
            assert!(
                is_error(&body, REFUSAL),
                "case {:?}: expected {REFUSAL}, got {body}",
                row.case
            );
            let reported = message(&body);
            assert_eq!(
                reported, expected,
                "case {:?}: expected the refusal {expected:?}, got {reported:?}",
                row.case
            );
            // `Subscriber$Address` is `@sensitive`, so a refusal that echoed it would put
            // a caller's topic in every error body and log.
            assert!(
                !reported.contains(&address),
                "case {:?}: expected the refusal not to echo the sensitive address, got \
                 {reported:?}",
                row.case
            );
            let (read_status, read) = call(
                &emulator.router,
                "DescribeBudget",
                &json!({"AccountId": account_id(), "BudgetName": &budget_name}),
            )
            .await;
            assert!(
                is_error(&read, "NotFoundException"),
                "case {:?}: expected the refused create to leave no budget, got \
                 {read_status} {read}",
                row.case
            );
            assert!(
                found.is_empty(),
                "case {:?}: expected the refused write to publish nothing, got {found:?}",
                row.case
            );
        }
    }
}

/// RULE, over the wire through `CreateSubscriber` — the write that attaches ONE subscriber
/// to a notification a budget already holds, so a refused address is asserted against a
/// notification that stays exactly as it was.
#[tokio::test]
async fn create_subscriber_holds_every_sns_address_to_the_budgets_own_account() {
    for (index, row) in rows().iter().enumerate() {
        let emulator = emulator();
        let budget_name = format!("topic-account-growth-{index}");
        let address = address(row);

        if row.verdict.accepted() {
            given_topic(&emulator, row.region, TOPIC).await;
        }

        let mut budget = cost_budget();
        budget["BudgetName"] = json!(&budget_name);
        let (status, body) = call(
            &emulator.router,
            "CreateBudget",
            &json!({"AccountId": account_id(), "Budget": budget}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: setup CreateBudget answered {status} {body}",
            row.case
        );
        let alerts_on = alerting_notification();
        let (status, body) = call(
            &emulator.router,
            "CreateNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": &budget_name,
                "Notification": &alerts_on,
                "Subscribers": [seed_subscriber()]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: setup CreateNotification answered {status} {body}",
            row.case
        );
        let before = delivered(&emulator);
        assert!(
            before.is_empty(),
            "case {:?}: setup published an alert before the SNS subscriber existed: {before:?}",
            row.case
        );

        let (status, body) = call(
            &emulator.router,
            "CreateSubscriber",
            &json!({
                "AccountId": account_id(),
                "BudgetName": &budget_name,
                "Notification": &alerts_on,
                "Subscriber": sns_subscriber(&address)
            }),
        )
        .await;

        // What the notification holds afterwards, read back through the implemented
        // reader, so each row proves what the write left behind rather than what it
        // answered.
        let (read_status, read) = call(
            &emulator.router,
            "DescribeSubscribersForNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": &budget_name,
                "Notification": &alerts_on
            }),
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {:?}: reading the subscribers back answered {read_status} {read}",
            row.case
        );
        let held = read["Subscribers"].as_array().cloned().unwrap_or_default();
        let found = delivered(&emulator);

        if row.verdict.accepted() {
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateSubscriber to accept {address}, got {status} {body}",
                row.case
            );
            assert!(
                held.iter()
                    .any(|subscriber| same(subscriber, &sns_subscriber(&address))),
                "case {:?}: expected the stored set to hold the SNS subscriber, got {held:?}",
                row.case
            );
            let expected = expected_alert(&budget_name);
            assert_eq!(
                found.len(),
                1,
                "case {:?}: expected one alert on {address}, got {found:?}",
                row.case
            );
            assert!(
                same(&found[0].1, &expected),
                "case {:?}: expected the alert {expected}, got {}",
                row.case,
                found[0].1
            );
        } else {
            let expected = row.verdict.refusal().unwrap_or_default();
            assert_eq!(
                status, 400,
                "case {:?}: expected CreateSubscriber to refuse {address}, got {status} {body}",
                row.case
            );
            assert!(
                is_error(&body, REFUSAL),
                "case {:?}: expected {REFUSAL}, got {body}",
                row.case
            );
            let reported = message(&body);
            assert_eq!(
                reported, expected,
                "case {:?}: expected the refusal {expected:?}, got {reported:?}",
                row.case
            );
            assert!(
                same(&json!(held), &json!([seed_subscriber()])),
                "case {:?}: expected the refused write to leave only the seeded subscriber, \
                 got {held:?}",
                row.case
            );
            assert!(
                found.is_empty(),
                "case {:?}: expected the refused write to publish nothing, got {found:?}",
                row.case
            );
        }
    }
}
