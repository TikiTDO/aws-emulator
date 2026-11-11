// SPDX-License-Identifier: CC0-1.0

//! What `PutMetricAlarm` refuses about the VALUES of one request, end to end over the wire, and
//! in which words.
//!
//! Four rules, each asserted over a table whose rows differ along what the rule generalises
//! over rather than along a name:
//!
//! 1. A member whose admissible values are a service rule the model cannot express —
//!    `TreatMissingData` and `EvaluateLowSampleCountPercentile` are plain strings with a length
//!    bound, and `ExtendedStatistic` is unconstrained — admits exactly the values its own
//!    documentation publishes: every published value is accepted and echoed back, and anything
//!    else is refused with the value it was given named, before anything is written.
//! 2. The M of an "M out of N" evaluation may not exceed its N
//!    (`DatapointsToAlarm` <= `EvaluationPeriods`), whichever of the two forms the alarm's
//!    series is stated in — one metric, or a `Metrics` query array — because neither form
//!    changes what M and N mean.
//! 3. A `Period` outside the cadence grammar an alarm is scheduled at is refused with the
//!    sentence that names that GRAMMAR, carrying nothing of the period that was asked for.
//! 4. A member modelled as an ENUM is refused by the constraint walk in a clause that QUOTES
//!    the submitted value and lists the type's values in the order the set's own table is
//!    walked.
//!
//! Every refusal is also asserted to store nothing: a create leaves no alarm behind, and an
//! update leaves the alarm it would have replaced exactly as it was — the whole document,
//! `AlarmConfigurationUpdatedTimestamp` included.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, call, described_alarm, error_code, error_message, merged, metric_alarm_request,
    ok, AlarmKind,
};

/// The error class the body of a refusal CloudWatch words itself names: the
/// request-validation class, whose legacy query code (`ValidationError`) travels in the
/// `x-amzn-query-error` header instead.
const VALIDATION: &str = "ValidationException";

/// The error class the body names for a parameter VALUE this service cannot serve on the member
/// it arrived on, whose legacy query code is `InvalidParameterValue`.
const PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The values `TreatMissingData` publishes: "Valid Values: breaching | notBreaching | ignore |
/// missing" (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="TreatMissingData")`).
const TREAT_MISSING_DATA_VALUES: [&str; 4] = ["breaching", "notBreaching", "ignore", "missing"];

/// The options `EvaluateLowSampleCountPercentile` publishes: "Valid Values: evaluate | ignore"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",
/// member="EvaluateLowSampleCountPercentile")`).
const LOW_SAMPLE_OPTIONS: [&str; 2] = ["evaluate", "ignore"];

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

/// The same seven values in the order a refusal STATES them: the order the value set's own
/// table is walked — ascending bucket of each value's own string hash, and two values sharing a
/// bucket read from the head each was pushed onto
/// (`service_common::validation_message::hashed_enum_order`). Neither the declaration order
/// above nor a sorted one; the test below asserts the derivation produces exactly this.
const STATED_COMPARISON_OPERATORS: [&str; 7] = [
    "GreaterThanUpperThreshold",
    "LessThanLowerThreshold",
    "GreaterThanThreshold",
    "LessThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanLowerOrGreaterThanUpperThreshold",
    "GreaterThanOrEqualToThreshold",
];

/// How this service states a value outside the set a named parameter supports.
fn value_not_supported_for_parameter(value: &str, parameter: &str, supported: &[&str]) -> String {
    format!(
        "The value {value} is not supported for {parameter} parameter. Supported values are [{}].",
        supported.join(", ")
    )
}

/// How it states an option outside the set a named parameter offers.
fn option_not_supported(value: &str, parameter: &str, options: &[&str]) -> String {
    format!(
        "Option {value} is not supported. Supported options for parameter {parameter} are {} and {}.",
        options[0], options[1]
    )
}

/// How it states a value it understands but cannot serve on the member it arrived on.
fn value_not_supported(value: &str, parameter: &str) -> String {
    format!("The value {value} for parameter {parameter} is not supported.")
}

/// Assert `request` is refused with HTTP 400 `code` carrying exactly `expected`, and that it
/// left no alarm under `name`.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    request: &Value,
    code: &str,
    expected: &str,
) {
    let (status, body) = call(router, "PutMetricAlarm", request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, code),
        "case {case:?}: expected HTTP 400 {code}, got {status} with {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal {expected:?}, got {message:?}"
    );
    let stored = described_alarm(router, AlarmKind::Metric, name).await;
    assert_eq!(
        stored, None,
        "case {case:?}: a refused request must store nothing, got {stored:?}"
    );
}

