// SPDX-License-Identifier: CC0-1.0

//! `UpdateRecord`: what it changes, what it leaves alone, and the three failures it
//! models.
//!
//! RULES, from the operation's own doc and from the modelled `ConflictException`:
//!
//! * "Features that you do not include in the request remain unchanged" — the posted
//!   features are MERGED onto the stored record.
//! * "If you omit EventTime, the service keeps the record's existing EventTime and
//!   applies the update."
//! * "The record must already exist. If the record does not exist or has been
//!   soft-deleted, the operation returns a ResourceNotFound error." (404)
//! * "If you provide an EventTime that is older than the record's current EventTime, the
//!   service rejects the update with a ConflictException. If the EventTime is equal to or
//!   newer than the current value, the service applies the update." (409, record
//!   unchanged)
//! * "If you specify a TtlDuration, you must also provide an EventTime in the request.
//!   Otherwise, the operation returns a ValidationError." (400)
//!
//! Every case goes through the real front door — the multi-service router, the restJson1
//! protocol, the modelled constraint validation — and every write is read back through
//! `GetRecord`, so what is asserted is the wire behaviour a client sees.

use std::sync::Arc;

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The feature group every case updates records in, and the two feature names its
/// definition fixes.
const GROUP: &str = "customers";
const IDENTIFIER_FEATURE: &str = "record_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// The event time the seeded record carries. A whole second, so the SAME instant has an
/// exact spelling in both forms a Feature Store event-time feature may take: this
/// ISO-8601 string and the epoch seconds [`secs`] reads out of it.
const SEEDED_EVENT_TIME: &str = "2026-10-23T18:20:13Z";

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
                // The feature groups these cases write to are online-only: no offline store
                // to replicate into.
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

    /// Ingest one record, so an update has something to update.
    async fn put_record(&self, identifier: &str, event_time: &str, body: Value) -> (u16, Value) {
        let mut record = vec![
            feature(IDENTIFIER_FEATURE, identifier),
            feature(EVENT_TIME_FEATURE, event_time),
            feature("feature_a", "original"),
            feature("feature_b", "kept"),
        ];
        if let Some(extra) = body.as_array() {
            record.extend(extra.iter().cloned());
        }
        self.send(
            "PUT",
            &format!("/FeatureGroup/{GROUP}"),
            Some(json!({ "Record": record })),
        )
        .await
    }

    async fn update_record(&self, body: Value) -> (u16, Value) {
        self.send("POST", &format!("/FeatureGroup/{GROUP}/Record"), Some(body))
            .await
    }

    async fn get_record(&self, identifier: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!(
                "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={identifier}\
                 &ExpirationTimeResponse=Enabled"
            ),
            None,
        )
        .await
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// One row of the merge table: the case name, the `Features` the update posts, and the
/// `(FeatureName, value)` pairs the record must hold afterwards. Named, rather than
/// written inline, because the nested tuple is past what `clippy::type_complexity`
/// admits in a signature or a binding.
type MergeCase<'a> = (&'a str, Value, Vec<(&'a str, Value)>);

/// One row of the event-time ordering table: the case name, the `EventTime` the update
/// posts (`None` for an omitted one), the status it must answer, the event time the
/// record must then carry, and whether the update was applied.
type OrderingCase<'a> = (&'a str, Option<&'a str>, u16, Value, bool);

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

/// The record a `GetRecord` response carries, as `(FeatureName, value)` pairs.
fn record_pairs(body: &Value) -> Vec<(String, Value)> {
    body.get("Record")
        .and_then(Value::as_array)
        .map(|features| {
            features
                .iter()
                .map(|feature| {
                    let name = feature["FeatureName"].as_str().unwrap_or_default().to_owned();
                    let value = feature
                        .get("ValueAsString")
                        .or_else(|| feature.get("ValueAsStringList"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    (name, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value one feature of the served record carries.
fn served(body: &Value, name: &str) -> Value {
    record_pairs(body)
        .into_iter()
        .find(|(held, _)| held == name)
        .map(|(_, value)| value)
        .unwrap_or(Value::Null)
}

/// An instant in epoch seconds, read from either accepted event-time spelling.
fn secs(iso_or_epoch: &str) -> f64 {
    DateTime::from_str(iso_or_epoch, Format::DateTime)
        .map(|parsed| parsed.as_secs_f64())
        .unwrap_or_else(|_| iso_or_epoch.parse::<f64>().expect("a parsable event time"))
}

#[tokio::test]
async fn an_update_merges_the_posted_features_and_leaves_the_others_unchanged() {
    // The class the rule spans: which of the record's features a request names. Rows
    // differ in how many features are posted, whether they are already on the record,
    // and the shape of the value.
    //
    // (case, the posted Features, the pairs the record must then hold)
    let rows: Vec<MergeCase<'_>> = vec![
        (
            "one feature the record already carries",
            json!([feature("feature_a", "updated")]),
            vec![
                ("record_id", json!("R1")),
                ("event_time", json!(SEEDED_EVENT_TIME)),
                ("feature_a", json!("updated")),
                ("feature_b", json!("kept")),
            ],
        ),
        (
            "two features the record already carries",
            json!([feature("feature_a", "a2"), feature("feature_b", "b2")]),
            vec![
                ("record_id", json!("R1")),
                ("event_time", json!(SEEDED_EVENT_TIME)),
                ("feature_a", json!("a2")),
                ("feature_b", json!("b2")),
            ],
        ),
        (
            "a feature the record does not carry yet",
            json!([feature("feature_c", "new")]),
            vec![
                ("record_id", json!("R1")),
                ("event_time", json!(SEEDED_EVENT_TIME)),
                ("feature_a", json!("original")),
                ("feature_b", json!("kept")),
                ("feature_c", json!("new")),
            ],
        ),
        (
            "a collection value",
            json!([{ "FeatureName": "feature_a", "ValueAsStringList": ["x", "y"] }]),
            vec![
                ("record_id", json!("R1")),
                ("event_time", json!(SEEDED_EVENT_TIME)),
                ("feature_a", json!(["x", "y"])),
                ("feature_b", json!("kept")),
            ],
        ),
        (
            "an empty string value",
            json!([feature("feature_a", "")]),
            vec![
                ("record_id", json!("R1")),
                ("event_time", json!(SEEDED_EVENT_TIME)),
                ("feature_a", json!("")),
                ("feature_b", json!("kept")),
            ],
        ),
        (
            "the record identifier feature itself",
            json!([feature(IDENTIFIER_FEATURE, "R1")]),
            vec![
                ("record_id", json!("R1")),
                ("event_time", json!(SEEDED_EVENT_TIME)),
                ("feature_a", json!("original")),
                ("feature_b", json!("kept")),
            ],
        ),
    ];

    for (case, features, expected) in rows {
        let emulator = Emulator::new();
        emulator.put_record("R1", SEEDED_EVENT_TIME, Value::Null).await;

        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "R1",
                "Features": features
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateRecord 200, got {status} with body {body:?}"
        );

        let (status, body) = emulator.get_record("R1").await;
        let actual = record_pairs(&body);
        let expected: Vec<(String, Value)> = expected
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        assert_eq!(
            (status, actual.clone()),
            (200, expected.clone()),
            "case {case:?}: expected the record to hold {expected:?}, got {status} with \
             {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_event_time_older_than_the_records_is_a_conflict_and_changes_nothing() {
    // The class the rule spans: where the posted EventTime falls relative to the
    // record's, in both accepted spellings. "If you provide an EventTime that is older …
    // ConflictException. If the EventTime is equal to or newer than the current value,
    // the service applies the update. If you omit EventTime, the service keeps the
    // record's existing EventTime."
    //
    // (case, the EventTime the update posts, expected status, the event time the record
    //  must then carry, whether feature_a must have been updated)
    let one_second_older = format!("{}", secs(SEEDED_EVENT_TIME) - 1.0);
    let one_second_newer = format!("{}", secs(SEEDED_EVENT_TIME) + 1.0);
    let much_older = format!("{}", secs(SEEDED_EVENT_TIME) - 500.0);
    let equal_epoch = format!("{}", secs(SEEDED_EVENT_TIME));
    let rows: Vec<OrderingCase<'_>> = vec![
        (
            "much older, as epoch seconds",
            Some(much_older.as_str()),
            409,
            json!(SEEDED_EVENT_TIME),
            false,
        ),
        (
            "one second older",
            Some(one_second_older.as_str()),
            409,
            json!(SEEDED_EVENT_TIME),
            false,
        ),
        (
            "older, as ISO-8601",
            Some("2020-01-01T00:00:00Z"),
            409,
            json!(SEEDED_EVENT_TIME),
            false,
        ),
        (
            "exactly equal, as epoch seconds",
            Some(equal_epoch.as_str()),
            200,
            json!(equal_epoch.clone()),
            true,
        ),
        (
            "exactly equal, as the ISO-8601 spelling of the same instant",
            Some(SEEDED_EVENT_TIME),
            200,
            json!(SEEDED_EVENT_TIME),
            true,
        ),
        (
            "one second newer",
            Some(one_second_newer.as_str()),
            200,
            json!(one_second_newer.clone()),
            true,
        ),
        (
            "much newer, as ISO-8601",
            Some("2030-01-01T00:00:00Z"),
            200,
            json!("2030-01-01T00:00:00Z"),
            true,
        ),
        ("omitted", None, 200, json!(SEEDED_EVENT_TIME), true),
    ];

    for (case, event_time, expected_status, expected_event_time, updated) in rows {
        let emulator = Emulator::new();
        emulator.put_record("R1", SEEDED_EVENT_TIME, Value::Null).await;

        let mut features = vec![feature("feature_a", "updated")];
        if let Some(at) = event_time {
            features.push(feature(EVENT_TIME_FEATURE, at));
        }
        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "R1",
                "Features": features
            }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected UpdateRecord {expected_status}, got {status} with body \
             {body:?}"
        );
        if expected_status == 409 {
            let code = error_code(&body);
            assert_eq!(
                code, "ConflictException",
                "case {case:?}: expected the modelled ConflictException error code, got \
                 {code:?} with body {body:?}"
            );
        }

        let (_, body) = emulator.get_record("R1").await;
        let feature_a = served(&body, "feature_a");
        let expected_a = if updated {
            json!("updated")
        } else {
            json!("original")
        };
        assert_eq!(
            feature_a, expected_a,
            "case {case:?}: expected feature_a {expected_a:?}, got {feature_a:?}"
        );
        let held = served(&body, EVENT_TIME_FEATURE);
        assert_eq!(
            held, expected_event_time,
            "case {case:?}: expected the record's event time {expected_event_time:?}, got \
             {held:?}"
        );
    }
}

#[tokio::test]
async fn a_record_that_is_not_there_is_resource_not_found() {
    // The class the rule spans: every way a record can fail to be there. "The record
    // must already exist. If the record does not exist or has been soft-deleted, the
    // operation returns a ResourceNotFound error."
    //
    // (case, the identifier to update, how the record got into that state)
    let rows: [(&str, &str, &str); 4] = [
        ("never ingested", "R-absent", "nothing"),
        ("soft-deleted", "R1", "soft-delete"),
        ("hard-deleted", "R1", "hard-delete"),
        ("expired by its TtlDuration", "R-expired", "expire"),
    ];

    for (case, identifier, how) in rows {
        let emulator = Emulator::new();
        emulator.put_record("R1", SEEDED_EVENT_TIME, Value::Null).await;
        match how {
            "soft-delete" | "hard-delete" => {
                let mode = if how == "soft-delete" {
                    "SoftDelete"
                } else {
                    "HardDelete"
                };
                let (status, body) = emulator
                    .send(
                        "DELETE",
                        &format!(
                            "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString=R1\
                             &EventTime=2030-01-01T00:00:00Z&DeletionMode={mode}"
                        ),
                        None,
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the preparing DeleteRecord 200, got {status} \
                     with body {body:?}"
                );
            }
            "expire" => {
                // An event time far in the past plus a one-second TTL: the expiry is
                // already due, so the record is hard-deleted by the time this reads it.
                let (status, _) = emulator
                    .send(
                        "PUT",
                        &format!("/FeatureGroup/{GROUP}"),
                        Some(json!({
                            "Record": [
                                feature(IDENTIFIER_FEATURE, "R-expired"),
                                feature(EVENT_TIME_FEATURE, "2000-01-01T00:00:00Z")
                            ],
                            "TtlDuration": { "Unit": "Seconds", "Value": 1 }
                        })),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: expected the preparing PutRecord 200");
            }
            _ => {}
        }

        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": identifier,
                "Features": [feature("feature_a", "updated")]
            }))
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFound"),
            "case {case:?}: expected HTTP 404 ResourceNotFound for a record that is not \
             there, got {status} {code:?} with body {body:?}"
        );
    }
}

#[tokio::test]
async fn a_ttl_duration_without_an_event_time_is_refused_and_one_with_it_sets_expires_at() {
    // "If you specify a TtlDuration, you must also provide an EventTime in the request.
    // Otherwise, the operation returns a ValidationError." Rows: every unit the model's
    // `TtlDurationUnit` enum declares (source: `TTL_DURATION_UNIT_VALUES` in
    // `src/ops/types/shared_types.rs`), each without an EventTime and then with one.
    //
    // (case, the TtlDuration, the seconds it denotes)
    let rows: [(&str, Value, f64); 6] = [
        ("Seconds, the lower bound of TtlDurationValue", json!({ "Unit": "Seconds", "Value": 1 }), 1.0),
        ("Minutes", json!({ "Unit": "Minutes", "Value": 90 }), 5_400.0),
        ("Hours", json!({ "Unit": "Hours", "Value": 36 }), 129_600.0),
        ("Days", json!({ "Unit": "Days", "Value": 2 }), 172_800.0),
        ("Weeks", json!({ "Unit": "Weeks", "Value": 52 }), 31_449_600.0),
        ("Weeks, one", json!({ "Unit": "Weeks", "Value": 1 }), 604_800.0),
    ];

    // An event time well in the future, so no row's expiry is already due.
    let event_time = "2035-01-01T00:00:00Z";

    for (case, ttl, seconds) in rows {
        let emulator = Emulator::new();
        emulator.put_record("R1", event_time, Value::Null).await;

        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "R1",
                "Features": [feature("feature_a", "updated")],
                "TtlDuration": ttl.clone()
            }))
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected HTTP 400 ValidationError for a TtlDuration with no \
             EventTime, got {status} {code:?} with body {body:?}"
        );
        // A refused update changes nothing.
        let (_, body) = emulator.get_record("R1").await;
        let feature_a = served(&body, "feature_a");
        assert_eq!(
            feature_a,
            json!("original"),
            "case {case:?}: expected the refused update to change nothing, got {feature_a:?}"
        );

        // With an EventTime in the same request the update is applied, and
        // `ExpiresAt = EventTime + TtlDuration`.
        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "R1",
                "Features": [
                    feature("feature_a", "updated"),
                    feature(EVENT_TIME_FEATURE, event_time)
                ],
                "TtlDuration": ttl
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateRecord 200 with an EventTime, got {status} with \
             body {body:?}"
        );
        let (_, body) = emulator.get_record("R1").await;
        let expires_at = body
            .get("ExpiresAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let delta = secs(&expires_at) - secs(event_time);
        assert!(
            (delta - seconds).abs() < 0.001,
            "case {case:?}: expected ExpiresAt {seconds} s after the event time, got {delta} \
             s (ExpiresAt {expires_at:?})"
        );
    }
}

