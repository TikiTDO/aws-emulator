// SPDX-License-Identifier: CC0-1.0

//! The two planes a client reads ONE invocation's time on have to agree: the `REPORT` line
//! the run's own log tail carries, and the `Duration` datapoint Lambda publishes about that
//! same run — read back where a client reads it, the `AWS/Lambda` series CloudWatch answers
//! for, with CloudWatch mounted beside Lambda on one engine.
//!
//! The RULE asserted over the table below: an invocation that RAN is measured once, and both
//! planes report that one measurement —
//!
//! * `Duration`, in `Milliseconds`, holds one datapoint per invocation that ran, so over `n`
//!   invocations of a function its `SampleCount` is `n`, its `Sum` is the total of the `n`
//!   durations those runs' own `REPORT` lines state, and its `Maximum`/`Minimum` are the
//!   greatest and least of them — each within the half-unit the report's two decimals round
//!   by, because the report states the same measurement to two decimal places;
//! * `Invocations`, in `Count`, sums to `n`: every run was an invocation Lambda admitted;
//! * `ConcurrentExecutions`, in `Count`, is a GAUGE of how many executions the function holds
//!   at an instant, so its `Maximum` over the window is the greatest number the function ever
//!   held at once — exactly 1 for invocations made strictly one after another, and the number
//!   started together when several are in flight at once.
//!
//! So a client can chart function latency, throughput and concurrency, and the number a
//! latency dashboard reads is the number that run's own log reported.
//!
//! The rows differ along what the rule generalises over: how many invocations are made, how
//! long each run takes (none at all, and a handler that sleeps), and whether the invocations
//! are made one after another or at the same time — which is what the concurrency gauge
//! distinguishes.
//!
//! Every metric, dimension and unit name here is written as a literal, taken from the AWS
//! Lambda metric reference rather than from this crate's constants, so a series published
//! under a name no AWS reader would ask for fails here instead of agreeing with itself. The
//! Lambda-side harness (`send`, `Answer`, the activation window) is `function_lifecycle.rs`'s,
//! the deployment archive is `synchronous_invoke_run.rs`'s, the CloudWatch call is
//! `kinesis_event_source_delivery.rs`'s, and the single invocation stream this target's
//! handler-running suites share is `invoke_request_id.rs`'s.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{make_activation_due, send, Answer, ROLE};
use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::kinesis_event_source_delivery::cloudwatch;
use crate::synchronous_invoke_run::package;

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";

/// The names that reference gives the series under test, and the units it reports them in.
const DURATION_SERIES: &str = "Duration";
/// See [`DURATION_SERIES`].
const INVOCATIONS_SERIES: &str = "Invocations";
/// See [`DURATION_SERIES`].
const CONCURRENT_EXECUTIONS_SERIES: &str = "ConcurrentExecutions";
/// See [`DURATION_SERIES`].
const MILLISECONDS_UNIT: &str = "Milliseconds";
/// See [`DURATION_SERIES`].
const COUNT_UNIT: &str = "Count";

/// The dimension that reference reports a function's own series per.
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The module and entry point every package below holds its handler at.
const HANDLER: &str = "lambda_function.lambda_handler";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The request header `Invoke$LogType` is bound to, and the value that asks for the log tail
/// the `REPORT` line is read out of.
const LOG_TYPE_HEADER: &str = "x-amz-log-type";
/// See [`LOG_TYPE_HEADER`].
const LOG_TYPE_TAIL: &str = "Tail";

/// The fixed text of the runtime's closing report, and the field the run's own measured time
/// is stated in. Source: the `REPORT RequestId: <id> Duration: <d> ms Billed Duration: <b> ms`
/// line Lambda writes at the end of every invocation.
const REPORT_PREFIX: &str = "REPORT RequestId: ";
/// See [`REPORT_PREFIX`].
const REPORT_DURATION_FIELD: &str = "Duration: ";
/// See [`REPORT_PREFIX`].
const REPORT_BILLED_FIELD: &str = "Billed Duration: ";

/// How far a published `Duration` may differ from the one its run's own `REPORT` line states,
/// in milliseconds: the report states the measurement to two decimal places, so it rounds by
/// at most half of the last place it prints.
const REPORT_ROUNDING_MS: f64 = 0.005;

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's own
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds — wide enough that the
/// minute a datapoint was published in is inside it however a row straddles a minute boundary.
const WINDOW_SECS: f64 = 600.0;

/// The function's own `Timeout`, in seconds: longer than any row's handler sleeps for.
const FUNCTION_TIMEOUT_SECS: u32 = 10;

/// How long after the first invocation a row that makes its calls TOGETHER makes the second:
/// well inside the run the first one started, so the two are in flight at the same instant.
const SECOND_CALL_AFTER: Duration = Duration::from_millis(100);

/// The handler every row runs: it sleeps for as long as the event asks and answers how long
/// that was, so the run's own time is the row's to choose.
const SOURCE: &str = r#"
import time


