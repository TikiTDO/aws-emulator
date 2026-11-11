// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `DeleteAlarms` does with names it cannot simply remove, and for
//! how the alarm writers refuse a name that is already taken.
//!
//! Three rules, each asserted over a table whose rows differ along what the rule generalises
//! over rather than repeating one request:
//!
//! * a delete names the state it wants — no alarm under these names — so a name this account
//!   holds no alarm for removes nothing and succeeds, whatever kind of alarm once held it and
//!   whatever else the call names;
//! * an alarm a composite alarm this call leaves standing references, through its `AlarmRule`
//!   or as its `ActionsSuppressor`, cannot be deleted while that reference stands: the call is
//!   refused and nothing is removed;
//! * a name an alarm of another kind already holds is not a name a create may take: every
//!   alarm writer refuses it with HTTP 400 under the request-validation class and leaves the alarm already
//!   there untouched.

use crate::harness::{build_router, call, error_code, error_message, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN of the form a log alarm's scheduled query names
/// (`ScheduledQueryConfiguration$ScheduledQueryRoleARN`).
const QUERY_ROLE: &str = "arn:aws:iam::000000000000:role/cwl-scheduled-query";

/// The sentence a `DeleteAlarms` call naming more than one composite alarm is refused with:
/// the service's own wording for the constraint "this total can include no more than one
/// composite alarm" (`DeleteAlarms`), carrying one value from the request — how many composite
/// alarms it named — and nothing else of it.
fn multiple_composite_alarms(composites: usize) -> String {
    format!(
        "Cannot delete [{composites}] Composite Alarms as only one composite alarm delete is \
         allowed in single request"
    )
}

/// The smallest `PutMetricAlarm` request: every required member and nothing else.
fn metric_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": "MyApp",
        "MetricName": "Latency",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// The smallest `PutCompositeAlarm` request for `name`, evaluating `rule`.
fn composite_alarm(name: &str, rule: &str) -> Value {
    json!({"AlarmName": name, "AlarmRule": rule})
}

/// The smallest `PutLogAlarm` request: every required member and nothing else.
fn log_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "QueryResultsToEvaluate": 3,
        "QueryResultsToAlarm": 2,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "ScheduledQueryConfiguration": {
            "QueryString": "fields @message | filter @message like /ERROR/",
            "ScheduledQueryRoleARN": QUERY_ROLE,
            "AggregationExpression": "count(*)",
            "ScheduleConfiguration": {
                "ScheduleExpression": "rate(5 minutes)",
                "StartTimeOffset": 360,
            },
        },
    })
}

/// One row of the no-op delete table: the case, the alarms to create first as (kind, name),
/// the names one call deletes, and the alarm names left afterwards.
type DeleteCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<&'a str>, Vec<&'a str>);

