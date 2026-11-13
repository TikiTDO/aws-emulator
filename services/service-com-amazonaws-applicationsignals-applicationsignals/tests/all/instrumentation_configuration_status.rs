// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ReportInstrumentationConfigurationStatus` and the status read it
//! writes for, `GetInstrumentationConfigurationStatus`, over HTTP.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the routes, the status codes and the JSON bodies are the ones a client sees.
//!
//! The rules under test:
//!
//! * a reported status event is recorded against the configuration STORED under the identity
//!   the report names — service and environment from the request, instrumentation type,
//!   signal type and location hash from the report — and a later status read of that
//!   configuration reports it back;
//! * a report that names no stored configuration records nothing and comes back in
//!   `UnprocessedStatusEvents`, while the rest of the batch is still recorded;
//! * the operation is `@idempotent`: re-reporting one event records one event;
//! * an event is held at the MINUTE its report named, so the instant a read reports is a
//!   whole minute (the rounding itself is tabled in `instrumentation_status_minute`);
//! * a status read reports the events of ONE status (the requested one, else `ACTIVE`) that
//!   fall in the requested window (else the last hour), most recent first, a page at a time;
//! * a status read of an identity that names no stored configuration is the modelled
//!   `ResourceNotFoundException` (404), and a read never changes what a later caller sees;
//! * a configuration's history goes when the configuration goes — whether it was deleted or
//!   its `ExpiresAt` fell due, the one write this read materialises.
//!
//! The rows of each table differ along what the rules generalise over: the whole modelled
//! `InstrumentationConfigurationStatus` and `InstrumentationErrorCause` catalogues, both
//! `InstrumentationType` values, the form of the `LocationIdentifier`, the size of the batch,
//! the window bounds and the page size.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    instrumentation_key, ComAmazonawsApplicationsignalsApplicationsignalsState,
    InstrumentationScope, MAX_STATUS_EVENTS_PER_CONFIGURATION,
};
use smithy_server_runtime::MultiServiceRouter;

// The two fixed sentences an instrumentation-configuration refusal states, written once for
// this crate's tests in the suite that asserts the rule choosing between them.
use crate::refusal_wording::{CONFIGURATION_ABSENT, CONFIGURATION_DELETED};

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
pub(crate) const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
pub(crate) const BREAKPOINT: &str = "BREAKPOINT";
pub(crate) const PROBE: &str = "PROBE";

/// The whole modelled `InstrumentationConfigurationStatus` catalogue
/// (`smithy_query(query="shape",shape="InstrumentationConfigurationStatus")` -> `["READY",
/// "ERROR", "ACTIVE", "DISABLED"]`).
pub(crate) const STATUSES: [&str; 4] = ["READY", "ERROR", "ACTIVE", "DISABLED"];

/// The WIRE spelling of one `InstrumentationConfigurationStatus`: the token a response states
/// a status with, where a request names it by the modelled name in [`STATUSES`].
///
/// One row per modelled value, each the name capitalised — the spelling the recorded
/// responses of `GetInstrumentationConfigurationStatus` carry.
pub(crate) fn status_wire_token(status: &str) -> &'static str {
    match status {
        "READY" => "Ready",
        "ERROR" => "Error",
        "ACTIVE" => "Active",
        "DISABLED" => "Disabled",
        other => panic!("case {other:?}: not a modelled InstrumentationConfigurationStatus"),
    }
}

/// The whole modelled `InstrumentationErrorCause` catalogue
/// (`smithy_query(query="shape",shape="InstrumentationErrorCause")` -> `["FILE_NOT_FOUND",
/// "METHOD_NOT_FOUND", "LINE_NOT_EXECUTABLE", "OVERLOADED_METHODS", "LANGUAGE_MISMATCH",
/// "RUNTIME_ERROR"]`).
pub(crate) const ERROR_CAUSES: [&str; 6] = [
    "FILE_NOT_FOUND",
    "METHOD_NOT_FOUND",
    "LINE_NOT_EXECUTABLE",
    "OVERLOADED_METHODS",
    "LANGUAGE_MISMATCH",
    "RUNTIME_ERROR",
];

/// The `UnprocessedStatusEventFailureReason` a report of an identity nothing is stored under
/// earns (`smithy_query(query="shape",shape="UnprocessedStatusEventFailureReason")` ->
/// `["THROTTLED", "INTERNAL_ERROR", "VALIDATION_ERROR"]`).
const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

/// A location hash the create path never mints for the locations below, at the modelled
/// `length 16..16`.
const UNKNOWN_HASH: &str = "0123456789abcdef";

/// A router with just this service mounted on a fresh engine, together with the engine the
/// handlers share so a test can read the store the write actually reached.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// [`router_with_engine`] when the test only drives the wire surface.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
pub(crate) async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = serde_json::to_vec(body).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `POST /create-instrumentation-configuration`.
async fn create(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "/create-instrumentation-configuration", request).await
}

/// `POST /delete-instrumentation-configuration`.
async fn delete(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "/delete-instrumentation-configuration", request).await
}