/// Assert `request` is accepted and the alarm it configures reads back with `member` holding
/// `value`.
async fn assert_accepted_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    request: &Value,
    member: &str,
    value: &Value,
) {
    let (status, body) = call(router, "PutMetricAlarm", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the configuration to be accepted, got {status} with {body}"
    );
    let stored = described_alarm(router, AlarmKind::Metric, name)
        .await
        .unwrap_or_default();
    assert_eq!(
        stored.get(member),
        Some(value),
        "case {case:?}: expected the stored alarm to carry {member}={value}, got {stored}"
    );
}

#[tokio::test]
async fn a_missing_data_policy_outside_the_published_set_is_refused_and_stores_nothing() {
    // Rows differ along HOW the value misses the published set — a near miss of a documented
    // value, a documented value in another case, a documented value's stem, a word naming no
    // policy at all — and along which published value is accepted, which covers all four
    // including the three the issue did not cite.
    let refused: [(&str, &str); 5] = [
        ("a near miss of a documented value", "breach"),
        ("a documented value in another case", "Breaching"),
        ("another documented value mis-cased", "NOTBREACHING"),
        ("a documented value's stem", "notBreach"),
        ("a word naming no policy at all", "whatever"),
    ];

    let router = build_router();
    for (case, policy) in refused {
        let name = format!("missing-data-{policy}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({"TreatMissingData": policy}),
        );
        assert_refused(
            &router,
            case,
            &name,
            &request,
            VALIDATION,
            &value_not_supported_for_parameter(policy, "TreatMissingData", &TREAT_MISSING_DATA_VALUES),
        )
        .await;
    }

    for policy in TREAT_MISSING_DATA_VALUES {
        let name = format!("missing-data-ok-{policy}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({"TreatMissingData": policy}),
        );
        assert_accepted_with(
            &router,
            "a published missing-data policy",
            &name,
            &request,
            "TreatMissingData",
            &json!(policy),
        )
        .await;
    }
}

#[tokio::test]
async fn a_low_sample_count_option_outside_the_published_set_is_refused_and_stores_nothing() {
    // Rows differ the same way: a word naming no option, a published option mis-cased, a
    // published option with a suffix — and both published options are accepted and echoed.
    let refused: [(&str, &str); 4] = [
        ("a word naming no option at all", "sometimes"),
        ("a published option mis-cased", "Ignore"),
        ("the other published option mis-cased", "EVALUATE"),
        ("a published option with a suffix", "evaluates"),
    ];

    let router = build_router();
    for (case, option) in refused {
        let name = format!("low-sample-{option}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({
                "Statistic": Value::Null,
                "ExtendedStatistic": "p90",
                "EvaluateLowSampleCountPercentile": option,
            }),
        );
        assert_refused(
            &router,
            case,
            &name,
            &request,
            VALIDATION,
            &option_not_supported(
                option,
                "EvaluateLowSampleCountPercentile",
                &LOW_SAMPLE_OPTIONS,
            ),
        )
        .await;
    }

    for option in LOW_SAMPLE_OPTIONS {
        let name = format!("low-sample-ok-{option}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({
                "Statistic": Value::Null,
                "ExtendedStatistic": "p90",
                "EvaluateLowSampleCountPercentile": option,
            }),
        );
        assert_accepted_with(
            &router,
            "a published low-sample-count option",
            &name,
            &request,
            "EvaluateLowSampleCountPercentile",
            &json!(option),
        )
        .await;
    }
}

