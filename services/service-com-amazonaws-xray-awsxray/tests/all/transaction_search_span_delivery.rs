// SPDX-License-Identifier: CC0-1.0

//! Transaction Search: WHERE an accepted span goes, end to end over each
//! service's own wire protocol.
//!
//! X-Ray and CloudWatch Logs are mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so a delivered span lands in the
//! store CloudWatch Logs' OWN readers answer from and `FilterLogEvents` is what
//! observes it.
//!
//! The rules under test:
//!
//! 1. the account's trace segment destination selects the store an ingested span
//!    is written to, not merely a reported value: settled on `CloudWatchLogs`,
//!    every accepted span is also written into the log group that destination
//!    vends for spans, so a `filterPattern` naming a value the span document
//!    carries finds it there — and the group is created, because X-Ray is what
//!    vends it;
//! 2. the delivered event is the span, with nothing added: its message is the
//!    submitted document byte for byte and its instant is the span's own
//!    `start_time`;
//! 3. a document the ingest did NOT accept is delivered nowhere;
//! 4. settled on `XRay`, nothing is delivered — the spans stay readable through
//!    this service's own trace reads only — and a destination switched back to
//!    `XRay` stops delivering while leaving what was already delivered alone.
//!
//! The rows differ along what the rules generalise over: how many documents one
//! batch carries, where in the document the queried value sits (an annotation, a
//! nested annotation, the segment name), whether the batch also carries a
//! document the ingest refuses, and the destination in effect.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_xray_awsxray::state::span_delivery::{
    SPANS_LOG_GROUP, SPANS_LOG_STREAM,
};
use service_com_amazonaws_xray_awsxray::state::{
    CLOUD_WATCH_LOGS_DESTINATION, DEFAULT_TRACE_SEGMENT_DESTINATION, DESTINATION_STATUS_ACTIVE,
};

/// The Region every call below lands in.
const REGION: &str = "us-east-1";

/// How long the switch to a destination is waited for before the case gives up.
///
/// Generous on purpose: the switch is due at a fixed instant after the call that
/// asked for it, and the condition — not the clock — is what each wait below is
/// bounded by.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often the settled destination is looked at again while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// X-Ray and CloudWatch Logs over ONE engine, addressed through the production
/// router.
struct Spans {
    router: MultiServiceRouter,
}

impl Spans {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// One X-Ray call: restJson1, addressed by path.
    async fn xray(&self, path: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://xray.{REGION}.amazonaws.com{path}"))
            .header("host", format!("xray.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization("xray"))
            .body(Bytes::from(body.to_string()))
            .expect("the request builds");
        self.dispatch(request).await
    }

    /// One CloudWatch Logs call: awsJson1_1, addressed by `x-amz-target`.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://logs.{REGION}.amazonaws.com/"))
            .header("host", format!("logs.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header("authorization", authorization("logs"))
            .body(Bytes::from(body.to_string()))
            .expect("the request builds");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: ::http::Request<Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, body)
    }
}

/// A credential-scope header for `signing_name`: these cases exercise handlers
/// rather than SigV4, but the scope is where a handler reads its Region from.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20240101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=not-verified"
    )
}

/// Now, in seconds of epoch time.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// A 35-character X-Ray trace id, distinct per `n`.
fn trace_id(n: u32) -> String {
    format!("1-{n:08x}-{n:024x}")
}

