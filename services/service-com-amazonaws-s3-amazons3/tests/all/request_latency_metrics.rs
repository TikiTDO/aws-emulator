// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two per-request LATENCIES a bucket's request metrics carry,
//! asserted where a client reads them: through CloudWatch's own `GetMetricStatistics` and
//! `ListMetrics`, with both services mounted on one shared engine exactly as `core/router`
//! assembles them.
//!
//! RULE under test: every request this service counts into a bucket's request metrics also
//! contributes ONE observation of each per-request latency S3 publishes — `FirstByteLatency`,
//! "the per-request time from the complete request being received by an Amazon S3 bucket to
//! when the response starts to be returned", and `TotalRequestLatency`, "the elapsed
//! per-request time from the first byte received to the last byte sent to an Amazon S3 bucket"
//! (Amazon S3 User Guide, *Amazon S3 CloudWatch request metrics*) — in `Milliseconds`,
//! dimensioned like every other series of that configuration, and the value of each is the span
//! this server MEASURED while serving that request, the first-byte span lying inside the
//! whole-request one.
//!
//! The table below spans what the rule generalises over: the request type, how many requests
//! the client makes, the status class of the answers, and whether the configuration counting
//! them is filtered — a latency observation accompanies the counters of exactly the requests
//! that configuration counts, whichever verb each one was.

use serde_json::{json, Value};
// The namespace, dimensions, metric names and unit under test, read from this service's own
// publication rule so the suite and the publisher cannot disagree about which series is meant.
use service_com_amazonaws_s3_amazons3::service::request_metrics::{
    ALL_REQUESTS, BUCKET_NAME_DIMENSION, FILTER_ID_DIMENSION, FIRST_BYTE_LATENCY,
    MILLISECONDS_UNIT, NAMESPACE, TOTAL_REQUEST_LATENCY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, send, send_bytes};
use super::bucket_metrics_configuration::{put_metrics, Filter};
use super::object_and_tagging_roundtrip::{get_object, put_object};
use super::request_metrics::{cloudwatch, emulator, now_secs, PERIOD_SECS, WINDOW_SECS};

/// The two series under test: both latencies, each published in the same unit.
const LATENCIES: [&str; 2] = [FIRST_BYTE_LATENCY, TOTAL_REQUEST_LATENCY];

/// The id of the one metrics configuration each row stores.
const FILTER_ID: &str = "latencies";

