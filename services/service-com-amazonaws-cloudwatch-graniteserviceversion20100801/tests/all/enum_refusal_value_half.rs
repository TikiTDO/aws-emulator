// SPDX-License-Identifier: CC0-1.0

//! Which halves a refusal of a value outside an enumerated type states, over every operation of
//! this service whose refusals the boundary walk words.
//!
//! RULE: a value outside an enumerated type is refused under this service's request-validation
//! class with the counting envelope, naming the member path the value arrived on and the whole
//! set the type admits in the order the set's own table is walked; whether that clause ALSO
//! states the value the request sent is a property of the enumerated TYPE, read off what this
//! service answers for that type and derivable from nothing the model carries — so one type is
//! stated alike whichever operation was called, whichever member carried the value, and at
//! whatever depth the value sat.
//!
//! The rows differ along what the rule generalises over: WHICH enumerated type the member
//! targets, which operation the value arrived on, HOW the value misses the type's set (a word
//! the type never offered, a declared value mis-cased, no characters at all), and WHERE in the
//! request the member sits — since the member path a clause names is taken from the request. The
//! value a recorded request happened to carry is one row among them.

use serde_json::{json, Value};
use service_common::validation_message::hashed_enum_order;

use crate::harness::{
    build_router, call, error_code, error_message, merged, metric_alarm_request, now_secs, ok,
};
use crate::unit_read_identity::STANDARD_UNITS;

/// The class this service answers a value its model does not admit with, as the body names it:
/// the request-validation class, whose legacy query code (`ValidationError`) travels in the
/// `x-amzn-query-error` header.
const VALIDATION: &str = "ValidationException";

/// The values `ScanBy` declares, in the order the MODEL declares them
/// (`smithy_query(query="shape",shape="ScanBy")`).
const DECLARED_SCAN_BY: [&str; 2] = ["TimestampDescending", "TimestampAscending"];

/// The same two values in the order a refusal STATES them, which is neither the declaration
/// order above nor a sorted one. Recorded from the service's own refusal; the test below asserts
/// the derivation from the declared values reproduces exactly this.
const STATED_SCAN_BY: [&str; 2] = ["TimestampAscending", "TimestampDescending"];

/// The values `MetricStreamOutputFormat` declares, in the order the MODEL declares them
/// (`smithy_query(query="shape",shape="MetricStreamOutputFormat")`).
const DECLARED_OUTPUT_FORMATS: [&str; 3] = ["json", "opentelemetry0.7", "opentelemetry1.0"];

/// The same three values in the order a refusal STATES them. Recorded from the service's own
/// refusal, like [`STATED_SCAN_BY`].
const STATED_OUTPUT_FORMATS: [&str; 3] = ["opentelemetry1.0", "json", "opentelemetry0.7"];

/// The values `HistoryItemType` declares, in the order the MODEL declares them
/// (`smithy_query(query="shape",shape="HistoryItemType")`).
const DECLARED_HISTORY_ITEM_TYPES: [&str; 5] = [
    "ConfigurationUpdate",
    "StateUpdate",
    "Action",
    "AlarmContributorStateUpdate",
    "AlarmContributorAction",
];

/// The values `StateValue` declares, in the order the MODEL declares them
/// (`smithy_query(query="shape",shape="StateValue")`).
const DECLARED_STATE_VALUES: [&str; 3] = ["OK", "ALARM", "INSUFFICIENT_DATA"];

/// The values `ComparisonOperator` declares, in the order the MODEL declares them
/// (`smithy_query(query="shape",shape="ComparisonOperator")`).
const DECLARED_COMPARISON_OPERATORS: [&str; 7] = [
    "GreaterThanOrEqualToThreshold",
    "GreaterThanThreshold",
    "LessThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanLowerOrGreaterThanUpperThreshold",
    "LessThanLowerThreshold",
    "GreaterThanUpperThreshold",
];

/// The enumerated TYPES this service states a refusal of with NO submitted value, each row
/// holding the set its type admits.
///
/// Recorded from the service's own refusals, exactly as the stated ORDER of a type's values is:
/// one type so far, the scan DIRECTION a read is ordered by. Every other type's refusal states
/// the value that was sent.
const VALUE_WITHHELD_TYPES: [&[&str]; 1] = [&DECLARED_SCAN_BY];

/// Whether a refusal of a value outside the type admitting `declared` states the value that was
/// sent: every type but the ones listed above, matched as a SET so the row holds however the
/// values were presented.
fn states_the_submitted_value(declared: &[&str]) -> bool {
    !VALUE_WITHHELD_TYPES.iter().any(|withheld| {
        withheld.len() == declared.len() && withheld.iter().all(|value| declared.contains(value))
    })
}

/// The whole sentence a refusal of `submitted` at `path` states, for a member targeting the type
/// admitting `declared`.
fn expected_refusal(path: &str, declared: &[&str], submitted: &str) -> String {
    let half = if states_the_submitted_value(declared) {
        format!("Value '{submitted}' at '{path}'")
    } else {
        format!("Value at '{path}'")
    };
    format!(
        "1 validation error detected: {half} failed to satisfy constraint: Member must satisfy \
         enum value set: [{}]",
        hashed_enum_order(declared).join(", ")
    )
}