def lambda_handler(event, context):
    sleep_ms = event.get("sleep_ms") or 0
    if sleep_ms:
        time.sleep(sleep_ms / 1000.0)
    return {"slept_ms": sleep_ms}
"#;

/// Lambda and CloudWatch on one engine: the series Lambda publishes is CloudWatch's, so it is
/// read in-process through CloudWatch's own surface — the shape `core/router/src/lib.rs`
/// mounts them in.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// Now, in the epoch seconds a stored datapoint is stamped with.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Create the function a row invokes and bring it to `Active`.
async fn create_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    function: &str,
) {
    let created = send(
        router,
        "POST",
        FUNCTIONS_URI,
        "application/json",
        serde_json::to_vec(&json!({
            "FunctionName": function,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT_SECS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(SOURCE)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateFunction must succeed, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// One synchronous invocation asking for the log tail the `REPORT` line is read out of.
async fn invoke(router: &MultiServiceRouter, function: &str, sleep_ms: u64) -> Answer {
    send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        "application/json",
        json!({ "sleep_ms": sleep_ms }).to_string().into_bytes(),
        &[(LOG_TYPE_HEADER, LOG_TYPE_TAIL.to_string())],
    )
    .await
}

/// The `Duration` and `Billed Duration`, in milliseconds, the `REPORT` line of `answered`'s
/// own log tail states for that run.
fn reported_duration_ms(case: &str, answered: &Answer) -> (f64, f64) {
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: the invocation must be served, got {} {}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    let encoded = answered
        .header("x-amz-log-result")
        .unwrap_or_else(|| panic!("case {case:?}: LogType=Tail must answer a log tail"));
    let tail = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the log tail must be base64, got {error}")
            }),
    )
    .unwrap_or_else(|error| panic!("case {case:?}: the log tail must be text, got {error}"));
    let report = tail
        .lines()
        .find(|line| line.starts_with(REPORT_PREFIX))
        .unwrap_or_else(|| panic!("case {case:?}: the tail must carry a report line, got {tail:?}"));
    (
        report_field(case, report, REPORT_DURATION_FIELD),
        report_field(case, report, REPORT_BILLED_FIELD),
    )
}

/// The milliseconds the field `field` names states in the report line `report`.
fn report_field(case: &str, report: &str, field: &str) -> f64 {
    let (_, after) = report
        .split_once(field)
        .unwrap_or_else(|| panic!("case {case:?}: the report must state {field:?}, got {report:?}"));
    let value = after
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: {field:?} must carry a number, got {report:?}"));
    value
        .parse::<f64>()
        .unwrap_or_else(|error| panic!("case {case:?}: {field:?} is {value:?}, which {error}"))
}

/// The `Datapoints` `GetMetricStatistics` answers for the series `metric` identifies on the
/// function `function` names, selected by the unit the AWS Lambda metric reference gives it.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    metric: &str,
    unit: &str,
) -> Vec<Value> {
    let now = now_secs();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum", "SampleCount", "Minimum", "Maximum"],
            "Unit": unit,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics({metric}) answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The total of one statistic over every datapoint of a series.
fn total(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point[statistic].as_f64())
        .sum()
}

/// The greatest `Maximum` any datapoint of a series reports, or `None` for a series with no
/// datapoint at all.
fn greatest(points: &[Value]) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point["Maximum"].as_f64())
        .max_by(f64::total_cmp)
}

/// The least `Minimum` any datapoint of a series reports, or `None` for a series with no
/// datapoint at all.
fn least(points: &[Value]) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point["Minimum"].as_f64())
        .min_by(f64::total_cmp)
}

/// One row: how many invocations are made, how long each run sleeps, and whether they are made
/// one after another or all at once.
struct Row {
    case: &'static str,
    function: &'static str,
    /// How long each run sleeps, in milliseconds.
    sleep_ms: u64,
    /// How many invocations the row makes.
    invocations: usize,
    /// Whether the invocations are in flight at the same time.
    together: bool,
    /// The greatest number of executions the function holds at one instant, which is what the
    /// concurrency gauge's `Maximum` must report: one for invocations made one after another,
    /// and all of them when they are made together.
    concurrency_peak: f64,
}

/// The rows. Every expectation is written out rather than computed from the row, so a
/// publisher that counted calls and one that counted invocations cannot both pass.
const ROWS: &[Row] = &[
    Row {
        case: "one invocation that sleeps for nothing",
        function: "measured-once",
        sleep_ms: 0,
        invocations: 1,
        together: false,
        concurrency_peak: 1.0,
    },
    Row {
        case: "two sequential invocations of a handler that sleeps",
        function: "measured-twice",
        sleep_ms: 60,
        invocations: 2,
        together: false,
        concurrency_peak: 1.0,
    },
    Row {
        case: "four sequential invocations of a handler that sleeps",
        function: "measured-four-times",
        sleep_ms: 150,
        invocations: 4,
        together: false,
        concurrency_peak: 1.0,
    },
    Row {
        case: "two invocations in flight at the same time",
        function: "measured-together",
        sleep_ms: 400,
        invocations: 2,
        together: true,
        concurrency_peak: 2.0,
    },
];