/// `POST /report-instrumentation-configuration-status`.
pub(crate) async fn report(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(
        router,
        "/report-instrumentation-configuration-status",
        request,
    )
    .await
}

/// `POST /get-instrumentation-configuration-status`.
pub(crate) async fn get_status(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "/get-instrumentation-configuration-status", request).await
}

/// The whole MINUTE at or before now, in the epoch seconds `restJson1` puts a `Timestamp` on
/// the wire as.
///
/// A minute boundary rather than the current second, because an event is held at the minute
/// nearest the instant its report named (see [`nearest_minute`]): an instant this answer plus
/// or minus whole minutes is then a recorded instant of its own, whichever second of the
/// minute the suite happens to run in.
pub(crate) fn now_minute() -> f64 {
    let seconds = DateTime::from(std::time::SystemTime::now())
        .as_secs_f64()
        .trunc();
    (seconds / 60.0).floor() * 60.0
}

/// The instant a read reports for an event whose report named `time`.
///
/// `InstrumentationStatusEvent.Time` is "The time when the status was reported, rounded to the
/// nearest minute" (`smithy_query(query="shape",shape="InstrumentationStatusEvent")`), so it is
/// the whole minute nearest `time`, half a minute going to the later boundary.
pub(crate) fn nearest_minute(time: f64) -> f64 {
    ((time + 30.0) / 60.0).floor() * 60.0
}

/// A `CaptureConfiguration` with only its `@required` members, whose own members are all
/// optional (`smithy_query(query="shape",shape="CodeCaptureConfiguration,CaptureLimitsConfig")`).
fn minimal_capture() -> Value {
    json!({ "CodeCapture": { "CaptureLimits": {} } })
}

/// A create request for one configuration of `instrumentation_type` at `file_path`.
pub(crate) fn create_request(
    service: &str,
    environment: &str,
    kind: &str,
    file_path: &str,
) -> Value {
    json!({
        "Service": service,
        "Environment": environment,
        "SignalType": SNAPSHOT,
        "InstrumentationType": kind,
        "Location": { "CodeLocation": {
            "Language": "Java",
            "ClassName": "Checkout",
            "FilePath": file_path,
            "LineNumber": 42
        }},
        "CaptureConfiguration": minimal_capture()
    })
}

/// Create one configuration and answer its `LocationHash`.
pub(crate) async fn create_configuration(
    router: &MultiServiceRouter,
    request: &Value,
) -> String {
    let (status, created) = create(router, request).await;
    assert_eq!(
        status, 200,
        "expected the setup create to succeed, got {status} with {created}"
    );
    created["LocationHash"]
        .as_str()
        .expect("the create answers the stored location hash")
        .to_string()
}

/// One entry of a report batch.
pub(crate) fn report_entry(
    kind: &str,
    hash: &str,
    status: &str,
    time: f64,
    error_cause: Option<&str>,
) -> Value {
    let mut entry = json!({
        "InstrumentationType": kind,
        "SignalType": SNAPSHOT,
        "LocationHash": hash,
        "Status": status,
        "Time": time,
    });
    if let Some(cause) = error_cause {
        entry["ErrorCause"] = json!(cause);
    }
    entry
}

/// What a status read asks for beyond the configuration it names: the rows of the tables
/// below differ in these members, so they travel together.
#[derive(Debug, Clone, Default)]
pub(crate) struct Query<'a> {
    pub(crate) status: Option<&'a str>,
    pub(crate) window: Option<(f64, f64)>,
    pub(crate) max_results: Option<i64>,
    pub(crate) next_token: Option<&'a str>,
}

/// A status read of the configuration `identifier` names, asking what `query` asks.
pub(crate) fn status_request(
    service: &str,
    environment: &str,
    kind: &str,
    identifier: Value,
    query: &Query<'_>,
) -> Value {
    let mut request = json!({
        "Service": service,
        "Environment": environment,
        "SignalType": SNAPSHOT,
        "InstrumentationType": kind,
        "LocationIdentifier": identifier,
    });
    if let Some(status) = query.status {
        request["Status"] = json!(status);
    }
    if let Some((start, end)) = query.window {
        request["StartTime"] = json!(start);
        request["EndTime"] = json!(end);
    }
    if let Some(max_results) = query.max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = query.next_token {
        request["NextToken"] = json!(token);
    }
    request
}

