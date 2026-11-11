// SPDX-License-Identifier: CC0-1.0

//! The STATISTIC an evaluated metric alarm's `StateReasonData` names, end to end over the wire.
//!
//! ONE rule, asserted over a table: the machine-readable explanation of a state an evaluation
//! computed ("An explanation for the alarm state, in JSON format.",
//! `smithy_query(query="shape",shape="StateReasonData")`) names the statistic the evaluation
//! folded each window by under ONE member, `statistic`, carrying the name the alarm was
//! configured with — whichever of the two statistic members it arrived on, since "you must
//! specify either Statistic or ExtendedStatistic but not both"
//! (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Statistic")`). So a client
//! that reconstructs an alarm's decision reads the statistic from the same place for a basic
//! statistic and for a percentile, and an alarm whose statistic nothing folds names none.
//!
//! The rows differ along what the rule generalises over: which member carried the statistic,
//! which statistic it names, the value that statistic folds the same window to, and the
//! direction the verdict went. Beside the statistic, each row also pins the rest of the
//! document the evaluation rests on — the window of values it had (`recentDatapoints`) and the
//! datapoints the verdict rests on, each with the instant of its window and how many
//! observations that window held.

use serde_json::{json, Value};

use crate::alarm_evaluation::{datum, patched, publish};
use crate::harness::{build_router, document_instant_secs, now_secs, ok, wait_for_evaluation};

/// The values every row publishes into its own series, inside ONE window: a distribution whose
/// basic statistics and percentiles are all distinct, so a row's expected value identifies the
/// statistic that produced it.
const PUBLISHED: [f64; 3] = [2.0, 4.0, 12.0];

/// How many observations each row's window therefore holds, which is what the document reports
/// beside the value folded over them.
const OBSERVATIONS: f64 = 3.0;

/// [`OBSERVATIONS`] is one per published value.
const _: () = assert!(PUBLISHED.len() == 3);

/// The threshold every row compares its folded value against, chosen so that the rows span both
/// directions of the verdict: the statistics that fold above it report `ALARM` and the ones that
/// fold below it `OK`.
const THRESHOLD: f64 = 5.0;

/// The `Period` every row evaluates, in seconds: the base alarm's own, which is also the
/// cadence a regular-resolution series is stored at.
const PERIOD_SECS: f64 = 60.0;

/// How many periods back an evaluation of one evaluation period searches for real datapoints in
/// this emulator (`crate::state::alarm_evaluation::EVALUATION_RANGE_FACTOR`), which bounds how
/// old the window a graded datapoint came from can be.
const EVALUATION_RANGE_PERIODS: f64 = 3.0;

/// One row of the table: how the alarm names its statistic, and what that statistic folds
/// [`PUBLISHED`] to.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The statistic members this row sets on the base alarm.
    patch: Value,
    /// The name the document must report under `statistic`.
    statistic: &'static str,
    /// The value the statistic takes over [`PUBLISHED`].
    value: f64,
}

/// One row of the table, for a statistic the `Statistic` enum declares.
fn basic(case: &'static str, statistic: &'static str, value: f64) -> Row {
    Row {
        case,
        patch: json!({"Statistic": statistic, "Threshold": THRESHOLD}),
        statistic,
        value,
    }
}

/// One row of the table, for a percentile the `ExtendedStatistic` member carries — which
/// replaces `Statistic` rather than joining it.
fn percentile(case: &'static str, statistic: &'static str, value: f64) -> Row {
    Row {
        case,
        patch: json!({
            "Statistic": Value::Null,
            "ExtendedStatistic": statistic,
            "Threshold": THRESHOLD,
        }),
        statistic,
        value,
    }
}

/// The state a row's folded value puts the alarm in: the base alarm compares it against
/// `THRESHOLD` with `GreaterThanThreshold` over one evaluation period.
fn expected_state(value: f64) -> &'static str {
    if value > THRESHOLD {
        "ALARM"
    } else {
        "OK"
    }
}

/// The `StateReasonData` of a described alarm, parsed.
fn reason_data(case: &str, alarm: &Value) -> Value {
    let text = alarm
        .get("StateReasonData")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a StateReasonData member, got {alarm}"));
    serde_json::from_str(text)
        .unwrap_or_else(|_| panic!("case {case:?}: StateReasonData must be JSON, got {text:?}"))
}

