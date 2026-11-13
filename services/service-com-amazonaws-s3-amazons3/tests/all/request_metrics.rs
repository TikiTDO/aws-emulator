// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS/S3` REQUEST metrics a bucket publishes about the requests
//! this service serves on it, asserted where a client reads them: through CloudWatch's own
//! `GetMetricStatistics`, `ListMetrics` and `DescribeAlarms`, with both services mounted on
//! one shared engine exactly as `core/router` assembles them.
//!
//! RULE under test: a request this service serves on a bucket contributes one observation to
//! each request-metric series of EVERY metrics configuration of that bucket whose `Filter`
//! selects the request — `AllRequests`, the counter of the request's own type, the bytes it
//! carried in each direction, and `4xxErrors` / `5xxErrors` "with a value of either 0 or 1"
//! (Amazon S3 User Guide, *Amazon S3 CloudWatch request metrics*) — dimensioned by the bucket
//! and by that configuration's id. So a period with served requests carries a ZERO-valued
//! error datapoint rather than no datapoint at all, a bucket carrying no metrics
//! configuration publishes no series, and an alarm over one of those series is graded on the
//! requests the bucket really served.
//!
//! The table below spans what the rule generalises over: the filter form (none, a prefix, an
//! object tag, a conjunction, an access point ARN), the request type (`GET`, `PUT`, `HEAD`,
//! `DELETE`, a listing), the answer's status class, and how many requests the client makes.
//! The recorded case — two prefix-filtered configurations, a successful read under each and
//! then a read of a key that is not there under one — is ONE row.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::{
    ALARM_STATE_ALARM, ALARM_STATE_OK,
};
// The namespace, dimensions, metric names and units under test, read from this service's own
// publication rule so the suite and the publisher cannot disagree about which series is meant.
use service_com_amazonaws_s3_amazons3::service::request_metrics::{
    ALL_REQUESTS, BUCKET_NAME_DIMENSION, BYTES_DOWNLOADED, BYTES_UNIT, BYTES_UPLOADED,
    COUNT_UNIT, DELETE_REQUESTS, FILTER_ID_DIMENSION, FIVE_XX_ERRORS, FOUR_XX_ERRORS,
    GET_REQUESTS, HEAD_REQUESTS, LIST_REQUESTS, NAMESPACE, PUT_REQUESTS,
};
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, send, send_bytes, Answer};
use super::bucket_metrics_configuration::{put_metrics, Filter, ACCESS_POINT_ARN};
use super::object_and_tagging_roundtrip::{get_object, put_object};

/// The `X-Amz-Target` prefix the CloudWatch model declares.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// How many seconds either side of the requests a metric read spans, and the period it folds
/// them into: one window wide enough that every datapoint of the suite falls in the span
/// whichever minute the clock is in when it runs.
pub(crate) const WINDOW_SECS: f64 = 300.0;

/// The same span as the period a read asks for.
pub(crate) const PERIOD_SECS: i64 = 300;

/// The aggregation period the alarms of the last test below are configured with, which is the
/// granularity S3 publishes request metrics at.
const ALARM_PERIOD_SECS: i64 = 60;

/// How long the alarm test waits for an evaluation the clock owes, and how often it looks: an
/// alarm's state is not written by the call that configures it, so a reader polls for it the
/// way a client does.
const ALARM_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ALARM_POLL_ATTEMPTS: usize = 100;

/// One request a row's client makes.
#[derive(Debug)]
enum Call {
    /// A read of an object, and the status it must be answered with.
    Get {
        key: &'static str,
        status: u16,
    },
    /// A read of an object's metadata alone.
    Head {
        key: &'static str,
    },
    /// A write of an object, with the tag set it carries on the write.
    Put {
        key: &'static str,
        body: &'static str,
        tags: &'static str,
    },
    /// A delete of an object.
    Delete {
        key: &'static str,
    },
    /// A listing of the bucket's contents.
    List,
}

/// What one series must report: the configuration id it is dimensioned by, the metric, and
/// the `Sum` over the window — or `None` for a series the bucket must not publish at all,
/// which is what tells "no request fell under this configuration" from "every request that
/// did was fine".
type Expectation = (&'static str, &'static str, Option<f64>);