/// One row of the table: a request carrying one value outside one enumerated type.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The operation the request is sent to.
    operation: &'static str,
    /// The request itself.
    request: Value,
    /// The member path the clause must name: the walk's lowerCamel spelling of the member.
    path: String,
    /// The values the type that member targets admits.
    declared: &'static [&'static str],
    /// The value the request carried.
    submitted: &'static str,
}

/// A whole-minute instant a few minutes in the past, so no row is refused for its time range.
fn closed_minute() -> f64 {
    (now_secs() / 60.0).floor() * 60.0 - 3.0 * 60.0
}

/// A `GetMetricData` request whose `position`th query (1-based) carries `unit`, with `patch`
/// merged over the whole request — so both a top-level member and one nested inside the query
/// array can be put outside its type.
fn metric_data_request(position: usize, unit: &str, patch: &Value) -> Value {
    let bucket = closed_minute();
    let queries: Vec<Value> = (1..=position)
        .map(|index| {
            json!({
                "Id": format!("q{index}"),
                "MetricStat": {
                    "Metric": {"Namespace": "MyApp/EnumHalves", "MetricName": "Latency"},
                    "Period": 60,
                    "Stat": "Sum",
                    "Unit": if index == position { unit } else { "Bytes" },
                },
            })
        })
        .collect();
    merged(
        json!({
            "MetricDataQueries": queries,
            "StartTime": bucket - 600.0,
            "EndTime": bucket + 600.0,
        }),
        patch,
    )
}

/// A `PutMetricStream` request for `name` whose destination and role are both well formed, with
/// `patch` merged over it: a value outside `MetricStreamOutputFormat` is then the only thing
/// wrong with the request.
fn metric_stream_request(name: &str, patch: &Value) -> Value {
    merged(
        json!({
            "Name": name,
            "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
            "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
            "OutputFormat": "json",
        }),
        patch,
    )
}

#[tokio::test]
async fn the_stated_order_of_a_value_set_is_derived_from_the_values_themselves() {
    // The order a refusal lists a type's values in is not written out row by row: it is the
    // order the set's own table is walked, derived from the values. Both recorded orders have
    // to come out of that derivation, or the rows below would be pinning a table of orders.
    for (case, declared, stated) in [
        (
            "the order a read scans in",
            DECLARED_SCAN_BY.to_vec(),
            STATED_SCAN_BY.to_vec(),
        ),
        (
            "a metric stream's output format",
            DECLARED_OUTPUT_FORMATS.to_vec(),
            STATED_OUTPUT_FORMATS.to_vec(),
        ),
    ] {
        let derived = hashed_enum_order(&declared);
        assert_eq!(
            derived, stated,
            "case {case:?}: expected the stated order {stated:?}, got {derived:?}"
        );
        // The derivation reads the values and nothing of their presentation, so a set handed
        // over backwards is stated the same way.
        let reversed: Vec<&str> = declared.iter().rev().copied().collect();
        let from_reversed = hashed_enum_order(&reversed);
        assert_eq!(
            from_reversed, stated,
            "case {case:?}: the stated order must not depend on the presentation, got \
             {from_reversed:?}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_an_enumerated_type_is_refused_with_the_halves_that_type_states() {
    let mut rows: Vec<Row> = Vec::new();

    // The scan DIRECTION, the one type stated without the value that was sent, on both
    // operations that carry it and over three ways of missing its set.
    for submitted in ["NotAValidScanByValue", "timestampascending", ""] {
        rows.push(Row {
            case: "the order a metric read scans in",
            operation: "GetMetricData",
            request: metric_data_request(1, "Bytes", &json!({"ScanBy": submitted})),
            path: "scanBy".to_string(),
            declared: &DECLARED_SCAN_BY,
            submitted,
        });
        rows.push(Row {
            case: "the order an alarm history scans in",
            operation: "DescribeAlarmHistory",
            request: json!({"ScanBy": submitted}),
            path: "scanBy".to_string(),
            declared: &DECLARED_SCAN_BY,
            submitted,
        });
    }

    // A metric stream's output format: a type stated WITH the value that was sent, over a word
    // the type never offered and a declared value mis-cased.
    for (index, submitted) in ["not-a-real-format", "JSON"].into_iter().enumerate() {
        rows.push(Row {
            case: "a metric stream's output format",
            operation: "PutMetricStream",
            request: metric_stream_request(
                &format!("enum-halves-{index}"),
                &json!({"OutputFormat": submitted}),
            ),
            path: "outputFormat".to_string(),
            declared: &DECLARED_OUTPUT_FORMATS,
            submitted,
        });
    }

    // Three more types, each stated with the value that was sent: the kind of history a read
    // asks for, the state an alarm is forced into, and the operator an alarm compares with.
    rows.push(Row {
        case: "the kind of alarm history asked for",
        operation: "DescribeAlarmHistory",
        request: json!({"HistoryItemType": "Nonsense"}),
        path: "historyItemType".to_string(),
        declared: &DECLARED_HISTORY_ITEM_TYPES,
        submitted: "Nonsense",
    });
    rows.push(Row {
        case: "the state an alarm is forced into",
        operation: "SetAlarmState",
        request: json!({
            "AlarmName": "an-alarm",
            "StateValue": "PENDING",
            "StateReason": "driven by the test",
        }),
        path: "stateValue".to_string(),
        declared: &DECLARED_STATE_VALUES,
        submitted: "PENDING",
    });
    rows.push(Row {
        case: "the operator an alarm compares with",
        operation: "PutMetricAlarm",
        request: merged(
            metric_alarm_request("enum-halves-alarm"),
            &json!({"ComparisonOperator": "Exceeds"}),
        ),
        path: "comparisonOperator".to_string(),
        declared: &DECLARED_COMPARISON_OPERATORS,
        submitted: "Exceeds",
    });

    // The same rule at DEPTH: the unit a query inside the array measures in, at two positions,
    // because the member path a clause names is the request's and not a fixed one.
    for position in [1, 3] {
        rows.push(Row {
            case: "the unit a query inside the array measures in",
            operation: "GetMetricData",
            request: metric_data_request(position, "Furlongs", &json!({})),
            path: format!("metricDataQueries.{position}.member.metricStat.unit"),
            declared: &STANDARD_UNITS,
            submitted: "Furlongs",
        });
    }

    let router = build_router();
    for Row {
        case,
        operation,
        request,
        path,
        declared,
        submitted,
    } in &rows
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, VALIDATION),
            "case {case:?} ({operation}, {submitted:?}): expected HTTP 400 {VALIDATION}, got \
             {status} with {body}"
        );
        let message = error_message(&body);
        let expected = expected_refusal(path, declared, submitted);
        assert_eq!(
            message, expected,
            "case {case:?} ({operation}, {submitted:?}): expected {expected:?}, got {message:?}"
        );
        // Stated separately, because withholding the value is the point: a type stated without
        // it carries no trace of what the caller sent, wherever in the sentence it might sit.
        if !states_the_submitted_value(declared) && !submitted.is_empty() {
            assert!(
                !message.contains(submitted),
                "case {case:?} ({operation}): expected no trace of the submitted value \
                 {submitted:?}, got {message:?}"
            );
        }
    }
}

