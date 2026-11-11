// SPDX-License-Identifier: CC0-1.0

//! How this service STATES the two rejections whose error code a client branches on
//! first: a `BatchGetRecord` batch larger than the modelled maximum, and a `DeleteRecord`
//! whose `EventTime` is before the record's.
//!
//! RULES:
//!
//! * A value outside a modelled constraint is refused with `400 ValidationException` and
//!   a message BUILT FROM THE REQUEST: `N validation error(s) detected: Value '<value>'
//!   at '<member path>' failed to satisfy constraint: <the constraint the model states>`.
//!   For `BatchGetRecordRequest$Identifiers` (`@length` min 1, max 100, `@required`) the
//!   member path is `identifiers` and the constraint is the `@length` maximum, whatever
//!   the batch's size past it.
//! * A deletion is ordered against the stored record's own `EventTime`: the request's
//!   must be the SAME as it or after it. A strictly older one does not delete — a
//!   `SoftDelete` leaves the record standing, a `HardDelete` is refused with the modelled
//!   `ValidationError` (400) stating that rule.
//!
//! Every case goes through the real front door, so what is asserted is the wire behaviour
//! a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The feature group every case reads from, and the two feature names its definition
/// fixes.
const GROUP: &str = "customers";
const IDENTIFIER_FEATURE: &str = "record_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// The modelled maximum of `BatchGetRecordRequest$Identifiers`.
///
/// Source: `smithy_query(query="operation", operation="BatchGetRecord")` on this crate's
/// `smithy-model/` — `Identifiers` targets `BatchGetRecordIdentifiers` with
/// `length {min: 1, max: 100}` and is `@required`.
const MAX_IDENTIFIERS: usize = 100;