/// The `Time` and `ErrorCause` of each event a read reported, in the order it reported them.
///
/// `Time` is read as a number rather than compared as JSON: an epoch-second timestamp is on
/// the wire as a number whose JSON spelling (`1.0` against `1`) is not part of the value.
pub(crate) fn events_of(body: &Value) -> Vec<(f64, Option<String>)> {
    body["Events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    (
                        event["Time"].as_f64().unwrap_or(f64::NAN),
                        event["ErrorCause"].as_str().map(ToString::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `Time` of each event a read reported, in the order it reported them.
pub(crate) fn event_times(body: &Value) -> Vec<f64> {
    events_of(body).into_iter().map(|(time, _)| time).collect()
}

/// A `NextToken` carrying `payload` under the tag of `token`.
///
/// A token is opaque but NOT authenticated — it is the hex of a tag and a cursor — so this is
/// what a caller does to forge one: keep the tag of a token it was issued and write its own
/// payload inside it. The tag of a status-event token ends after its third separator.
fn retagged_token(token: &str, payload: &str) -> String {
    let decoded =
        String::from_utf8(hex::decode(token).expect("a token is hex")).expect("a token is UTF-8");
    let boundary = decoded
        .match_indices('/')
        .nth(2)
        .map(|(index, _)| index + 1)
        .expect("the tag of a status-event token carries three separators");
    hex::encode(format!("{}{payload}", &decoded[..boundary]))
}

/// A reported event is recorded against the configuration the report names, and the status
/// read of that configuration reports it back — for every status the model declares, every
/// error cause, both instrumentation types and both forms of `LocationIdentifier`.
#[tokio::test]
async fn a_reported_event_is_read_back_for_every_status_and_error_cause() {
    let service = "checkout";
    let environment = "eks:cluster-prod/namespace";
    let now = now_minute();

    // One row per status; the ERROR rows walk the whole error-cause catalogue, and each row
    // is reported at its own instant inside the default window.
    let mut cases: Vec<(&str, Option<&str>, f64)> = Vec::new();
    for (index, status) in STATUSES.iter().enumerate() {
        cases.push((status, None, now - 60.0 * (index as f64 + 1.0)));
    }
    for (index, cause) in ERROR_CAUSES.iter().enumerate() {
        cases.push(("ERROR", Some(cause), now - 600.0 - index as f64));
    }

    for kind in [BREAKPOINT, PROBE] {
        for (status, cause, time) in cases.clone() {
            let label = format!("{kind} {status} cause {cause:?} at {time}");
            let router = router();
            let request = create_request(service, environment, kind, "src/main/java/App.java");
            let hash = create_configuration(&router, &request).await;

            let (code, reported) = report(
                &router,
                &json!({
                    "Service": service,
                    "Environment": environment,
                    "Configurations": [report_entry(kind, &hash, status, time, cause)],
                }),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {label:?}: expected HTTP 200 from the report, got {code} with {reported}"
            );
            assert_eq!(
                reported["Service"].as_str(),
                Some(service),
                "case {label:?}: expected the service echoed, got {reported}"
            );
            assert_eq!(
                reported["Environment"].as_str(),
                Some(environment),
                "case {label:?}: expected the environment echoed, got {reported}"
            );
            assert_eq!(
                reported["UnprocessedStatusEvents"],
                json!([]),
                "case {label:?}: expected every report of a stored configuration to be processed, got {reported}"
            );

            // The read names the SAME status, by the code location rather than the hash, so
            // both forms of `LocationIdentifier` reach the record the report wrote to.
            for (form, identifier) in [
                ("location hash", json!({ "LocationHash": hash.clone() })),
                (
                    "code location",
                    json!({ "CodeLocation": request["Location"]["CodeLocation"] }),
                ),
            ] {
                // Twice, because a read must not change what a later caller sees.
                for attempt in 1..=2 {
                    let (code, body) = get_status(
                        &router,
                        &status_request(
                            service,
                            environment,
                            kind,
                            identifier.clone(),
                            &Query {
                                status: Some(status),
                                ..Query::default()
                            },
                        ),
                    )
                    .await;
                    assert_eq!(
                        code, 200,
                        "case {label:?} by {form} attempt {attempt}: expected HTTP 200 from the read, got {code} with {body}"
                    );
                    assert_eq!(
                        body["Status"].as_str(),
                        Some(status_wire_token(status)),
                        "case {label:?} by {form} attempt {attempt}: expected the queried status in its wire spelling, got {body}"
                    );
                    assert_eq!(
                        body["SignalType"].as_str(),
                        Some(SNAPSHOT),
                        "case {label:?} by {form} attempt {attempt}: expected the signal type echoed, got {body}"
                    );
                    assert_eq!(
                        body["Location"], request["Location"],
                        "case {label:?} by {form} attempt {attempt}: expected the stored location, got {:?}",
                        body["Location"]
                    );
                    assert_eq!(
                        events_of(&body),
                        vec![(nearest_minute(time), cause.map(ToString::to_string))],
                        "case {label:?} by {form} attempt {attempt}: expected the reported event at the minute nearest {time}, got {:?}",
                        body["Events"]
                    );
                    assert!(
                        body["NextToken"].is_null(),
                        "case {label:?} by {form} attempt {attempt}: expected no NextToken on a single-event page, got {:?}",
                        body["NextToken"]
                    );
                }
            }

            // A read of another status reports nothing: the events are per status.
            for other in STATUSES.iter().filter(|other| **other != status) {
                let (code, body) = get_status(
                    &router,
                    &status_request(
                        service,
                        environment,
                        kind,
                        json!({ "LocationHash": hash.clone() }),
                        &Query {
                            status: Some(other),
                            ..Query::default()
                        },
                    ),
                )
                .await;
                assert_eq!(
                    code, 200,
                    "case {label:?} read as {other}: expected HTTP 200, got {code} with {body}"
                );
                assert_eq!(
                    body["Events"],
                    json!([]),
                    "case {label:?} read as {other}: expected no event of another status, got {:?}",
                    body["Events"]
                );
            }
        }
    }
}

/// A batch spanning several configurations records every report whose identity names a stored
/// configuration, and answers the rest in `UnprocessedStatusEvents` — whatever the batch's
/// size and whichever part of the identity is wrong.
#[tokio::test]
async fn reports_of_unknown_identities_are_unprocessed_and_the_rest_are_recorded() {
    let service = "checkout";
    let environment = "ec2:production";
    let now = now_minute();
    let router = router();

    // Two stored configurations, one per instrumentation type, at different locations.
    let breakpoint_request = create_request(service, environment, BREAKPOINT, "src/A.java");
    let probe_request = create_request(service, environment, PROBE, "src/B.java");
    let breakpoint_hash = create_configuration(&router, &breakpoint_request).await;
    let probe_hash = create_configuration(&router, &probe_request).await;

    // The rows differ in WHICH part of the identity names nothing, and the batch mixes them
    // with reports that do name a stored configuration.
    let unknown = vec![
        (
            "a location hash nothing is stored under",
            report_entry(BREAKPOINT, UNKNOWN_HASH, "ACTIVE", now - 10.0, None),
        ),
        (
            "the other instrumentation type at a stored location",
            report_entry(PROBE, &breakpoint_hash, "READY", now - 11.0, None),
        ),
        (
            "an error event at an unknown location",
            report_entry(
                PROBE,
                UNKNOWN_HASH,
                "ERROR",
                now - 12.0,
                Some("RUNTIME_ERROR"),
            ),
        ),
    ];

    let mut configurations = vec![
        report_entry(BREAKPOINT, &breakpoint_hash, "ACTIVE", now - 5.0, None),
        report_entry(PROBE, &probe_hash, "DISABLED", now - 6.0, None),
    ];
    configurations.extend(unknown.iter().map(|(_, entry)| entry.clone()));

    let (code, body) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": configurations,
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from a mixed batch, got {code} with {body}"
    );

    let unprocessed = body["UnprocessedStatusEvents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        unprocessed.len(),
        unknown.len(),
        "expected one unprocessed event per unknown identity, got {body}"
    );
    for (why, entry) in &unknown {
        let found = unprocessed.iter().find(|event| {
            event["LocationHash"] == entry["LocationHash"]
                && event["InstrumentationType"] == entry["InstrumentationType"]
                && event["Status"] == entry["Status"]
                && event["Time"].as_f64() == entry["Time"].as_f64()
        });
        let Some(found) = found else {
            panic!("case {why:?}: expected {entry} to be reported unprocessed, got {body}");
        };
        assert_eq!(
            found["FailedReason"].as_str(),
            Some(VALIDATION_ERROR),
            "case {why:?}: expected the validation failure reason, got {found}"
        );
        assert_eq!(
            found["SignalType"].as_str(),
            Some(SNAPSHOT),
            "case {why:?}: expected the signal type of the report, got {found}"
        );
    }

    // The two reports that named stored configurations were recorded, and the unknown ones
    // created nothing.
    for (label, kind, hash, status, time) in [
        (
            "the BREAKPOINT configuration",
            BREAKPOINT,
            &breakpoint_hash,
            "ACTIVE",
            now - 5.0,
        ),
        (
            "the PROBE configuration",
            PROBE,
            &probe_hash,
            "DISABLED",
            now - 6.0,
        ),
    ] {
        let (code, body) = get_status(
            &router,
            &status_request(
                service,
                environment,
                kind,
                json!({ "LocationHash": hash }),
                &Query {
                    status: Some(status),
                    ..Query::default()
                },
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {label:?}: expected HTTP 200 from the read, got {code} with {body}"
        );
        assert_eq!(
            event_times(&body),
            vec![nearest_minute(time)],
            "case {label:?}: expected the recorded event at the minute nearest {time}, got {:?}",
            body["Events"]
        );
    }

    // The unknown identities are still unknown: nothing was minted for them.
    let (code, error) = get_status(
        &router,
        &status_request(
            service,
            environment,
            BREAKPOINT,
            json!({ "LocationHash": UNKNOWN_HASH }),
            &Query {
                status: Some("ACTIVE"),
                ..Query::default()
            },
        ),
    )
    .await;
    assert_eq!(
        code, 404,
        "expected a read of an unknown identity to be 404, got {code} with {error}"
    );
    assert_eq!(
        error["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "expected a ResourceNotFoundException, got {error}"
    );
    assert_eq!(
        error["Message"].as_str(),
        Some(CONFIGURATION_ABSENT),
        "expected the sentence for a configuration that never existed, got {error}"
    );
}

/// Re-reporting the same event records the SAME event rather than a second one (the operation
/// is `@idempotent`), while a later `ErrorCause` for that instant replaces the one recorded.
#[tokio::test]
async fn re_reporting_one_event_records_one_event() {
    let service = "pricing";
    let environment = "ec2:staging";
    let now = now_minute();
    let router = router();
    let request = create_request(service, environment, BREAKPOINT, "src/Quote.java");
    let hash = create_configuration(&router, &request).await;

    let batch = json!({
        "Service": service,
        "Environment": environment,
        "Configurations": [
            report_entry(BREAKPOINT, &hash, "ERROR", now - 30.0, Some("FILE_NOT_FOUND")),
            report_entry(BREAKPOINT, &hash, "ACTIVE", now - 30.0, None),
        ],
    });

    // The same batch, several times over, and once more with the two entries repeated inside
    // ONE batch.
    for attempt in 1..=3 {
        let (code, body) = report(&router, &batch).await;
        assert_eq!(
            code, 200,
            "attempt {attempt}: expected HTTP 200 from the repeated report, got {code} with {body}"
        );
    }
    let mut doubled = batch.clone();
    doubled["Configurations"] = json!([
        report_entry(BREAKPOINT, &hash, "ERROR", now - 30.0, Some("FILE_NOT_FOUND")),
        report_entry(BREAKPOINT, &hash, "ERROR", now - 30.0, Some("FILE_NOT_FOUND")),
    ]);
    let (code, body) = report(&router, &doubled).await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from a batch repeating one event, got {code} with {body}"
    );

    for (status, expected) in [
        (
            "ERROR",
            vec![(nearest_minute(now - 30.0), Some("FILE_NOT_FOUND".to_string()))],
        ),
        ("ACTIVE", vec![(nearest_minute(now - 30.0), None)]),
    ] {
        let (code, body) = get_status(
            &router,
            &status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some(status),
                    ..Query::default()
                },
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {status:?}: expected HTTP 200 from the read, got {code} with {body}"
        );
        assert_eq!(
            events_of(&body),
            expected,
            "case {status:?}: expected exactly one event however often it was reported, got {:?}",
            body["Events"]
        );
    }

    // A later report of the same status and instant with another cause replaces the cause
    // rather than adding an event.
    let (code, body) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": [report_entry(
                BREAKPOINT,
                &hash,
                "ERROR",
                now - 30.0,
                Some("LANGUAGE_MISMATCH"),
            )],
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the corrected report, got {code} with {body}"
    );
    let (code, body) = get_status(
        &router,
        &status_request(
            service,
            environment,
            BREAKPOINT,
            json!({ "LocationHash": hash.clone() }),
            &Query {
                status: Some("ERROR"),
                ..Query::default()
            },
        ),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the read, got {code} with {body}"
    );
    assert_eq!(
        events_of(&body),
        vec![(
            nearest_minute(now - 30.0),
            Some("LANGUAGE_MISMATCH".to_string())
        )],
        "expected the corrected cause on the single event, got {:?}",
        body["Events"]
    );
}

