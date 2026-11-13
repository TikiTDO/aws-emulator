// SPDX-License-Identifier: CC0-1.0

//! An INGESTED log event drives the metric-filter -> metric -> alarm -> SNS -> SQS chain,
//! through the WHOLE mounted service set — the one `core/router/src/lib.rs` assembles for the
//! binary.
//!
//! The RULE under test: **every log event `PutLogEvents` stores is run against each metric
//! filter of its log group, and each `metricTransformations` entry of a filter whose
//! `filterPattern` matches that event publishes its `metricValue` as a datapoint of the
//! CloudWatch metric it names, so an alarm over that metric whose `Statistic` over its period
//! crosses the `Threshold` in the direction its `ComparisonOperator` names leaves the state it
//! held for ALARM on the evaluation the clock owes — no client call forces it — publishes the
//! alarm notification to every topic its `AlarmActions` names, and SNS fans that notification
//! out to the queue subscribed to the topic; while a filter of the same group whose pattern
//! NONE of those events matches publishes no datapoint at all, the transformation declaring no
//! `defaultValue`, so an alarm over THAT metric stays out of ALARM and the queue subscribed to
//! its topic receives nothing.**
//!
//! Four services have to agree for that to hold — CloudWatch Logs ingests and extracts,
//! CloudWatch holds the series and evaluates the alarm, SNS fans the notification out, SQS
//! holds what arrived — and each hop CloudWatch Logs and CloudWatch make is an in-process call
//! addressed the way a client's own scoped call is addressed, so only the production service
//! set can show the whole chain. The crate-local and router suites pin the links separately:
//! `core/router/tests/all/logs_metric_filter_extraction.rs` pins the extraction over a table of
//! filter forms and stops at the metric,
//! `core/router/tests/all/cloudwatch_alarm_action_reaches_its_subscribed_queue.rs` pins the
//! delivery of a transition a client FORCES, and
//! `core/router/tests/all/lambda_log_metric_filter_drives_the_alarm_chain.rs` drives the same
//! chain from what a Lambda invocation PRINTS. What only this suite reaches is the chain from a
//! client's own `PutLogEvents` to the queue the topic fans out to.
//!
//! The rows differ along what the rule generalises over: the FORM of the filter pattern (a JSON
//! pattern over a member, an unstructured term, a space-delimited field list, a JSON pattern
//! with a numeric condition), the FORM of the metric value (a literal number, a space-delimited
//! field, a JSON selector), the `Statistic` the alarm aggregates the period with, the direction
//! of its `ComparisonOperator`, how many of the ingested events match, and the Region the whole
//! wiring lives in. Every expected aggregate is computed from the row's own messages, never
//! taken from a recording.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    authorization, cw, drain, given_topic_with_subscriber, router as emulator, Grant,
};

/// CloudWatch Logs' `X-Amz-Target` prefix and the signing name its credential scope carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// The alarm's period in seconds and how many of those periods one evaluation grades: one
/// period is enough for the rule, which is about the window the ingested events land in.
const ALARM_PERIOD: i64 = 60;
const EVALUATION_PERIODS: i64 = 1;

/// How long a row keeps looking for something the clock owes before calling it undelivered.
///
/// Generous on purpose, and never measured: what a row asserts is WHAT arrived, never how long
/// it took. The transition is owed inside the framework's window for one the clock makes due
/// and the delivery follows it, so this deadline is only the point at which a delivery that is
/// not coming becomes a failure rather than a hang — a loaded host takes longer and still
/// decides the same way.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);

/// How many further rounds the quiet alarm's queue is read AFTER that alarm has settled into
/// the state its empty series implies: enough rounds to span several of the alarm clock's own
/// wake-ups, each round waiting on the queue's own long poll rather than on a span of this
/// host's clock.
const QUIET_ROUNDS: usize = 3;

/// The `StateValue` an alarm reports once its series breaches, and the one it reports while the
/// series it watches holds nothing and its `TreatMissingData` is `notBreaching`. Source: the
/// model's `StateValue` enum.
const STATE_ALARM: &str = "ALARM";
const STATE_OK: &str = "OK";

/// One Amazon CloudWatch Logs call over its `awsJson1_1` wire, which must answer 200.
///
/// The `Authorization` is built by the suite that owns this router's wire harness, so a
/// request here is addressed exactly the way its CloudWatch, SNS and SQS calls are.
async fn logs(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{LOGS_SIGNING_NAME}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization(LOGS_SIGNING_NAME, region))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch Logs {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// Wall-clock now as epoch seconds, the clock the series' period windows are measured against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// Wall-clock now as epoch milliseconds, the unit a log event's `timestamp` carries.
fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_millis(),
    )
    .expect("the epoch milliseconds fit in an i64")
}