/// A booted emulator holding this service only.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        let emulator = Self { engine, router };
        emulator.create_feature_group(GROUP);
        emulator
    }

    /// Create a feature group with an OnlineStore, the way the SageMaker control plane's
    /// `CreateFeatureGroup` does.
    fn create_feature_group(&self, name: &str) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!("arn:aws:sagemaker:us-east-1:123456789012:feature-group/{name}"),
                record_identifier_feature_name: IDENTIFIER_FEATURE.to_owned(),
                event_time_feature_name: EVENT_TIME_FEATURE.to_owned(),
                online_store_enabled: true,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
            });
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20240101/us-east-1/sagemaker/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("host", "featurestore-runtime.sagemaker.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(
                body.map(|value| bytes::Bytes::from(value.to_string()))
                    .unwrap_or_default(),
            )
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Ingest one record at `event_time`.
    async fn put_record(&self, identifier: &str, event_time: &str) -> (u16, Value) {
        self.send(
            "PUT",
            &format!("/FeatureGroup/{GROUP}"),
            Some(json!({ "Record": [
                feature(IDENTIFIER_FEATURE, identifier),
                feature(EVENT_TIME_FEATURE, event_time),
                feature("feature_a", "original")
            ] })),
        )
        .await
    }

    /// Whether `GetRecord` still serves the record.
    async fn record_is_served(&self, identifier: &str) -> bool {
        let (_, body) = self
            .send(
                "GET",
                &format!(
                    "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={identifier}"
                ),
                None,
            )
            .await;
        body.get("Record").and_then(Value::as_array).is_some()
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// The wire error code of an error response: restJson1 carries it as the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The message an error response carries.
///
/// The framework's own constraint refusal carries it as `message`, while this service's
/// modelled errors declare a `Message` member (`ValidationError$Message`), which
/// restJson1 serialises under that name — so both spellings are read here.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn a_batch_past_the_modeled_identifier_maximum_is_refused_with_the_constraint_clause() {
    let emulator = Emulator::new();
    emulator.put_record("R1", "2024-01-01T00:00:00Z").await;

    /// One `BatchGetRecordIdentifier` naming the one feature group.
    fn identifier(index: usize) -> Value {
        json!({
            "FeatureGroupName": GROUP,
            "RecordIdentifiersValueAsString": [format!("id-{index}")]
        })
    }

    // The class the rule spans: the batch's size, on both sides of the modelled bound.
    // The bound itself and one past it are the two rows that decide the constraint, and
    // the larger ones show the clause does not change with how far past it the request
    // is.
    //
    // (case, how many identifiers the batch carries, expected status)
    let rows: [(&str, usize, u16); 5] = [
        ("one identifier", 1, 200),
        ("the modelled maximum", MAX_IDENTIFIERS, 200),
        ("one identifier past the maximum", MAX_IDENTIFIERS + 1, 400),
        ("fifty past the maximum", MAX_IDENTIFIERS + 50, 400),
        ("ten times the maximum", MAX_IDENTIFIERS * 10, 400),
    ];

    // The clause AWS states for this violation, built from the request: the member path
    // the walk reports the list at and the constraint the model declares on it.
    let clause = format!(
        "at 'identifiers' failed to satisfy constraint: Member must have length less than or \
         equal to {MAX_IDENTIFIERS}"
    );

    for (case, count, expected_status) in rows {
        let identifiers: Vec<Value> = (0..count).map(identifier).collect();
        let (status, body) = emulator
            .send(
                "POST",
                "/BatchGetRecord",
                Some(json!({ "Identifiers": identifiers })),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {count} identifiers, got \
             {status} with body {body:?}"
        );
        if expected_status == 200 {
            // A batch within the bound is served, so the refusal above is the bound and
            // nothing else.
            assert!(
                body.get("Records").and_then(Value::as_array).is_some(),
                "case {case:?}: expected the batch to be served, got {body:?}"
            );
            continue;
        }
        let code = error_code(&body);
        let text = message(&body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the constraint-refusal error code, got {code:?} with \
             message {text:?}"
        );
        assert!(
            text.starts_with("1 validation error detected: Value '"),
            "case {case:?}: expected the AWS constraint-violation envelope stating one \
             violation and echoing the value, got {text:?}"
        );
        assert!(
            text.ends_with(&clause),
            "case {case:?}: expected the clause to end {clause:?}, got {text:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_at_or_after_the_records_event_time_deletes_and_an_earlier_one_does_not() {
    // The class the rule spans: where the deletion's `EventTime` falls relative to the
    // record's, in both accepted spellings and both deletion modes. The record is
    // written at one instant; each row deletes at another.
    let stored_iso = "2026-10-23T18:20:13Z";
    let stored_secs = 1_792_779_613.0_f64;

    // (case, the EventTime the delete carries)
    let earlier: Vec<(&str, String)> = vec![
        ("much earlier, as epoch seconds", format!("{}", stored_secs - 500.0)),
        ("one second earlier", format!("{}", stored_secs - 1.0)),
        ("earlier, as ISO-8601", "2020-01-01T00:00:00Z".to_owned()),
    ];
    let same_or_after: Vec<(&str, String)> = vec![
        ("exactly the record's, as ISO-8601", stored_iso.to_owned()),
        (
            "exactly the record's, as epoch seconds",
            format!("{stored_secs}"),
        ),
        ("one second after", format!("{}", stored_secs + 1.0)),
        ("much later, as ISO-8601", "2030-01-01T00:00:00Z".to_owned()),
    ];

    // The rule as this service words it: a message naming no value from the request,
    // because the rule names none.
    let expected_message =
        "Validation Error: Delete event time must be same or after latest record event time.";

    for (mode, hard) in [("", false), ("&DeletionMode=SoftDelete", false), ("&DeletionMode=HardDelete", true)] {
        for (case, event_time) in &earlier {
            let emulator = Emulator::new();
            emulator.put_record("R1", stored_iso).await;
            let (status, body) = emulator
                .send(
                    "DELETE",
                    &format!(
                        "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString=R1\
                         &EventTime={event_time}{mode}"
                    ),
                    None,
                )
                .await;
            let case = format!("{case} ({})", if hard { "HardDelete" } else { "SoftDelete" });
            if hard {
                let code = error_code(&body);
                let text = message(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationError"),
                    "case {case:?}: expected HTTP 400 ValidationError for a HardDelete \
                     before the record's event time, got {status} {code:?} with message \
                     {text:?}"
                );
                assert_eq!(
                    text, expected_message,
                    "case {case:?}: expected the refusal to state the rule, got {text:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a SoftDelete before the record's event time to \
                     answer 200 and not delete, got {status} with body {body:?}"
                );
            }
            assert!(
                emulator.record_is_served("R1").await,
                "case {case:?}: expected the record to be left standing"
            );
        }

        for (case, event_time) in &same_or_after {
            let emulator = Emulator::new();
            emulator.put_record("R1", stored_iso).await;
            let (status, body) = emulator
                .send(
                    "DELETE",
                    &format!(
                        "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString=R1\
                         &EventTime={event_time}{mode}"
                    ),
                    None,
                )
                .await;
            let case = format!("{case} ({})", if hard { "HardDelete" } else { "SoftDelete" });
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for a delete at or after the record's \
                 event time, got {status} with body {body:?}"
            );
            assert!(
                !emulator.record_is_served("R1").await,
                "case {case:?}: expected the record to be deleted"
            );
        }
    }
}
