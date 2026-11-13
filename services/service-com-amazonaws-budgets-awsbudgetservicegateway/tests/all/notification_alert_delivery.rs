// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ALERT a budget notification delivers: a notification that
//! the service holds in the `ALARM` state publishes a budget alert to each of its `SNS`
//! subscribers' topics, driven over the wire through the same `MultiServiceRouter` the
//! production router serves from, with SNS mounted beside budgets on ONE engine so the
//! alert is asserted where a client sees it — on the topic the subscriber named.
//!
//! The RULE asserted here: a notification in `ALARM` has passed the threshold it alerts
//! on (`Notification$NotificationState`: "specifies whether this notification is in
//! alarm. If a budget notification is in the `ALARM` state, you passed the set threshold
//! for the budget"), so every write that leaves such a notification holding an `SNS`
//! subscriber publishes ONE alert, naming the budget and what the notification alerts
//! on, to the topic that subscriber's `Address` names ("the address that Amazon Web
//! Services sends budget notifications to"). A notification that is not in alarm owes
//! nothing, and an address that names no topic of this emulator is published to nothing.
//!
//! Whether a notification IS in alarm is the service's own reading of the spend behind
//! the budget against the threshold it alerts on (the table in
//! `tests/all/notification_alarm_state.rs`), never a state the request asked for — rows
//! below name `NotificationState` on the request precisely to assert that it buys no
//! alert.
//!
//! The table spans the class rather than one recorded request: its rows differ along the
//! write that establishes the pairing (`CreateBudget`, `CreateNotification`,
//! `CreateSubscriber`), the `NotificationState` the request named (absent, `OK`, `ALARM`),
//! the subscriber types in the set, the Region of the topic, the budget type and the limit
//! the alert names, and what the notification alerts on — the notification type, the
//! comparison operator, both published `Threshold` bounds and a fractional value in
//! between, and whether `ThresholdType` was supplied. The tables after it carry the same
//! rule onto the writes that REPLACE part of the pairing — the notification, a subscriber,
//! and the budget whose own figures the alarm decision is read from.
//!
//! The last test is the one assertion the delivery path cannot show: an alert is owed only
//! on a topic of the account whose budget it names, and no topic of another account can
//! exist here for a delivery to be observed on.
//!
//! An address that names no SNS topic at all — one whose Region field is no Region SNS can
//! be addressed in, say — never reaches a delivery: the write that would establish such a
//! subscriber refuses it (`tests/all/subscriber_address_validation.rs`), and
//! `tests/all/subscriber_sns_topic_region.rs` asserts the two halves agree.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    Budget, Notification, Spend, Subscriber,
};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    alerts, budget_as_read, PERCENTAGE_LIMIT_AMOUNT, PERCENTAGE_LIMIT_UNIT,
    THRESHOLD_TYPE_PERCENTAGE,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::ComAmazonawsSnsAmazonsimplenotificationserviceState;

use super::budget_lifecycle::{caller_account, call, same};
use super::budget_read_projection::month_start_offset;

/// The Region the budgets requests below are signed for, and the Region the topic a row
/// names lives in unless the row says otherwise.
const REGION: &str = "us-east-1";

/// A second Region, for the row whose topic is NOT in the Region the budgets request was
/// signed for: a budget is global, so the Region an alert is published in can only come
/// from the topic's own ARN.
const OTHER_REGION: &str = "eu-west-2";

/// The account every budget below belongs to: the one account this emulator answers
/// for, which is the only `AccountId` a budgets operation serves.
pub(crate) fn account_id() -> String {
    caller_account()
}

/// The emulator: budgets and SNS on ONE engine — the shape `core/router/src/lib.rs`
/// mounts them in — because a budget alert is published through the SNS API and read
/// back where SNS delivered it.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    Emulator { router, engine }
}

/// An SNS client for `region` over the in-process callback into this emulator — the same
/// seam the service itself publishes an alert through, so the suite's setup reaches SNS
/// exactly the way the handler does.
fn sns(emulator: &Emulator, region: &str) -> ::aws_sdk_sns::Client {
    let callback = EmbeddedEmulator::from_router(emulator.router.clone());
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}

/// The ARN of the topic `name` in `region`, as SNS mints it for this emulator's account.
/// Every setup below asserts the ARN SNS answered against this, so the rows can name
/// their subscriber addresses up front.
pub(crate) fn topic_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:sns:{region}:{}:{name}",
        ::engine::emulator_account_id()
    )
}

/// The topic `name` in `region`, with one confirmed subscription on it so a publish onto
/// it is observable — SNS "delivers the message to each endpoint that is subscribed to
/// the topic", and a topic with no subscription holds no delivery to read back.
pub(crate) async fn given_topic(emulator: &Emulator, region: &str, name: &str) {
    let client = sns(emulator, region);
    let created = client
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: CreateTopic({name}) failed: {error:?}"));
    let arn = created.topic_arn().unwrap_or_default();
    assert_eq!(
        arn,
        topic_arn(region, name),
        "setup: CreateTopic({name}) answered an ARN the rows do not name"
    );
    let queue = format!(
        "arn:aws:sqs:{region}:{}:{name}-inbox",
        ::engine::emulator_account_id()
    );
    client
        .subscribe()
        .topic_arn(arn)
        .protocol("sqs")
        .endpoint(&queue)
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: Subscribe({name}) failed: {error:?}"));
}