/// Configure `destination` and look again until the switch the clock owes it is
/// in effect.
async fn settled(case: &str, spans: &Spans, destination: &str) {
    let (status, answered) = spans
        .xray(
            "/UpdateTraceSegmentDestination",
            &json!({ "Destination": destination }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: configuring {destination:?} must be accepted, got {status} {answered:?}"
    );
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, read) = spans.xray("/GetTraceSegmentDestination", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the destination must be readable, got {status} {read:?}"
        );
        let settled = (
            read.get("Destination").and_then(Value::as_str),
            read.get("Status").and_then(Value::as_str),
        );
        if settled == (Some(destination), Some(DESTINATION_STATUS_ACTIVE)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the switch to {destination:?} never took effect; the last read was \
             {read:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every event of the spans log group whose message carries `marker`, as the
/// group's own reader answers it.
async fn spans_carrying(spans: &Spans, marker: &str, from: f64) -> (u16, Vec<Value>) {
    #[allow(clippy::cast_possible_truncation)]
    let start_time = ((from - 60.0) * 1_000.0) as i64;
    let (status, answered) = spans
        .logs(
            "FilterLogEvents",
            &json!({
                "logGroupName": SPANS_LOG_GROUP,
                "filterPattern": format!("\"{marker}\""),
                "startTime": start_time,
            }),
        )
        .await;
    let events = answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (status, events)
}

/// One root span document carrying `annotations`.
fn span_document(id: &str, trace: &str, name: &str, at: f64, annotations: Value) -> Value {
    json!({
        "id": id,
        "trace_id": trace,
        "name": name,
        "start_time": at,
        "end_time": at + 0.05,
        "annotations": annotations,
    })
}

/// One row of the delivery table.
struct Delivery {
    case: &'static str,
    /// The documents one `PutTraceSegments` call carries.
    documents: Vec<String>,
    /// How many of them the ingest accepts.
    accepted: usize,
    /// The value queried for, and how many delivered events must carry it.
    queried: Vec<(String, usize)>,
}

fn delivery_rows(at: f64) -> Vec<Delivery> {
    let one = trace_id(21);
    let many = trace_id(22);
    let nested = trace_id(23);
    let named = trace_id(24);
    let mixed = trace_id(25);
    vec![
        Delivery {
            case: "one span carrying an annotation",
            documents: vec![span_document(
                "2100000000000001",
                &one,
                "cwl-one",
                at,
                json!({ "marker": "probe-cwl-single" }),
            )
            .to_string()],
            accepted: 1,
            queried: vec![("probe-cwl-single".to_string(), 1)],
        },
        Delivery {
            case: "three spans in one batch, each its own annotation",
            documents: (0..3)
                .map(|index| {
                    span_document(
                        &format!("220000000000000{index}"),
                        &many,
                        &format!("cwl-many-{index}"),
                        at,
                        json!({ "marker": format!("probe-cwl-batch-{index}") }),
                    )
                    .to_string()
                })
                .collect(),
            accepted: 3,
            queried: (0..3)
                .map(|index| (format!("probe-cwl-batch-{index}"), 1))
                .collect(),
        },
        Delivery {
            case: "a span whose queried value sits in a nested annotation",
            documents: vec![span_document(
                "2300000000000001",
                &nested,
                "cwl-nested",
                at,
                json!({ "release": { "build": "probe-cwl-nested" } }),
            )
            .to_string()],
            accepted: 1,
            queried: vec![("probe-cwl-nested".to_string(), 1)],
        },
        Delivery {
            case: "a span whose queried value is its own service name",
            documents: vec![span_document(
                "2400000000000001",
                &named,
                "probe-cwl-by-name",
                at,
                json!({ "marker": "unqueried" }),
            )
            .to_string()],
            accepted: 1,
            queried: vec![("probe-cwl-by-name".to_string(), 1)],
        },
        Delivery {
            case: "a batch carrying one accepted span and one document the ingest refuses",
            documents: vec![
                span_document(
                    "2500000000000001",
                    &mixed,
                    "cwl-mixed",
                    at,
                    json!({ "marker": "probe-cwl-accepted" }),
                )
                .to_string(),
                // Not a segment document at all, so the ingest refuses it and
                // nothing of it can be delivered.
                "{ this is not json probe-cwl-refused".to_string(),
            ],
            accepted: 1,
            queried: vec![
                ("probe-cwl-accepted".to_string(), 1),
                ("probe-cwl-refused".to_string(), 0),
            ],
        },
    ]
}

#[tokio::test]
async fn an_accepted_span_is_delivered_to_the_spans_log_group_for_the_cloud_watch_logs_destination()
{
    let at = now();
    for row in delivery_rows(at) {
        let case = row.case;
        let spans = Spans::new();
        settled(case, &spans, CLOUD_WATCH_LOGS_DESTINATION).await;

        let (status, answered) = spans
            .xray(
                "/TraceSegments",
                &json!({ "TraceSegmentDocuments": row.documents }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutTraceSegments must answer 200, got {status} {answered:?}"
        );
        let unprocessed = answered
            .get("UnprocessedTraceSegments")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(
            row.documents.len() - unprocessed,
            row.accepted,
            "case {case:?}: expected {} of {} documents accepted, got {answered:?}",
            row.accepted,
            row.documents.len()
        );

        for (marker, expected) in &row.queried {
            let (status, events) = spans_carrying(&spans, marker, at).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the spans log group must be readable for {marker:?}, got \
                 {status}"
            );
            assert_eq!(
                events.len(),
                *expected,
                "case {case:?}: expected {expected} delivered span(s) carrying {marker:?}, got \
                 {events:?}"
            );
            for event in &events {
                let message = event.get("message").and_then(Value::as_str).unwrap_or("");
                assert!(
                    message.contains(marker.as_str()),
                    "case {case:?}: a delivered event must carry {marker:?}, got {event:?}"
                );
                assert!(
                    row.documents.iter().any(|document| document == message),
                    "case {case:?}: a delivered event's message must be the submitted document \
                     byte for byte, got {event:?}"
                );
                assert_eq!(
                    event.get("logStreamName").and_then(Value::as_str),
                    Some(SPANS_LOG_STREAM),
                    "case {case:?}: a span is delivered to the spans stream, got {event:?}"
                );
                // The instant is the span's own `start_time`, in the epoch
                // milliseconds a log event carries.
                #[allow(clippy::cast_possible_truncation)]
                let expected_ms = (at * 1_000.0) as i64;
                assert_eq!(
                    event.get("timestamp").and_then(Value::as_i64),
                    Some(expected_ms),
                    "case {case:?}: a delivered event carries the span's own start_time, got \
                     {event:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_delivered_span_is_still_readable_through_the_trace_store() {
    // Delivering a span to CloudWatch Logs does not move it: the same document
    // is still what `BatchGetTraces` returns.
    let at = now();
    let trace = trace_id(31);
    let document = span_document(
        "3100000000000001",
        &trace,
        "cwl-round-trip",
        at,
        json!({ "marker": "probe-cwl-round-trip" }),
    )
    .to_string();

    let spans = Spans::new();
    settled("round trip", &spans, CLOUD_WATCH_LOGS_DESTINATION).await;
    let (status, answered) = spans
        .xray(
            "/TraceSegments",
            &json!({ "TraceSegmentDocuments": [document.clone()] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the span must be accepted, got {status} {answered:?}"
    );

    let (status, events) = spans_carrying(&spans, "probe-cwl-round-trip", at).await;
    assert_eq!((status, events.len()), (200, 1), "expected the span delivered once, got {events:?}");

    let (status, read) = spans
        .xray("/Traces", &json!({ "TraceIds": [trace.clone()] }))
        .await;
    assert_eq!(status, 200, "the trace must be readable, got {read:?}");
    let documents: Vec<String> = read
        .get("Traces")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .flat_map(|trace| {
            trace
                .get("Segments")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|segment| {
            segment
                .get("Document")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        documents.contains(&document),
        "the delivered span must still be stored verbatim, got {documents:?}"
    );
}

#[tokio::test]
async fn nothing_is_delivered_while_the_destination_is_x_ray() {
    let at = now();
    let spans = Spans::new();
    let trace = trace_id(41);
    let document = span_document(
        "4100000000000001",
        &trace,
        "xray-destination",
        at,
        json!({ "marker": "probe-xray-destination" }),
    )
    .to_string();
    let (status, answered) = spans
        .xray(
            "/TraceSegments",
            &json!({ "TraceSegmentDocuments": [document] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the span must be accepted, got {status} {answered:?}"
    );

    // The log group is X-Ray's to vend, so with the account's spans going
    // nowhere near CloudWatch Logs the group does not exist and its own reader
    // says so.
    let (status, refused) = spans
        .logs(
            "FilterLogEvents",
            &json!({ "logGroupName": SPANS_LOG_GROUP }),
        )
        .await;
    let code = refused
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        status, 400,
        "case the account default destination: expected the spans log group to be absent, got \
         {status} {refused:?}"
    );
    assert!(
        code.contains("ResourceNotFoundException"),
        "case the account default destination: expected ResourceNotFoundException for the \
         absent spans log group, got {code:?}"
    );
}

#[tokio::test]
async fn a_destination_switched_back_to_x_ray_stops_delivering() {
    // Two spans, one submitted under each destination: the first is delivered,
    // the second is not, and switching back leaves the first where it is.
    let at = now();
    let spans = Spans::new();
    let trace = trace_id(51);

    settled("while CloudWatchLogs", &spans, CLOUD_WATCH_LOGS_DESTINATION).await;
    let delivered = span_document(
        "5100000000000001",
        &trace,
        "cwl-before-switch",
        at,
        json!({ "marker": "probe-before-switch" }),
    )
    .to_string();
    let (status, answered) = spans
        .xray(
            "/TraceSegments",
            &json!({ "TraceSegmentDocuments": [delivered] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the first span must be accepted, got {status} {answered:?}"
    );

    settled("back to XRay", &spans, DEFAULT_TRACE_SEGMENT_DESTINATION).await;
    let withheld = span_document(
        "5100000000000002",
        &trace,
        "xray-after-switch",
        at,
        json!({ "marker": "probe-after-switch" }),
    )
    .to_string();
    let (status, answered) = spans
        .xray(
            "/TraceSegments",
            &json!({ "TraceSegmentDocuments": [withheld] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the second span must be accepted, got {status} {answered:?}"
    );

    for (case, marker, expected) in [
        ("submitted while CloudWatchLogs was in effect", "probe-before-switch", 1),
        ("submitted after the switch back", "probe-after-switch", 0),
    ] {
        let (status, events) = spans_carrying(&spans, marker, at).await;
        assert_eq!(
            (status, events.len()),
            (200, expected),
            "case {case:?}: expected {expected} delivered span(s) carrying {marker:?}, got \
             {status} with {events:?}"
        );
    }
}