/// A read reports the events of the window it names, most recent first — the window is
/// inclusive at both ends, an event outside it is not reported, and a request naming neither
/// bound covers the last hour.
#[tokio::test]
async fn a_read_reports_the_events_of_its_window_most_recent_first() {
    let service = "fulfilment";
    let environment = "eks:cluster-prod/orders";
    let now = now_minute();
    let router = router();
    let request = create_request(service, environment, PROBE, "app/orders.py");
    let hash = create_configuration(&router, &request).await;

    // Instants spanning the default window and well outside it, reported out of order so the
    // ordering a read reports is the store's and not the request's. The newest is named a few
    // seconds into a minute, so the window selects it by the minute it is HELD at.
    let two_hours_ago = now - 2.0 * 60.0 * 60.0;
    let instants = [now - 10.0, now - 1800.0, two_hours_ago, now - 300.0];
    let (code, body) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": instants
                .iter()
                .map(|time| report_entry(PROBE, &hash, "ACTIVE", *time, None))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the report, got {code} with {body}"
    );

    // Each row is a window and the events it must report, most recent first.
    struct WindowCase {
        label: &'static str,
        window: Option<(f64, f64)>,
        expected: Vec<f64>,
    }
    let cases = vec![
        WindowCase {
            label: "the documented default window of the last hour",
            window: None,
            expected: vec![nearest_minute(now - 10.0), now - 300.0, now - 1800.0],
        },
        WindowCase {
            label: "a window covering every event",
            window: Some((two_hours_ago, now)),
            expected: vec![
                nearest_minute(now - 10.0),
                now - 300.0,
                now - 1800.0,
                two_hours_ago,
            ],
        },
        WindowCase {
            label: "a window whose bounds are two recorded instants, inclusive at both ends",
            window: Some((now - 1800.0, now - 300.0)),
            expected: vec![now - 300.0, now - 1800.0],
        },
        WindowCase {
            label: "a single-instant window at a recorded event",
            window: Some((two_hours_ago, two_hours_ago)),
            expected: vec![two_hours_ago],
        },
        WindowCase {
            label: "a window one second inside the oldest event",
            window: Some((two_hours_ago + 1.0, now - 1801.0)),
            expected: vec![],
        },
        WindowCase {
            label: "a window entirely before every event",
            window: Some((now - 86_400.0, now - 7200.0 - 1.0)),
            expected: vec![],
        },
        WindowCase {
            label: "a window entirely after every event",
            window: Some((now + 60.0, now + 3600.0)),
            expected: vec![],
        },
    ];

    for WindowCase {
        label,
        window,
        expected,
    } in cases
    {
        let (code, body) = get_status(
            &router,
            &status_request(
                service,
                environment,
                PROBE,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    window,
                    ..Query::default()
                },
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {label:?}: expected HTTP 200 from the read, got {code} with {body}"
        );
        // `Status` defaults to ACTIVE, which is what every event above was reported with.
        assert_eq!(
            body["Status"].as_str(),
            Some(status_wire_token("ACTIVE")),
            "case {label:?}: expected the default status, got {body}"
        );
        assert_eq!(
            event_times(&body),
            expected,
            "case {label:?}: expected {expected:?}, got {:?}",
            body["Events"]
        );
    }
}