#[tokio::test]
async fn an_extended_statistic_outside_the_grammar_is_refused_as_a_parameter_value() {
    // The names `ExtendedStatistic` publishes are a grammar, not a list: "p90 tm90 tc90 ts90
    // wm90 IQM PR(n:m) … TM(X%:X%) …"
    // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="ExtendedStatistic")`), with
    // the percentile taken "between p0.0 and p100"
    // (`smithy_query(query="doc",shape="GetMetricStatisticsInput",member="ExtendedStatistics")`).
    // The refused rows differ along how the name misses the grammar: past the upper bound of the
    // percentile range (by a whole step and by a fraction), below its lower bound, the
    // percentile form mis-cased, a family prefix that names none, and a name of no form at all.
    let refused: [(&str, &str); 7] = [
        ("one whole step past the top of the percentile range", "p101"),
        ("a fraction past the top of it", "p100.5"),
        ("far past the top of it", "p1000"),
        ("below the bottom of it", "p-1"),
        ("the percentile form spelled with an upper-case p", "P90"),
        ("a family prefix that names no family", "xm90"),
        ("a word of no statistic form at all", "percentile99"),
    ];

    let router = build_router();
    for (case, statistic) in refused {
        let name = format!("extended-{statistic}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({"Statistic": Value::Null, "ExtendedStatistic": statistic}),
        );
        assert_refused(
            &router,
            case,
            &name,
            &request,
            PARAMETER_VALUE,
            &value_not_supported(statistic, "ExtendedStatistic"),
        )
        .await;
    }

    // The accepted rows span the grammar's forms: both bounds of the percentile range, a
    // fractional percentile inside it, each bare trimming family the member's documentation
    // lists, the interquartile mean under its own name, and both span forms.
    let accepted: [&str; 10] = [
        "p0",
        "p50",
        "p99.9",
        "p100",
        "tm90",
        "tc90",
        "ts90",
        "wm90",
        "IQM",
        "TM(10%:90%)",
    ];
    for (index, statistic) in accepted.into_iter().enumerate() {
        let name = format!("extended-ok-{index}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({"Statistic": Value::Null, "ExtendedStatistic": statistic}),
        );
        assert_accepted_with(
            &router,
            "a name the statistic grammar admits",
            &name,
            &request,
            "ExtendedStatistic",
            &json!(statistic),
        )
        .await;
    }
}

#[tokio::test]
async fn naming_both_statistic_members_is_refused_and_leaves_the_stored_alarm_alone() {
    // "you must specify either Statistic or ExtendedStatistic, but not both"
    // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Statistic")`). The rows
    // differ along which basic statistic and which extended statistic are named together, so
    // the refusal is about the PAIR rather than about either value.
    let rows: [(&str, &str, &str); 3] = [
        ("an average beside a percentile", "Average", "p90"),
        ("a maximum beside a trimmed mean", "Maximum", "tm90"),
        ("a sum beside the interquartile mean", "Sum", "IQM"),
    ];
    const EXPECTED: &str = "PutMetricAlarm request can not be made with both Statistic and \
                            ExtendedStatistic parameters. Only one of them should be present";

    let router = build_router();
    for (case, statistic, extended) in rows {
        let name = format!("both-statistics-{statistic}");
        // The alarm the refused UPDATE would have replaced, created with one statistic only.
        let created = merged(
            metric_alarm_request(&name),
            &json!({"Statistic": statistic}),
        );
        assert_accepted_with(
            &router,
            case,
            &name,
            &created,
            "Statistic",
            &json!(statistic),
        )
        .await;
        let before = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_default();

        let update = merged(
            metric_alarm_request(&name),
            &json!({"Statistic": statistic, "ExtendedStatistic": extended}),
        );
        let (status, body) = call(&router, "PutMetricAlarm", &update).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, VALIDATION),
            "case {case:?}: expected HTTP 400 {VALIDATION}, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected the refusal {EXPECTED:?}, got {message:?}"
        );
        // A refused update changes nothing about the alarm it would have replaced: the whole
        // document, the configuration's own update stamp included, is the one it already had.
        let after = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_default();
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the stored alarm untouched, got {after}"
        );
    }
}

