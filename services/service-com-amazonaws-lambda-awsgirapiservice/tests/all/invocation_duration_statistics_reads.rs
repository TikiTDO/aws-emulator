// SPDX-License-Identifier: CC0-1.0

//! Where a client READS the latency of an invocation: the `Duration` series Lambda publishes
//! about every invocation that ran, asked for through CloudWatch's `GetMetricStatistics` with
//! CloudWatch mounted beside Lambda on one engine — the shape `core/router/src/lib.rs` mounts
//! them in.
//!
//! The RULE asserted over the table below: an invocation that RAN is measured once, and that
//! measurement is published as one `Duration` observation, in `Milliseconds`, on the series
//! identified by the `AWS/Lambda` namespace and the invoked function's own `FunctionName`
//! dimension. So a read of that namespace, metric and dimension set over a window holding the
//! invocation answers with at least one datapoint; the datapoint reports the time the run
//! really took (no less than the time its handler spent), carries `Milliseconds` as its unit,
//! and reports EXACTLY the basic statistics the caller named (`Statistic`: `SampleCount`,
//! `Average`, `Sum`, `Minimum`, `Maximum`) and none of the others.
//!
//! Two selection rules the same read is subject to are asserted beside it, because a series
//! answered under the wrong identity is as unusable as one that is never published:
//!
//! * the unit filter — "If you omit Unit, all data that was collected with any unit is
//!   returned […] If you specify a unit, the operation returns only data that was collected
//!   with that unit specified. If you specify a unit that does not match the data collected,
//!   the results of the operation are null." (`GetMetricStatisticsInput$Unit`): a read naming
//!   no unit and one naming `Milliseconds` both answer, and one naming `Count` answers
//!   nothing;
//! * the dimension set — "CloudWatch treats each unique combination of dimensions as a
//!   separate metric. If a specific combination of dimensions was not published, you can't
//!   retrieve statistics for it." (`GetMetricStatisticsInput$Dimensions`): a read naming
//!   another function, or naming no dimension at all, answers nothing.
//!
//! The rows differ along what the rule generalises over: how long each run takes (nothing at
//! all, a fraction of a second, and a second), how many invocations the function serves, and
//! whether the runs of one function are of equal length. The queries differ along what the
//! reader asks for: which statistics, which unit filter, and which dimension set.
//!
//! This is the reader's half of the rule. `invocation_duration_report_agreement.rs` asserts
//! that the published number is the one the run's own `REPORT` line states, over its own
//! table; here the question is whether a client asking CloudWatch the way a dashboard or an
//! alarm asks is served at all, and with the statistics it asked for.
//!
//! Every namespace, metric, dimension, statistic and unit name here is written as a literal
//! — taken from the AWS Lambda metric reference and from this service's own model
//! (`Statistic`, `StandardUnit`) rather than from the crate's constants — so a series
//! published under a name no AWS reader would ask for fails here instead of agreeing with
//! itself. The Lambda-side harness (`send`, `Answer`, the activation window) is
//! `function_lifecycle.rs`'s, the deployment archive is `synchronous_invoke_run.rs`'s, the
//! CloudWatch call is `kinesis_event_source_delivery.rs`'s, and the single invocation stream
//! this target's handler-running suites share is `invoke_request_id.rs`'s.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{make_activation_due, send, Answer, ROLE};
use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::kinesis_event_source_delivery::cloudwatch;
use crate::synchronous_invoke_run::package;

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";

/// The name that reference gives the series under test, and the dimension it reports a
/// function's own series per.
const DURATION_SERIES: &str = "Duration";
/// See [`DURATION_SERIES`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The `StandardUnit` the reference states `Duration` is reported in, and one this series is
/// never collected with — the mismatching filter a row reads with.
const MILLISECONDS_UNIT: &str = "Milliseconds";
/// See [`MILLISECONDS_UNIT`].
const COUNT_UNIT: &str = "Count";

/// The basic statistics a `Datapoint` can report, in the order the model's `Statistic` enum
/// declares them. A read names a subset of these and must be answered with exactly that
/// subset.
const BASIC_STATISTICS: &[&str] = &["SampleCount", "Average", "Sum", "Minimum", "Maximum"];

/// The module and entry point the package below holds its handler at.
const HANDLER: &str = "lambda_function.lambda_handler";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's own
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds — wide enough that the
/// minute a datapoint was published in is inside it however a row straddles a minute boundary.
const WINDOW_SECS: f64 = 600.0;

/// The function's own `Timeout`, in seconds: longer than any row's handler sleeps for.
const FUNCTION_TIMEOUT_SECS: u32 = 10;

/// How far below the time a handler slept a published measurement may fall, in milliseconds:
/// the run is measured on the host's own clock while the handler waits on the runtime's, and
/// the two need not agree to better than a millisecond. The bound is one-sided — a measurement
/// may be longer than the sleep (the run does more than sleep) and never meaningfully shorter.
const CLOCK_AGREEMENT_MS: f64 = 1.0;