/// The aggregate one period window reports for `values` under `statistic`.
///
/// Source: the model's `Statistic` enum — the arms here are the four the table states, so the
/// arithmetic a row expects is the row's own, never a recorded number.
#[allow(clippy::cast_precision_loss)]
fn aggregate(case: &str, statistic: &str, values: &[f64]) -> f64 {
    match statistic {
        "Sum" => values.iter().sum(),
        "SampleCount" => values.len() as f64,
        "Minimum" => values.iter().copied().fold(f64::INFINITY, f64::min),
        "Maximum" => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        other => panic!("case {case:?}: no row states the statistic {other:?}"),
    }
}

/// Whether `value` crosses `threshold` in the direction `comparison` names.
///
/// Source: the model's `ComparisonOperator` enum; the four single-threshold operators are the
/// ones an alarm over a plain numeric `Threshold` uses.
fn breaches(case: &str, comparison: &str, value: f64, threshold: f64) -> bool {
    match comparison {
        "GreaterThanThreshold" => value > threshold,
        "GreaterThanOrEqualToThreshold" => value >= threshold,
        "LessThanThreshold" => value < threshold,
        "LessThanOrEqualToThreshold" => value <= threshold,
        other => panic!("case {case:?}: no row states the comparison {other:?}"),
    }
}

/// A log group with one stream inside it, both created.
async fn given_group_with_stream(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    group: &str,
    stream: &str,
) {
    logs(
        router,
        case,
        region,
        "CreateLogGroup",
        &json!({"logGroupName": group}),
    )
    .await;
    logs(
        router,
        case,
        region,
        "CreateLogStream",
        &json!({"logGroupName": group, "logStreamName": stream}),
    )
    .await;
}

/// One metric filter to store: the name it is stored under, the pattern it runs over each
/// ingested event, and the series its one transformation publishes into.
struct FilterSpec<'a> {
    name: &'a str,
    pattern: &'a str,
    namespace: &'a str,
    metric: &'a str,
    metric_value: &'a str,
}

/// Store one metric filter on `group`, publishing its `metric_value` into its own series for
/// every event its `pattern` matches.
async fn given_metric_filter(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    group: &str,
    filter: &FilterSpec<'_>,
) {
    logs(
        router,
        case,
        region,
        "PutMetricFilter",
        &json!({
            "logGroupName": group,
            "filterName": filter.name,
            "filterPattern": filter.pattern,
            "metricTransformations": [{
                "metricName": filter.metric,
                "metricNamespace": filter.namespace,
                "metricValue": filter.metric_value,
            }],
        }),
    )
    .await;
}

/// One alarm to create: the name it is created under, the series it watches, how it grades that
/// series' period window, and the topic its one `AlarmActions` entry names.
struct AlarmSpec<'a> {
    name: &'a str,
    namespace: &'a str,
    metric: &'a str,
    statistic: &'a str,
    comparison: &'a str,
    threshold: f64,
    topic: &'a str,
}

/// Create one alarm over the series it names, with its topic as the one alarm action.
///
/// `TreatMissingData` is `notBreaching` — "If `TreatMissingData` is omitted, the default
/// behavior of `missing` is used" — so an alarm over a series nothing published to settles in
/// `OK` rather than in insufficient data, which is what makes the quiet twin's silence a
/// decision rather than an absence of one.
async fn given_alarm(router: &MultiServiceRouter, case: &str, region: &str, alarm: &AlarmSpec<'_>) {
    cw(
        router,
        case,
        region,
        "PutMetricAlarm",
        &json!({
            "AlarmName": alarm.name,
            "Namespace": alarm.namespace,
            "MetricName": alarm.metric,
            "Statistic": alarm.statistic,
            "Period": ALARM_PERIOD,
            "EvaluationPeriods": EVALUATION_PERIODS,
            "Threshold": alarm.threshold,
            "ComparisonOperator": alarm.comparison,
            "TreatMissingData": "notBreaching",
            "AlarmActions": [alarm.topic],
        }),
    )
    .await;
}