#[tokio::test]
async fn a_refused_value_leaves_the_configuration_update_stamp_of_the_alarm_it_would_replace() {
    // The same holds for every value rule above, on an UPDATE: the refusal is answered before
    // the write, so the stored alarm — including the stamp that reports when its configuration
    // last changed ("The time stamp of the last update to the alarm configuration.",
    // `smithy_query(query="doc",shape="MetricAlarm",
    // member="AlarmConfigurationUpdatedTimestamp")`) — is left exactly as it was. The rows are
    // one per rule.
    let rows: [(&str, Value); 4] = [
        (
            "an unknown missing-data policy",
            json!({"TreatMissingData": "breach"}),
        ),
        (
            "an unknown low-sample-count option",
            json!({"EvaluateLowSampleCountPercentile": "sometimes"}),
        ),
        (
            "an extended statistic outside the grammar",
            json!({"Statistic": Value::Null, "ExtendedStatistic": "p101"}),
        ),
        (
            "a period outside the cadence grammar",
            json!({"Period": 45}),
        ),
    ];

    let router = build_router();
    for (index, (case, patch)) in rows.into_iter().enumerate() {
        let name = format!("refused-update-{index}");
        ok(&router, "PutMetricAlarm", &metric_alarm_request(&name)).await;
        let before = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_default();
        assert!(
            before
                .get("AlarmConfigurationUpdatedTimestamp")
                .and_then(Value::as_f64)
                .is_some(),
            "case {case:?}: the created alarm reports when its configuration was written, got \
             {before}"
        );

        let update = merged(metric_alarm_request(&name), &patch);
        let (status, body) = call(&router, "PutMetricAlarm", &update).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} with {body}"
        );
        let after = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_default();
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the stored alarm untouched, got {after}"
        );
    }
}

/// A `Metrics` query array that watches `METRICS_NAMESPACE`/`METRICS_METRIC`: one query that
/// retrieves the metric and one expression over it, which is the one the alarm watches.
fn metrics_array() -> Value {
    json!([
        {
            "Id": "m1",
            "ReturnData": false,
            "MetricStat": {
                "Metric": {"Namespace": "MyApp/Maths", "MetricName": "Load"},
                "Period": 60,
                "Stat": "Average",
            },
        },
        {"Id": "e1", "ReturnData": true, "Expression": "m1"},
    ])
}

/// The alarm request for `name` in the form `form` states its series in: one metric, or a
/// `Metrics` query array (which may carry none of the single-metric members).
fn alarm_in_form(name: &str, form: &str, datapoints: i64, periods: i64) -> Value {
    let counts = json!({"DatapointsToAlarm": datapoints, "EvaluationPeriods": periods});
    match form {
        "a single metric" => merged(metric_alarm_request(name), &counts),
        _ => merged(
            json!({
                "AlarmName": name,
                "Metrics": metrics_array(),
                "Threshold": 0.0,
                "ComparisonOperator": "GreaterThanThreshold",
            }),
            &counts,
        ),
    }
}

#[tokio::test]
async fn the_m_of_n_relation_holds_in_both_forms_an_alarms_series_can_be_stated_in() {
    // "The number of data points that must be breaching to trigger the alarm. … this value is
    // the M." (`PutMetricAlarmInput$DatapointsToAlarm`) against "The number of periods over
    // which data is compared to the specified threshold. … this value is the N."
    // (`PutMetricAlarmInput$EvaluationPeriods`) — the M cannot exceed the N. The rows differ
    // along the pair of counts (equal, M one below N, M one above N, M far above N, both at the
    // smallest value the model admits) and along the form the series is stated in, which the
    // relation does not speak about.
    const EXPECTED: &str = "DatapointsToAlarm must be less than or equal to EvaluationPeriods";
    let pairs: [(&str, i64, i64, bool); 6] = [
        ("M one below N", 2, 3, true),
        ("M equal to N", 3, 3, true),
        ("the smallest pair the model admits", 1, 1, true),
        ("M one above N", 3, 2, false),
        ("M far above N", 10, 2, false),
        ("M above the smallest N", 2, 1, false),
    ];

    let router = build_router();
    for form in ["a single metric", "a Metrics query array"] {
        for (pair, datapoints, periods, accepted) in pairs {
            let case = format!("{pair}, stated as {form}");
            let name = format!(
                "m-of-n-{}-{datapoints}-{periods}",
                if form == "a single metric" { "metric" } else { "maths" }
            );
            let request = alarm_in_form(&name, form, datapoints, periods);
            if accepted {
                assert_accepted_with(
                    &router,
                    &case,
                    &name,
                    &request,
                    "DatapointsToAlarm",
                    &json!(datapoints),
                )
                .await;
            } else {
                assert_refused(&router, &case, &name, &request, VALIDATION, EXPECTED).await;
            }
        }
    }
}