/// Every alert SNS delivered, as `(topic ARN, message)` pairs with the message parsed,
/// sorted so a comparison does not depend on which subscription SNS recorded first.
pub(crate) fn delivered(emulator: &Emulator) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = emulator
        .engine
        .state::<ComAmazonawsSnsAmazonsimplenotificationserviceState>()
        .lock()
        .delivered
        .values()
        .flatten()
        .map(|published| {
            let parsed = serde_json::from_str::<Value>(&published.message).unwrap_or_else(|error| {
                panic!(
                    "a published alert is not JSON ({error}): {:?}",
                    published.message
                )
            });
            (published.topic_arn.clone(), parsed)
        })
        .collect();
    found.sort_by_key(|(topic, message)| (topic.clone(), message.to_string()));
    found
}

/// An email subscriber: "either an Amazon SNS topic or an email address", on the half
/// this emulator hosts no transport for.
fn email_subscriber(n: usize) -> Value {
    json!({"SubscriptionType": "EMAIL", "Address": format!("ops{n}@example.com")})
}

/// An SNS subscriber whose `Address` is `address`.
pub(crate) fn sns_subscriber(address: &str) -> Value {
    json!({"SubscriptionType": "SNS", "Address": address})
}

/// Which write attaches the row's notification to its budget.
#[derive(Debug, Clone, Copy)]
enum Attach {
    /// `CreateBudget`, which "creates a budget and, if included, notifications and
    /// subscribers".
    WithBudget,
    /// `CreateNotification`, against a budget that already exists.
    AfterBudget,
}

/// One row: the budget, the notification and subscribers attached to it, a subscriber
/// attached afterwards with `CreateSubscriber`, and the alerts that must have been
/// published once all of that has run.
struct Row {
    case: &'static str,
    attach: Attach,
    /// The `Budget` shape, minus its name (the runner gives each row its own).
    budget: Value,
    notification: Value,
    subscribers: Value,
    /// A subscriber attached with `CreateSubscriber` after the notification exists.
    then_subscribe: Option<Value>,
    /// `(topic ARN, alert)` for every alert expected, in any order.
    expected: Vec<(String, Value)>,
}

/// The topic every row but the cross-Region one names.
fn primary_topic() -> String {
    topic_arn(REGION, "budget-alerts")
}

/// The topic in [`OTHER_REGION`].
fn other_topic() -> String {
    topic_arn(OTHER_REGION, "budget-alerts")
}

/// The alert a notification of `budget_name` publishes, as the rule describes it: the
/// budget it is about, what that budget tracks and over what period, and what the
/// notification alerts on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn alert(
    budget_name: &str,
    budget_type: &str,
    time_unit: &str,
    limit: Option<(&str, &str)>,
    notification_type: &str,
    comparison_operator: &str,
    threshold: f64,
    threshold_type: &str,
) -> Value {
    let mut expected = json!({
        "accountId": account_id(),
        "budgetName": budget_name,
        "budgetType": budget_type,
        "timeUnit": time_unit,
        "notificationType": notification_type,
        "comparisonOperator": comparison_operator,
        "threshold": threshold,
        "thresholdType": threshold_type,
        "notificationState": "ALARM",
    });
    if let Some((amount, unit)) = limit {
        expected["budgetLimit"] = json!({"amount": amount, "unit": unit});
    }
    expected
}

/// A `COST` budget on a monthly period with a 100 USD limit — the budget shape the rows
/// that vary only the notification use.
pub(crate) fn cost_budget() -> Value {
    json!({
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    })
}