/// One request a row's client makes, and the status it must be answered with.
#[derive(Debug)]
enum Call {
    /// A write of an object.
    Put { key: &'static str, body: &'static str },
    /// A read of an object, served or refused as the key is there or not.
    Get { key: &'static str, status: u16 },
    /// A read of an object's metadata alone.
    Head { key: &'static str },
    /// A delete of an object.
    Delete { key: &'static str },
    /// A listing of the bucket's contents, which addresses no object.
    List,
}

/// One row: the configuration the bucket carries, the requests the client then makes, and how
/// many of them that configuration counts.
struct Row {
    case: &'static str,
    bucket: &'static str,
    filter: Option<Filter>,
    calls: Vec<Call>,
    counted: f64,
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one write and one read of the whole bucket",
            bucket: "request-latency-pair",
            filter: None,
            calls: vec![
                Call::Put {
                    key: "busy/one",
                    body: "hello",
                },
                Call::Get {
                    key: "busy/one",
                    status: 200,
                },
            ],
            counted: 2.0,
        },
        Row {
            case: "every verb the data plane serves",
            bucket: "request-latency-verbs",
            filter: None,
            calls: vec![
                Call::Put {
                    key: "verbs/one",
                    body: "body",
                },
                Call::Get {
                    key: "verbs/one",
                    status: 200,
                },
                Call::Head { key: "verbs/one" },
                Call::List,
                Call::Delete { key: "verbs/one" },
            ],
            counted: 5.0,
        },
        Row {
            case: "a prefix filter counts only the requests it selects",
            bucket: "request-latency-prefix",
            filter: Some(Filter::Prefix("busy/".to_string())),
            calls: vec![
                Call::Put {
                    key: "busy/one",
                    body: "loud",
                },
                Call::Get {
                    key: "busy/one",
                    status: 200,
                },
                Call::Put {
                    key: "quiet/one",
                    body: "soft",
                },
                Call::Get {
                    key: "quiet/one",
                    status: 200,
                },
                Call::List,
            ],
            counted: 2.0,
        },
        Row {
            case: "a read of a key that is not there is a served request too",
            bucket: "request-latency-refused",
            filter: None,
            calls: vec![Call::Get {
                key: "absent/one",
                status: 404,
            }],
            counted: 1.0,
        },
        Row {
            case: "many reads under one filter",
            bucket: "request-latency-many",
            filter: Some(Filter::Prefix("busy/".to_string())),
            calls: vec![
                Call::Put {
                    key: "busy/one",
                    body: "loud",
                },
                Call::Get {
                    key: "busy/one",
                    status: 200,
                },
                Call::Get {
                    key: "busy/one",
                    status: 200,
                },
                Call::Get {
                    key: "busy/one",
                    status: 200,
                },
                Call::Get {
                    key: "busy/absent",
                    status: 404,
                },
            ],
            counted: 5.0,
        },
    ]
}

/// Drive one of a row's calls through S3's own wire, requiring the status it must answer.
async fn drive(router: &MultiServiceRouter, bucket: &str, call: &Call) {
    let (status, body) = match call {
        Call::Put { key, body } => {
            let answer = put_object(router, bucket, key, body.as_bytes(), &[]).await;
            (answer.status, answer.body)
        }
        Call::Get { key, status } => {
            let answer = get_object(router, bucket, key, "", &[]).await;
            assert_eq!(
                answer.status, *status,
                "the read of {key:?} must answer {status}, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            return;
        }
        Call::Head { key } => {
            let answer =
                send_bytes(router, "HEAD", &format!("/{bucket}/{key}"), &[], b"").await;
            (answer.status, answer.body)
        }
        Call::Delete { key } => {
            let answer =
                send_bytes(router, "DELETE", &format!("/{bucket}/{key}"), &[], b"").await;
            (answer.status, answer.body)
        }
        Call::List => {
            let answer = send(router, "GET", &format!("/{bucket}?list-type=2"), &[], "").await;
            (answer.status, answer.body)
        }
    };
    assert!(
        (200..300).contains(&status),
        "the call {call:?} must be served, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `GetMetricStatistics` over one configuration's series of `metric`, across the whole span
/// around the requests: every statistic a client reads a latency through.
async fn statistics(
    router: &MultiServiceRouter,
    bucket: &str,
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
                {"Name": FILTER_ID_DIMENSION, "Value": FILTER_ID},
            ],
            "StartTime": start,
            "EndTime": end,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum", "SampleCount", "Minimum", "Maximum", "Average"],
        }),
    )
    .await;
    answered
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The total of one statistic over a span's datapoints.
fn total_of(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point.get(statistic).and_then(Value::as_f64))
        .sum()
}