/// A read pages over the events of its window, each event exactly once and still most recent
/// first, for every page size — and a `NextToken` from another query is refused.
#[tokio::test]
async fn a_read_pages_over_its_events() {
    let service = "checkout";
    let environment = "ec2:production";
    let now = now_minute();
    let router = router();
    let request = create_request(service, environment, BREAKPOINT, "src/Cart.java");
    let hash = create_configuration(&router, &request).await;

    // Seven events inside the default window, newest first once read back.
    let instants: Vec<f64> = (1..=7).map(|step| now - 60.0 * step as f64).collect();
    let (code, body) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": instants
                .iter()
                .map(|time| report_entry(BREAKPOINT, &hash, "READY", *time, None))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the report, got {code} with {body}"
    );

    // Page sizes spanning one, several, exactly the event count and more than it, plus the
    // modelled `MaxResults` bounds (`range 1..100`).
    for page_size in [1_i64, 2, 3, 7, 8, 100] {
        let mut seen: Vec<f64> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (code, body) = get_status(
                &router,
                &status_request(
                    service,
                    environment,
                    BREAKPOINT,
                    json!({ "LocationHash": hash.clone() }),
                    &Query {
                        status: Some("READY"),
                        max_results: Some(page_size),
                        next_token: token.as_deref(),
                        ..Query::default()
                    },
                ),
            )
            .await;
            assert_eq!(
                code, 200,
                "case page size {page_size}: expected HTTP 200 from the read, got {code} with {body}"
            );
            let page = event_times(&body);
            assert!(
                page.len() <= page_size as usize,
                "case page size {page_size}: expected at most {page_size} events on a page, got {:?}",
                body["Events"]
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: expected the pages to run out, got {pages} pages so far"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, instants,
            "case page size {page_size}: expected every event exactly once, most recent first, got {seen:?}"
        );
    }

    // A token this service did not issue for THIS query is refused rather than honoured.
    let (code, body) = get_status(
        &router,
        &status_request(
            service,
            environment,
            BREAKPOINT,
            json!({ "LocationHash": hash.clone() }),
            &Query {
                status: Some("READY"),
                max_results: Some(2),
                ..Query::default()
            },
        ),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the first page, got {code} with {body}"
    );
    let token = body["NextToken"]
        .as_str()
        .expect("a page of two of seven events issues a NextToken")
        .to_string();
    for (label, request) in [
        (
            "a token issued for another status",
            status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("ACTIVE"),
                    max_results: Some(2),
                    next_token: Some(&token),
                    ..Query::default()
                },
            ),
        ),
        (
            "a token issued over another window",
            status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("READY"),
                    window: Some((now - 7200.0, now - 10.0)),
                    max_results: Some(2),
                    next_token: Some(&token),
                },
            ),
        ),
        (
            "a token that is not a token at all",
            status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("READY"),
                    max_results: Some(2),
                    next_token: Some("not-a-token"),
                    ..Query::default()
                },
            ),
        ),
        (
            // A token is opaque, not signed, so a caller can rewrite the payload inside the
            // tag of a token it was issued. A subsecond part of a second or more is no
            // instant, and must be refused rather than reached by anything that reads it.
            "a token whose instant carries more nanoseconds than a second holds",
            status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("READY"),
                    max_results: Some(2),
                    next_token: Some(&retagged_token(&token, "0.2000000000/0.0/0.0")),
                    ..Query::default()
                },
            ),
        ),
        (
            "a token whose payload carries too few instants",
            status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("READY"),
                    max_results: Some(2),
                    next_token: Some(&retagged_token(&token, "1.0/2.0")),
                    ..Query::default()
                },
            ),
        ),
        (
            "a token whose instant is not a number",
            status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("READY"),
                    max_results: Some(2),
                    next_token: Some(&retagged_token(&token, "later.0/0.0/0.0")),
                    ..Query::default()
                },
            ),
        ),
    ] {
        let (code, error) = get_status(&router, &request).await;
        assert_eq!(
            code, 400,
            "case {label:?}: expected HTTP 400, got {code} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );
    }
}