/// The table. Thresholds and value sets come from the model
/// (`smithy_query(query="shape",shape="Notification,NotificationThreshold,NotificationState,SubscriptionType")`):
/// `NotificationThreshold` is `@range(min: 0, max: 15000000000000)`, `NotificationState`
/// is `OK | ALARM` and `SubscriptionType` is `SNS | EMAIL`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a notification the service holds in alarm, created by \
                   CreateNotification, with an SNS subscriber",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-0",
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "LESS_THAN",
                    80.0,
                    // Supplied by neither the row nor the budget: "thresholds are always
                    // a percentage", so a notification naming no `ThresholdType` alerts
                    // as the percentage it is.
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        Row {
            case: "a notification attached in alarm by CreateBudget, beside email subscribers \
                   that have no transport",
            attach: Attach::WithBudget,
            budget: json!({
                "TimeUnit": "ANNUALLY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "2500.5", "Unit": "GB"}
            }),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 42.5,
                "ThresholdType": "ABSOLUTE_VALUE"
            }),
            subscribers: json!([
                email_subscriber(1),
                sns_subscriber(&primary_topic()),
                email_subscriber(2)
            ]),
            then_subscribe: None,
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-1",
                    "USAGE",
                    "ANNUALLY",
                    Some(("2500.5", "GB")),
                    "ACTUAL",
                    "LESS_THAN",
                    42.5,
                    "ABSOLUTE_VALUE",
                ),
            )],
        },
        Row {
            case: "an SNS subscriber attached by CreateSubscriber to a notification already in \
                   alarm, at the published minimum threshold",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "EQUAL_TO",
                "Threshold": 0
            }),
            subscribers: json!([email_subscriber(1)]),
            then_subscribe: Some(sns_subscriber(&primary_topic())),
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-2",
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "EQUAL_TO",
                    0.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        Row {
            case: "a topic in another Region, at the published maximum threshold",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 15_000_000_000_000_i64,
                "ThresholdType": "PERCENTAGE"
            }),
            subscribers: json!([sns_subscriber(&other_topic())]),
            then_subscribe: None,
            expected: vec![(
                other_topic(),
                alert(
                    "alert-row-3",
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "LESS_THAN",
                    15_000_000_000_000.0,
                    "PERCENTAGE",
                ),
            )],
        },
        Row {
            case: "a budget whose limit the SERVICE defaulted, so the alert names the stored \
                   limit rather than the request's",
            attach: Attach::WithBudget,
            budget: json!({
                "TimeUnit": "QUARTERLY",
                "BudgetType": "RI_UTILIZATION"
            }),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 75
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: vec![(
                primary_topic(),
                alert(
                    "alert-row-4",
                    "RI_UTILIZATION",
                    "QUARTERLY",
                    Some((PERCENTAGE_LIMIT_AMOUNT, PERCENTAGE_LIMIT_UNIT)),
                    "ACTUAL",
                    "LESS_THAN",
                    75.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        Row {
            case: "a notification in alarm whose only subscribers are email addresses",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80
            }),
            subscribers: json!([email_subscriber(1), email_subscriber(2)]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "a notification whose threshold nothing has passed, so it is not in alarm",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "a notification whose REQUEST named ALARM, which the service does not \
                   hold in alarm",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "GREATER_THAN",
                "Threshold": 80,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "a forecasted notification attached by CreateBudget naming ALARM, with \
                   no forecast to have passed its threshold",
            attach: Attach::WithBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "FORECASTED",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 120,
                "NotificationState": "ALARM"
            }),
            subscribers: json!([sns_subscriber(&primary_topic())]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "an EMAIL subscriber attached by CreateSubscriber to a notification in alarm",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 90
            }),
            subscribers: json!([email_subscriber(1)]),
            then_subscribe: Some(email_subscriber(2)),
            expected: Vec::new(),
        },
        Row {
            case: "an SNS subscriber naming a topic in another partition",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80
            }),
            subscribers: json!([sns_subscriber(&format!(
                "arn:aws-cn:sns:cn-north-1:{}:budget-alerts",
                account_id()
            ))]),
            then_subscribe: None,
            expected: Vec::new(),
        },
        Row {
            case: "an SNS subscriber naming a topic this emulator does not hold",
            attach: Attach::AfterBudget,
            budget: cost_budget(),
            notification: json!({
                "NotificationType": "ACTUAL",
                "ComparisonOperator": "LESS_THAN",
                "Threshold": 80
            }),
            subscribers: json!([sns_subscriber(&topic_arn(REGION, "no-such-topic"))]),
            then_subscribe: None,
            expected: Vec::new(),
        },
    ]
}

