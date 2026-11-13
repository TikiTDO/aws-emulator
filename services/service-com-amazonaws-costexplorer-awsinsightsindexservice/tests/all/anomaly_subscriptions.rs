// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the cost anomaly subscription registry
//! (`CreateAnomalySubscription`, `GetAnomalySubscriptions`).
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`], so nothing here reaches past the HTTP surface a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::state::{
    MAX_ANOMALY_SUBSCRIPTIONS, MONITOR_TYPE_DIMENSIONAL, SUBSCRIBER_STATUS_CONFIRMED,
    THRESHOLD_ABSOLUTE_DIMENSION, THRESHOLD_MATCH_OPTION, THRESHOLD_VALUE_MAX,
};

use crate::harness::{error_code, Emulator};

/// An email subscriber address, in the form `SubscriberAddress`'s modelled pattern admits
/// for the email channel.
const EMAIL_ADDRESS: &str = "cost.ops@example.com";

/// An SNS topic ARN, in the form `SubscriberAddress`'s modelled pattern admits for the SNS
/// channel. Nothing in this emulator holds a topic at it: `CreateAnomalySubscription`
/// registers the subscriber the caller named, it does not resolve the topic.
const SNS_TOPIC_ARN: &str = "arn:aws:sns:us-east-1:123456789012:cost-anomaly-alerts";

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

/// The ARN a successful create answered with.
fn arn_of(created: &Value) -> String {
    created["SubscriptionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("SubscriptionArn must be present, got {created}"))
        .to_string()
}

/// Every subscription the account holds, in the order `GetAnomalySubscriptions` reports
/// them.
async fn all_subscriptions(emulator: &Emulator) -> Vec<Value> {
    emulator.ok("GetAnomalySubscriptions", &json!({})).await["AnomalySubscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Assert a minted subscription ARN is the one AWS shape:
/// `arn:aws:ce::<account>:anomalysubscription/` plus the 8-4-4-4-12 lowercase-hex id those
/// ARNs carry.
fn assert_subscription_arn(label: &str, arn: &str) {
    let expected_prefix = format!(
        "arn:aws:ce::{}:anomalysubscription/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_else(|| {
        panic!("case {label:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}")
    });
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {label:?}: expected an 8-4-4-4-12 id, got {id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())),
        "case {label:?}: expected lowercase hex in the id, got {id:?}"
    );
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

/// The `ThresholdExpression` a subscription written with the deprecated scalar
/// `Threshold` is stored carrying: the absolute-impact dimension compared with the one
/// required match option against the value the caller named
/// (`AnomalySubscription$Threshold`: "Continued use of Threshold will be treated as
/// shorthand syntax for a ThresholdExpression").
fn shorthand_expansion(rendered: &str) -> Value {
    dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, rendered)
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

/// A `CreateAnomalySubscription` body.
fn subscription_body(
    name: &str,
    monitors: &[String],
    frequency: &str,
    subscribers: Value,
    threshold: Value,
) -> Value {
    let mut subscription = json!({
        "SubscriptionName": name,
        "MonitorArnList": monitors,
        "Subscribers": subscribers,
        "Frequency": frequency,
    });
    for (member, value) in threshold.as_object().expect("a threshold object") {
        subscription[member] = value.clone();
    }
    json!({"AnomalySubscription": subscription})
}

/// RULE: `CreateAnomalySubscription` mints the subscription's ARN, persists the
/// subscription as the caller described it — with `AccountId` and each subscriber's
/// `Status` owned by the service, the deprecated scalar `Threshold` expanded into the
/// `ThresholdExpression` it is shorthand for and reported back as the dollar threshold
/// that expression names — and `GetAnomalySubscriptions` reports that stored subscription.
///
/// The table spans the class of subscriptions the model admits: every
/// `AnomalySubscriptionFrequency` value, both `SubscriberType` channels, one and several
/// subscribers, one and several monitors, a create with and without `ResourceTags`, and
/// every threshold form the members document — the deprecated scalar (whole and
/// fractional), an absolute and a percentage expression, and `And`- and `Or`-nested
/// expressions.
#[tokio::test]
async fn a_created_subscription_is_reported_as_it_was_described() {
    let emulator = Emulator::start();
    let first = create_monitor(&emulator, "subscribed-monitor-1").await;
    let second = create_monitor(&emulator, "subscribed-monitor-2").await;
    let both = vec![first.clone(), second.clone()];
    let one = vec![first.clone()];

    let cases: Vec<(&str, Value)> = vec![
        (
            "a daily email subscription with the deprecated scalar threshold",
            subscription_body(
                "daily-email",
                &one,
                "DAILY",
                json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                json!({"Threshold": 100}),
            ),
        ),
        (
            "a weekly email subscription with a fractional scalar threshold",
            subscription_body(
                "weekly-email",
                &one,
                "WEEKLY",
                json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                json!({"Threshold": 12.5}),
            ),
        ),
        (
            "an immediate SNS subscription with an absolute threshold expression",
            subscription_body(
                "immediate-sns",
                &one,
                "IMMEDIATE",
                json!([{"Address": SNS_TOPIC_ARN, "Type": "SNS"}]),
                json!({"ThresholdExpression":
                    dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "250")}),
            ),
        ),
        (
            "a subscription with a percentage threshold expression",
            subscription_body(
                "percentage-threshold",
                &one,
                "DAILY",
                json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                json!({"ThresholdExpression":
                    dimension_threshold("ANOMALY_TOTAL_IMPACT_PERCENTAGE", "40")}),
            ),
        ),
        (
            "a subscription over several monitors and several subscribers",
            subscription_body(
                "many-monitors",
                &both,
                "IMMEDIATE",
                json!([
                    {"Address": SNS_TOPIC_ARN, "Type": "SNS"},
                    {"Address": EMAIL_ADDRESS, "Type": "EMAIL"},
                ]),
                json!({"ThresholdExpression": {"And": [
                    dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100"),
                    dimension_threshold("ANOMALY_TOTAL_IMPACT_PERCENTAGE", "10"),
                ]}}),
            ),
        ),
        (
            "a subscription whose thresholds are OR-nested",
            subscription_body(
                "or-nested",
                &both,
                "WEEKLY",
                json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                json!({"ThresholdExpression": {"Or": [
                    dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "0"),
                    dimension_threshold("ANOMALY_TOTAL_IMPACT_PERCENTAGE", "100"),
                ]}}),
            ),
        ),
    ];

    // The same table once more, tagged: resource tags are an optional member of the
    // create, not of the subscription, so they must not change what is stored or reported.
    let mut requested: Vec<(String, Value)> = Vec::new();
    for (label, body) in &cases {
        requested.push(((*label).to_string(), body.clone()));
        let mut tagged = body.clone();
        tagged["AnomalySubscription"]["SubscriptionName"] =
            json!(format!("{}-tagged", body["AnomalySubscription"]["SubscriptionName"]
                .as_str()
                .unwrap_or_default()));
        tagged["ResourceTags"] = json!([{"Key": "team", "Value": "platform"}]);
        requested.push((format!("{label}, tagged"), tagged));
    }

    let mut minted: Vec<String> = Vec::new();
    for (label, body) in &requested {
        let created = emulator.ok("CreateAnomalySubscription", body).await;
        let arn = arn_of(&created);
        assert_subscription_arn(label, &arn);
        assert!(
            !minted.contains(&arn),
            "case {label:?}: every subscription gets its own ARN, got a repeat of {arn:?}"
        );
        minted.push(arn);
    }

    let reported = all_subscriptions(&emulator).await;
    assert_eq!(
        reported.len(),
        requested.len(),
        "every created subscription must be reported, got {reported:?}"
    );

    for ((label, body), arn) in requested.iter().zip(&minted) {
        let sent = &body["AnomalySubscription"];
        let subscription = reported
            .iter()
            .find(|candidate| candidate["SubscriptionArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected the subscription {arn:?} to be reported")
            });
        for member in ["SubscriptionName", "Frequency", "MonitorArnList"] {
            assert_eq!(
                subscription[member], sent[member],
                "case {label:?}: {member} must round-trip, got {subscription}"
            );
        }
        assert_eq!(
            subscription["AccountId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {label:?}: AccountId must be the account the emulator serves, got \
             {subscription}"
        );

        // The subscribers: the caller's channel and address, with the status the service
        // owns.
        let sent_subscribers = sent["Subscribers"].as_array().expect("subscribers");
        let got_subscribers = subscription["Subscribers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: Subscribers must be present"));
        assert_eq!(
            got_subscribers.len(),
            sent_subscribers.len(),
            "case {label:?}: every subscriber must be reported, got {subscription}"
        );
        for (index, (got, sent_subscriber)) in
            got_subscribers.iter().zip(sent_subscribers).enumerate()
        {
            for member in ["Address", "Type"] {
                assert_eq!(
                    got[member], sent_subscriber[member],
                    "case {label:?}: subscriber {index}'s {member} must round-trip, got {got}"
                );
            }
            assert_eq!(
                got["Status"].as_str(),
                Some(SUBSCRIBER_STATUS_CONFIRMED),
                "case {label:?}: subscriber {index}'s status is the service's own, got {got}"
            );
        }

        // The threshold: the scalar is reported only when the caller wrote it, and the
        // stored expression is the caller's or the one their scalar expands into.
        match sent["Threshold"].as_f64() {
            Some(threshold) => {
                assert_eq!(
                    subscription["Threshold"].as_f64(),
                    Some(threshold),
                    "case {label:?}: the scalar Threshold must round-trip, got {subscription}"
                );
                let rendered = if threshold.fract() == 0.0 {
                    format!("{threshold:.1}")
                } else {
                    format!("{threshold}")
                };
                assert_eq!(
                    subscription["ThresholdExpression"],
                    shorthand_expansion(&rendered),
                    "case {label:?}: the scalar must be stored as the expression it is \
                     shorthand for, got {subscription}"
                );
            }
            None => {
                // The deprecated scalar is a dollar READING of the stored expression: the
                // value of the absolute-impact dimension it names, and absent when the
                // expression names only a percentage.
                assert_eq!(
                    subscription.get("Threshold").and_then(Value::as_f64),
                    absolute_threshold_in(&sent["ThresholdExpression"]),
                    "case {label:?}: expected the deprecated scalar to report the dollar \
                     threshold the expression names, got {subscription}"
                );
                assert_eq!(
                    subscription["ThresholdExpression"], sent["ThresholdExpression"],
                    "case {label:?}: ThresholdExpression must round-trip, got {subscription}"
                );
            }
        }
    }
}

/// RULE: the deprecated scalar `Threshold` is shorthand for an absolute threshold
/// expression, whatever value it carries, and the value it expands to is the double the
/// caller sent rendered in full decimal form.
///
/// The table spans the published range the expansion's `Values` member must stay inside —
/// `AnomalySubscription$ThresholdExpression`: "Values must be numbers between 0 and
/// 10,000,000,000 in string format" — at both bounds, and a whole and a fractional value
/// in between, so the rendering is exercised on more than the one value the recorded case
/// used.
#[tokio::test]
async fn the_deprecated_scalar_threshold_expands_over_its_whole_range() {
    let emulator = Emulator::start();
    let monitor = create_monitor(&emulator, "shorthand-monitor").await;
    let monitors = vec![monitor];

    let cases: Vec<(f64, &str)> = vec![
        (0.0, "0.0"),
        (1.0, "1.0"),
        (99.5, "99.5"),
        (1234.0, "1234.0"),
        (THRESHOLD_VALUE_MAX, "10000000000.0"),
    ];

    for (threshold, rendered) in cases {
        let created = emulator
            .ok(
                "CreateAnomalySubscription",
                &subscription_body(
                    &format!("shorthand-{rendered}"),
                    &monitors,
                    "DAILY",
                    json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                    json!({"Threshold": threshold}),
                ),
            )
            .await;
        let arn = arn_of(&created);
        let reported = emulator
            .ok(
                "GetAnomalySubscriptions",
                &json!({"SubscriptionArnList": [arn]}),
            )
            .await;
        let subscription = &reported["AnomalySubscriptions"][0];
        assert_eq!(
            subscription["ThresholdExpression"],
            shorthand_expansion(rendered),
            "case {threshold:?}: expected the scalar to expand to the absolute threshold \
             expression at {rendered:?}, got {subscription}"
        );
        assert_eq!(
            subscription["Threshold"].as_f64(),
            Some(threshold),
            "case {threshold:?}: the scalar itself must still be reported, got {subscription}"
        );
    }
}

/// RULE: a subscription whose members do not hold together as those members document is
/// refused 400 `ValidationException`, and nothing is persisted.
///
/// The table spans the class: a subscription naming no monitor and one naming no
/// subscriber, both threshold members and neither of them, and a `ThresholdExpression`
/// that is not one of the two forms the member documents — a dimension it does not
/// support, a missing or wrong match option, a value outside the published range or not a
/// number at all, a selector that filters costs rather than an anomaly's impact, both
/// nested types at once, an empty nesting and an expression selecting nothing.
#[tokio::test]
async fn a_subscription_whose_members_contradict_each_other_is_refused() {
    let emulator = Emulator::start();
    let monitor = create_monitor(&emulator, "refusal-monitor").await;
    let monitors = vec![monitor];
    let subscribers = json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]);

    let mut cases: Vec<(String, Value)> = vec![
        (
            "a subscription naming no monitor".to_string(),
            subscription_body(
                "no-monitors",
                &[],
                "DAILY",
                subscribers.clone(),
                json!({"Threshold": 100}),
            ),
        ),
        (
            "a subscription naming no subscriber".to_string(),
            subscription_body(
                "no-subscribers",
                &monitors,
                "DAILY",
                json!([]),
                json!({"Threshold": 100}),
            ),
        ),
        (
            "a subscription with no threshold at all".to_string(),
            subscription_body(
                "no-threshold",
                &monitors,
                "DAILY",
                subscribers.clone(),
                json!({}),
            ),
        ),
        (
            "a subscription with both threshold members".to_string(),
            subscription_body(
                "both-thresholds",
                &monitors,
                "DAILY",
                subscribers.clone(),
                json!({
                    "Threshold": 100,
                    "ThresholdExpression":
                        dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "100"),
                }),
            ),
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
            dimension_threshold(THRESHOLD_ABSOLUTE_DIMENSION, "10000000001"),
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
                "Or": [dimension_threshold("ANOMALY_TOTAL_IMPACT_PERCENTAGE", "10")],
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
    for (index, (label, expression)) in expressions.into_iter().enumerate() {
        cases.push((
            label.to_string(),
            subscription_body(
                &format!("bad-expression-{index}"),
                &monitors,
                "IMMEDIATE",
                subscribers.clone(),
                json!({"ThresholdExpression": expression}),
            ),
        ));
    }

    for (label, body) in &cases {
        let (status, answered) = emulator.call("CreateAnomalySubscription", body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {status}: {answered}"
        );
    }
    assert!(
        all_subscriptions(&emulator).await.is_empty(),
        "a refused create must persist nothing, got {:?}",
        all_subscriptions(&emulator).await
    );
}

/// RULE: a subscription may only alert on monitors the account holds, so a create naming
/// an ARN no monitor is registered under is refused `UnknownMonitorException` and nothing
/// is persisted. The status and the sentence that refusal carries are asserted in
/// [`crate::unknown_monitor_references`].
#[tokio::test]
async fn a_subscription_on_a_monitor_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::start();
    let known = create_monitor(&emulator, "the-only-monitor").await;
    let account = engine::emulator_account_id();

    let unknown = [
        (
            "an ARN of the right shape naming no monitor",
            format!("arn:aws:ce::{account}:anomalymonitor/00000000-0000-8000-8000-000000000000"),
        ),
        (
            "an ARN of another of this service's resources",
            engine::emulator_arn("ce", "", "costcategory/not-a-monitor"),
        ),
        (
            "an ARN of another service's resource",
            format!("arn:aws:sns:us-east-1:{account}:not-a-monitor"),
        ),
    ];
    for (label, arn) in &unknown {
        // Alone, and beside an ARN the account does hold: one unknown monitor refuses the
        // whole create either way.
        for (shape, list) in [
            ("on its own", vec![arn.clone()]),
            ("beside a known monitor", vec![known.clone(), arn.clone()]),
        ] {
            let (status, answered) = emulator
                .call(
                    "CreateAnomalySubscription",
                    &subscription_body(
                        "unknown-monitor",
                        &list,
                        "DAILY",
                        json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                        json!({"Threshold": 100}),
                    ),
                )
                .await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "UnknownMonitorException"),
                "case {label:?} ({shape}): expected a 400 UnknownMonitorException, got \
                 {status}: {answered}"
            );
        }
    }
    assert!(
        all_subscriptions(&emulator).await.is_empty(),
        "a refused create must persist nothing"
    );
}

/// RULE: the account holds at most [`MAX_ANOMALY_SUBSCRIPTIONS`] subscriptions, and a
/// create past that is refused with the failure the operation models rather than stored.
#[tokio::test]
async fn a_create_past_the_subscription_limit_is_refused() {
    let emulator = Emulator::start();
    let monitors = vec![create_monitor(&emulator, "crowded-monitor").await];
    let subscribers = json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]);
    for index in 0..MAX_ANOMALY_SUBSCRIPTIONS {
        emulator
            .ok(
                "CreateAnomalySubscription",
                &subscription_body(
                    &format!("at-the-limit-{index}"),
                    &monitors,
                    "DAILY",
                    subscribers.clone(),
                    json!({"Threshold": 10}),
                ),
            )
            .await;
    }
    // Twice over: the limit holds for every later call, not only the first one past it.
    for attempt in 0..2 {
        let (status, answered) = emulator
            .call(
                "CreateAnomalySubscription",
                &subscription_body(
                    "over-the-limit",
                    &monitors,
                    "DAILY",
                    subscribers.clone(),
                    json!({"Threshold": 10}),
                ),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "LimitExceededException"),
            "case {attempt:?}: expected a 400 LimitExceededException, got {status}: {answered}"
        );
    }
    assert_eq!(
        all_subscriptions(&emulator).await.len(),
        MAX_ANOMALY_SUBSCRIPTIONS,
        "a refused create must persist nothing"
    );
}

/// RULE: `GetAnomalySubscriptions` reports the subscriptions the account holds, filtered
/// to `SubscriptionArnList` and to `MonitorArn` when either is given, and pages at
/// `MaxResults` with a `NextPageToken` that resumes exactly where the previous page
/// stopped.
///
/// The table spans the page sizes the result can be cut at: one subscription per page, a
/// size that divides the result, one that does not, exactly the result size and more than
/// it.
#[tokio::test]
async fn subscriptions_are_filtered_by_arn_and_monitor_and_paged_by_max_results() {
    let emulator = Emulator::start();
    let watched = create_monitor(&emulator, "watched-monitor").await;
    let other = create_monitor(&emulator, "other-monitor").await;

    let mut minted: Vec<String> = Vec::new();
    let mut on_watched: Vec<String> = Vec::new();
    for index in 0..6 {
        // Half the subscriptions alert on each monitor, so the monitor filter has
        // something to leave out.
        let monitors = if index % 2 == 0 {
            vec![watched.clone()]
        } else {
            vec![other.clone()]
        };
        let created = emulator
            .ok(
                "CreateAnomalySubscription",
                &subscription_body(
                    &format!("paged-{index}"),
                    &monitors,
                    "DAILY",
                    json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                    json!({"Threshold": 10}),
                ),
            )
            .await;
        let arn = arn_of(&created);
        if index % 2 == 0 {
            on_watched.push(arn.clone());
        }
        minted.push(arn);
    }
    // The registry is keyed on the ARN, so that is the order a page walks.
    let mut expected_order = minted.clone();
    expected_order.sort();

    let reported: Vec<String> = all_subscriptions(&emulator)
        .await
        .iter()
        .map(|subscription| {
            subscription["SubscriptionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        reported, expected_order,
        "an unpaged read must report every subscription in ARN order"
    );
    // A read must not change what a later caller sees.
    let again = all_subscriptions(&emulator).await;
    assert_eq!(
        again.len(),
        expected_order.len(),
        "reading twice must report the same subscriptions, got {again:?}"
    );

    for page_size in [1_usize, 2, 4, 6, 10] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextPageToken"] = json!(token);
            }
            let answered = emulator.ok("GetAnomalySubscriptions", &request).await;
            let page = answered["AnomalySubscriptions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {page_size:?}: a page must hold at most MaxResults subscriptions, got \
                 {} in {answered}",
                page.len()
            );
            for subscription in &page {
                walked.push(
                    subscription["SubscriptionArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= expected_order.len() + 1,
                "case {page_size:?}: the walk must terminate, got {pages} pages"
            );
            match answered["NextPageToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected_order,
            "case {page_size:?}: paging must report every subscription exactly once, in order"
        );
    }

    // The ARN filter: a subset, in any order, reports exactly those subscriptions.
    let selected = vec![expected_order[4].clone(), expected_order[1].clone()];
    let filtered = emulator
        .ok(
            "GetAnomalySubscriptions",
            &json!({"SubscriptionArnList": selected.clone()}),
        )
        .await;
    let filtered_arns: Vec<String> = filtered["AnomalySubscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|subscription| {
            subscription["SubscriptionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let mut expected_filtered = selected.clone();
    expected_filtered.sort();
    assert_eq!(
        filtered_arns, expected_filtered,
        "the filter must report exactly the subscriptions it names"
    );

    // The monitor filter: exactly the subscriptions that alert on that monitor.
    for (label, monitor, mut expected) in [
        ("the watched monitor", watched.clone(), on_watched.clone()),
        (
            "the other monitor",
            other.clone(),
            expected_order
                .iter()
                .filter(|arn| !on_watched.contains(arn))
                .cloned()
                .collect::<Vec<String>>(),
        ),
        (
            "a monitor no subscription alerts on",
            engine::emulator_arn("ce", "", "anomalymonitor/unsubscribed"),
            Vec::new(),
        ),
    ] {
        expected.sort();
        let answered = emulator
            .ok("GetAnomalySubscriptions", &json!({"MonitorArn": monitor}))
            .await;
        let got: Vec<String> = answered["AnomalySubscriptions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|subscription| {
                subscription["SubscriptionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            got, expected,
            "case {label:?}: expected exactly the subscriptions alerting on that monitor, \
             got {answered}"
        );
    }

    // A filter naming no ARN filters nothing out.
    let unfiltered = emulator
        .ok("GetAnomalySubscriptions", &json!({"SubscriptionArnList": []}))
        .await;
    assert_eq!(
        unfiltered["AnomalySubscriptions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        expected_order.len(),
        "an empty SubscriptionArnList must filter nothing out, got {unfiltered}"
    );
}

/// RULE: the two members `GetAnomalySubscriptions` can be asked wrongly about are each
/// refused with the failure the model declares for it — a cursor this service never issued
/// is 400 `InvalidNextTokenException`, and a page size below one is a 400
/// `ValidationException`.
///
/// `SubscriptionArnList` is not one of them: it is a filter, so an ARN the account holds no
/// subscription under is selected out rather than refused — asserted in
/// [`crate::unknown_monitor_references`].
#[tokio::test]
async fn a_read_asked_wrongly_is_refused() {
    let emulator = Emulator::start();
    let monitors = vec![create_monitor(&emulator, "read-refusal-monitor").await];
    emulator
        .ok(
            "CreateAnomalySubscription",
            &subscription_body(
                "the-only-subscription",
                &monitors,
                "DAILY",
                json!([{"Address": EMAIL_ADDRESS, "Type": "EMAIL"}]),
                json!({"Threshold": 100}),
            ),
        )
        .await;

    for (label, token) in [
        ("a cursor that is not hex", "not-a-token"),
        ("a cursor with an odd number of hex digits", "abc"),
    ] {
        let (status, answered) = emulator
            .call("GetAnomalySubscriptions", &json!({"NextPageToken": token}))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidNextTokenException"),
            "case {label:?}: expected a 400 InvalidNextTokenException, got {status}: {answered}"
        );
    }

    for page_size in [0, -1] {
        let (status, answered) = emulator
            .call("GetAnomalySubscriptions", &json!({"MaxResults": page_size}))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {page_size:?}: expected a 400 ValidationException, got {status}: {answered}"
        );
    }
}