/// `StartTime` and `EndTime` "must both be provided together or both be omitted", and a
/// window cannot end before it begins.
#[tokio::test]
async fn a_half_stated_window_is_refused() {
    let service = "checkout";
    let environment = "ec2:production";
    let now = now_minute();
    let router = router();
    let request = create_request(service, environment, PROBE, "src/Cart.java");
    let hash = create_configuration(&router, &request).await;

    let base = status_request(
        service,
        environment,
        PROBE,
        json!({ "LocationHash": hash }),
        &Query::default(),
    );
    for (label, window) in [
        ("only StartTime", json!({ "StartTime": now - 60.0 })),
        ("only EndTime", json!({ "EndTime": now })),
        (
            "a window that ends before it begins",
            json!({ "StartTime": now, "EndTime": now - 60.0 }),
        ),
    ] {
        let mut probe = base.clone();
        for (member, value) in window.as_object().expect("an object of members") {
            probe[member] = value.clone();
        }
        let (code, error) = get_status(&router, &probe).await;
        assert_eq!(
            code, 400,
            "case {label:?}: expected HTTP 400, got {code} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );
    }
}

/// A configuration's status history belongs to the configuration: deleting it takes the
/// history with it, and a configuration created again at the same location has reported
/// nothing.
#[tokio::test]
async fn a_deleted_configuration_takes_its_history_with_it() {
    let service = "checkout";
    let environment = "ec2:production";
    let now = now_minute();
    let (router, engine) = router_with_engine();
    let request = create_request(service, environment, PROBE, "src/Cart.java");
    let hash = create_configuration(&router, &request).await;

    let (code, body) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": [report_entry(PROBE, &hash, "ACTIVE", now - 30.0, None)],
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the report, got {code} with {body}"
    );

    let scope = InstrumentationScope::new(PROBE, service, environment);
    let key = instrumentation_key(SNAPSHOT, &hash);
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();
        let held = store
            .instrumentation_status_history(&scope, &key)
            .map(|history| history.len());
        assert_eq!(
            held,
            Some(1),
            "expected the report to have reached the store, holding {held:?}"
        );
    }

    let identity = json!({
        "Service": service,
        "Environment": environment,
        "SignalType": SNAPSHOT,
        "InstrumentationType": PROBE,
        "LocationIdentifier": { "LocationHash": hash.clone() },
    });
    let (code, deleted) = delete(&router, &identity).await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the delete, got {code} with {deleted}"
    );
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();
        assert!(
            store
                .instrumentation_status_history(&scope, &key)
                .is_none_or(|history| history.is_empty()),
            "expected the deleted configuration's history to be dropped"
        );
    }

    // Created again at the same location, the configuration is a new resource with an empty
    // history.
    let recreated = create_configuration(&router, &request).await;
    assert_eq!(
        recreated, hash,
        "expected the same location to hash the same, got {recreated:?}"
    );
    let (code, body) = get_status(
        &router,
        &status_request(
            service,
            environment,
            PROBE,
            json!({ "LocationHash": hash }),
            &Query {
                status: Some("ACTIVE"),
                ..Query::default()
            },
        ),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected HTTP 200 from the read, got {code} with {body}"
    );
    assert_eq!(
        body["Events"],
        json!([]),
        "expected a new configuration to have reported nothing, got {:?}",
        body["Events"]
    );
}