/// The `AWS/S3` metric names `ListMetrics` indexes for `bucket`.
async fn indexed_metric_names(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let listed = cloudwatch(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{"Name": BUCKET_NAME_DIMENSION, "Value": bucket}],
        }),
    )
    .await;
    let mut names: Vec<String> = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .filter_map(|metric| metric.get("MetricName").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

/// RULE: each request a configuration counts contributes one observation of each latency, in
/// `Milliseconds`, measured on this server.
#[tokio::test]
async fn every_counted_request_observes_both_latencies_once() {
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
        let put = put_metrics(&router, bucket, FILTER_ID, row.filter.as_ref()).await;
        assert_eq!(
            put.status, 204,
            "case {case:?}: the configuration must be stored, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        for call in &row.calls {
            drive(&router, bucket, call).await;
        }

        let window = (opened - WINDOW_SECS, now_secs() + WINDOW_SECS);

        // The counters and the latencies account for the SAME requests: whatever the verbs
        // were, the configuration observed each counted request exactly once in each series.
        let counters = statistics(&router, bucket, ALL_REQUESTS, window.0, window.1).await;
        let counted = total_of(&counters, "Sum");
        assert!(
            (counted - row.counted).abs() < f64::EPSILON,
            "case {case:?}: {ALL_REQUESTS} must count {} request(s), got {counted} from \
             {counters:?}",
            row.counted
        );

        for metric in LATENCIES {
            let points = statistics(&router, bucket, metric, window.0, window.1).await;
            assert!(
                !points.is_empty(),
                "case {case:?}: {metric} must carry a datapoint for the {} counted \
                 request(s), got none",
                row.counted
            );
            let samples = total_of(&points, "SampleCount");
            assert!(
                (samples - row.counted).abs() < f64::EPSILON,
                "case {case:?}: {metric} must hold one observation per counted request \
                 ({}), got {samples} from {points:?}",
                row.counted
            );
            for point in &points {
                assert_eq!(
                    point.get("Unit").and_then(Value::as_str),
                    Some(MILLISECONDS_UNIT),
                    "case {case:?}: {metric} must be published in {MILLISECONDS_UNIT}, got \
                     {point:?}"
                );
                let minimum = point.get("Minimum").and_then(Value::as_f64);
                assert!(
                    minimum.is_some_and(|minimum| minimum >= 0.0),
                    "case {case:?}: {metric} is a measured span, so no observation of it is \
                     negative, got {minimum:?} in {point:?}"
                );
            }
        }

        // Each request's first-byte span lies inside its whole-request span — "the elapsed
        // per-request time from the first byte received to the last byte sent" includes it —
        // so the totals over the same requests are ordered the same way.
        let first_byte = statistics(&router, bucket, FIRST_BYTE_LATENCY, window.0, window.1).await;
        let whole =
            statistics(&router, bucket, TOTAL_REQUEST_LATENCY, window.0, window.1).await;
        let (to_first_byte, total) = (total_of(&first_byte, "Sum"), total_of(&whole, "Sum"));
        assert!(
            to_first_byte <= total,
            "case {case:?}: {FIRST_BYTE_LATENCY} ({to_first_byte} ms) cannot exceed \
             {TOTAL_REQUEST_LATENCY} ({total} ms) over the same requests"
        );

        // A client discovers both series through `ListMetrics`, dimensioned on the bucket.
        let indexed = indexed_metric_names(&router, bucket).await;
        for metric in LATENCIES {
            assert!(
                indexed.contains(&metric.to_string()),
                "case {case:?}: ListMetrics must index {metric}, got {indexed:?}"
            );
        }
    }
}

/// RULE, the other half: the latencies exist because a configuration asked for the bucket's
/// request metrics, so a bucket carrying none publishes neither series however many requests
/// it serves.
#[tokio::test]
async fn a_bucket_with_no_metrics_configuration_publishes_no_latency() {
    let router = emulator();
    let bucket = "request-latency-unconfigured";
    let opened = now_secs();
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {}",
        created.status
    );
    put_object(&router, bucket, "a/one", b"a", &[]).await;
    get_object(&router, bucket, "a/one", "", &[]).await;

    let window = (opened - WINDOW_SECS, now_secs() + WINDOW_SECS);
    for metric in LATENCIES {
        let points = statistics(&router, bucket, metric, window.0, window.1).await;
        assert!(
            points.is_empty(),
            "an unconfigured bucket must publish no {metric} datapoint, got {points:?}"
        );
    }
    let indexed = indexed_metric_names(&router, bucket).await;
    assert!(
        indexed.is_empty(),
        "an unconfigured bucket must index no AWS/S3 metric at all, got {indexed:?}"
    );
}