/// Every alarm name `DescribeAlarms` reports, over all three kinds, sorted.
async fn remaining_alarm_names(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(
        router,
        "DescribeAlarms",
        &json!({"AlarmTypes": ["MetricAlarm", "CompositeAlarm", "LogAlarm"]}),
    )
    .await;
    let mut names: Vec<String> = ["MetricAlarms", "CompositeAlarms", "LogAlarms"]
        .iter()
        .filter_map(|group| body.get(*group))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

/// The ARN the service minted for the alarm `name`, whichever kind it is.
async fn alarm_arn(router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(
        router,
        "DescribeAlarms",
        &json!({
            "AlarmNames": [name],
            "AlarmTypes": ["MetricAlarm", "CompositeAlarm", "LogAlarm"],
        }),
    )
    .await;
    ["MetricAlarms", "CompositeAlarms", "LogAlarms"]
        .iter()
        .filter_map(|group| body.get(*group))
        .filter_map(Value::as_array)
        .flatten()
        .find(|alarm| alarm.get("AlarmName").and_then(Value::as_str) == Some(name))
        .and_then(|alarm| alarm.get("AlarmArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the stored alarm {name} carries its ARN, got {body}"))
        .to_string()
}

#[tokio::test]
async fn deleting_a_name_no_alarm_holds_is_a_no_op_success() {
    // The rows differ along why the store holds no alarm for the name — never created, or
    // created and already deleted — along the kind of alarm that once held it, and along what
    // else the same call names. Each row's expectation follows from the rule alone: the call
    // succeeds, and afterwards none of the names it mentioned name an alarm.
    //
    // `(case, alarms to create first, the names to delete, the names left afterwards)`
    let cases: Vec<DeleteCase<'_>> = vec![
        ("a name nothing ever created", vec![], vec!["absent"], vec![]),
        (
            "a metric alarm deleted a second time",
            vec![("metric", "once")],
            vec!["once"],
            vec![],
        ),
        (
            "a composite alarm deleted a second time",
            vec![("composite", "once")],
            vec!["once"],
            vec![],
        ),
        (
            "a log alarm deleted a second time",
            vec![("log", "once")],
            vec!["once"],
            vec![],
        ),
        (
            "several names nothing holds, in one call",
            vec![],
            vec!["absent-one", "absent-two", "absent-three"],
            vec![],
        ),
        (
            "an absent name repeated",
            vec![],
            vec!["absent", "absent"],
            vec![],
        ),
        (
            "an alarm that is there beside names that are not",
            vec![("metric", "present")],
            vec!["absent", "present", "also-absent"],
            vec![],
        ),
        (
            "an alarm left alone beside a name nothing holds",
            vec![("metric", "kept"), ("metric", "removed")],
            vec!["removed", "absent"],
            vec!["kept"],
        ),
    ];

    for (case, created, deleted, left) in cases {
        let router = build_router();
        for (kind, name) in &created {
            let (operation, request) = match *kind {
                "metric" => ("PutMetricAlarm", metric_alarm(name)),
                "composite" => ("PutCompositeAlarm", composite_alarm(name, "TRUE")),
                _ => ("PutLogAlarm", log_alarm(name)),
            };
            ok(&router, operation, &request).await;
        }
        // The first delete removes whatever was created under a name this row deletes; the
        // second is the one the rule is about, and it must answer exactly as the first did.
        for attempt in ["first", "second"] {
            let (status, body) = call(&router, "DeleteAlarms", &json!({"AlarmNames": deleted}))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (200, ""),
                "case {case:?}: the {attempt} delete of {deleted:?} must succeed, got {status} \
                 with {body}"
            );
        }
        let remaining = remaining_alarm_names(&router).await;
        assert_eq!(
            remaining, left,
            "case {case:?}: expected {left:?} left after deleting {deleted:?}, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn an_alarm_a_live_composite_alarm_references_cannot_be_deleted() {
    // The rows differ along the kind of alarm the reference points at, the member that
    // carries the reference (`AlarmRule` or `ActionsSuppressor`), and the form the reference
    // takes — "You can use either alarm names or ARNs to reference the other alarms that are
    // to be evaluated." (`PutCompositeAlarmInput$AlarmRule`).
    //
    // `(case, kind of the referenced alarm, reference member, reference by ARN)`
    let cases: Vec<(&str, &str, &str, bool)> = vec![
        ("a metric alarm named by a rule", "metric", "AlarmRule", false),
        (
            "a metric alarm named by a rule as an ARN",
            "metric",
            "AlarmRule",
            true,
        ),
        (
            "a composite alarm named by a rule",
            "composite",
            "AlarmRule",
            false,
        ),
        (
            "a metric alarm set as the actions suppressor",
            "metric",
            "ActionsSuppressor",
            false,
        ),
        (
            "a metric alarm set as the actions suppressor by ARN",
            "metric",
            "ActionsSuppressor",
            true,
        ),
        (
            "a composite alarm set as the actions suppressor",
            "composite",
            "ActionsSuppressor",
            false,
        ),
    ];

    for (case, kind, member, by_arn) in cases {
        let router = build_router();
        if kind == "metric" {
            ok(&router, "PutMetricAlarm", &metric_alarm("child")).await;
        } else {
            ok(
                &router,
                "PutCompositeAlarm",
                &composite_alarm("child", "TRUE"),
            )
            .await;
        }
        let reference = if by_arn {
            alarm_arn(&router, "child").await
        } else {
            "child".to_string()
        };
        let watcher = if member == "AlarmRule" {
            composite_alarm("watcher", &format!("ALARM(\"{reference}\")"))
        } else {
            json!({
                "AlarmName": "watcher",
                "AlarmRule": "TRUE",
                "ActionsSuppressor": reference,
                "ActionsSuppressorWaitPeriod": 120,
                "ActionsSuppressorExtensionPeriod": 180,
            })
        };
        ok(&router, "PutCompositeAlarm", &watcher).await;

        let (status, body) = call(&router, "DeleteAlarms", &json!({"AlarmNames": ["child"]})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: deleting an alarm a live composite alarm references must be \
             refused, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            "Cannot delete child as there are composite alarm(s) depending on it.",
            "case {case:?}: the refusal must name the alarm it would not delete, got \
             {message:?}"
        );
        let remaining = remaining_alarm_names(&router).await;
        assert_eq!(
            remaining,
            vec!["child".to_string(), "watcher".to_string()],
            "case {case:?}: a refused delete removed an alarm anyway, leaving {remaining:?}"
        );

        // The reference is what holds the delete back, so a call that takes the composite
        // alarm away with its child — and a call made after the composite alarm is gone —
        // both go through.
        let together = call(
            &router,
            "DeleteAlarms",
            &json!({"AlarmNames": ["child", "watcher"]}),
        )
        .await;
        if kind == "metric" {
            assert_eq!(
                (together.0, error_code(&together.1).as_str()),
                (200, ""),
                "case {case:?}: deleting the composite alarm together with its child leaves no \
                 reference behind, got {:?}",
                together
            );
            let remaining = remaining_alarm_names(&router).await;
            assert!(
                remaining.is_empty(),
                "case {case:?}: expected both alarms gone, got {remaining:?}"
            );
        } else {
            // Two composite alarms in one call is refused on its own count rule, so the
            // reference is released by deleting the composite alarm that holds it first.
            assert_eq!(
                (together.0, error_code(&together.1).as_str()),
                (400, "ValidationException"),
                "case {case:?}: one call can delete no more than one composite alarm, got {:?}",
                together
            );
            ok(&router, "DeleteAlarms", &json!({"AlarmNames": ["watcher"]})).await;
            ok(&router, "DeleteAlarms", &json!({"AlarmNames": ["child"]})).await;
            let remaining = remaining_alarm_names(&router).await;
            assert!(
                remaining.is_empty(),
                "case {case:?}: expected both alarms gone, got {remaining:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_call_deletes_no_more_than_one_composite_alarm() {
    // "However, this total can include no more than one composite alarm." (`DeleteAlarms`) —
    // the rows differ along how many composite alarms the call names and what they are named
    // beside, and each row states the count the refusal must report: a name the call repeats
    // names ONE alarm, so the repeated row is still two.
    let cases: Vec<(&str, Vec<&str>, usize)> = vec![
        ("two composite alarms", vec!["one", "two"], 2),
        ("three composite alarms", vec!["one", "two", "three"], 3),
        (
            "two composite alarms and a metric alarm",
            vec!["one", "leaf", "two"],
            2,
        ),
        (
            "the same two composite alarms, one of them repeated",
            vec!["one", "two", "two"],
            2,
        ),
    ];

    for (case, deleted, composites) in cases {
        let router = build_router();
        ok(&router, "PutMetricAlarm", &metric_alarm("leaf")).await;
        for name in ["one", "two", "three"] {
            ok(&router, "PutCompositeAlarm", &composite_alarm(name, "TRUE")).await;
        }

        let (status, body) = call(&router, "DeleteAlarms", &json!({"AlarmNames": deleted})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let message = error_message(&body);
        let expected = multiple_composite_alarms(composites);
        assert_eq!(
            message, expected,
            "case {case:?}: the refusal must name the constraint it enforced in CloudWatch's \
             own words, expected {expected:?}, got {message:?}"
        );
        let remaining = remaining_alarm_names(&router).await;
        assert_eq!(
            remaining,
            vec![
                "leaf".to_string(),
                "one".to_string(),
                "three".to_string(),
                "two".to_string()
            ],
            "case {case:?}: a refused delete removed an alarm anyway, leaving {remaining:?}"
        );
    }

    // One composite alarm, named beside as many metric alarms as the caller likes, is the
    // call the constraint allows.
    let router = build_router();
    ok(&router, "PutMetricAlarm", &metric_alarm("leaf")).await;
    ok(&router, "PutCompositeAlarm", &composite_alarm("one", "TRUE")).await;
    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["one", "leaf"]}),
    )
    .await;
    let remaining = remaining_alarm_names(&router).await;
    assert!(
        remaining.is_empty(),
        "one composite alarm beside a metric alarm is one call's work, got {remaining:?}"
    );
}

#[tokio::test]
async fn a_name_an_alarm_of_another_kind_holds_is_refused_as_a_validation_error() {
    // "This name must be unique within the Amazon Web Services account and Region."
    // (`PutLogAlarmInput$AlarmName`) — one name names ONE alarm, so no writer may take a name
    // an alarm of another kind holds. The rows are every ordered pair of the three kinds of
    // alarm: which kind holds the name, and which writer then asks for it.
    let kinds = ["metric", "composite", "log"];
    for holder in kinds {
        for writer in kinds {
            if holder == writer {
                continue;
            }
            let case = format!("a {writer} alarm over a name a {holder} alarm holds");
            let router = build_router();
            let (holder_operation, holder_request) = request_for(holder, "taken");
            ok(&router, holder_operation, &holder_request).await;

            let (writer_operation, writer_request) = request_for(writer, "taken");
            let (status, body) = call(&router, writer_operation, &writer_request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                "AlarmName [taken] is already used by another alarm of a different type",
                "case {case:?}: the refusal must say the name is held by an alarm of another \
                 type, got {message:?}"
            );

            // "the existing alarm is left unchanged": the name still names the alarm that
            // held it, of the kind that held it.
            let described = ok(
                &router,
                "DescribeAlarms",
                &json!({
                    "AlarmNames": ["taken"],
                    "AlarmTypes": ["MetricAlarm", "CompositeAlarm", "LogAlarm"],
                }),
            )
            .await;
            let group = match holder {
                "metric" => "MetricAlarms",
                "composite" => "CompositeAlarms",
                _ => "LogAlarms",
            };
            let held = described
                .get(group)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: the alarm already there must be left alone, got {described}"
            );
        }
    }
}

/// The create request of `kind` for the alarm `name`, with the operation that serves it.
fn request_for(kind: &str, name: &str) -> (&'static str, Value) {
    match kind {
        "metric" => ("PutMetricAlarm", metric_alarm(name)),
        "composite" => ("PutCompositeAlarm", composite_alarm(name, "TRUE")),
        _ => ("PutLogAlarm", log_alarm(name)),
    }
}