/// One row of the table: a case name, the metrics configurations the bucket carries, the
/// requests the client then makes, and what each series must report.
struct Row {
    case: &'static str,
    bucket: &'static str,
    configurations: Vec<(&'static str, Option<Filter>)>,
    calls: Vec<Call>,
    expected: Vec<Expectation>,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "two prefix-filtered configurations, a read under each and a read of a key \
                   that is not there",
            bucket: "request-metrics-prefixes",
            configurations: vec![
                ("loud", Some(Filter::Prefix("loud/".to_string()))),
                ("quiet", Some(Filter::Prefix("quiet/".to_string()))),
            ],
            calls: vec![
                Call::Put {
                    key: "loud/present",
                    body: "loud-bytes",
                    tags: "",
                },
                Call::Put {
                    key: "quiet/present",
                    body: "quiet",
                    tags: "",
                },
                Call::Get {
                    key: "loud/present",
                    status: 200,
                },
                Call::Get {
                    key: "quiet/present",
                    status: 200,
                },
                Call::Get {
                    key: "loud/absent",
                    status: 404,
                },
            ],
            expected: vec![
                ("loud", ALL_REQUESTS, Some(3.0)),
                ("loud", PUT_REQUESTS, Some(1.0)),
                ("loud", GET_REQUESTS, Some(2.0)),
                ("loud", FOUR_XX_ERRORS, Some(1.0)),
                ("loud", FIVE_XX_ERRORS, Some(0.0)),
                ("loud", BYTES_UPLOADED, Some(10.0)),
                ("quiet", ALL_REQUESTS, Some(2.0)),
                ("quiet", PUT_REQUESTS, Some(1.0)),
                ("quiet", GET_REQUESTS, Some(1.0)),
                ("quiet", FOUR_XX_ERRORS, Some(0.0)),
                ("quiet", FIVE_XX_ERRORS, Some(0.0)),
                ("quiet", BYTES_UPLOADED, Some(5.0)),
                ("quiet", LIST_REQUESTS, None),
            ],
        },
        Row {
            case: "an unfiltered configuration counts every request type, a prefix-filtered \
                   one counts no listing",
            bucket: "request-metrics-every-type",
            configurations: vec![
                ("EntireBucket", None),
                ("loud", Some(Filter::Prefix("loud/".to_string()))),
            ],
            calls: vec![
                Call::Put {
                    key: "loud/one",
                    body: "x",
                    tags: "",
                },
                Call::Get {
                    key: "loud/one",
                    status: 200,
                },
                Call::Head { key: "loud/one" },
                Call::List,
                Call::Delete { key: "loud/one" },
            ],
            expected: vec![
                ("EntireBucket", ALL_REQUESTS, Some(5.0)),
                ("EntireBucket", PUT_REQUESTS, Some(1.0)),
                ("EntireBucket", GET_REQUESTS, Some(1.0)),
                ("EntireBucket", HEAD_REQUESTS, Some(1.0)),
                ("EntireBucket", LIST_REQUESTS, Some(1.0)),
                ("EntireBucket", DELETE_REQUESTS, Some(1.0)),
                ("EntireBucket", FOUR_XX_ERRORS, Some(0.0)),
                ("loud", ALL_REQUESTS, Some(4.0)),
                ("loud", PUT_REQUESTS, Some(1.0)),
                ("loud", GET_REQUESTS, Some(1.0)),
                ("loud", HEAD_REQUESTS, Some(1.0)),
                ("loud", DELETE_REQUESTS, Some(1.0)),
                ("loud", LIST_REQUESTS, None),
                ("loud", FOUR_XX_ERRORS, Some(0.0)),
            ],
        },
        Row {
            case: "a write and a read of the same object account for their bytes in each \
                   direction",
            bucket: "request-metrics-bytes",
            configurations: vec![("EntireBucket", None)],
            calls: vec![
                Call::Put {
                    key: "data/one",
                    body: "hello world",
                    tags: "",
                },
                Call::Get {
                    key: "data/one",
                    status: 200,
                },
            ],
            expected: vec![
                ("EntireBucket", ALL_REQUESTS, Some(2.0)),
                ("EntireBucket", BYTES_UPLOADED, Some(11.0)),
                ("EntireBucket", BYTES_DOWNLOADED, Some(11.0)),
            ],
        },
        Row {
            case: "a tag-filtered configuration counts the requests on the objects carrying \
                   that tag",
            bucket: "request-metrics-tagged",
            configurations: vec![(
                "tagged",
                Some(Filter::Tag("tier".to_string(), "gold".to_string())),
            )],
            calls: vec![
                Call::Put {
                    key: "gold/one",
                    body: "g",
                    tags: "tier=gold",
                },
                Call::Put {
                    key: "plain/one",
                    body: "p",
                    tags: "",
                },
                Call::Get {
                    key: "gold/one",
                    status: 200,
                },
                Call::Get {
                    key: "plain/one",
                    status: 200,
                },
            ],
            expected: vec![
                ("tagged", ALL_REQUESTS, Some(2.0)),
                ("tagged", PUT_REQUESTS, Some(1.0)),
                ("tagged", GET_REQUESTS, Some(1.0)),
                ("tagged", FOUR_XX_ERRORS, Some(0.0)),
            ],
        },
        Row {
            case: "a conjunction counts only the requests every one of its predicates holds \
                   for",
            bucket: "request-metrics-conjunction",
            configurations: vec![(
                "both",
                Some(Filter::and(Some("reports/"), &[("tier", "gold")], None)),
            )],
            calls: vec![
                Call::Put {
                    key: "reports/gold",
                    body: "a",
                    tags: "tier=gold",
                },
                Call::Put {
                    key: "reports/silver",
                    body: "b",
                    tags: "tier=silver",
                },
                Call::Put {
                    key: "other/gold",
                    body: "c",
                    tags: "tier=gold",
                },
                Call::Get {
                    key: "reports/gold",
                    status: 200,
                },
                Call::Get {
                    key: "reports/silver",
                    status: 200,
                },
                Call::Get {
                    key: "other/gold",
                    status: 200,
                },
            ],
            expected: vec![
                ("both", ALL_REQUESTS, Some(2.0)),
                ("both", PUT_REQUESTS, Some(1.0)),
                ("both", GET_REQUESTS, Some(1.0)),
            ],
        },
        Row {
            case: "an access-point-filtered configuration counts no request addressed to the \
                   bucket itself",
            bucket: "request-metrics-access-point",
            configurations: vec![(
                "viaAccessPoint",
                Some(Filter::AccessPointArn(ACCESS_POINT_ARN.to_string())),
            )],
            calls: vec![
                Call::Put {
                    key: "a/one",
                    body: "a",
                    tags: "",
                },
                Call::Get {
                    key: "a/one",
                    status: 200,
                },
            ],
            expected: vec![
                ("viaAccessPoint", ALL_REQUESTS, None),
                ("viaAccessPoint", GET_REQUESTS, None),
                ("viaAccessPoint", FOUR_XX_ERRORS, None),
            ],
        },
    ]
}