/// The time stamps one `GetMetricData` result carries, in the order the response listed them.
fn reported_timestamps(body: &Value) -> Vec<f64> {
    let results = body
        .get("MetricDataResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("GetMetricData returns MetricDataResults, got {body}"));
    assert_eq!(
        results.len(),
        1,
        "a one-query request answers one result, got {body}"
    );
    results[0]
        .get("Timestamps")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn every_value_the_scan_direction_declares_is_served_and_orders_what_it_names() {
    // The refusals above are about the value being outside the type rather than about the member
    // being checked at all, so each declared value is accepted — and READ: two datapoints in
    // different windows come back in opposite orders under the two directions, so a direction
    // accepted and then ignored would fail here. The alarm history is ordered by the same member
    // (`crate::alarm_history` tables that), and is driven here for the acceptance alone.
    let router = build_router();
    let bucket = closed_minute();
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": "MyApp/EnumHalves",
            "MetricData": [
                {"MetricName": "Latency", "Value": 1.0, "Unit": "Bytes",
                 "Timestamp": bucket - 120.0},
                {"MetricName": "Latency", "Value": 2.0, "Unit": "Bytes", "Timestamp": bucket},
            ],
        }),
    )
    .await;

    let mut reported: Vec<(&str, Vec<f64>)> = Vec::new();
    for scan_by in DECLARED_SCAN_BY {
        let read = ok(
            &router,
            "GetMetricData",
            &metric_data_request(1, "Bytes", &json!({"ScanBy": scan_by})),
        )
        .await;
        let stamps = reported_timestamps(&read);
        assert_eq!(
            stamps.len(),
            2,
            "case {scan_by:?}: both published windows must be reported, got {read}"
        );
        let ascending = scan_by == "TimestampAscending";
        assert_eq!(
            stamps[0] < stamps[1],
            ascending,
            "case {scan_by:?}: expected the windows ordered ascending: {ascending}, got {stamps:?}"
        );
        reported.push((scan_by, stamps));

        let history = ok(&router, "DescribeAlarmHistory", &json!({"ScanBy": scan_by})).await;
        assert!(
            history.get("AlarmHistoryItems").is_some(),
            "case {scan_by:?}: a declared scan direction must be served, got {history}"
        );
    }

    // The two directions report the SAME windows, so the member decides the order and nothing
    // else about what is reported.
    let (first, second) = (&reported[0], &reported[1]);
    let reversed: Vec<f64> = second.1.iter().rev().copied().collect();
    assert_eq!(
        first.1, reversed,
        "the two directions must report one set of windows in opposite orders, got {:?} and {:?}",
        first.1, second.1
    );
}