/// Run one row's writes against a fresh emulator and answer what was delivered.
async fn run(row: &Row, name: &str) -> Vec<(String, Value)> {
    let emulator = emulator();
    given_topic(&emulator, REGION, "budget-alerts").await;
    given_topic(&emulator, OTHER_REGION, "budget-alerts").await;

    let mut budget = row.budget.clone();
    budget["BudgetName"] = json!(name);
    let notification_with_subscribers = json!([{
        "Notification": row.notification,
        "Subscribers": row.subscribers
    }]);
    let create = match row.attach {
        Attach::WithBudget => json!({
            "AccountId": account_id(),
            "Budget": budget,
            "NotificationsWithSubscribers": notification_with_subscribers
        }),
        Attach::AfterBudget => json!({"AccountId": account_id(), "Budget": budget}),
    };
    let (status, body) = call(&emulator.router, "CreateBudget", &create).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateBudget answered {status} {body}",
        row.case
    );

    if matches!(row.attach, Attach::AfterBudget) {
        let (status, body) = call(
            &emulator.router,
            "CreateNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": row.notification,
                "Subscribers": row.subscribers
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateNotification answered {status} {body}",
            row.case
        );
    }

    if let Some(subscriber) = &row.then_subscribe {
        let (status, body) = call(
            &emulator.router,
            "CreateSubscriber",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": row.notification,
                "Subscriber": subscriber
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateSubscriber answered {status} {body}",
            row.case
        );
    }

    // The notification is read back through the implemented reader, so every row also
    // proves the write it asserts on actually persisted.
    let (status, read) = call(
        &emulator.router,
        "DescribeNotificationsForBudget",
        &json!({"AccountId": account_id(), "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: DescribeNotificationsForBudget answered {status} {read}",
        row.case
    );
    let held = read["Notifications"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        held.len(),
        1,
        "case {:?}: expected the budget to hold one notification, got {held:?}",
        row.case
    );

    delivered(&emulator)
}

/// Rule: a notification in `ALARM` publishes one budget alert — naming the budget, what
/// it tracks and what the notification alerts on — to each of its `SNS` subscribers'
/// topics, and a notification that is not in alarm, or whose address names no topic this
/// emulator holds, publishes nothing.
#[tokio::test]
async fn a_notification_in_alarm_publishes_a_budget_alert_to_each_sns_subscribers_topic() {
    for (index, row) in rows().iter().enumerate() {
        let name = format!("alert-row-{index}");
        let found = run(row, &name).await;
        let mut expected = row.expected.clone();
        expected.sort_by_key(|(topic, message)| (topic.clone(), message.to_string()));
        assert_eq!(
            found.len(),
            expected.len(),
            "case {:?}: expected {} alert(s), got {found:?}",
            row.case,
            expected.len()
        );
        for (found, expected) in found.iter().zip(&expected) {
            assert_eq!(
                found.0, expected.0,
                "case {:?}: expected the alert on {}, got it on {}",
                row.case, expected.0, found.0
            );
            assert!(
                same(&found.1, &expected.1),
                "case {:?}: expected the alert {}, got {}",
                row.case,
                expected.1,
                found.1
            );
        }
    }
}

/// Rule: reading a notification back reports what is stored and delivers nothing — an
/// alert is owed by the WRITE that put the notification in alarm, so no number of reads
/// adds another, and a second notification in alarm on the same budget publishes its own.
#[tokio::test]
async fn alerts_are_owed_by_writes_only_and_one_per_notification_in_alarm() {
    let emulator = emulator();
    given_topic(&emulator, REGION, "budget-alerts").await;
    let topic = primary_topic();
    let name = "alert-reads";

    let (status, body) = call(
        &emulator.router,
        "CreateBudget",
        &json!({
            "AccountId": account_id(),
            "Budget": {
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"}
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateBudget answered {status} {body}");

    // Two notifications of the same budget, each one the service holds in alarm (nothing
    // spent is below either threshold) with the same SNS subscriber: what a notification
    // alerts on identifies it, so these are two notifications and each owes its own
    // alert.
    for threshold in [50, 90] {
        let notification = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "LESS_THAN",
            "Threshold": threshold
        });
        let (status, body) = call(
            &emulator.router,
            "CreateNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": notification,
                "Subscribers": [sns_subscriber(&topic)]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case threshold {threshold}: CreateNotification answered {status} {body}"
        );
    }

    let after_writes = delivered(&emulator);
    assert_eq!(
        after_writes.len(),
        2,
        "case two notifications in alarm: expected one alert each, got {after_writes:?}"
    );
    let thresholds: Vec<Value> = after_writes
        .iter()
        .map(|(_, message)| message["threshold"].clone())
        .collect();
    for threshold in [json!(50.0), json!(90.0)] {
        assert!(
            thresholds.iter().any(|found| same(found, &threshold)),
            "case two notifications in alarm: expected an alert for threshold {threshold}, got \
             {thresholds:?}"
        );
    }

    // Every read of the notifications and their subscribers, run twice over.
    for round in 0..2 {
        for (operation, body) in [
            (
                "DescribeNotificationsForBudget",
                json!({"AccountId": account_id(), "BudgetName": name}),
            ),
            (
                "DescribeSubscribersForNotification",
                json!({
                    "AccountId": account_id(),
                    "BudgetName": name,
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "LESS_THAN",
                        "Threshold": 50
                    }
                }),
            ),
            (
                "DescribeBudget",
                json!({"AccountId": account_id(), "BudgetName": name}),
            ),
        ] {
            let (status, answered) = call(&emulator.router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case read {operation} round {round}: answered {status} {answered}"
            );
        }
    }

    let after_reads = delivered(&emulator);
    assert_eq!(
        after_reads, after_writes,
        "case reads deliver nothing: expected the alerts to be unchanged by reads, got \
         {after_reads:?}"
    );
}

/// Rule, on the write that REPLACES a notification: `UpdateNotification` leaves the
/// budget holding the notification it was given, with the subscribers that were already
/// attached, so the alert owed is decided from that state — one alert per `SNS`
/// subscriber when the notification it leaves behind is one the service holds in alarm
/// (naming what the REPLACEMENT alerts on), and none when it leaves the notification out
/// of alarm.
///
/// The rows differ along what the REPLACEMENT alerts on, which is what the rule turns on:
/// with nothing spent behind the budget, an `ACTUAL` notification below an absolute
/// threshold is in alarm, while the `FORECASTED` one is not (no forecast is reported for
/// a budget here, so none of its thresholds was ever passed). Each is crossed with the
/// `NotificationState` the replacement NAMED, which buys no alert and suppresses none.
#[tokio::test]
async fn an_update_that_leaves_a_notification_in_alarm_publishes_its_alert() {
    for (case, watches, new_state, alerts_expected) in [
        ("the replacement is in alarm, naming no state", "ACTUAL", None, true),
        (
            "the replacement is in alarm although it names OK",
            "ACTUAL",
            Some("OK"),
            true,
        ),
        (
            "the replacement is in alarm and names ALARM",
            "ACTUAL",
            Some("ALARM"),
            true,
        ),
        (
            "the replacement is not in alarm although it names ALARM",
            "FORECASTED",
            Some("ALARM"),
            false,
        ),
        (
            "the replacement is not in alarm, naming no state",
            "FORECASTED",
            None,
            false,
        ),
    ] {
        let emulator = emulator();
        given_topic(&emulator, REGION, "budget-alerts").await;
        let topic = primary_topic();
        let name = "alert-update";

        // The notification the update replaces: above a percentage threshold nothing has
        // passed, so the creating write owes no alert of its own.
        let old = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": "GREATER_THAN",
            "Threshold": 80
        });
        let (status, body) = call(
            &emulator.router,
            "CreateBudget",
            &json!({
                "AccountId": account_id(),
                "Budget": {
                    "BudgetName": name,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "100", "Unit": "USD"}
                },
                "NotificationsWithSubscribers": [{
                    "Notification": old,
                    "Subscribers": [sns_subscriber(&topic), email_subscriber(0)]
                }]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBudget answered {status} {body}"
        );
        // Whatever the creating write already owed is set aside, so what is asserted
        // below is what the UPDATE published.
        let before = delivered(&emulator);

        let mut new = json!({
            "NotificationType": watches,
            "ComparisonOperator": "LESS_THAN",
            "Threshold": 42.5,
            "ThresholdType": "ABSOLUTE_VALUE"
        });
        if let Some(state) = new_state {
            new["NotificationState"] = json!(state);
        }
        let (status, body) = call(
            &emulator.router,
            "UpdateNotification",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "OldNotification": old,
                "NewNotification": new
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateNotification answered {status} {body}"
        );

        let found = delivered(&emulator);
        let published: Vec<&(String, Value)> = found
            .iter()
            .filter(|alert| !before.contains(alert))
            .collect();
        if alerts_expected {
            // One alert, on the SNS subscriber's topic, naming what the REPLACEMENT
            // alerts on — never the notification it replaced. The email subscriber is a
            // mailbox no service here hosts, so it adds none.
            let want = alert(
                name,
                "COST",
                "MONTHLY",
                Some(("100", "USD")),
                watches,
                "LESS_THAN",
                42.5,
                "ABSOLUTE_VALUE",
            );
            assert_eq!(
                published.len(),
                1,
                "case {case:?}: expected one alert from the update, got {published:?}"
            );
            assert_eq!(
                published[0].0, topic,
                "case {case:?}: expected the alert on {topic}, got it on {}",
                published[0].0
            );
            assert!(
                same(&published[0].1, &want),
                "case {case:?}: expected the alert {want}, got {}",
                published[0].1
            );
        } else {
            assert!(
                published.is_empty(),
                "case {case:?}: expected the update to publish nothing, got {published:?}"
            );
        }
    }
}

/// The alerts in `found` that `before` does not already account for — a MULTISET
/// difference, not a containment filter: a write can owe an alert IDENTICAL to one an
/// earlier write already published, and a containment filter would hide that second copy
/// instead of reporting it as the alert the write under test published.
fn published_since(
    before: &[(String, Value)],
    found: Vec<(String, Value)>,
) -> Vec<(String, Value)> {
    let mut remaining = before.to_vec();
    let mut published: Vec<(String, Value)> = Vec::new();
    for alert in found {
        match remaining.iter().position(|held| *held == alert) {
            Some(position) => {
                remaining.remove(position);
            }
            None => published.push(alert),
        }
    }
    published
}

/// Rule, on the write that REPLACES a subscriber: `UpdateSubscriber` attaches
/// `NewSubscriber` to the notification in `OldSubscriber`'s place, so a notification the
/// service holds in alarm owes its alert to the subscriber that call ATTACHES — one alert
/// on the topic a replacement `SNS` subscriber names, and none when the notification is
/// not in alarm, when the replacement is an address no service here hosts a transport for,
/// or when it names the subscriber the notification already held (which attaches
/// nothing).
///
/// The rows differ along what the notification alerts on — below a percentage threshold,
/// which nothing spent is, or above it, which nothing spent is not — along whether the
/// replacement keeps or changes the subscription type, and along the Region of the topic
/// it moves to.
#[tokio::test]
async fn updating_a_subscriber_of_a_notification_in_alarm_publishes_its_alert() {
    for (case, operator, held, new, expected) in [
        (
            "an email replaced by an SNS subscriber",
            "LESS_THAN",
            email_subscriber(0),
            sns_subscriber(&primary_topic()),
            Some(primary_topic()),
        ),
        (
            "an SNS subscriber moved to a topic in another Region",
            "LESS_THAN",
            sns_subscriber(&primary_topic()),
            sns_subscriber(&other_topic()),
            Some(other_topic()),
        ),
        (
            "an email replaced by an SNS subscriber, the notification not in alarm",
            "GREATER_THAN",
            email_subscriber(0),
            sns_subscriber(&primary_topic()),
            None,
        ),
        (
            "an SNS subscriber replaced by an email address",
            "LESS_THAN",
            sns_subscriber(&primary_topic()),
            email_subscriber(1),
            None,
        ),
        (
            "a replacement naming the subscriber already held",
            "LESS_THAN",
            sns_subscriber(&primary_topic()),
            sns_subscriber(&primary_topic()),
            None,
        ),
    ] {
        let emulator = emulator();
        given_topic(&emulator, REGION, "budget-alerts").await;
        given_topic(&emulator, OTHER_REGION, "budget-alerts").await;
        let name = "alert-subscriber-update";

        let notification = json!({
            "NotificationType": "ACTUAL",
            "ComparisonOperator": operator,
            "Threshold": 80
        });
        let (status, body) = call(
            &emulator.router,
            "CreateBudget",
            &json!({
                "AccountId": account_id(),
                "Budget": {
                    "BudgetName": name,
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": {"Amount": "100", "Unit": "USD"}
                },
                "NotificationsWithSubscribers": [{
                    "Notification": notification,
                    "Subscribers": [&held]
                }]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateBudget answered {status} {body}"
        );
        // Whatever the creating write already owed is set aside, so what is asserted
        // below is what the UPDATE published.
        let before = delivered(&emulator);

        let (status, body) = call(
            &emulator.router,
            "UpdateSubscriber",
            &json!({
                "AccountId": account_id(),
                "BudgetName": name,
                "Notification": notification,
                "OldSubscriber": &held,
                "NewSubscriber": &new
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSubscriber answered {status} {body}"
        );

        // The row whose replacement names the subscriber already held would publish an
        // alert IDENTICAL to the one the create owed, which is why this is a multiset
        // difference ([`published_since`]) and not a containment filter.
        let published = published_since(&before, delivered(&emulator));
        match expected {
            Some(topic) => {
                // One alert, on the topic the REPLACEMENT names, naming what the
                // notification alerts on — its threshold type absent, so rendered as the
                // percentage it means.
                let want = alert(
                    name,
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    operator,
                    80.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                );
                assert_eq!(
                    published.len(),
                    1,
                    "case {case:?}: expected one alert from the update, got {published:?}"
                );
                assert_eq!(
                    published[0].0, topic,
                    "case {case:?}: expected the alert on {topic}, got it on {}",
                    published[0].0
                );
                assert!(
                    same(&published[0].1, &want),
                    "case {case:?}: expected the alert {want}, got {}",
                    published[0].1
                );
            }
            None => assert!(
                published.is_empty(),
                "case {case:?}: expected the update to publish nothing, got {published:?}"
            ),
        }
    }
}

/// Rule: an alert is owed only on a topic of the ACCOUNT whose budget it names. The alert
/// carries that budget's own figures — the account, the budget, what it tracks, the limit
/// the threshold is measured against — so a subscriber address naming a topic in any other
/// account names no topic this alert goes to, however well-formed that address is.
///
/// Asserted where the write DECIDES the alert (`state::alerts::owed`) rather than through
/// a delivery: every topic this emulator holds is minted under the one account it answers
/// for, so a topic of another account cannot be stood up for a delivery to be observed on.
/// The rows differ along the account and the partition the address names, and along the
/// Region, which may differ from the budget's own requests.
#[test]
fn an_alert_is_owed_only_on_a_topic_of_the_budgets_own_account() {
    let budget = Budget {
        budget_name: "own-account".to_string(),
        budget_type: "COST".to_string(),
        time_unit: "MONTHLY".to_string(),
        budget_limit: Some(Spend {
            amount: "100".to_string(),
            unit: "USD".to_string(),
        }),
        ..Budget::default()
    };
    let source = alerts::source(&account_id(), &budget_as_read(&budget, SystemTime::now()));
    // A notification the service holds in alarm: nothing spent is below 80% of the limit,
    // so every row below differs only in the address the alert would go to.
    let notification = Notification {
        notification_type: "ACTUAL".to_string(),
        comparison_operator: "LESS_THAN".to_string(),
        threshold: 80.0,
        threshold_type: None,
        notification_state: None,
        consecutive_breach_count: None,
    };
    // An account that is NOT the one this emulator answers for; 12 digits, as
    // `AccountId`'s `^\d{12}$` requires.
    let other_account = "123456789012";
    for (case, address, expected) in [
        (
            "a topic of the budget's own account",
            topic_arn(REGION, "budget-alerts"),
            1,
        ),
        (
            "a topic of the budget's own account in another Region",
            topic_arn(OTHER_REGION, "budget-alerts"),
            1,
        ),
        (
            "a topic of another account",
            format!("arn:aws:sns:{REGION}:{other_account}:budget-alerts"),
            0,
        ),
        (
            "a topic of another account in another partition",
            format!("arn:aws-cn:sns:cn-north-1:{other_account}:budget-alerts"),
            0,
        ),
    ] {
        let subscribers = vec![Subscriber {
            subscription_type: "SNS".to_string(),
            address: address.clone(),
            verification_status: None,
        }];
        let owed = alerts::owed(&source, &notification, &subscribers);
        assert_eq!(
            owed.len(),
            expected,
            "case {case:?}: expected {expected} alert(s) owed on {address}, got {owed:?}"
        );
    }
}

/// The budget every row of [`update_rows`] creates and then replaces.
const UPDATED_BUDGET: &str = "alert-budget-update";

/// One row of the budget-replacing table: the budget as created with its notifications,
/// the `NewBudget` that replaces it, the `NotificationState` each notification reads back
/// with afterwards, and the alerts the UPDATE itself must publish.
struct UpdateRow {
    case: &'static str,
    /// The `Budget` the row creates, minus its name.
    created: Value,
    /// The `NotificationsWithSubscribers` entries created with it.
    attached: Value,
    /// The `NewBudget` `UpdateBudget` replaces it with, minus its name.
    updated: Value,
    /// The state each attached notification reads back with after the update, in the
    /// order the notifications were attached.
    states: Vec<&'static str>,
    /// `(topic ARN, alert)` for every alert the update must publish, in any order.
    expected: Vec<(String, Value)>,
}

/// A `COST` budget on a monthly period limited to `amount` USD.
fn cost_budget_limited(amount: &str) -> Value {
    json!({
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": amount, "Unit": "USD"}
    })
}

/// The budget-replacing table. Nothing spends behind a budget here, so the watched spend
/// is zero in the budget's own unit and what each row turns on is the limit the threshold
/// is measured against — which is exactly what the update replaces.
///
/// The rows differ along what the update changes (the limit, what the budget tracks and
/// over what period, a fixed limit replaced by a `PlannedBudgetLimits` map), along what
/// the notifications of it alert on (threshold type, comparison operator, notification
/// type, how many are attached), along the subscriber types held with them, and along the
/// Region of the topic addressed.
fn update_rows() -> Vec<UpdateRow> {
    let percentage_notification = json!({
        "NotificationType": "ACTUAL",
        "ComparisonOperator": "LESS_THAN",
        "Threshold": 50
    });
    vec![
        UpdateRow {
            // The limit was zero, so 50% of it was zero and nothing spent was below it;
            // the replacement puts the threshold above the spend.
            case: "a new limit that moves a percentage notification into alarm",
            created: cost_budget_limited("0"),
            attached: json!([{
                "Notification": percentage_notification,
                "Subscribers": [sns_subscriber(&primary_topic()), email_subscriber(0)]
            }]),
            updated: cost_budget_limited("100"),
            states: vec!["ALARM"],
            expected: vec![(
                primary_topic(),
                alert(
                    UPDATED_BUDGET,
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "LESS_THAN",
                    50.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        UpdateRow {
            // The mirror image: the create owed the alert, and the update leaves the
            // notification with nothing passed, so it owes none.
            case: "a new limit that moves a percentage notification out of alarm",
            created: cost_budget_limited("100"),
            attached: json!([{
                "Notification": percentage_notification,
                "Subscribers": [sns_subscriber(&primary_topic())]
            }]),
            updated: cost_budget_limited("0"),
            states: vec!["OK"],
            expected: Vec::new(),
        },
        UpdateRow {
            // An absolute threshold does not move with the limit, so this notification was
            // in alarm before and after — and the alert the update owes names the limit
            // this call stored, not the one the create alert named.
            case: "a notification left in alarm, whose alert names the updated limit",
            created: cost_budget_limited("0"),
            attached: json!([{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "LESS_THAN",
                    "Threshold": 42.5,
                    "ThresholdType": "ABSOLUTE_VALUE"
                },
                "Subscribers": [sns_subscriber(&primary_topic())]
            }]),
            updated: cost_budget_limited("250.75"),
            states: vec!["ALARM"],
            expected: vec![(
                primary_topic(),
                alert(
                    UPDATED_BUDGET,
                    "COST",
                    "MONTHLY",
                    Some(("250.75", "USD")),
                    "ACTUAL",
                    "LESS_THAN",
                    42.5,
                    "ABSOLUTE_VALUE",
                ),
            )],
        },
        UpdateRow {
            // The same budget figures decide both notifications, and only one of them
            // compares in a direction the new limit satisfies.
            case: "two notifications, only one of which the update leaves in alarm",
            created: cost_budget_limited("0"),
            attached: json!([
                {
                    "Notification": percentage_notification,
                    "Subscribers": [sns_subscriber(&primary_topic())]
                },
                {
                    "Notification": {
                        "NotificationType": "ACTUAL",
                        "ComparisonOperator": "GREATER_THAN",
                        "Threshold": 50
                    },
                    "Subscribers": [sns_subscriber(&primary_topic())]
                }
            ]),
            updated: cost_budget_limited("100"),
            states: vec!["ALARM", "OK"],
            expected: vec![(
                primary_topic(),
                alert(
                    UPDATED_BUDGET,
                    "COST",
                    "MONTHLY",
                    Some(("100", "USD")),
                    "ACTUAL",
                    "LESS_THAN",
                    50.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        UpdateRow {
            // In alarm on every read, with no address any transport here hosts: the state
            // is reported and nothing is published.
            case: "a notification the update leaves in alarm holding only email addresses",
            created: cost_budget_limited("0"),
            attached: json!([{
                "Notification": percentage_notification,
                "Subscribers": [email_subscriber(0), email_subscriber(1)]
            }]),
            updated: cost_budget_limited("100"),
            states: vec!["ALARM"],
            expected: Vec::new(),
        },
        UpdateRow {
            // What the budget tracks and over what period is part of the alert, so an
            // update that changes them is named by the alert it owes; the Region the
            // publish is addressed in comes from the topic's own ARN, not the budget.
            case: "an update that changes what the budget tracks, on a topic in another \
                   Region",
            created: cost_budget_limited("0"),
            attached: json!([{
                "Notification": {
                    "NotificationType": "ACTUAL",
                    "ComparisonOperator": "LESS_THAN",
                    "Threshold": 100
                },
                "Subscribers": [sns_subscriber(&other_topic())]
            }]),
            updated: json!({
                "TimeUnit": "ANNUALLY",
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "5", "Unit": "GB"}
            }),
            states: vec!["ALARM"],
            expected: vec![(
                other_topic(),
                alert(
                    UPDATED_BUDGET,
                    "USAGE",
                    "ANNUALLY",
                    Some(("5", "GB")),
                    "ACTUAL",
                    "LESS_THAN",
                    100.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        UpdateRow {
            // The limit a percentage is taken of is the one the READ reports, which for a
            // planned-limit budget is the entry in force — so an update that replaces a
            // fixed limit with a plan is judged against that entry.
            case: "a fixed limit replaced by the planned entry in force",
            created: cost_budget_limited("0"),
            attached: json!([{
                "Notification": percentage_notification,
                "Subscribers": [sns_subscriber(&primary_topic())]
            }]),
            updated: json!({
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {
                    month_start_offset(-1).to_string(): {"Amount": "400", "Unit": "USD"}
                }
            }),
            states: vec!["ALARM"],
            expected: vec![(
                primary_topic(),
                alert(
                    UPDATED_BUDGET,
                    "COST",
                    "MONTHLY",
                    Some(("400", "USD")),
                    "ACTUAL",
                    "LESS_THAN",
                    50.0,
                    THRESHOLD_TYPE_PERCENTAGE,
                ),
            )],
        },
        UpdateRow {
            // No forecast is reported for a budget here, so no threshold of a
            // `FORECASTED` notification was ever measured and no limit the update stores
            // brings it into alarm.
            case: "a forecasted notification, which no new limit brings into alarm",
            created: cost_budget_limited("0"),
            attached: json!([{
                "Notification": {
                    "NotificationType": "FORECASTED",
                    "ComparisonOperator": "LESS_THAN",
                    "Threshold": 50
                },
                "Subscribers": [sns_subscriber(&primary_topic())]
            }]),
            updated: cost_budget_limited("100"),
            states: vec!["OK"],
            expected: Vec::new(),
        },
    ]
}

/// Rule, on the write that REPLACES the BUDGET: a notification's alarm state is read off
/// the figures of the budget that owns it — the limit its threshold is measured against
/// and the spend it watches — so `UpdateBudget` decides, from the budget it leaves stored,
/// the alert every notification attached to that budget owes: one per `SNS` subscriber of
/// each notification it leaves in alarm, naming the UPDATED budget, and none for one it
/// leaves out of alarm, exactly as every other write that leaves such a pairing decides
/// it.
///
/// Each row reads the notifications back through `DescribeNotificationsForBudget`, so the
/// alerts asserted are asserted beside the states the service reports after the update:
/// no row can report a notification in `ALARM` to every later reader while the subscriber
/// held with it was told nothing.
#[tokio::test]
async fn an_update_to_a_budget_publishes_the_alerts_its_new_figures_leave_owed() {
    for row in update_rows() {
        let emulator = emulator();
        given_topic(&emulator, REGION, "budget-alerts").await;
        given_topic(&emulator, OTHER_REGION, "budget-alerts").await;

        let mut created = row.created.clone();
        created["BudgetName"] = json!(UPDATED_BUDGET);
        let (status, body) = call(
            &emulator.router,
            "CreateBudget",
            &json!({
                "AccountId": account_id(),
                "Budget": created,
                "NotificationsWithSubscribers": row.attached
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateBudget answered {status} {body}",
            row.case
        );
        // Whatever the creating write already owed is set aside, so what is asserted below
        // is what the UPDATE published.
        let before = delivered(&emulator);

        let mut updated = row.updated.clone();
        updated["BudgetName"] = json!(UPDATED_BUDGET);
        let (status, body) = call(
            &emulator.router,
            "UpdateBudget",
            &json!({"AccountId": account_id(), "NewBudget": updated}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateBudget answered {status} {body}",
            row.case
        );

        // The states the service reports for the notifications of the budget this call
        // stored — the read half of the same decision the alerts are owed by.
        let (status, read) = call(
            &emulator.router,
            "DescribeNotificationsForBudget",
            &json!({"AccountId": account_id(), "BudgetName": UPDATED_BUDGET}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeNotificationsForBudget answered {status} {read}",
            row.case
        );
        let held = read["Notifications"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            held.len(),
            row.states.len(),
            "case {:?}: expected the budget to still hold {} notification(s), got {held:?}",
            row.case,
            row.states.len()
        );
        for (position, expected) in row.states.iter().enumerate() {
            let found = held[position]["NotificationState"].as_str().unwrap_or("");
            assert_eq!(
                found, *expected,
                "case {:?}: expected notification {position} to read back {expected}, got \
                 {found:?}",
                row.case
            );
        }

        let mut published = published_since(&before, delivered(&emulator));
        published.sort_by_key(|(topic, message)| (topic.clone(), message.to_string()));
        let mut expected = row.expected.clone();
        expected.sort_by_key(|(topic, message)| (topic.clone(), message.to_string()));
        assert_eq!(
            published.len(),
            expected.len(),
            "case {:?}: expected {} alert(s) from the update, got {published:?}",
            row.case,
            expected.len()
        );
        for (published, expected) in published.iter().zip(&expected) {
            assert_eq!(
                published.0, expected.0,
                "case {:?}: expected the alert on {}, got it on {}",
                row.case, expected.0, published.0
            );
            assert!(
                same(&published.1, &expected.1),
                "case {:?}: expected the alert {}, got {}",
                row.case,
                expected.1,
                published.1
            );
        }
    }
}