/// The emulator under test: the production router with S3 and CloudWatch mounted on ONE
/// engine, which is what makes the metric store the served requests write the same one
/// CloudWatch reads.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// Wall-clock now as epoch seconds, the clock a metric window is measured against.
pub(crate) fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// One `awsJson1_0` CloudWatch call, requiring a 200 — the protocol and target prefix that
/// service serves.
pub(crate) async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://monitoring.us-east-1.amazonaws.com/")
        .header("host", "monitoring.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=emulator-test-key/20260101/us-east-1/monitoring/aws4_request, \
             SignedHeaders=host, Signature=not-verified",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a serializable request"),
        ))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with body {parsed}"
    );
    parsed
}

/// `GetMetricStatistics` over one configuration's series of `metric`, across the whole span
/// around the requests: its datapoints, each `{Timestamp, Sum, Unit}`.
async fn datapoints(
    router: &MultiServiceRouter,
    bucket: &str,
    filter_id: &str,
    metric: &str,
    start: f64,
    end: f64,
) -> Vec<Value> {
    let answered = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [
                {"Name": BUCKET_NAME_DIMENSION, "Value": bucket},
                {"Name": FILTER_ID_DIMENSION, "Value": filter_id},
            ],
            "StartTime": start,
            "EndTime": end,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum", "SampleCount"],
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Sum` of every datapoint of a span, which is what the series accounts for over it.
fn total(points: &[Value]) -> f64 {
    points
        .iter()
        .filter_map(|point| point.get("Sum").and_then(Value::as_f64))
        .sum()
}

/// The unit a metric is published in: the byte counters in `Bytes`, every other series — a
/// count of requests — in `Count`.
fn expected_unit(metric: &str) -> &'static str {
    if metric == BYTES_DOWNLOADED || metric == BYTES_UPLOADED {
        BYTES_UNIT
    } else {
        COUNT_UNIT
    }
}

/// The `FilterId` dimension values `ListMetrics` indexes for `bucket` in the `AWS/S3`
/// namespace: which configurations a client discovering the bucket's series finds.
async fn indexed_filter_ids(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let listed = cloudwatch(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": BUCKET_NAME_DIMENSION, "Value": bucket}],
        }),
    )
    .await;
    let mut ids: Vec<String> = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .filter_map(|metric| metric.get("Dimensions").and_then(Value::as_array))
                .flatten()
                .filter(|dimension| {
                    dimension.get("Name").and_then(Value::as_str) == Some(FILTER_ID_DIMENSION)
                })
                .filter_map(|dimension| dimension.get("Value").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

/// Drive one of a row's calls through S3's own wire, requiring the status it must answer.
async fn drive(router: &MultiServiceRouter, bucket: &str, call: &Call) {
    let answer: Answer = match call {
        Call::Get { key, status } => {
            let read = get_object(router, bucket, key, "", &[]).await;
            assert_eq!(
                read.status, *status,
                "the read of {key:?} must answer {status}, got {} with body {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            return;
        }
        Call::Head { key } => send_bytes(router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await,
        Call::Put { key, body, tags } => {
            let headers: Vec<(&str, &str)> = if tags.is_empty() {
                Vec::new()
            } else {
                vec![("x-amz-tagging", tags)]
            };
            put_object(router, bucket, key, body.as_bytes(), &headers).await
        }
        Call::Delete { key } => {
            send_bytes(router, "DELETE", &format!("/{bucket}/{key}"), &[], b"").await
        }
        Call::List => send(router, "GET", &format!("/{bucket}?list-type=2"), &[], "").await,
    };
    assert!(
        (200..300).contains(&answer.status),
        "the call {call:?} must be served, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// RULE: every request this service serves on a bucket is counted into the request-metric
/// series of each of that bucket's metrics configurations whose filter selects it — including
/// a ZERO-valued error observation for a request that was served — and into no other
/// configuration's.
#[tokio::test]
async fn a_served_request_is_counted_under_every_configuration_whose_filter_selects_it() {
    for row in rows() {
        let case = row.case;
        let bucket = row.bucket;
        let router = emulator();
        let opened = now_secs();

        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );
        for (id, filter) in &row.configurations {
            let put = put_metrics(&router, bucket, id, filter.as_ref()).await;
            assert_eq!(
                put.status, 204,
                "case {case:?}: the configuration {id:?} must be stored, got {} with body {:?}",
                put.status,
                String::from_utf8_lossy(&put.body)
            );
        }
        for call in &row.calls {
            drive(&router, bucket, call).await;
        }

        let window = (opened - WINDOW_SECS, now_secs() + WINDOW_SECS);
        for (filter_id, metric, expected) in &row.expected {
            let points =
                datapoints(&router, bucket, filter_id, metric, window.0, window.1).await;
            match expected {
                None => assert!(
                    points.is_empty(),
                    "case {case:?}: {metric} under {filter_id:?} must have no datapoint at \
                     all, got {points:?}"
                ),
                Some(sum) => {
                    assert!(
                        !points.is_empty(),
                        "case {case:?}: {metric} under {filter_id:?} must carry a datapoint, \
                         got none"
                    );
                    let actual = total(&points);
                    assert!(
                        (actual - sum).abs() < f64::EPSILON,
                        "case {case:?}: {metric} under {filter_id:?} must sum to {sum}, got \
                         {actual} from {points:?}"
                    );
                    let unit = expected_unit(metric);
                    for point in &points {
                        assert_eq!(
                            point.get("Unit").and_then(Value::as_str),
                            Some(unit),
                            "case {case:?}: {metric} must be published in {unit}, got {point:?}"
                        );
                    }
                }
            }
        }

        // A client discovers those series through `ListMetrics`, dimensioned on the bucket:
        // every configuration a request fell under is indexed there, and one no request fell
        // under is not.
        let indexed = indexed_filter_ids(&router, bucket).await;
        for (id, _) in &row.configurations {
            let has_series = row
                .expected
                .iter()
                .any(|(filter_id, _, sum)| filter_id == id && sum.is_some());
            assert_eq!(
                indexed.contains(&(*id).to_string()),
                has_series,
                "case {case:?}: ListMetrics must index {id:?} == {has_series}, got {indexed:?}"
            );
        }
    }
}

/// RULE, the other half of "a bucket publishes what its client asked for": a bucket carrying
/// NO metrics configuration publishes no request metrics at all, however many requests it
/// serves — the series exist because a configuration asked for them.
#[tokio::test]
async fn a_bucket_with_no_metrics_configuration_publishes_no_series() {
    let router = emulator();
    let bucket = "request-metrics-unconfigured";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    put_object(&router, bucket, "a/one", b"a", &[]).await;
    get_object(&router, bucket, "a/one", "", &[]).await;
    get_object(&router, bucket, "a/absent", "", &[]).await;

    let listed = cloudwatch(
        &router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": BUCKET_NAME_DIMENSION, "Value": bucket}],
        }),
    )
    .await;
    let metrics = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        metrics.is_empty(),
        "a bucket carrying no metrics configuration must publish no {NAMESPACE} series, got \
         {metrics:?}"
    );
}

/// `PutMetricAlarm` over one configuration's `4xxErrors` series, as a client that watches a
/// prefix for client errors configures it.
async fn put_alarm(router: &MultiServiceRouter, name: &str, bucket: &str, filter_id: &str) {
    cloudwatch(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": NAMESPACE,
            "MetricName": FOUR_XX_ERRORS,
            "Dimensions": [
                {"Name": BUCKET_NAME_DIMENSION, "Value": bucket},
                {"Name": FILTER_ID_DIMENSION, "Value": filter_id},
            ],
            "Statistic": "Sum",
            "Period": ALARM_PERIOD_SECS,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanOrEqualToThreshold",
            "TreatMissingData": "notBreaching",
            "ActionsEnabled": true,
        }),
    )
    .await;
}

/// The `StateValue` `DescribeAlarms` reports for `name`.
async fn alarm_state(router: &MultiServiceRouter, name: &str) -> String {
    let described = cloudwatch(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["MetricAlarm"]}),
    )
    .await;
    described
        .pointer("/MetricAlarms/0/StateValue")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Poll `DescribeAlarms` until `name` reports `state`, the way a client waits for an
/// evaluation the clock owes, and report what it reported last.
async fn await_alarm_state(router: &MultiServiceRouter, name: &str, state: &str) -> String {
    let mut last = String::new();
    for _ in 0..ALARM_POLL_ATTEMPTS {
        last = alarm_state(router, name).await;
        if last == state {
            return last;
        }
        tokio::time::sleep(ALARM_POLL_INTERVAL).await;
    }
    last
}

/// RULE: an alarm over a bucket's published `4xxErrors` series is graded on the requests that
/// bucket really served — it settles in `OK` once the served requests have published their
/// zero-valued observations, and the alarm of the configuration whose prefix a failing read
/// fell under, and only that one, then moves to `ALARM`.
#[tokio::test]
async fn an_alarm_over_a_published_error_series_grades_the_requests_the_bucket_served() {
    let router = emulator();
    let bucket = "request-metrics-alarmed";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    for (id, prefix) in [("loud", "loud/"), ("quiet", "quiet/")] {
        let put = put_metrics(
            &router,
            bucket,
            id,
            Some(&Filter::Prefix(prefix.to_string())),
        )
        .await;
        assert_eq!(
            put.status, 204,
            "the configuration {id:?} must be stored, got {}",
            put.status
        );
    }

    // Both prefixes receive a successful read, so both configurations publish a `4xxErrors`
    // observation of 0 — which is what an alarm needs to leave INSUFFICIENT_DATA.
    for key in ["loud/present", "quiet/present"] {
        put_object(&router, bucket, key, b"present", &[]).await;
        let read = get_object(&router, bucket, key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "the read of {key:?} must be served, got {}",
            read.status
        );
    }
    put_alarm(&router, "loud-4xx", bucket, "loud").await;
    put_alarm(&router, "quiet-4xx", bucket, "quiet").await;

    for alarm in ["loud-4xx", "quiet-4xx"] {
        let reported = await_alarm_state(&router, alarm, ALARM_STATE_OK).await;
        assert_eq!(
            reported, ALARM_STATE_OK,
            "the alarm {alarm:?} must settle in {ALARM_STATE_OK} over the zero-valued \
             observations the served reads published, got {reported:?}"
        );
    }

    // A read of a key that is not there under the loud prefix: one 4xx of that prefix.
    let missing = get_object(&router, bucket, "loud/absent", "", &[]).await;
    assert_eq!(
        missing.status, 404,
        "the read of a key that is not there must answer 404, got {}",
        missing.status
    );
    let loud = await_alarm_state(&router, "loud-4xx", ALARM_STATE_ALARM).await;
    assert_eq!(
        loud, ALARM_STATE_ALARM,
        "the alarm over the prefix the failing read fell under must move to \
         {ALARM_STATE_ALARM}, got {loud:?}"
    );
    let quiet = alarm_state(&router, "quiet-4xx").await;
    assert_eq!(
        quiet, ALARM_STATE_OK,
        "the alarm over the other prefix must stay in {ALARM_STATE_OK}, got {quiet:?}"
    );
}