/// What `DescribeAlarms` reports as `name`'s `StateValue`.
async fn alarm_state(router: &MultiServiceRouter, case: &str, region: &str, name: &str) -> String {
    let described = cw(
        router,
        case,
        region,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The datapoints CloudWatch reports for `namespace`/`metric` over a window around `at` wide
/// enough to hold the period the ingested events land in, whichever side of a boundary they
/// fell.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    namespace: &str,
    metric: &str,
    at: f64,
) -> Vec<Value> {
    #[allow(clippy::cast_precision_loss)]
    let span = 2.0 * ALARM_PERIOD as f64;
    let answered = cw(
        router,
        case,
        region,
        "GetMetricStatistics",
        &json!({
            "Namespace": namespace,
            "MetricName": metric,
            "StartTime": at - span,
            "EndTime": at + span,
            "Period": ALARM_PERIOD,
            "Statistics": ["SampleCount", "Average", "Sum", "Minimum", "Maximum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Read the queue at `url` until `wanted` notifications have arrived on it, or the deadline
/// passes, and answer every notification that did arrive.
///
/// The wait is on the CONDITION: each round is one long-polling receive
/// ([`drain`] asks for `WaitTimeSeconds`), which answers the instant the queue has something
/// and otherwise costs that poll's own wait — so a slow host delays a row instead of deciding
/// it, and nothing here sleeps for a span of its own.
async fn delivered_until(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        arrived.extend(drain(router, case, region, url).await);
        if arrived.len() >= wanted || Instant::now() >= deadline {
            return arrived;
        }
    }
}

/// Watch the quiet alarm until the evaluation the clock owes has SETTLED it — looking again
/// until it reports the state a series with no datapoint and `notBreaching` implies — and
/// answer the state it last reported beside every notification its own queue received while
/// the watch ran.
///
/// The settled state is the condition the watch waits for, so the window the silence is
/// established over is decided by what the emulator did rather than by a span of this host's
/// clock; the watch then keeps reading for [`QUIET_ROUNDS`] more rounds, so a delivery made on
/// a later wake-up of that same settled state is still caught. Every round reads the queue
/// first, so nothing that arrived at any point in the watch is missed, and the `DescribeAlarms`
/// each round makes can only strengthen the silence: it materialises whatever the clock has
/// made due, so a transition this alarm did owe would have fired its action and shown up in a
/// later round. [`DELIVERY_DEADLINE`] only decides when a settle that never comes is a failure
/// rather than a hang.
async fn watched_quiet(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    url: &str,
) -> (String, Vec<Value>) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    let mut settled_rounds = 0;
    loop {
        arrived.extend(drain(router, case, region, url).await);
        let state = alarm_state(router, case, region, alarm).await;
        if state == STATE_OK {
            settled_rounds += 1;
        }
        if settled_rounds >= QUIET_ROUNDS || Instant::now() >= deadline {
            return (state, arrived);
        }
    }
}

/// One row: the filter the alarm watches, the quiet twin whose pattern nothing matches, the
/// batch ingested, and how the alarm grades the window those events land in.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The Region the whole wiring lives in.
    region: &'static str,
    /// The `filterPattern` of the filter the subject alarm's metric comes from.
    pattern: &'static str,
    /// The `metricValue` that filter's one transformation publishes.
    metric_value: &'static str,
    /// The `filterPattern` of the QUIET twin, stored on the same log group: a pattern NONE of
    /// the messages below matches.
    quiet_pattern: &'static str,
    /// The messages ingested as one batch, each paired with the value it publishes through the
    /// subject filter — `None` for a message the pattern does not match, which publishes
    /// nothing because the transformation declares no `defaultValue`.
    messages: &'static [(&'static str, Option<f64>)],
    /// The `Statistic` the alarm aggregates the period with.
    statistic: &'static str,
    /// The `ComparisonOperator` it grades that aggregate with.
    comparison: &'static str,
    /// The `Threshold` the aggregate has to cross in that direction.
    threshold: f64,
}

const ROWS: &[Row] = &[
    Row {
        case: "a JSON pattern over a member, a literal metric value and a Sum above its \
               threshold",
        region: "us-east-1",
        pattern: r#"{ $.level = "ERROR" }"#,
        metric_value: "1",
        quiet_pattern: r#"{ $.level = "FATAL" }"#,
        messages: &[
            (r#"{"level":"ERROR","request":1}"#, Some(1.0)),
            (r#"{"level":"ERROR","request":2}"#, Some(1.0)),
            (r#"{"level":"ERROR","request":3}"#, Some(1.0)),
            (r#"{"level":"INFO","request":4}"#, None),
        ],
        statistic: "Sum",
        comparison: "GreaterThanThreshold",
        threshold: 2.0,
    },
    Row {
        case: "an unstructured term pattern graded by SampleCount at its threshold, in another \
               Region",
        region: "us-west-2",
        pattern: "ERROR",
        metric_value: "1",
        quiet_pattern: "NEVERLOGGED",
        messages: &[
            ("ERROR disk full", Some(1.0)),
            ("ERROR connection timeout", Some(1.0)),
            ("INFO all healthy", None),
        ],
        statistic: "SampleCount",
        comparison: "GreaterThanOrEqualToThreshold",
        threshold: 2.0,
    },
    Row {
        case: "a space-delimited pattern whose field is the metric value, graded by Maximum",
        region: "eu-west-1",
        pattern: "[ip, status, size]",
        metric_value: "$size",
        quiet_pattern: "[ip, status, size, extra]",
        messages: &[
            ("10.0.0.1 500 2048", Some(2048.0)),
            ("10.0.0.2 500 512", Some(512.0)),
            ("unparseable", None),
        ],
        statistic: "Maximum",
        comparison: "GreaterThanThreshold",
        threshold: 1024.0,
    },
    Row {
        case: "a numeric JSON condition, a JSON selector metric value and a Minimum BELOW its \
               threshold",
        region: "us-east-1",
        pattern: "{ $.latency > 0 }",
        metric_value: "$.latency",
        quiet_pattern: "{ $.latency > 1000 }",
        messages: &[
            (r#"{"latency":5}"#, Some(5.0)),
            (r#"{"latency":9}"#, Some(9.0)),
            (r#"{"other":1}"#, None),
        ],
        statistic: "Minimum",
        comparison: "LessThanThreshold",
        threshold: 7.0,
    },
];

#[tokio::test]
async fn an_ingested_log_event_drives_its_metric_filters_alarm_to_the_subscribed_queue() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        // One emulator per row, so what a row reads is that row's alone whichever Region it
        // signs for.
        let router = emulator();

        let group = format!("/metric-filter-chain/{index}");
        let namespace = format!("Custom/MetricFilterChain{index}");
        let metric = "Matched";
        let quiet_metric = "Unmatched";
        let subject_alarm = format!("metric-filter-chain-{index}");
        let quiet_alarm = format!("metric-filter-chain-quiet-{index}");

        // 1. The log group, and the two filters on it: the one the row's events match and the
        //    quiet twin whose pattern none of them matches. They differ in the pattern alone,
        //    so the difference between what they publish is the matching alone.
        given_group_with_stream(&router, case, region, &group, "events").await;
        given_metric_filter(
            &router,
            case,
            region,
            &group,
            &FilterSpec {
                name: "matched",
                pattern: row.pattern,
                namespace: &namespace,
                metric,
                metric_value: row.metric_value,
            },
        )
        .await;
        given_metric_filter(
            &router,
            case,
            region,
            &group,
            &FilterSpec {
                name: "unmatched",
                pattern: row.quiet_pattern,
                namespace: &namespace,
                metric: quiet_metric,
                metric_value: row.metric_value,
            },
        )
        .await;

        // 2. A topic per alarm with a queue of this emulator subscribed to it. The quiet twin's
        //    queue grants SNS the send UNSCOPED, so its silence cannot be a policy decision.
        let (topic, subject_url) = given_topic_with_subscriber(
            &router,
            case,
            region,
            &format!("metric-filter-chain-{index}"),
            Grant::ScopedToTopic,
        )
        .await;
        let (quiet_topic, quiet_url) = given_topic_with_subscriber(
            &router,
            case,
            region,
            &format!("metric-filter-chain-quiet-{index}"),
            Grant::Unscoped,
        )
        .await;

        // 3. The two alarms, identical but for the metric each watches.
        given_alarm(
            &router,
            case,
            region,
            &AlarmSpec {
                name: &subject_alarm,
                namespace: &namespace,
                metric,
                statistic: row.statistic,
                comparison: row.comparison,
                threshold: row.threshold,
                topic: &topic,
            },
        )
        .await;
        given_alarm(
            &router,
            case,
            region,
            &AlarmSpec {
                name: &quiet_alarm,
                namespace: &namespace,
                metric: quiet_metric,
                statistic: row.statistic,
                comparison: row.comparison,
                threshold: row.threshold,
                topic: &quiet_topic,
            },
        )
        .await;
        let before = alarm_state(&router, case, region, &subject_alarm).await;
        assert_ne!(
            before, STATE_ALARM,
            "case {case:?}: the alarm must not be in {STATE_ALARM} before anything is ingested, \
             got {before:?}"
        );

        // 4. The ingest: one batch carrying the row's messages, matching and not.
        let at = now_secs();
        let timestamp = now_ms();
        let events: Vec<Value> = row
            .messages
            .iter()
            .map(|(message, _)| json!({"timestamp": timestamp, "message": message}))
            .collect();
        let accepted = logs(
            &router,
            case,
            region,
            "PutLogEvents",
            &json!({
                "logGroupName": &group,
                "logStreamName": "events",
                "logEvents": events,
            }),
        )
        .await;
        assert!(
            accepted.get("rejectedLogEventsInfo").is_none(),
            "case {case:?}: the batch must be accepted whole, got {accepted}"
        );

        // 5. What the filters made of those events: the matching ones aggregated into the
        //    subject's series, and no datapoint at all in the quiet one.
        let published: Vec<f64> = row
            .messages
            .iter()
            .filter_map(|(_, value)| *value)
            .collect();
        let expected = aggregate(case, row.statistic, &published);
        assert!(
            breaches(case, row.comparison, expected, row.threshold),
            "case {case:?}: the row must state a threshold its own events breach: {expected} \
             {} {}",
            row.comparison,
            row.threshold
        );
        let points = datapoints(&router, case, region, &namespace, metric, at).await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: the matching events land in one period window, got {points:?}"
        );
        let reported = points[0][row.statistic].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the datapoint must carry a numeric {}, got {}",
                row.statistic, points[0]
            )
        });
        assert!(
            (reported - expected).abs() <= f64::EPSILON * expected.abs().max(1.0),
            "case {case:?}: expected the series to report {} {expected} — the aggregate of what \
             the matching events published — got {reported}",
            row.statistic
        );
        let quiet_points = datapoints(&router, case, region, &namespace, quiet_metric, at).await;
        assert!(
            quiet_points.is_empty(),
            "case {case:?}: a filter whose pattern no ingested event matched must publish no \
             datapoint, got {quiet_points:?}"
        );

        // 6. The chain: the evaluation the clock owes moves the alarm, its action publishes to
        //    the topic, and the subscribed queue receives the notification — with no call
        //    forcing any of it.
        let delivered = delivered_until(&router, case, region, &subject_url, 1).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the alarm's action must reach the subscribed queue exactly once, got \
             {delivered:?}"
        );
        let notification = &delivered[0];
        assert_eq!(
            notification["AlarmName"].as_str(),
            Some(subject_alarm.as_str()),
            "case {case:?}: the notification must name the alarm, got {notification}"
        );
        assert_eq!(
            notification["NewStateValue"].as_str(),
            Some(STATE_ALARM),
            "case {case:?}: the notification must carry the state moved to, got {notification}"
        );
        assert_eq!(
            notification["Trigger"]["MetricName"].as_str(),
            Some(metric),
            "case {case:?}: the Trigger must report the filter's metric, got {notification}"
        );
        assert_eq!(
            notification["Trigger"]["Namespace"].as_str(),
            Some(namespace.as_str()),
            "case {case:?}: the Trigger must report the filter's namespace, got {notification}"
        );
        let state = alarm_state(&router, case, region, &subject_alarm).await;
        assert_eq!(
            state, STATE_ALARM,
            "case {case:?}: the alarm the matching events breached must report {STATE_ALARM}, \
             got {state:?}"
        );

        // 7. And the quiet twin, whose filter matched nothing, never transitioned: its state
        //    stays OK and its own subscribed queue stays empty, however many wake-ups pass.
        //    The watch waits for that settled state and keeps reading the queue across several
        //    further wake-ups, so what it establishes is the alarm's silence rather than how
        //    fast this host ran.
        let (quiet_state, quiet_delivered) =
            watched_quiet(&router, case, region, &quiet_alarm, &quiet_url).await;
        assert_eq!(
            quiet_state, STATE_OK,
            "case {case:?}: an alarm over a metric no matching event published to must stay \
             {STATE_OK}, got {quiet_state:?}"
        );
        assert!(
            quiet_delivered.is_empty(),
            "case {case:?}: an alarm that never fired may deliver nothing, got \
             {quiet_delivered:?}"
        );
    }
}
