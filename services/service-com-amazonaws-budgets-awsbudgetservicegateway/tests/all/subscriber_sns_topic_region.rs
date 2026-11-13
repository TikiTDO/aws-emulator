// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the REGION of an `SNS` subscriber's address: the write that
//! establishes such a subscriber and the alert published to it read the address the same
//! way, so an address a write accepts is one the notification's alert actually reaches.
//!
//! The RULE asserted here: a `Subscriber` "consists of a subscription type and either an
//! Amazon SNS topic or an email address" and `Subscriber$Address` is "the address that
//! Amazon Web Services sends budget notifications to, either an SNS topic or an email"
//! (`Subscriber`, `Subscriber$Address`) — and Amazon SNS is a REGIONAL service, so the
//! ARN of a topic names the Region the topic lives in, which is also the Region a publish
//! to it is addressed in. A value whose Region field names no Region therefore names no
//! topic: every write that establishes a subscriber refuses it with the client error each
//! of them models (`InvalidParameterException`, HTTP 400) and stores nothing, and every
//! address those writes DO accept is one the alert of a notification in alarm is
//! published to.
//!
//! That pairing is the point: the acceptance and the delivery halves are asserted
//! together, over one table, so neither half can admit an address the other drops. The
//! rows differ along the Region field — two commercial Regions, the shortest label the
//! grammar admits and one exactly at its published bound, against an empty field, a field
//! in upper case, one a character past the bound, and two carrying what would re-point an
//! endpoint's authority.
//!
//! `tests/all/subscriber_address_validation.rs` spans the address rule over every write
//! that establishes a subscriber and every way an address can fail it;
//! `tests/all/notification_alert_delivery.rs` spans what an alert says and which writes
//! owe one. What this suite adds is the agreement between the two.

use serde_json::json;
use std::time::SystemTime;

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    Budget, Notification, Spend, Subscriber,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    alerts, budget_as_read, subscriber_address_violation, THRESHOLD_TYPE_PERCENTAGE,
};

use super::budget_lifecycle::{caller_account, call, is_error, same};
use super::notification_alert_delivery::{
    account_id, alert, cost_budget, delivered, emulator, given_topic, sns_subscriber, topic_arn,
};
use super::subscriber_address_validation::REFUSAL;

/// The topic name every row's address names, so the rows differ in their Region field
/// alone.
const TOPIC: &str = "budget-alerts";

/// One row: the Region field an `SNS` subscriber's address carries, and whether that
/// address names an SNS topic at all.
///
/// The grammar the rows step along is the one `service_common::arn::region_label` reads —
/// non-empty, at most 20 characters, `[a-z0-9-]` only — which is the Region field of every
/// `*Arn` `@pattern` in the AWS models. `names_a_topic` is true for exactly the rows
/// inside it.
struct Row {
    case: &'static str,
    region: &'static str,
    names_a_topic: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a commercial Region",
            region: "us-east-1",
            names_a_topic: true,
        },
        Row {
            case: "a Region other than the one the budgets request is signed for",
            region: "eu-west-2",
            names_a_topic: true,
        },
        Row {
            case: "the shortest Region label the grammar admits",
            region: "a",
            names_a_topic: true,
        },
        Row {
            case: "a Region label exactly at the published bound",
            region: "us-east-1-us-east-1x",
            names_a_topic: true,
        },
        Row {
            case: "an empty Region field, which names a partition rather than a Region",
            region: "",
            names_a_topic: false,
        },
        Row {
            case: "a Region field in upper case",
            region: "US-EAST-1",
            names_a_topic: false,
        },
        Row {
            case: "a Region label one character past the published bound",
            region: "us-east-1-us-east-1xy",
            names_a_topic: false,
        },
        Row {
            case: "a Region field carrying a host label after a path separator",
            region: "us-east-1/evil.example.com",
            names_a_topic: false,
        },
        Row {
            case: "a Region field carrying userinfo",
            region: "us-east-1@evil.example.com",
            names_a_topic: false,
        },
    ]
}

/// The address a row names: a topic of the account this emulator answers for, in the
/// row's Region. The account is the caller's so that the only thing a row varies is the
/// Region — an alert is owed only on a topic of the budget's own account
/// (`tests/all/notification_alert_delivery.rs`).
fn address(row: &Row) -> String {
    format!("arn:aws:sns:{}:{}:{TOPIC}", row.region, caller_account())
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

/// The seed subscriber a notification is created with: `Subscribers` is `@length(min: 1)`,
/// and an email mailbox is an address this emulator hosts no transport for, so it adds no
/// alert of its own.
fn seed_subscriber() -> serde_json::Value {
    json!({"SubscriptionType": "EMAIL", "Address": "ops@example.com"})
}

/// RULE, on the two halves at once: the address gate accepts exactly the addresses that
/// name an SNS topic, and the delivery path owes an alert for exactly those — so no
/// address is accepted by one half and dropped by the other.
///
/// Asserted where each half DECIDES (`state::subscriber_address_violation` and
/// `state::alerts::owed`), so every row is covered whether or not a topic in its Region
/// could be stood up for a publish to be observed on.
#[test]
fn the_address_gate_and_the_alert_path_agree_on_which_addresses_name_a_topic() {
    let budget = Budget {
        budget_name: "topic-region".to_string(),
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
        let accepted = subscriber_address_violation(&caller_account(), &subscribers).is_ok();
        assert_eq!(
            accepted, row.names_a_topic,
            "case {:?}: expected the address gate to accept {address}: {}, got {accepted}",
            row.case, row.names_a_topic
        );
        let owed = alerts::owed(&source, &notification, &subscribers);
        assert_eq!(
            owed.len() == 1,
            row.names_a_topic,
            "case {:?}: expected the notification in alarm to owe an alert on {address}: {}, \
             got {owed:?}",
            row.case,
            row.names_a_topic
        );
        if row.names_a_topic {
            assert_eq!(
                owed[0].topic_arn, address,
                "case {:?}: expected the alert on {address}, got it on {}",
                row.case, owed[0].topic_arn
            );
        }
    }
}

/// RULE, over the wire through `CreateSubscriber` — the write that attaches one subscriber
/// to a notification a budget already holds: an address naming a topic is stored, read
/// back and PUBLISHED to; an address naming none is refused at 400 with the modelled client
/// error, stored nowhere, and publishes nothing.
#[tokio::test]
async fn create_subscriber_publishes_to_every_sns_address_it_accepts() {
    for (index, row) in rows().iter().enumerate() {
        let emulator = emulator();
        let budget_name = format!("topic-region-{index}");
        let address = address(row);

        // The topic a row that names one is published to has to exist for SNS to hold the
        // delivery: "the address that Amazon Web Services sends budget notifications to"
        // is a topic inside this emulator, which answers that it does not exist otherwise.
        if row.names_a_topic {
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

        if row.names_a_topic {
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateSubscriber to accept {address}, got {status} {body}",
                row.case
            );
            assert!(
                held.iter().any(|subscriber| same(
                    subscriber,
                    &sns_subscriber(&address)
                )),
                "case {:?}: expected the stored set to hold the SNS subscriber, got {held:?}",
                row.case
            );
            let expected = alert(
                &budget_name,
                "COST",
                "MONTHLY",
                Some(("100", "USD")),
                "ACTUAL",
                "LESS_THAN",
                80.0,
                // Named by neither the notification nor the budget: "thresholds are always
                // a percentage", so a notification naming no `ThresholdType` alerts as the
                // percentage it means.
                THRESHOLD_TYPE_PERCENTAGE,
            );
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