/// The history one configuration holds is bounded: past the cap the OLDEST event is dropped
/// and the recent ones a read asks for are still served.
#[tokio::test]
async fn a_history_is_bounded_and_keeps_its_newest_events() {
    let service = "checkout";
    let environment = "ec2:production";
    let now = now_minute();
    let (router, engine) = router_with_engine();
    let request = create_request(service, environment, BREAKPOINT, "src/Cart.java");
    let hash = create_configuration(&router, &request).await;

    // One event per MINUTE — the granularity an event is held at, so each report is an event
    // of its own — over more instants than the cap holds. `Configurations` is modelled
    // `length 1..100`, so the reports go in batches of a hundred.
    let total = MAX_STATUS_EVENTS_PER_CONFIGURATION + 100;
    let oldest = now - 60.0 * total as f64;
    let newest = oldest + 60.0 * (total - 1) as f64;
    let mut reported = 0;
    while reported < total {
        let batch: Vec<Value> = (reported..(reported + 100).min(total))
            .map(|step| {
                report_entry(
                    BREAKPOINT,
                    &hash,
                    "ACTIVE",
                    oldest + 60.0 * step as f64,
                    None,
                )
            })
            .collect();
        reported += batch.len();
        let (code, body) = report(
            &router,
            &json!({
                "Service": service,
                "Environment": environment,
                "Configurations": batch,
            }),
        )
        .await;
        assert_eq!(
            code, 200,
            "expected HTTP 200 from the report at {reported} events, got {code} with {body}"
        );
    }

    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();
        let scope = InstrumentationScope::new(BREAKPOINT, service, environment);
        let held = store
            .instrumentation_status_history(&scope, &instrumentation_key(SNAPSHOT, &hash))
            .map(|history| history.len());
        assert_eq!(
            held,
            Some(MAX_STATUS_EVENTS_PER_CONFIGURATION),
            "expected the history to be capped at {MAX_STATUS_EVENTS_PER_CONFIGURATION}, holding {held:?}"
        );
    }

    // The newest event is still served, and the oldest reported instant is gone.
    for (label, window, expected) in [
        (
            "the newest reported instant",
            (newest, now),
            vec![newest],
        ),
        ("the oldest reported instant", (oldest, oldest), vec![]),
    ] {
        let (code, body) = get_status(
            &router,
            &status_request(
                service,
                environment,
                BREAKPOINT,
                json!({ "LocationHash": hash.clone() }),
                &Query {
                    status: Some("ACTIVE"),
                    window: Some(window),
                    ..Query::default()
                },
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {label:?}: expected HTTP 200 from the read, got {code} with {body}"
        );
        assert_eq!(
            event_times(&body),
            expected,
            "case {label:?}: expected {expected:?}, got {:?}",
            body["Events"]
        );
    }
}

