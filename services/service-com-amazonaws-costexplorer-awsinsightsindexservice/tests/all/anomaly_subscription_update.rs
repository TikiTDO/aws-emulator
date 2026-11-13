// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAnomalySubscription`.
//!
//! Three rules, each asserted over a table of inputs:
//!
//! * an update writes exactly the members the request names and leaves every omitted
//!   member of the stored subscription as it was, with the members the service owns
//!   (`SubscriptionArn`, `AccountId`, each subscriber's `Status`) still the service's and
//!   the deprecated scalar `Threshold` expanded into the `ThresholdExpression` it is
//!   shorthand for;
//! * a request whose named members do not hold together as those members document is
//!   refused 400 `ValidationException` and writes nothing;
//! * a request naming a subscription or a monitor the account does not hold is refused
//!   with the modelled unknown-resource failure and writes nothing.
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`], so nothing here reaches past the HTTP surface a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::state::{
    MONITOR_ID_AS_MONITOR, MONITOR_TYPE_DIMENSIONAL, SUBSCRIBER_STATUS_CONFIRMED,
    SUBSCRIPTION_ID_AS_RESOURCE, THRESHOLD_ABSOLUTE_DIMENSION, THRESHOLD_MATCH_OPTION,
    THRESHOLD_VALUE_MAX,
};

use crate::harness::{error_code, error_message, Emulator};

/// An email subscriber address, in the form `SubscriberAddress`'s modelled pattern admits
/// for the email channel.
const EMAIL_ADDRESS: &str = "cost.ops@example.com";

/// A second email subscriber address, so a subscriber-list update is a different list and
/// not the same one re-sent.
const ESCALATION_ADDRESS: &str = "cost.escalation@example.com";

/// An SNS topic ARN, in the form `SubscriberAddress`'s modelled pattern admits for the SNS
/// channel.
const SNS_TOPIC_ARN: &str = "arn:aws:sns:us-east-1:123456789012:cost-anomaly-alerts";

/// The `AnomalySubscriptionFrequency` values, from the model's own enum
/// (`smithy_query(query="shape", shape="AnomalySubscriptionFrequency")`).
const FREQUENCIES: [&str; 3] = ["DAILY", "IMMEDIATE", "WEEKLY"];

/// The other dimension a `ThresholdExpression` may select on, beside the absolute one
/// `THRESHOLD_ABSOLUTE_DIMENSION` names
/// (`AnomalySubscription$ThresholdExpression`: "The supported dimensions are
/// ANOMALY_TOTAL_IMPACT_ABSOLUTE and ANOMALY_TOTAL_IMPACT_PERCENTAGE").
const THRESHOLD_PERCENTAGE_DIMENSION: &str = "ANOMALY_TOTAL_IMPACT_PERCENTAGE";

/// The status Cost Explorer serves an unknown-subscription refusal at: the one
/// `UnknownSubscriptionException` declares
/// (`smithy_query(query="shape", shape="UnknownSubscriptionException")`: errorKind client,
/// `@httpError 404`).
const UNKNOWN_SUBSCRIPTION_STATUS: u16 = 404;

/// The status Cost Explorer serves an unknown-monitor refusal at — not the 404 the shape's
/// `@httpError` declares; see [`crate::unknown_monitor_references`], which asserts that
/// rule for the operations that resolved a monitor before this one did.
const UNKNOWN_MONITOR_STATUS: u16 = 400;

/// The sentence Cost Explorer refuses a write naming both threshold members with: the
/// service states the rule in terms of the two members and the condition on them.
const BOTH_THRESHOLDS_MESSAGE: &str = "Threshold and ThresholdExpression cannot both be non-null";

/// Create one managed cost anomaly monitor and answer with its ARN — a subscription has to
/// name monitors the account holds.
async fn create_monitor(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateAnomalyMonitor",
            &json!({"AnomalyMonitor": {
                "MonitorName": name,
                "MonitorType": MONITOR_TYPE_DIMENSIONAL,
                "MonitorDimension": "SERVICE",
            }}),
        )
        .await;
    created["MonitorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("MonitorArn must be present, got {created}"))
        .to_string()
}

/// A threshold expression selecting one dimension at one value, as
/// `AnomalySubscription$ThresholdExpression` documents the form.
fn dimension_threshold(dimension: &str, value: &str) -> Value {
    json!({"Dimensions": {
        "Key": dimension,
        "MatchOptions": [THRESHOLD_MATCH_OPTION],
        "Values": [value],
    }})
}

/// The decimal form a double threshold is rendered in when it is expanded into the
/// expression it is shorthand for.
fn render_threshold(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// The `ThresholdExpression` a subscription written with the deprecated scalar `Threshold`
/// is left carrying (`AnomalySubscription$Threshold`: "Continued use of Threshold will be
/// treated as shorthand syntax for a ThresholdExpression").
fn shorthand_expansion(threshold: f64) -> Value {
    dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, &render_threshold(threshold))
}

/// Create one subscription alerting on `monitors` with `threshold` (the members of a
/// threshold half) and answer with its ARN.
async fn create_subscription(
    emulator: &Emulator,
    name: &str,
    monitors: &[String],
    threshold: &Value,
) -> String {
    let mut subscription = json!({
        "SubscriptionName": name,
        "MonitorArnList": monitors,
        "Subscribers": [{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}],
        "Frequency": "DAILY",
    });
    for (member, value) in threshold.as_object().expect("a threshold object") {
        subscription[member] = value.clone();
    }
    let created = emulator
        .ok(
            "CreateAnomalySubscription",
            &json!({"AnomalySubscription": subscription}),
        )
        .await;
    created["SubscriptionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("SubscriptionArn must be present, got {created}"))
        .to_string()
}

/// The stored subscription `arn` names, as `GetAnomalySubscriptions` reports it.
async fn subscription_by_arn(emulator: &Emulator, arn: &str) -> Value {
    let answered = emulator
        .ok(
            "GetAnomalySubscriptions",
            &json!({"SubscriptionArnList": [arn]}),
        )
        .await;
    let reported = answered["AnomalySubscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reported.len(),
        1,
        "expected exactly the subscription {arn:?} to be reported, got {answered}"
    );
    reported[0].clone()
}

/// An `UpdateAnomalySubscription` body: the required `SubscriptionArn` plus the members
/// `patch` names.
fn update_body(arn: &str, patch: &Value) -> Value {
    let mut body = json!({"SubscriptionArn": arn});
    for (member, value) in patch.as_object().expect("an update body") {
        body[member] = value.clone();
    }
    body
}

/// The dollar threshold an expression names, read off the JSON the test sent: the value of
/// the absolute-impact dimension it selects on, searched through its `And`/`Or` branches.
/// Computed here from the request so the assertion does not borrow the service's own
/// reading.
fn absolute_threshold_in(expression: &Value) -> Option<f64> {
    let dimensions = &expression["Dimensions"];
    if dimensions["Key"].as_str() == Some(THRESHOLD_ABSOLUTE_DIMENSION) {
        return dimensions["Values"]
            .as_array()?
            .iter()
            .find_map(|value| value.as_str()?.parse::<f64>().ok());
    }
    ["And", "Or"]
        .iter()
        .filter_map(|nested| expression[nested].as_array())
        .flatten()
        .find_map(absolute_threshold_in)
}

/// The stored subscription an update carrying `patch` must leave behind, given the one it
/// started from.
///
/// Omitted members are unchanged; a named one is replaced — except that each subscriber's
/// `Status` is the service's own, and naming either threshold member replaces the one
/// threshold the subscription holds, which the deprecated scalar is only a dollar reading
/// of.
fn expected_after(before: &Value, patch: &Value) -> Value {
    let mut expected = before.clone();
    for (member, value) in patch.as_object().expect("an update body") {
        match member.as_str() {
            "Subscribers" => {
                let subscribers: Vec<Value> = value
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|subscriber| {
                        json!({
                            "Address": subscriber["Address"],
                            "Type": subscriber["Type"],
                            "Status": SUBSCRIBER_STATUS_CONFIRMED,
                        })
                    })
                    .collect();
                expected["Subscribers"] = json!(subscribers);
            }
            "Threshold" => {
                let threshold = value.as_f64().expect("a threshold number");
                expected["Threshold"] = value.clone();
                expected["ThresholdExpression"] = shorthand_expansion(threshold);
            }
            "ThresholdExpression" => {
                let members = expected.as_object_mut().expect("a subscription object");
                match absolute_threshold_in(value) {
                    Some(dollars) => {
                        members.insert("Threshold".to_string(), json!(dollars));
                    }
                    None => {
                        members.remove("Threshold");
                    }
                }
                expected["ThresholdExpression"] = value.clone();
            }
            other => {
                expected[other] = value.clone();
            }
        }
    }
    expected
}

/// Assert a reported subscription is the one expected, member by member.
///
/// `Threshold` is compared as a number rather than as JSON, so the assertion does not
/// depend on how a double is spelled on the wire.
fn assert_subscription_eq(case: &str, got: &Value, expected: &Value) {
    for member in [
        "SubscriptionArn",
        "AccountId",
        "SubscriptionName",
        "Frequency",
        "MonitorArnList",
        "Subscribers",
        "ThresholdExpression",
    ] {
        assert_eq!(
            got.get(member),
            expected.get(member),
            "case {case:?}: expected {member} to be {:?}, got {:?}",
            expected.get(member),
            got.get(member)
        );
    }
    assert_eq!(
        got.get("Threshold").is_some(),
        expected.get("Threshold").is_some(),
        "case {case:?}: expected the deprecated scalar Threshold to be \
         {}, got {got}",
        if expected.get("Threshold").is_some() {
            "reported"
        } else {
            "absent"
        }
    );
    assert_eq!(
        got.get("Threshold").and_then(Value::as_f64),
        expected.get("Threshold").and_then(Value::as_f64),
        "case {case:?}: expected Threshold {:?}, got {:?}",
        expected.get("Threshold"),
        got.get("Threshold")
    );
}

/// The bare id an ARN carries: the last segment of its resource portion. Computed here
/// from the ARN the test sent, so the assertion does not borrow the service's own helper.
fn id_of(arn: &str) -> &str {
    match arn.rsplit_once('/') {
        Some((_, id)) => id,
        None => arn,
    }
}

/// The two threshold halves a subscription can be created with: the deprecated scalar, and
/// an expression. Every update case is driven against a subscription of each form, so a
/// rule that only holds for one of them cannot pass.
fn baseline_thresholds() -> Vec<(&'static str, Value)> {
    vec![
        ("from a scalar threshold", json!({"Threshold": 100.0})),
        (
            "from a threshold expression",
            json!({"ThresholdExpression":
                dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "15")}),
        ),
    ]
}

/// RULE: `UpdateAnomalySubscription` writes exactly the members the request names onto the
/// subscription its `SubscriptionArn` resolves to and leaves every omitted member
/// unchanged, keeping the members the service owns (`SubscriptionArn`, `AccountId` and each
/// subscriber's `Status`) its own, expanding the deprecated scalar `Threshold` into the
/// expression it is shorthand for, and answering with the subscription's ARN.
///
/// The table spans the class of updates the input admits: each member alone, every
/// `AnomalySubscriptionFrequency` value, one and several monitors, one and several
/// subscribers over both channels, both threshold forms (whole, fractional and at each
/// published bound), every member at once, and an update that names no member at all —
/// each driven against a subscription created with a scalar threshold and one created with
/// an expression.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest() {
    let emulator = Emulator::start();
    let first = create_monitor(&emulator, "updated-subscription-monitor-1").await;
    let second = create_monitor(&emulator, "updated-subscription-monitor-2").await;
    let one = vec![first.clone()];
    let both = vec![first.clone(), second.clone()];

    // A subscription no update in this test names: whatever an update writes, it writes to
    // one subscription only.
    let bystander = create_subscription(
        &emulator,
        "the-subscription-nothing-updates",
        &one,
        &json!({"Threshold": 42.0}),
    )
    .await;
    let bystander_before = subscription_by_arn(&emulator, &bystander).await;

    let mut patches: Vec<(String, Value)> = vec![
        (
            "the name alone".to_string(),
            json!({"SubscriptionName": "renamed-subscription"}),
        ),
        (
            "the monitors replaced by another monitor".to_string(),
            json!({"MonitorArnList": [second.clone()]}),
        ),
        (
            "the monitors widened to several".to_string(),
            json!({"MonitorArnList": both.clone()}),
        ),
        (
            "the subscribers replaced by an SNS topic".to_string(),
            json!({"Subscribers": [{"Address": SNS_TOPIC_ARN, "Type": "SNS"}]}),
        ),
        (
            "the subscribers widened to several over both channels".to_string(),
            json!({"Subscribers": [
                {"Address": ESCALATION_ADDRESS, "Type": "EMAIL"},
                {"Address": SNS_TOPIC_ARN, "Type": "SNS"},
            ]}),
        ),
        (
            "a subscriber whose status the caller tried to set".to_string(),
            json!({"Subscribers": [
                {"Address": EMAIL_ADDRESS, "Type": "EMAIL", "Status": "DECLINED"},
            ]}),
        ),
        (
            "a whole-dollar scalar threshold".to_string(),
            json!({"Threshold": 250.0}),
        ),
        (
            "a fractional scalar threshold".to_string(),
            json!({"Threshold": 12.5}),
        ),
        (
            "a scalar threshold at the published lower bound".to_string(),
            json!({"Threshold": 0.0}),
        ),
        (
            "an absolute threshold expression".to_string(),
            json!({"ThresholdExpression":
                dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "500")}),
        ),
        (
            "a percentage threshold expression".to_string(),
            json!({"ThresholdExpression":
                dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "40")}),
        ),
        (
            "a threshold expression at the published upper bound".to_string(),
            json!({"ThresholdExpression": dimension_threshold(
                THRESHOLD_ABSOLUTE_DIMENSION,
                &format!("{THRESHOLD_VALUE_MAX:.0}"),
            )}),
        ),
        (
            "an AND-nested threshold expression".to_string(),
            json!({"ThresholdExpression": {"And": [
                dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100"),
                dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "10"),
            ]}}),
        ),
        (
            "an OR-nested threshold expression".to_string(),
            json!({"ThresholdExpression": {"Or": [
                dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "0"),
                dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "100"),
            ]}}),
        ),
        (
            "every member at once".to_string(),
            json!({
                "SubscriptionName": "everything-updated",
                "Frequency": "WEEKLY",
                "MonitorArnList": both.clone(),
                "Subscribers": [{"Address": ESCALATION_ADDRESS, "Type": "EMAIL"}],
                "ThresholdExpression":
                    dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "75"),
            }),
        ),
        (
            "no member beyond the subscription ARN".to_string(),
            json!({}),
        ),
    ];
    // Every frequency the enum declares, including the one the baseline already carries:
    // an update to the value a subscription already holds is still an update.
    for frequency in FREQUENCIES {
        patches.push((
            format!("the frequency alone, to {frequency}"),
            json!({"Frequency": frequency}),
        ));
    }

    for (baseline_label, baseline_threshold) in baseline_thresholds() {
        for (index, (label, patch)) in patches.iter().enumerate() {
            let case = format!("{label}, {baseline_label}");
            let arn = create_subscription(
                &emulator,
                &format!("updated-{index}-{baseline_label}"),
                &one,
                &baseline_threshold,
            )
            .await;
            let before = subscription_by_arn(&emulator, &arn).await;
            let expected = expected_after(&before, patch);

            let answered = emulator
                .ok("UpdateAnomalySubscription", &update_body(&arn, patch))
                .await;
            assert_eq!(
                answered["SubscriptionArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: the update must answer with the subscription's ARN, got \
                 {answered}"
            );

            let after = subscription_by_arn(&emulator, &arn).await;
            assert_subscription_eq(&case, &after, &expected);
        }
    }

    assert_subscription_eq(
        "the subscription nothing updates",
        &subscription_by_arn(&emulator, &bystander).await,
        &bystander_before,
    );
}

/// RULE: an update whose named members do not hold together as those members document is
/// refused 400 `ValidationException`, and the subscription is left exactly as it was.
///
/// The table spans the class: both threshold members at once (the pair the member
/// documents as mutually exclusive), a monitor list and a subscriber list that name
/// nothing, and every threshold expression the member's own rules exclude — a dimension a
/// threshold cannot be read from, a missing and a wrong match option, no value, a value
/// past each published bound and one that is no number, a selector that filters costs
/// rather than an anomaly's impact, both nested types at once, an empty nesting and an
/// expression selecting nothing.
#[tokio::test]
async fn an_update_whose_members_contradict_each_other_is_refused() {
    let emulator = Emulator::start();
    let monitor = create_monitor(&emulator, "refused-update-monitor").await;
    let monitors = vec![monitor];
    let arn = create_subscription(
        &emulator,
        "the-subscription-no-refusal-touches",
        &monitors,
        &json!({"Threshold": 100.0}),
    )
    .await;
    let before = subscription_by_arn(&emulator, &arn).await;

    let mut cases: Vec<(String, Value, Option<&'static str>)> = vec![
        (
            "both threshold members at once".to_string(),
            json!({
                "Threshold": 100.0,
                "ThresholdExpression":
                    dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100"),
            }),
            Some(BOTH_THRESHOLDS_MESSAGE),
        ),
        (
            "both threshold members beside every other member".to_string(),
            json!({
                "SubscriptionName": "would-be-renamed",
                "Frequency": "IMMEDIATE",
                "MonitorArnList": monitors.clone(),
                "Subscribers": [{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}],
                "Threshold": 10.0,
                "ThresholdExpression":
                    dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "10"),
            }),
            Some(BOTH_THRESHOLDS_MESSAGE),
        ),
        (
            "a monitor list naming no monitor".to_string(),
            json!({"MonitorArnList": []}),
            None,
        ),
        (
            "a subscriber list naming no subscriber".to_string(),
            json!({"Subscribers": []}),
            None,
        ),
    ];

    // The threshold expressions the member's own rules exclude.
    let expressions: Vec<(&str, Value)> = vec![
        (
            "an expression on a dimension thresholds cannot be read from",
            dimension_threshold("SERVICE", "100"),
        ),
        (
            "an expression with no match option",
            json!({"Dimensions": {
                "Key": THRESHOLD_ABSOLUTE_DIMENSION,
                "Values": ["100"],
            }}),
        ),
        (
            "an expression matching on equality rather than the required option",
            json!({"Dimensions": {
                "Key": THRESHOLD_ABSOLUTE_DIMENSION,
                "MatchOptions": ["EQUALS"],
                "Values": ["100"],
            }}),
        ),
        (
            "an expression with no threshold value",
            json!({"Dimensions": {
                "Key": THRESHOLD_ABSOLUTE_DIMENSION,
                "MatchOptions": [THRESHOLD_MATCH_OPTION],
                "Values": [],
            }}),
        ),
        (
            "an expression one past the published upper bound",
            dimension_threshold(
                THRESHOLD_ABSOLUTE_DIMENSION,
                &format!("{:.0}", THRESHOLD_VALUE_MAX + 1.0),
            ),
        ),
        (
            "an expression below the published lower bound",
            dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "-1"),
        ),
        (
            "an expression whose value is not a number",
            dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "one hundred"),
        ),
        (
            "an expression selecting cost allocation tags",
            json!({"Tags": {"Key": "team", "Values": ["platform"]}}),
        ),
        (
            "an expression negating a threshold",
            json!({"Not": dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100")}),
        ),
        (
            "an expression nesting under both And and Or",
            json!({
                "And": [dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100")],
                "Or": [dimension_threshold(THRESHOLD_PERCENTAGE_DIMENSION, "10")],
            }),
        ),
        (
            "an expression nesting nothing under And",
            json!({"And": []}),
        ),
        ("an expression selecting nothing", json!({})),
        (
            "an expression nesting a selector thresholds cannot be read from",
            json!({"Or": [
                dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100"),
                {"CostCategories": {"Key": "Department", "Values": ["Engineering"]}},
            ]}),
        ),
    ];
    for (label, expression) in expressions {
        cases.push((
            label.to_string(),
            json!({"ThresholdExpression": expression}),
            None,
        ));
    }

    for (label, patch, expected_message) in &cases {
        let (status, answered) = emulator
            .call("UpdateAnomalySubscription", &update_body(&arn, patch))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {status}: {answered}"
        );
        match expected_message {
            Some(message) => assert_eq!(
                error_message(&answered),
                *message,
                "case {label:?}: expected the service's own sentence, got {answered}"
            ),
            None => assert!(
                !error_message(&answered).is_empty(),
                "case {label:?}: the refusal must say what was wrong with the request, got \
                 {answered}"
            ),
        }
    }

    assert_subscription_eq(
        "after every refused update",
        &subscription_by_arn(&emulator, &arn).await,
        &before,
    );
}

/// The ways a well-formed ARN can name no monitor the account holds.
///
/// Every row satisfies the `Arn` shape `MonitorArnList`'s member targets
/// (`smithy_query(query="shape", shape="Arn")`), so each one reaches the handler rather
/// than the boundary validator.
fn well_formed_unknown_arns() -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        (
            "a well-formed ARN of this account naming nothing",
            format!("arn:aws:ce::{account}:anomalymonitor/00000000-0000-8000-8000-000000000000"),
        ),
        (
            "an ARN of another account",
            "arn:aws:ce::999999999999:anomalymonitor/11111111-1111-8111-8111-111111111111"
                .to_string(),
        ),
        (
            "the ARN of another of this service's resource kinds",
            engine::emulator_arn("ce", "", "costcategory/not-a-monitor"),
        ),
        (
            "the ARN of another service's resource",
            format!("arn:aws:sns:us-east-1:{account}:not-a-monitor"),
        ),
    ]
}

/// RULE: an update may only be made against a subscription the account holds, so one whose
/// `SubscriptionArn` resolves to nothing is refused `UnknownSubscriptionException` carrying
/// the service's fixed "no subscription present" sentence over the bare id component of
/// the value the request named, and writes nothing.
///
/// The table spans the values `SubscriptionArn` admits (`GenericString`, length 0..1024,
/// pattern `^[\S\s]*$`): every kind of well-formed ARN that names nothing, values that are
/// no ARN at all, the empty string, and the ARN of a monitor rather than a subscription.
#[tokio::test]
async fn an_update_against_a_subscription_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::start();
    let monitor = create_monitor(&emulator, "unknown-subscription-monitor").await;
    let monitors = vec![monitor.clone()];
    let kept = create_subscription(
        &emulator,
        "the-only-subscription",
        &monitors,
        &json!({"Threshold": 100.0}),
    )
    .await;
    let before = subscription_by_arn(&emulator, &kept).await;

    let mut rows: Vec<(String, String)> = well_formed_unknown_arns()
        .into_iter()
        .map(|(label, arn)| (label.to_string(), arn))
        .collect();
    rows.push((
        "the ARN of a monitor rather than a subscription".to_string(),
        monitor,
    ));
    rows.push((
        "a value that is no ARN at all".to_string(),
        "subscription-1".to_string(),
    ));
    rows.push((
        "an id with no ARN around it".to_string(),
        "00000000-0000-8000-8000-000000000000".to_string(),
    ));
    rows.push(("the empty string".to_string(), String::new()));

    // Each unknown subscription is named once alone and once beside members the request
    // would otherwise have written, so the refusal does not depend on the update being
    // empty.
    let patches: [(&str, Value); 2] = [
        ("naming no other member", json!({})),
        (
            "naming every other member",
            json!({
                "SubscriptionName": "would-be-renamed",
                "Frequency": "WEEKLY",
                "MonitorArnList": monitors.clone(),
                "Subscribers": [{"Address": SNS_TOPIC_ARN, "Type": "SNS"}],
                "Threshold": 7.5,
            }),
        ),
    ];

    for (label, arn) in &rows {
        for (shape, patch) in &patches {
            let case = format!("{label} ({shape}): {arn:?}");
            let (status, answered) = emulator
                .call("UpdateAnomalySubscription", &update_body(arn, patch))
                .await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (UNKNOWN_SUBSCRIPTION_STATUS, "UnknownSubscriptionException"),
                "case {case:?}: expected {UNKNOWN_SUBSCRIPTION_STATUS} \
                 UnknownSubscriptionException, got {status}: {answered}"
            );
            assert_eq!(
                error_message(&answered),
                format!(
                    "No subscription present with {SUBSCRIPTION_ID_AS_RESOURCE}: {}",
                    id_of(arn)
                ),
                "case {case:?}: expected the service's own sentence over the bare \
                 subscription id, got {answered}"
            );
        }
    }

    assert_subscription_eq(
        "after every refused update",
        &subscription_by_arn(&emulator, &kept).await,
        &before,
    );
}

/// RULE: a subscription may only alert on monitors the account holds, so an update whose
/// `MonitorArnList` names an ARN no monitor is registered under is refused
/// `UnknownMonitorException` — carrying the service's fixed sentence over the bare monitor
/// id, under the word an operation that only REFERENCES a monitor uses — and writes
/// nothing.
///
/// The table spans every kind of well-formed ARN that names no monitor, each named alone,
/// beside a monitor the account does hold, and together with every other update member,
/// plus a monitor an earlier delete removed.
#[tokio::test]
async fn an_update_onto_a_monitor_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::start();
    let kept_monitor = create_monitor(&emulator, "the-monitor-that-stays").await;
    let removed_monitor = create_monitor(&emulator, "deleted-once").await;
    let arn = create_subscription(
        &emulator,
        "the-subscription-kept-as-it-was",
        std::slice::from_ref(&kept_monitor),
        &json!({"Threshold": 100.0}),
    )
    .await;
    emulator
        .ok(
            "DeleteAnomalyMonitor",
            &json!({"MonitorArn": removed_monitor}),
        )
        .await;
    let before = subscription_by_arn(&emulator, &arn).await;

    let mut rows: Vec<(String, String)> = well_formed_unknown_arns()
        .into_iter()
        .map(|(label, unknown)| (label.to_string(), unknown))
        .collect();
    rows.push((
        "a monitor an earlier delete already removed".to_string(),
        removed_monitor.clone(),
    ));

    for (label, unknown) in &rows {
        let bodies: Vec<(String, Value)> = vec![
            (
                format!("{label}, named alone"),
                json!({"MonitorArnList": [unknown]}),
            ),
            (
                format!("{label}, named beside a monitor the account holds"),
                json!({"MonitorArnList": [kept_monitor.clone(), unknown.clone()]}),
            ),
            (
                format!("{label}, named beside every other update member"),
                json!({
                    "SubscriptionName": "would-be-renamed",
                    "Frequency": "IMMEDIATE",
                    "MonitorArnList": [unknown],
                    "Subscribers": [{"Address": SNS_TOPIC_ARN, "Type": "SNS"}],
                    "Threshold": 3.5,
                }),
            ),
        ];
        for (case, patch) in &bodies {
            let (status, answered) = emulator
                .call("UpdateAnomalySubscription", &update_body(&arn, patch))
                .await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (UNKNOWN_MONITOR_STATUS, "UnknownMonitorException"),
                "case {case:?}: expected {UNKNOWN_MONITOR_STATUS} UnknownMonitorException, \
                 got {status}: {answered}"
            );
            assert_eq!(
                error_message(&answered),
                format!(
                    "No monitor present with {MONITOR_ID_AS_MONITOR}: {}",
                    id_of(unknown)
                ),
                "case {case:?}: expected the service's own sentence over the bare monitor \
                 id, got {answered}"
            );
        }
    }

    assert_subscription_eq(
        "after every refused update",
        &subscription_by_arn(&emulator, &arn).await,
        &before,
    );
}