/// Both planes report the SAME measurement of every run, the invocations are counted, and the
/// concurrency gauge peaks at the number of executions the function really held at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_run_is_measured_once_and_reported_the_same_on_both_planes() {
    let _stream = one_invocation_stream_at_a_time().await;
    for row in ROWS {
        // One account per row, so the only invocations in the window are this row's.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let case = row.case;
        create_function(&router, &engine, case, row.function).await;

        let answers: Vec<Answer> = if row.together {
            assert_eq!(
                row.invocations, 2,
                "case {case:?}: a row whose invocations are made together makes exactly two"
            );
            // The second call is made a moment after the first, while the first run is still
            // in flight: that is what puts two executions on the function at one instant.
            let first = invoke(&router, row.function, row.sleep_ms);
            let second = async {
                tokio::time::sleep(SECOND_CALL_AFTER).await;
                invoke(&router, row.function, row.sleep_ms).await
            };
            let (first, second) = tokio::join!(first, second);
            vec![first, second]
        } else {
            let mut answered = Vec::new();
            for _ in 0..row.invocations {
                answered.push(invoke(&router, row.function, row.sleep_ms).await);
            }
            answered
        };

        let reported: Vec<f64> = answers
            .iter()
            .map(|answered| {
                let (duration_ms, billed_ms) = reported_duration_ms(case, answered);
                assert!(
                    duration_ms + REPORT_ROUNDING_MS >= row.sleep_ms as f64,
                    "case {case:?}: the run slept {} ms, so its report must state at least \
                     that, got {duration_ms}",
                    row.sleep_ms
                );
                assert!(
                    billed_ms + REPORT_ROUNDING_MS >= duration_ms,
                    "case {case:?}: the billed duration must be at least the measured one, \
                     got {billed_ms} for {duration_ms}"
                );
                duration_ms
            })
            .collect();

        // `Duration`: one measurement per run, and every statistic over them is the one the
        // reports state.
        let durations = datapoints(
            &router,
            case,
            row.function,
            DURATION_SERIES,
            MILLISECONDS_UNIT,
        )
        .await;
        let samples = total(&durations, "SampleCount");
        assert_eq!(
            samples, row.invocations as f64,
            "case {case:?}: expected {} Duration measurements, got {samples} in {durations:?}",
            row.invocations
        );
        let tolerance = REPORT_ROUNDING_MS * row.invocations as f64;
        let published_sum = total(&durations, "Sum");
        let reported_sum: f64 = reported.iter().sum();
        assert!(
            (published_sum - reported_sum).abs() <= tolerance,
            "case {case:?}: expected Duration Sum {reported_sum} (the reports' own total), \
             got {published_sum}"
        );
        let reported_max = reported.iter().copied().fold(f64::MIN, f64::max);
        let published_max = greatest(&durations)
            .unwrap_or_else(|| panic!("case {case:?}: Duration must hold a datapoint"));
        assert!(
            (published_max - reported_max).abs() <= REPORT_ROUNDING_MS,
            "case {case:?}: expected Duration Maximum {reported_max}, got {published_max}"
        );
        let reported_min = reported.iter().copied().fold(f64::MAX, f64::min);
        let published_min = least(&durations)
            .unwrap_or_else(|| panic!("case {case:?}: Duration must hold a datapoint"));
        assert!(
            (published_min - reported_min).abs() <= REPORT_ROUNDING_MS,
            "case {case:?}: expected Duration Minimum {reported_min}, got {published_min}"
        );

        // `Invocations`: every run was one invocation Lambda admitted.
        let invocations = datapoints(
            &router,
            case,
            row.function,
            INVOCATIONS_SERIES,
            COUNT_UNIT,
        )
        .await;
        let counted = total(&invocations, "Sum");
        assert_eq!(
            counted, row.invocations as f64,
            "case {case:?}: expected Invocations Sum {}, got {counted} in {invocations:?}",
            row.invocations
        );

        // `ConcurrentExecutions`: the gauge peaks at what the function really held at once.
        let concurrency = datapoints(
            &router,
            case,
            row.function,
            CONCURRENT_EXECUTIONS_SERIES,
            COUNT_UNIT,
        )
        .await;
        let peak = greatest(&concurrency).unwrap_or_else(|| {
            panic!(
                "case {case:?}: ConcurrentExecutions must hold a datapoint for a function that \
                 processed executions, got {concurrency:?}"
            )
        });
        assert_eq!(
            peak, row.concurrency_peak,
            "case {case:?}: expected ConcurrentExecutions Maximum {}, got {peak} in \
             {concurrency:?}",
            row.concurrency_peak
        );
    }
}