/// A configuration whose `ExpiresAt` the clock has reached is no longer served to clients, so
/// neither is its status history: the read reports the configuration as not found and the
/// stored history is gone with it, while a configuration of the type that does not expire
/// keeps serving its own events.
///
/// Both `InstrumentationType` values are rows, since the type is what decides whether a
/// configuration expires at all. The documented floor on a BREAKPOINT lifetime is 5 minutes,
/// which outlasts any test, so the passage of time is staged on the store the handlers share
/// through `put_instrumentation_configuration` — the very method the create path calls.
#[tokio::test]
async fn a_lapsed_configuration_serves_no_status_history() {
    let service = "checkout";
    let environment = "eks:cluster-prod/namespace";
    let now = now_minute();
    let (router, engine) = router_with_engine();

    // One configuration of each type, at locations of their own so the two records are
    // addressed under different location hashes.
    let expiring_hash = create_configuration(
        &router,
        &create_request(service, environment, BREAKPOINT, "src/Cart.java"),
    )
    .await;
    let permanent_hash = create_configuration(
        &router,
        &create_request(service, environment, PROBE, "src/Pricing.java"),
    )
    .await;

    let reported_at = now - 30.0;
    let (code, body) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": [
                report_entry(BREAKPOINT, &expiring_hash, "ACTIVE", reported_at, None),
                report_entry(PROBE, &permanent_hash, "ACTIVE", reported_at, None),
            ],
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected the setup report to succeed, got {code} with {body}"
    );

    // Both configurations serve that event while both are still live.
    for (label, kind, hash) in [
        (
            "a BREAKPOINT configuration, which expires",
            BREAKPOINT,
            &expiring_hash,
        ),
        (
            "a PROBE configuration, which does not expire",
            PROBE,
            &permanent_hash,
        ),
    ] {
        let (code, body) = get_status(
            &router,
            &status_request(
                service,
                environment,
                kind,
                json!({ "LocationHash": hash }),
                &Query::default(),
            ),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {label:?}: expected HTTP 200 before any lapse, got {code} with {body}"
        );
        assert_eq!(
            event_times(&body),
            vec![nearest_minute(reported_at)],
            "case {label:?}: expected the reported event, got {:?}",
            body["Events"]
        );
    }

    // Stage what the clock will do to the type that expires: the same record, with an expiry
    // it has already passed.
    let scope = InstrumentationScope::new(BREAKPOINT, service, environment);
    let key = instrumentation_key(SNAPSHOT, &expiring_hash);
    let lapsed_at = DateTime::from_secs_f64(now - 1.0);
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let mut store = state.lock();
        let mut record = store
            .instrumentation_configuration(&scope, &key)
            .expect("the create stored the configuration under its identity")
            .clone();
        record.expires_at = Some(lapsed_at);
        store.put_instrumentation_configuration(scope.clone(), record, lapsed_at);
    }

    let (code, error) = get_status(
        &router,
        &status_request(
            service,
            environment,
            BREAKPOINT,
            json!({ "LocationHash": &expiring_hash }),
            &Query::default(),
        ),
    )
    .await;
    assert_eq!(
        code, 404,
        "expected a read of a lapsed configuration to be 404, got {code} with {error}"
    );
    assert_eq!(
        error["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "expected a ResourceNotFoundException, got {error}"
    );
    assert_eq!(
        error["Message"].as_str(),
        Some(CONFIGURATION_DELETED),
        "expected the sentence for a configuration the lapse took away, got {error}"
    );

    // The lapse was materialised rather than merely hidden, and the history went with the
    // configuration it belonged to.
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();
        assert!(
            store.instrumentation_configuration(&scope, &key).is_none(),
            "expected the lapsed configuration to be dropped from the store"
        );
        let held = store
            .instrumentation_status_history(&scope, &key)
            .map(|history| history.len());
        assert!(
            held.is_none(),
            "expected the lapsed configuration's status history to be dropped, holding {held:?}"
        );
    }

    // The configuration that does not expire is untouched by another one's lapse.
    let (code, body) = get_status(
        &router,
        &status_request(
            service,
            environment,
            PROBE,
            json!({ "LocationHash": &permanent_hash }),
            &Query::default(),
        ),
    )
    .await;
    assert_eq!(
        code, 200,
        "expected a configuration that does not expire to keep serving its events, \
         got {code} with {body}"
    );
    assert_eq!(
        event_times(&body),
        vec![nearest_minute(reported_at)],
        "expected the reported event to still be served, got {:?}",
        body["Events"]
    );
}