/// How far two readings of the same aggregate may differ, in milliseconds: the statistics are
/// folded from the same stored observations, so the only difference between `Sum` and
/// `Average` times `SampleCount` is the rounding of the division.
const FOLD_TOLERANCE_MS: f64 = 1e-6;

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
/// read in-process through CloudWatch's own surface.
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

/// Which dimension set a query asks the series under: the function that was invoked, another
/// function, or no dimension at all.
#[derive(Clone, Copy, Debug)]
enum Dimensions {
    /// The invoked function's own dimension set.
    InvokedFunction,
    /// Another function's — a combination this invocation published nothing on.
    OtherFunction,
    /// None at all: the account-wide series, which no per-function measurement belongs to.
    None,
}

/// One read of the series: what statistics it asks for, what unit it filters on, what
/// dimension set it names, and whether that identity is the one the measurement was published
/// on.
#[derive(Clone, Copy, Debug)]
struct Query {
    case: &'static str,
    statistics: &'static [&'static str],
    unit: Option<&'static str>,
    dimensions: Dimensions,
    /// Whether this read must be answered with datapoints at all.
    served: bool,
}

/// The reads every row is answered for.
const QUERIES: &[Query] = &[
    Query {
        case: "every basic statistic, with no unit filter",
        statistics: BASIC_STATISTICS,
        unit: None,
        dimensions: Dimensions::InvokedFunction,
        served: true,
    },
    Query {
        case: "the average alone, filtered on the unit the series is collected with",
        statistics: &["Average"],
        unit: Some(MILLISECONDS_UNIT),
        dimensions: Dimensions::InvokedFunction,
        served: true,
    },
    Query {
        case: "the total and the count of the runs, with no unit filter",
        statistics: &["Sum", "SampleCount"],
        unit: None,
        dimensions: Dimensions::InvokedFunction,
        served: true,
    },
    Query {
        case: "filtered on a unit the series is not collected with",
        statistics: BASIC_STATISTICS,
        unit: Some(COUNT_UNIT),
        dimensions: Dimensions::InvokedFunction,
        served: false,
    },
    Query {
        case: "another function's dimension set",
        statistics: BASIC_STATISTICS,
        unit: None,
        dimensions: Dimensions::OtherFunction,
        served: false,
    },
    Query {
        case: "no dimension at all",
        statistics: BASIC_STATISTICS,
        unit: None,
        dimensions: Dimensions::None,
        served: false,
    },
];

/// One row: the function, and how long each of its invocations' handlers sleeps for.
struct Row {
    case: &'static str,
    function: &'static str,
    /// One entry per invocation the row makes: the milliseconds that run's handler sleeps.
    sleeps_ms: &'static [u64],
}

/// The rows.
const ROWS: &[Row] = &[
    Row {
        case: "one run that sleeps for nothing",
        function: "duration-read-instant",
        sleeps_ms: &[0],
    },
    Row {
        case: "one run that sleeps a fraction of a second",
        function: "duration-read-fraction",
        sleeps_ms: &[250],
    },
    Row {
        case: "one run that sleeps a whole second",
        function: "duration-read-second",
        sleeps_ms: &[1000],
    },
    Row {
        case: "three runs of unequal length",
        function: "duration-read-unequal",
        sleeps_ms: &[0, 120, 400],
    },
];

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

/// One synchronous invocation whose handler sleeps `sleep_ms` milliseconds.
async fn invoke(router: &MultiServiceRouter, function: &str, sleep_ms: u64) -> Answer {
    send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        "application/json",
        json!({ "sleep_ms": sleep_ms }).to_string().into_bytes(),
        &[],
    )
    .await
}

/// The `Datapoints` one read of the `Duration` series answers with.
async fn read_duration(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    query: &Query,
) -> Vec<Value> {
    let now = now_secs();
    let mut request = json!({
        "Namespace": NAMESPACE,
        "MetricName": DURATION_SERIES,
        "StartTime": now - WINDOW_SECS,
        "EndTime": now + WINDOW_SECS,
        "Period": PERIOD_SECS,
        "Statistics": query.statistics,
    });
    if let Some(unit) = query.unit {
        request["Unit"] = json!(unit);
    }
    match query.dimensions {
        Dimensions::InvokedFunction => {
            request["Dimensions"] = json!([{ "Name": FUNCTION_DIMENSION, "Value": function }]);
        }
        Dimensions::OtherFunction => {
            request["Dimensions"] =
                json!([{ "Name": FUNCTION_DIMENSION, "Value": format!("{function}-elsewhere") }]);
        }
        Dimensions::None => {}
    }
    let (status, answered) = cloudwatch(router, "GetMetricStatistics", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics must be served, got {status} {answered}"
    );
    answered["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The total of one statistic over every datapoint of a read.
fn total(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point[statistic].as_f64())
        .sum()
}

/// The greatest `Maximum` any datapoint reports, or `None` when none does.
fn greatest(points: &[Value]) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point["Maximum"].as_f64())
        .max_by(f64::total_cmp)
}

/// The least `Minimum` any datapoint reports, or `None` when none does.
fn least(points: &[Value]) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point["Minimum"].as_f64())
        .min_by(f64::total_cmp)
}