#[tokio::test]
async fn an_update_past_the_documented_feature_ceiling_is_refused() {
    // "You can update up to 100 features per call." The model states no `@length`
    // maximum on `Features`, so the ceiling is the handler's. Rows: the bound, one past
    // it, and well past it.
    //
    // (case, how many features the call posts, expected status)
    let rows: [(&str, usize, u16); 4] = [
        ("one feature", 1, 200),
        ("the documented ceiling", 100, 200),
        ("one feature past the ceiling", 101, 400),
        ("well past the ceiling", 250, 400),
    ];

    for (case, count, expected_status) in rows {
        let emulator = Emulator::new();
        emulator.put_record("R1", SEEDED_EVENT_TIME, Value::Null).await;

        let features: Vec<Value> = (0..count)
            .map(|index| feature(&format!("feature_{index}"), "v"))
            .collect();
        let (status, body) = emulator
            .update_record(json!({
                "RecordIdentifierValueAsString": "R1",
                "Features": features
            }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected UpdateRecord {expected_status} for {count} features, got \
             {status} with body {body:?}"
        );
        if expected_status == 400 {
            let code = error_code(&body);
            assert_eq!(
                code, "ValidationError",
                "case {case:?}: expected the modelled ValidationError error code, got {code:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_aimed_only_at_the_offline_store_is_still_validated_and_resolved() {
    // "The target stores for the record update. By default, Amazon SageMaker Feature
    // Store updates the record in all stores associated with the FeatureGroup." This
    // service serves the OnlineStore only, so an offline-only update writes nothing
    // here — but the request's own rules, the record's existence and the ordering
    // against its event time hold whichever store the update names, so only the WRITE is
    // skipped.
    //
    // (case, TargetStores, the value feature_a must then hold)
    let rows: [(&str, Value, &str); 4] = [
        ("absent", Value::Null, "updated"),
        ("OnlineStore", json!(["OnlineStore"]), "updated"),
        (
            "both stores",
            json!(["OnlineStore", "OfflineStore"]),
            "updated",
        ),
        ("OfflineStore only", json!(["OfflineStore"]), "original"),
    ];

    for (case, target_stores, expected) in rows {
        let emulator = Emulator::new();
        emulator.put_record("R1", SEEDED_EVENT_TIME, Value::Null).await;
        let mut request = json!({
            "RecordIdentifierValueAsString": "R1",
            "Features": [feature("feature_a", "updated")]
        });
        if !target_stores.is_null() {
            request["TargetStores"] = target_stores;
        }
        let (status, body) = emulator.update_record(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateRecord 200, got {status} with body {body:?}"
        );

        let (_, body) = emulator.get_record("R1").await;
        let feature_a = served(&body, "feature_a");
        assert_eq!(
            feature_a,
            json!(expected),
            "case {case:?}: expected feature_a {expected:?}, got {feature_a:?}"
        );
    }

    // What an offline-only update does NOT skip: the request's rules and the record it
    // names are resolved exactly as for an update that reaches the online store.
    //
    // (case, the identifier the update names, the request's extra members, expected
    //  status, expected error code)
    let refusals: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a record that was never ingested",
            "R-absent",
            json!({}),
            404,
            "ResourceNotFound",
        ),
        (
            "an EventTime older than the record's",
            "R1",
            json!({ "older_event_time": true }),
            409,
            "ConflictException",
        ),
        (
            "a TtlDuration with no EventTime",
            "R1",
            json!({ "TtlDuration": { "Unit": "Days", "Value": 1 } }),
            400,
            "ValidationError",
        ),
    ];

    for (case, identifier, extra, expected_status, expected_code) in refusals {
        let emulator = Emulator::new();
        emulator.put_record("R1", SEEDED_EVENT_TIME, Value::Null).await;
        let mut features = vec![feature("feature_a", "updated")];
        if extra.get("older_event_time").is_some() {
            features.push(feature(
                EVENT_TIME_FEATURE,
                &format!("{}", secs(SEEDED_EVENT_TIME) - 500.0),
            ));
        }
        let mut request = json!({
            "RecordIdentifierValueAsString": identifier,
            "Features": features,
            "TargetStores": ["OfflineStore"]
        });
        if let Some(ttl) = extra.get("TtlDuration") {
            request["TtlDuration"] = ttl.clone();
        }
        let (status, body) = emulator.update_record(request).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected an offline-only update to be refused with HTTP \
             {expected_status} {expected_code:?}, got {status} {code:?} with body {body:?}"
        );
    }
}