#[tokio::test]
async fn a_period_outside_the_cadence_grammar_is_refused_with_the_sentence_naming_it() {
    // "Valid values are 10, 20, 30, and any multiple of 60." (`PutMetricAlarmInput$Period`) —
    // one sentence for every period outside that grammar, carrying nothing of the period that
    // was asked for. The rows differ along the period: each sub-minute cadence the grammar
    // admits and one either side of it, the smallest period the model admits, periods between
    // two multiples of a minute, and a large one that is no multiple.
    const EXPECTED: &str = "Period must be 10, 20, 30 or a multiple of 60";
    let rows: [(&str, i64, bool); 14] = [
        ("the smallest period the model admits", 1, false),
        ("one below the shortest cadence", 9, false),
        ("the shortest cadence", 10, true),
        ("one above it", 11, false),
        ("the middle sub-minute cadence", 20, true),
        ("the longest sub-minute cadence", 30, true),
        ("one above the longest sub-minute cadence", 31, false),
        ("the period the issue cited", 45, false),
        ("one below a whole minute", 59, false),
        ("a whole minute", 60, true),
        ("one above a whole minute", 61, false),
        ("a multiple of a minute", 300, true),
        ("between two multiples of a minute", 90, false),
        ("a large period that is no multiple of a minute", 3_599, false),
    ];

    let router = build_router();
    for (case, period, accepted) in rows {
        let name = format!("period-grammar-{period}");
        let request = merged(metric_alarm_request(&name), &json!({"Period": period}));
        if accepted {
            assert_accepted_with(&router, case, &name, &request, "Period", &json!(period)).await;
        } else {
            assert_refused(&router, case, &name, &request, VALIDATION, EXPECTED).await;
        }
    }
}

#[tokio::test]
async fn a_comparison_operator_outside_the_enum_is_refused_quoting_the_value_it_was_given() {
    // The value set a refusal states is derived from the declared values, not written out: it
    // is the order the set's own table is walked.
    let derived =
        ::service_common::validation_message::hashed_enum_order(&DECLARED_COMPARISON_OPERATORS);
    assert_eq!(
        derived,
        STATED_COMPARISON_OPERATORS.to_vec(),
        "the stated order of an enumerated type's values is the order its own table is walked, \
         got {derived:?}"
    );

    // The rows differ along HOW the submitted value misses the enum — a declared value's stem,
    // a declared value mis-cased, a word naming no operator, and no characters at all — because
    // the clause has to quote back whatever was sent for a client to tell those apart.
    let rows: [(&str, &str); 4] = [
        ("a declared value's stem", "GreaterThan"),
        ("a declared value mis-cased", "greaterthanthreshold"),
        ("a word naming no operator", "Exceeds"),
        ("no characters at all", ""),
    ];

    let router = build_router();
    for (case, submitted) in rows {
        let name = format!("comparison-{case}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({"ComparisonOperator": submitted}),
        );
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at 'comparisonOperator' failed to \
             satisfy constraint: Member must satisfy enum value set: [{}]",
            STATED_COMPARISON_OPERATORS.join(", ")
        );
        assert_refused(&router, case, &name, &request, VALIDATION, &expected).await;
    }

    // Every value the type DOES declare is accepted, so the refusal above is about the value
    // being outside the type rather than about the member being checked. The three band
    // operators are "used only for alarms based on anomaly detection models"
    // (`PutMetricAlarmInput$ComparisonOperator`), so those rows carry the band's threshold
    // query instead of a static threshold.
    for operator in DECLARED_COMPARISON_OPERATORS {
        let name = format!("comparison-ok-{operator}");
        let band = operator.contains("Upper") || operator.contains("Lower");
        let patch = if band {
            json!({
                "ComparisonOperator": operator,
                "Threshold": Value::Null,
                "Statistic": Value::Null,
                "Period": Value::Null,
                "Namespace": Value::Null,
                "MetricName": Value::Null,
                "ThresholdMetricId": "band",
                "Metrics": json!([
                    {
                        "Id": "m1",
                        "ReturnData": true,
                        "MetricStat": {
                            "Metric": {"Namespace": "MyApp/Maths", "MetricName": "Load"},
                            "Period": 60,
                            "Stat": "Average",
                        },
                    },
                    {
                        "Id": "band",
                        "ReturnData": false,
                        "Expression": "ANOMALY_DETECTION_BAND(m1, 2)",
                    },
                ]),
            })
        } else {
            json!({"ComparisonOperator": operator})
        };
        let request = merged(metric_alarm_request(&name), &patch);
        assert_accepted_with(
            &router,
            "a declared comparison operator",
            &name,
            &request,
            "ComparisonOperator",
            &json!(operator),
        )
        .await;
    }
}