/// Every run is measured and that measurement is readable, on the identity it was published
/// on and on no other, with exactly the statistics the reader asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_run_publishes_a_duration_datapoint_a_client_can_read() {
    let _stream = one_invocation_stream_at_a_time().await;
    for row in ROWS {
        // One account per row, so the only invocations in the window are this row's.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let case = row.case;
        create_function(&router, &engine, case, row.function).await;

        for sleep_ms in row.sleeps_ms {
            let answered = invoke(&router, row.function, *sleep_ms).await;
            assert_eq!(
                answered.status,
                200,
                "case {case:?}: the invocation must be served, got {} {}",
                answered.status,
                String::from_utf8_lossy(&answered.body)
            );
            assert_eq!(
                answered.header("x-amz-function-error"),
                None,
                "case {case:?}: the invocation must carry no function error, got {}",
                String::from_utf8_lossy(&answered.body)
            );
        }

        let runs = row.sleeps_ms.len() as f64;
        let longest = row.sleeps_ms.iter().copied().max().unwrap_or(0) as f64;
        let shortest = row.sleeps_ms.iter().copied().min().unwrap_or(0) as f64;
        let slept_total: f64 = row.sleeps_ms.iter().map(|ms| *ms as f64).sum();

        for query in QUERIES {
            let read = query.case;
            let points = read_duration(&router, case, row.function, query).await;

            if !query.served {
                assert!(
                    points.is_empty(),
                    "case {case:?} read {read:?}: an identity this measurement was not \
                     published on must answer no datapoint, got {points:?}"
                );
                continue;
            }

            assert!(
                !points.is_empty(),
                "case {case:?} read {read:?}: expected at least one Duration datapoint for \
                 {} run(s), got none",
                row.sleeps_ms.len()
            );

            // A datapoint reports the unit its observations were collected with, and exactly
            // the statistics the read named.
            for point in &points {
                let unit = point["Unit"].as_str();
                assert_eq!(
                    unit,
                    Some(MILLISECONDS_UNIT),
                    "case {case:?} read {read:?}: expected the datapoint's Unit to be \
                     {MILLISECONDS_UNIT:?}, got {unit:?} in {point:?}"
                );
                for statistic in BASIC_STATISTICS {
                    let asked = query.statistics.contains(statistic);
                    let reported = point[*statistic].as_f64();
                    assert_eq!(
                        reported.is_some(),
                        asked,
                        "case {case:?} read {read:?}: the read {} {statistic}, so the \
                         datapoint must {} it, got {reported:?} in {point:?}",
                        if asked { "asked for" } else { "did not ask for" },
                        if asked { "report" } else { "omit" },
                    );
                }
            }

            if query.statistics.contains(&"SampleCount") {
                let samples = total(&points, "SampleCount");
                assert_eq!(
                    samples, runs,
                    "case {case:?} read {read:?}: expected {runs} measurement(s), got \
                     {samples} in {points:?}"
                );
            }
            if query.statistics.contains(&"Sum") {
                let sum = total(&points, "Sum");
                assert!(
                    sum + CLOCK_AGREEMENT_MS * runs >= slept_total,
                    "case {case:?} read {read:?}: the runs slept {slept_total} ms in total, \
                     so their Duration Sum must be at least that, got {sum}"
                );
            }
            if query.statistics.contains(&"Maximum") {
                let maximum = greatest(&points).unwrap_or_else(|| {
                    panic!("case {case:?} read {read:?}: Maximum was asked for and reported")
                });
                assert!(
                    maximum + CLOCK_AGREEMENT_MS >= longest,
                    "case {case:?} read {read:?}: the longest run slept {longest} ms, so the \
                     Duration Maximum must be at least that, got {maximum}"
                );
            }
            if query.statistics.contains(&"Minimum") {
                let minimum = least(&points).unwrap_or_else(|| {
                    panic!("case {case:?} read {read:?}: Minimum was asked for and reported")
                });
                assert!(
                    minimum + CLOCK_AGREEMENT_MS >= shortest,
                    "case {case:?} read {read:?}: the shortest run slept {shortest} ms, so \
                     the Duration Minimum must be at least that, got {minimum}"
                );
            }
            if query.statistics.contains(&"Average") {
                for point in &points {
                    let average = point["Average"].as_f64().unwrap_or_default();
                    assert!(
                        average + CLOCK_AGREEMENT_MS >= shortest,
                        "case {case:?} read {read:?}: no run slept less than {shortest} ms, \
                         so the Duration Average must be at least that, got {average} in \
                         {point:?}"
                    );
                    // The statistics of one window are folded from the same observations, so
                    // the average times the count is the sum.
                    if let (Some(sum), Some(samples)) =
                        (point["Sum"].as_f64(), point["SampleCount"].as_f64())
                    {
                        assert!(
                            (average * samples - sum).abs() <= FOLD_TOLERANCE_MS,
                            "case {case:?} read {read:?}: expected Average × SampleCount to \
                             be the Sum {sum}, got {average} × {samples} in {point:?}"
                        );
                    }
                }
            }
        }
    }
}