#[tokio::test]
async fn an_evaluated_reason_document_names_the_statistic_it_folded_the_window_by() {
    // The five names the `Statistic` enum declares
    // (`smithy_query(query="shape",shape="Statistic")`), each folding the same three published
    // values to a value of its own, and the percentile form of the other member across the
    // published range "between p0.0 and p100"
    // (`smithy_query(query="doc",shape="MetricAlarm",member="ExtendedStatistic")`) — both
    // bounds, the middle, and a fractional name near the upper bound. The percentiles are taken
    // by nearest rank over the three published values.
    let rows: Vec<Row> = vec![
        basic("the sample count of the window", "SampleCount", 3.0),
        basic("the average of the window", "Average", 6.0),
        basic("the sum of the window", "Sum", 18.0),
        basic("the smallest value in the window", "Minimum", 2.0),
        basic("the largest value in the window", "Maximum", 12.0),
        percentile("the lower bound of the percentile range", "p0.0", 2.0),
        percentile("the median of the window", "p50", 4.0),
        percentile("a percentile the issue cited", "p90", 12.0),
        percentile("a fractional percentile near the upper bound", "p99.9", 12.0),
        percentile("the upper bound of the percentile range", "p100", 12.0),
    ];

    // Every row's alarm watches its own series (its `DomainName` dimension is its own name), so
    // all of them are configured on ONE emulator and the evaluation delay is waited out once.
    let router = build_router();
    let published: Vec<_> = PUBLISHED.iter().map(|value| datum(*value, 0.0)).collect();
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("folded-{index}");
        ok(&router, "PutMetricAlarm", &patched(&name, &row.patch)).await;
        publish(&router, &name, &published).await;
        names.push(name);
    }
    let evaluated_after = now_secs();
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let reported = &described["MetricAlarms"][0];
        let state = expected_state(row.value);
        assert_eq!(
            reported.get("StateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: {} folds the window to {}, so the alarm must report {state}, got \
             {reported}",
            row.statistic,
            row.value
        );

        let document = reason_data(case, reported);
        // ONE member carries the statistic, whichever member the alarm named it on.
        assert_eq!(
            document.get("statistic").and_then(Value::as_str),
            Some(row.statistic),
            "case {case:?}: expected statistic {:?} in the reason document, got {document}",
            row.statistic
        );
        assert!(
            document.get("extendedStatistic").is_none(),
            "case {case:?}: the statistic is reported under one member only, got {document}"
        );
        assert_eq!(
            document.get("period").and_then(Value::as_f64),
            Some(PERIOD_SECS),
            "case {case:?}: expected the document to name the graded period, got {document}"
        );
        assert_eq!(
            document.get("threshold").and_then(Value::as_f64),
            Some(THRESHOLD),
            "case {case:?}: expected the document to name the threshold, got {document}"
        );

        // The window the state was decided over: the one value this statistic folded it to.
        let recent: Vec<f64> = document
            .get("recentDatapoints")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected recentDatapoints, got {document}")
            })
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        assert_eq!(
            recent,
            vec![row.value],
            "case {case:?}: expected the window the evaluation had to be {}, got {document}",
            row.value
        );

        // The datapoints the verdict rests on, each with the instant of its window and how many
        // observations that window held.
        let evaluated = document
            .get("evaluatedDatapoints")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            evaluated.len(),
            1,
            "case {case:?}: one evaluation period decides on one datapoint, got {document}"
        );
        let entry = &evaluated[0];
        assert_eq!(
            entry.get("value").and_then(Value::as_f64),
            Some(row.value),
            "case {case:?}: expected the verdict to rest on {}, got {document}",
            row.value
        );
        assert_eq!(
            entry.get("sampleCount").and_then(Value::as_f64),
            Some(OBSERVATIONS),
            "case {case:?}: expected the observations the window held, got {document}"
        );
        let instant = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(document_instant_secs)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a document instant on the datapoint, got {document}")
            });
        assert!(
            instant.rem_euclid(PERIOD_SECS).abs() <= f64::EPSILON,
            "case {case:?}: a graded datapoint is stamped at the start of its {PERIOD_SECS}-second \
             window, got {instant}"
        );
        let oldest = evaluated_after - EVALUATION_RANGE_PERIODS * PERIOD_SECS;
        assert!(
            (oldest..=evaluated_after + PERIOD_SECS).contains(&instant),
            "case {case:?}: a graded datapoint comes from a window the evaluation searched \
             ({oldest}..={evaluated_after}), got {instant}"
        );
    }
}

#[tokio::test]
async fn a_statistic_no_fold_describes_leaves_the_alarm_unevaluated_and_names_none() {
    // A percentile of a window CloudWatch computes no percentile over: "Percentile statistics
    // are not available for metrics when any of the metric values are negative numbers."
    // (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="ExtendedStatistics")`)
    // — the statistic is one the alarm may name, so the alarm is created, but nothing folds its
    // window by it, so the alarm keeps the state its creation left it in — "the alarm state is
    // immediately set to INSUFFICIENT_DATA"
    // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="AlarmName")`) — and reports
    // no machine-readable explanation at all rather than one naming a statistic nothing computed.
    let router = build_router();
    let name = "no-percentile-over-this-window";
    ok(
        &router,
        "PutMetricAlarm",
        &patched(
            name,
            &json!({"Statistic": Value::Null, "ExtendedStatistic": "p50"}),
        ),
    )
    .await;
    let published: Vec<_> = [-2.0, 4.0, 12.0]
        .iter()
        .map(|value| datum(*value, 0.0))
        .collect();
    publish(&router, name, &published).await;
    wait_for_evaluation();

    let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    let reported = &described["MetricAlarms"][0];
    assert_eq!(
        reported.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "a statistic nothing folds leaves the alarm in the state its creation set, got {reported}"
    );
    assert_eq!(
        reported.get("StateReasonData"),
        None,
        "an unevaluated alarm reports no reason document, got {reported}"
    );
}
