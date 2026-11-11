// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the OnlineStore record lifecycle: `PutRecord`,
//! `GetRecord`, `BatchGetRecord`, `ListRecords` and `DeleteRecord`.
//!
//! Every case goes through the real front door — the multi-service router, the
//! restJson1 protocol, the modelled constraint validation — so what is asserted is the
//! wire behaviour a client sees, not a handler's return value.

use std::sync::Arc;

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// A booted emulator holding this service only.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A booted emulator with the feature groups these suites write to already created.
    ///
    /// Every operation of this data plane resolves its `FeatureGroupName` against the
    /// feature groups the SageMaker CONTROL plane holds, so a group has to exist before a
    /// record can be written to it — exactly as a client has to call
    /// `CreateFeatureGroup` first.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        let emulator = Self { engine, router };
        for (group, identifier) in [
            ("customers", "customer_id"),
            ("orders", "order_id"),
            ("ttlgroup", "customer_id"),
            ("expiring", "customer_id"),
        ] {
            emulator.create_feature_group(group, identifier, "event_time", true);
        }
        emulator
    }

    /// Create a feature group with an OnlineStore, the way the SageMaker control plane's
    /// `CreateFeatureGroup` does: by registering its definition in the ONE shared
    /// feature-group directory this data plane resolves names against.
    fn create_feature_group(
        &self,
        name: &str,
        record_identifier_feature_name: &str,
        event_time_feature_name: &str,
        online_store_enabled: bool,
    ) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!(
                    "arn:aws:sagemaker:us-east-1:123456789012:feature-group/{name}"
                ),
                record_identifier_feature_name: record_identifier_feature_name.to_owned(),
                event_time_feature_name: event_time_feature_name.to_owned(),
                online_store_enabled,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
            });
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for
    /// an empty body).
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        // The SigV4 credential scope is how the router picks the claiming service: the
        // 4th field is this service's signing name.
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20240101/us-east-1/sagemaker/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("host", "featurestore-runtime.sagemaker.us-east-1.amazonaws.com")
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

    async fn put_record(&self, group: &str, body: Value) -> (u16, Value) {
        self.send("PUT", &format!("/FeatureGroup/{group}"), Some(body))
            .await
    }

    async fn get_record(&self, group: &str, query: &str) -> (u16, Value) {
        self.send("GET", &format!("/FeatureGroup/{group}?{query}"), None)
            .await
    }

    async fn delete_record(&self, group: &str, query: &str) -> (u16, Value) {
        self.send("DELETE", &format!("/FeatureGroup/{group}?{query}"), None)
            .await
    }

    async fn list_records(&self, group: &str, body: Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/FeatureGroup/{group}/ListRecords"),
            Some(body),
        )
        .await
    }

    async fn batch_get_record(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/BatchGetRecord", Some(body)).await
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// The `Record` member of a `GetRecord` response, as `(FeatureName, Value)` pairs where
/// the value is the scalar or the list the response carried.
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

/// The wire error code of an error response: restJson1 carries it as the body's
/// `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Whether `code` is one of the two 400 client-refusal codes this emulator answers: the
/// boundary validator's synthetic `ValidationException` or this service's own modelled
/// `ValidationError`.
fn is_client_refusal(code: &str) -> bool {
    matches!(code, "ValidationError" | "ValidationException")
}

/// `event_time + seconds`, as the instant an ISO-8601 `ExpiresAt` must denote.
fn secs(iso_or_epoch: &str) -> f64 {
    DateTime::from_str(iso_or_epoch, Format::DateTime)
        .map(|parsed| parsed.as_secs_f64())
        .unwrap_or_else(|_| iso_or_epoch.parse::<f64>().expect("a parsable event time"))
}

/// An event time one day from now, so a record written with a TTL of any size in these
/// tests is still live when the test reads it back.
fn tomorrow() -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs() as f64;
    now + 86_400.0
}

/// [`tomorrow`] as the ISO-8601 string an event-time feature of type `String` carries.
fn tomorrow_iso() -> String {
    DateTime::from_secs_f64(tomorrow())
        .fmt(Format::DateTime)
        .expect("format an ISO-8601 event time")
}

/// [`tomorrow`] as the epoch-seconds string an event-time feature of type `Fractional`
/// carries.
fn tomorrow_epoch() -> String {
    format!("{}", tomorrow() as i64)
}

#[tokio::test]
async fn get_record_returns_exactly_the_feature_values_put_record_stored() {
    let emulator = Emulator::new();

    // The class the rule spans: what a record's feature values can look like. One row
    // per shape of value — scalar, list, numeric string, empty string, unicode, a
    // single-feature record, and a record whose features are in an unusual order.
    let cases: Vec<(&str, Value)> = vec![
        (
            "scalar features",
            json!([
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle")
            ]),
        ),
        (
            "collection feature",
            json!([
                feature("customer_id", "C2"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                { "FeatureName": "tags", "ValueAsStringList": ["a", "b", "c"] }
            ]),
        ),
        (
            "numeric and empty values",
            json!([
                feature("customer_id", "C3"),
                feature("event_time", "1704067200"),
                feature("balance", "-17.5"),
                feature("note", "")
            ]),
        ),
        (
            "unicode value",
            json!([
                feature("customer_id", "C4"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "東京 / Zürich")
            ]),
        ),
        (
            "identifier-only record",
            json!([feature("customer_id", "C5")]),
        ),
        (
            "event time before the identifier is not the identifier",
            json!([
                feature("customer_id", "C6"),
                feature("event_time", "2024-01-02T00:00:00Z")
            ]),
        ),
    ];

    for (case, record) in &cases {
        let (status, _) = emulator
            .put_record("customers", json!({ "Record": record }))
            .await;
        assert_eq!(status, 200, "case {case:?}: expected PutRecord 200, got {status}");
    }

    for (case, record) in &cases {
        let identifier = record[0]["ValueAsString"].as_str().expect("an identifier");
        let expected: Vec<(String, Value)> = record
            .as_array()
            .expect("a record")
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
            .collect();

        let (status, body) = emulator
            .get_record(
                "customers",
                &format!("RecordIdentifierValueAsString={identifier}"),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual = record_pairs(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored record {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn put_record_is_a_full_over_write_of_the_identified_record() {
    let emulator = Emulator::new();

    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle"),
                feature("note", "first")
            ] }),
        )
        .await;
    // A later event time, so this is not a historic record.
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-02T00:00:00Z"),
                feature("city", "Boston")
            ] }),
        )
        .await;

    let (status, body) = emulator
        .get_record("customers", "RecordIdentifierValueAsString=C1")
        .await;
    let actual = record_pairs(&body);
    let expected: Vec<(String, Value)> = vec![
        ("customer_id".to_owned(), json!("C1")),
        ("event_time".to_owned(), json!("2024-01-02T00:00:00Z")),
        ("city".to_owned(), json!("Boston")),
    ];
    assert_eq!(status, 200, "expected GetRecord 200, got {status}");
    assert_eq!(
        actual, expected,
        "case \"over-write\": expected only the second record's values {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn put_record_of_a_historic_record_leaves_the_online_record_standing() {
    let emulator = Emulator::new();

    // "If a new record's EventTime is greater, the new record is written to both the
    // OnlineStore and OfflineStore. Otherwise, the record is a historic record and it
    // is written only to the OfflineStore." (PutRecord, Smithy model doc.)
    // Rows: an older, an equal and a later event time, in both accepted event-time
    // formats, plus a write aimed at the OfflineStore alone.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        ("older", "2023-12-31T00:00:00Z", Value::Null, "Seattle"),
        ("equal", "2024-01-01T00:00:00Z", Value::Null, "Seattle"),
        ("later", "2024-01-02T00:00:00Z", Value::Null, "Boston"),
        ("older epoch seconds", "1700000000", Value::Null, "Seattle"),
        ("later epoch seconds", "1800000000", Value::Null, "Boston"),
        (
            "later but offline only",
            "2030-01-01T00:00:00Z",
            json!(["OfflineStore"]),
            "Seattle",
        ),
    ];

    for (case, event_time, target_stores, expected_city) in cases {
        let group = "customers";
        emulator
            .put_record(
                group,
                json!({ "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", "2024-01-01T00:00:00Z"),
                    feature("city", "Seattle")
                ] }),
            )
            .await;

        let mut request = json!({ "Record": [
            feature("customer_id", "C1"),
            feature("event_time", event_time),
            feature("city", "Boston")
        ] });
        if !target_stores.is_null() {
            request["TargetStores"] = target_stores;
        }
        let (status, _) = emulator.put_record(group, request).await;
        assert_eq!(status, 200, "case {case:?}: expected PutRecord 200, got {status}");

        let (_, body) = emulator
            .get_record(group, "RecordIdentifierValueAsString=C1")
            .await;
        let actual = record_pairs(&body);
        let city = actual
            .iter()
            .find(|(name, _)| name == "city")
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            city,
            json!(expected_city),
            "case {case:?}: expected city {expected_city:?}, got {city:?} (record {actual:?})"
        );

        // Reset the group for the next row.
        emulator
            .delete_record(
                group,
                "RecordIdentifierValueAsString=C1&EventTime=2035-01-01T00:00:00Z&DeletionMode=HardDelete",
            )
            .await;
    }
}

#[tokio::test]
async fn get_record_projects_exactly_the_feature_names_the_caller_asked_for() {
    let emulator = Emulator::new();
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle"),
                feature("plan", "gold")
            ] }),
        )
        .await;

    // "List of names of Features to be retrieved. If not specified, the latest value
    // for all the Features are returned." (GetRecord$FeatureNames, Smithy model doc.)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("no projection", "", vec!["customer_id", "event_time", "city", "plan"]),
        ("one feature", "&FeatureName=city", vec!["city"]),
        (
            "two features, stored order kept",
            "&FeatureName=plan&FeatureName=customer_id",
            vec!["customer_id", "plan"],
        ),
        (
            "every feature named",
            "&FeatureName=customer_id&FeatureName=event_time&FeatureName=city&FeatureName=plan",
            vec!["customer_id", "event_time", "city", "plan"],
        ),
        ("a feature the record has not", "&FeatureName=nosuch", vec![]),
    ];

    for (case, query, expected) in cases {
        let (status, body) = emulator
            .get_record(
                "customers",
                &format!("RecordIdentifierValueAsString=C1{query}"),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual: Vec<String> = record_pairs(&body)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected features {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn ttl_duration_sets_expires_at_to_event_time_plus_the_duration() {
    let emulator = Emulator::new();

    // Every unit the model's `TtlDurationUnit` enum declares (see
    // `TTL_DURATION_UNIT_VALUES` in `ops::types::shared_types`), with the published
    // lower bound of `TtlDurationValue` (range 1..) and a larger value, over both
    // accepted event-time formats. The event time is a day ahead, so no row's expiry
    // is already due.
    let iso = tomorrow_iso();
    let epoch = tomorrow_epoch();
    let cases: Vec<(&str, &str, &str, i64, f64)> = vec![
        ("Seconds, lower bound", &iso, "Seconds", 1, 1.0),
        ("Seconds", &iso, "Seconds", 90, 90.0),
        ("Minutes", &iso, "Minutes", 90, 5_400.0),
        ("Hours", &iso, "Hours", 36, 129_600.0),
        ("Days", &iso, "Days", 1, 86_400.0),
        ("Weeks", &iso, "Weeks", 2, 1_209_600.0),
        ("Days on an epoch event time", &epoch, "Days", 3, 259_200.0),
        ("Weeks, large value", &iso, "Weeks", 52, 31_449_600.0),
    ];

    for (case, event_time, unit, value, expected_delta) in cases {
        let identifier = format!("C-{unit}-{value}");
        let (status, _) = emulator
            .put_record(
                "ttlgroup",
                json!({
                    "Record": [
                        feature("customer_id", &identifier),
                        feature("event_time", event_time),
                        feature("city", "Seattle")
                    ],
                    "TtlDuration": { "Unit": unit, "Value": value }
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected PutRecord 200, got {status}");

        let (status, body) = emulator
            .get_record(
                "ttlgroup",
                &format!(
                    "RecordIdentifierValueAsString={identifier}&ExpirationTimeResponse=Enabled"
                ),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let expires_at = body
            .get("ExpiresAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(
            DateTime::from_str(&expires_at, Format::DateTime).is_ok(),
            "case {case:?}: expected an ISO-8601 ExpiresAt, got {expires_at:?}"
        );
        let delta = secs(&expires_at) - secs(event_time);
        assert!(
            (delta - expected_delta).abs() < 0.001,
            "case {case:?}: expected ExpiresAt {expected_delta} s after the event time, \
             got {delta} s (ExpiresAt {expires_at:?})"
        );

        // "If Enabled, GetRecord will return the value of ExpiresAt": a request that
        // does not enable it gets no ExpiresAt.
        for (sub_case, query) in [
            ("absent", String::new()),
            ("Disabled", "&ExpirationTimeResponse=Disabled".to_owned()),
        ] {
            let (_, body) = emulator
                .get_record(
                    "ttlgroup",
                    &format!("RecordIdentifierValueAsString={identifier}{query}"),
                )
                .await;
            let actual = body.get("ExpiresAt").cloned().unwrap_or(Value::Null);
            assert!(
                actual.is_null(),
                "case {case:?}/{sub_case:?}: expected no ExpiresAt, got {actual:?}"
            );
        }
    }

    // A record written with no TtlDuration has no ExpiresAt at all.
    emulator
        .put_record(
            "ttlgroup",
            json!({ "Record": [
                feature("customer_id", "C-no-ttl"),
                feature("event_time", &iso)
            ] }),
        )
        .await;
    let (_, body) = emulator
        .get_record(
            "ttlgroup",
            "RecordIdentifierValueAsString=C-no-ttl&ExpirationTimeResponse=Enabled",
        )
        .await;
    let actual = body.get("ExpiresAt").cloned().unwrap_or(Value::Null);
    assert!(
        actual.is_null(),
        "case \"no TtlDuration\": expected no ExpiresAt, got {actual:?}"
    );
}

#[tokio::test]
async fn a_record_whose_ttl_has_passed_is_gone_from_the_online_store() {
    let emulator = Emulator::new();

    // An event time far in the past plus a one-second TTL: the expiry is already due,
    // so the record is hard-deleted by the time anything reads the group. The second
    // record's expiry is a year out, so it stays.
    emulator
        .put_record(
            "expiring",
            json!({
                "Record": [
                    feature("customer_id", "C-expired"),
                    feature("event_time", "2000-01-01T00:00:00Z")
                ],
                "TtlDuration": { "Unit": "Seconds", "Value": 1 }
            }),
        )
        .await;
    emulator
        .put_record(
            "expiring",
            json!({
                "Record": [
                    feature("customer_id", "C-live"),
                    feature("event_time", &tomorrow_iso())
                ],
                "TtlDuration": { "Unit": "Weeks", "Value": 52 }
            }),
        )
        .await;

    let (status, body) = emulator
        .get_record("expiring", "RecordIdentifierValueAsString=C-expired")
        .await;
    assert_eq!(status, 200, "expected GetRecord 200, got {status}");
    let actual = body.get("Record").cloned().unwrap_or(Value::Null);
    assert!(
        actual.is_null(),
        "case \"expired record\": expected no Record, got {actual:?}"
    );

    let (_, body) = emulator.list_records("expiring", json!({})).await;
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!(["C-live"]),
        "case \"expired record\": expected only the live identifier, got {listed:?}"
    );
}

#[tokio::test]
async fn list_records_pages_through_the_identifiers_in_state() {
    let emulator = Emulator::new();
    let written = ["C1", "C2", "C3", "C4", "C5"];
    for identifier in written {
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", identifier),
                    feature("event_time", "2024-01-01T00:00:00Z")
                ] }),
            )
            .await;
    }

    // The rule spans every page size the model allows: each page holds at most
    // MaxResults identifiers, NextToken resumes exactly where the page stopped, and
    // the pages together are every identifier in state, once.
    for max_results in [Value::Null, json!(1), json!(2), json!(3), json!(5), json!(100)] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if !max_results.is_null() {
                request["MaxResults"] = max_results.clone();
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.list_records("customers", request).await;
            assert_eq!(status, 200, "case {case:?}: expected ListRecords 200, got {status}");
            let page: Vec<String> = body["RecordIdentifiers"]
                .as_array()
                .expect("RecordIdentifiers is required")
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect();
            if let Some(max) = max_results.as_u64() {
                assert!(
                    page.len() as u64 <= max,
                    "case {case:?}: expected at most {max} identifiers in a page, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(pages <= 10, "case {case:?}: pagination did not terminate");
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if next_token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted,
            written.map(str::to_owned).to_vec(),
            "case {case:?}: expected every identifier exactly once, got {seen:?}"
        );
    }

    // A token this operation never handed out is a client error, not a page.
    let (status, body) = emulator
        .list_records("customers", json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        status, 400,
        "case \"bogus NextToken\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"bogus NextToken\": expected the modelled ValidationError error code, \
         got {code:?}"
    );
}

#[tokio::test]
async fn delete_record_removes_the_record_from_the_online_store() {
    // Rows: the two deletion modes, explicit and defaulted, plus a delete aimed at the
    // OfflineStore alone. `SoftDelete` keeps a tombstone `ListRecords` can still show;
    // `HardDelete` removes the record entirely.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("default (SoftDelete)", "", true),
        ("explicit SoftDelete", "&DeletionMode=SoftDelete", true),
        ("HardDelete", "&DeletionMode=HardDelete", false),
    ];

    for (case, mode, soft_deleted_listable) in cases {
        let emulator = Emulator::new();
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", "2024-01-01T00:00:00Z"),
                    feature("city", "Seattle")
                ] }),
            )
            .await;
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", "C2"),
                    feature("event_time", "2024-01-01T00:00:00Z")
                ] }),
            )
            .await;

        let (status, _) = emulator
            .delete_record(
                "customers",
                &format!(
                    "RecordIdentifierValueAsString=C1&EventTime=2024-01-02T00:00:00Z{mode}"
                ),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: expected DeleteRecord 200, got {status}");

        let (status, body) = emulator
            .get_record("customers", "RecordIdentifierValueAsString=C1")
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual = body.get("Record").cloned().unwrap_or(Value::Null);
        assert!(
            actual.is_null(),
            "case {case:?}: expected no Record after the delete, got {actual:?}"
        );

        let (_, body) = emulator.list_records("customers", json!({})).await;
        let listed = body["RecordIdentifiers"].clone();
        assert_eq!(
            listed,
            json!(["C2"]),
            "case {case:?}: expected the deleted identifier gone from ListRecords, got {listed:?}"
        );

        let (_, body) = emulator
            .list_records("customers", json!({ "IncludeSoftDeletedRecords": true }))
            .await;
        let listed: Vec<String> = body["RecordIdentifiers"]
            .as_array()
            .expect("RecordIdentifiers is required")
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_owned())
            .collect();
        let expected: Vec<String> = if soft_deleted_listable {
            vec!["C1".to_owned(), "C2".to_owned()]
        } else {
            vec!["C2".to_owned()]
        };
        assert_eq!(
            listed, expected,
            "case {case:?}: expected IncludeSoftDeletedRecords to list {expected:?}, got {listed:?}"
        );

        // BatchGetRecord reads the same store, so it no longer serves the record.
        let (_, body) = emulator
            .batch_get_record(json!({ "Identifiers": [
                { "FeatureGroupName": "customers", "RecordIdentifiersValueAsString": ["C1"] }
            ] }))
            .await;
        let records = body["Records"].clone();
        assert_eq!(
            records,
            json!([]),
            "case {case:?}: expected BatchGetRecord to serve no deleted record, got {records:?}"
        );
    }
}

#[tokio::test]
async fn delete_record_with_an_event_time_before_the_records_does_not_delete() {
    // "Note that the EventTime specified in DeleteRecord should be set later than the
    // EventTime of the existing record … If it is not, the deletion does not occur:
    // For SoftDelete, the existing (not deleted) record remains in the OnlineStore …
    // HardDelete returns EventTime: 400 ValidationException." (DeleteRecord doc.) The
    // boundary the service enforces is SAME-or-after, so a delete submitted at exactly
    // the record's own EventTime — the value a client reads back from GetRecord —
    // deletes it, and only a strictly older one does not.
    let cases: Vec<(&str, &str, &str, u16, bool)> = vec![
        ("soft, older", "", "2023-12-31T00:00:00Z", 200, true),
        ("soft, equal", "&DeletionMode=SoftDelete", "2024-01-01T00:00:00Z", 200, false),
        ("hard, older", "&DeletionMode=HardDelete", "2023-12-31T00:00:00Z", 400, true),
        ("hard, equal", "&DeletionMode=HardDelete", "2024-01-01T00:00:00Z", 200, false),
        (
            "hard, one second older",
            "&DeletionMode=HardDelete",
            "2023-12-31T23:59:59Z",
            400,
            true,
        ),
        (
            "hard, one second later",
            "&DeletionMode=HardDelete",
            "2024-01-01T00:00:01Z",
            200,
            false,
        ),
        ("hard, later", "&DeletionMode=HardDelete", "2024-01-02T00:00:00Z", 200, false),
        ("soft, later", "&DeletionMode=SoftDelete", "2024-01-02T00:00:00Z", 200, false),
        // The same instant in the other accepted event-time format: 2024-01-01T00:00:00Z
        // is 1 704 067 200 epoch seconds, so this row is "equal" spelled as a Fractional.
        (
            "hard, equal as epoch seconds",
            "&DeletionMode=HardDelete",
            "1704067200",
            200,
            false,
        ),
    ];

    for (case, mode, event_time, expected_status, still_there) in cases {
        let emulator = Emulator::new();
        emulator
            .put_record(
                "customers",
                json!({ "Record": [
                    feature("customer_id", "C1"),
                    feature("event_time", "2024-01-01T00:00:00Z"),
                    feature("city", "Seattle")
                ] }),
            )
            .await;

        let (status, body) = emulator
            .delete_record(
                "customers",
                &format!("RecordIdentifierValueAsString=C1&EventTime={event_time}{mode}"),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected DeleteRecord {expected_status}, got {status}"
        );
        if expected_status == 400 {
            let code = error_code(&body);
            assert_eq!(
                code, "ValidationError",
                "case {case:?}: expected the modelled ValidationError error code, got {code:?}"
            );
        }

        let (_, body) = emulator
            .get_record("customers", "RecordIdentifierValueAsString=C1")
            .await;
        let served = !body.get("Record").cloned().unwrap_or(Value::Null).is_null();
        assert_eq!(
            served, still_there,
            "case {case:?}: expected the record to be retrievable={still_there}, got {served}"
        );
    }
}

#[tokio::test]
async fn batch_get_record_fans_out_over_the_same_store_get_record_reads() {
    let emulator = Emulator::new();
    let event_time = tomorrow_iso();
    for (identifier, city) in [("C1", "Seattle"), ("C2", "Boston")] {
        emulator
            .put_record(
                "customers",
                json!({
                    "Record": [
                        feature("customer_id", identifier),
                        feature("event_time", &event_time),
                        feature("city", city)
                    ],
                    "TtlDuration": { "Unit": "Weeks", "Value": 52 }
                }),
            )
            .await;
    }
    emulator
        .put_record(
            "orders",
            json!({ "Record": [
                feature("order_id", "O1"),
                feature("event_time", &event_time)
            ] }),
        )
        .await;

    let (status, body) = emulator
        .batch_get_record(json!({
            "Identifiers": [
                { "FeatureGroupName": "customers",
                  "RecordIdentifiersValueAsString": ["C1", "C-missing", "C2"] },
                { "FeatureGroupName": "orders",
                  "RecordIdentifiersValueAsString": ["O1"],
                  "FeatureNames": ["order_id"] },
                { "FeatureGroupName": "nosuchgroup",
                  "RecordIdentifiersValueAsString": ["X1", "X2"] }
            ],
            "ExpirationTimeResponse": "Enabled"
        }))
        .await;
    assert_eq!(status, 200, "expected BatchGetRecord 200, got {status}");

    // All three output members are `@required`, so all three are always present.
    for member in ["Records", "Errors", "UnprocessedIdentifiers"] {
        assert!(
            body.get(member).and_then(Value::as_array).is_some(),
            "case {member:?}: expected the required member to be present, got {body}"
        );
    }
    let unprocessed = body["UnprocessedIdentifiers"].clone();
    assert_eq!(
        unprocessed,
        json!([]),
        "case \"UnprocessedIdentifiers\": expected an empty list, got {unprocessed:?}"
    );

    let served: Vec<(String, String)> = body["Records"]
        .as_array()
        .expect("Records is required")
        .iter()
        .map(|detail| {
            (
                detail["FeatureGroupName"].as_str().unwrap_or_default().to_owned(),
                detail["RecordIdentifierValueAsString"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect();
    let expected = vec![
        ("customers".to_owned(), "C1".to_owned()),
        ("customers".to_owned(), "C2".to_owned()),
        ("orders".to_owned(), "O1".to_owned()),
    ];
    assert_eq!(
        served, expected,
        "case \"Records\": expected every written identifier once {expected:?}, got {served:?}"
    );

    // The record detail carries what the store holds, projected and expiring exactly
    // as GetRecord would answer.
    let c1 = body["Records"]
        .as_array()
        .and_then(|records| {
            records
                .iter()
                .find(|r| r["RecordIdentifierValueAsString"] == json!("C1"))
        })
        .cloned()
        .expect("C1 is served");
    let c1_features = record_pairs(&json!({ "Record": c1["Record"].clone() }));
    let expected_features: Vec<(String, Value)> = vec![
        ("customer_id".to_owned(), json!("C1")),
        ("event_time".to_owned(), json!(event_time)),
        ("city".to_owned(), json!("Seattle")),
    ];
    assert_eq!(
        c1_features, expected_features,
        "case \"C1 record\": expected {expected_features:?}, got {c1_features:?}"
    );
    let expires_at = c1["ExpiresAt"].as_str().unwrap_or_default().to_owned();
    let delta = secs(&expires_at) - secs(&event_time);
    assert!(
        (delta - 31_449_600.0).abs() < 0.001,
        "case \"C1 ExpiresAt\": expected 52 weeks after the event time, got {delta} s \
         (ExpiresAt {expires_at:?})"
    );

    let o1 = body["Records"]
        .as_array()
        .and_then(|records| {
            records
                .iter()
                .find(|r| r["RecordIdentifierValueAsString"] == json!("O1"))
        })
        .cloned()
        .expect("O1 is served");
    let o1_features = record_pairs(&json!({ "Record": o1["Record"].clone() }));
    assert_eq!(
        o1_features,
        vec![("order_id".to_owned(), json!("O1"))],
        "case \"O1 projection\": expected only the named feature, got {o1_features:?}"
    );

    // Every identifier of an entry whose feature group does not resolve is reported per
    // identifier, with the error code this data plane gives an unresolvable name.
    let errors: Vec<(String, String, String)> = body["Errors"]
        .as_array()
        .expect("Errors is required")
        .iter()
        .map(|e| {
            (
                e["FeatureGroupName"].as_str().unwrap_or_default().to_owned(),
                e["RecordIdentifierValueAsString"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                e["ErrorCode"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected_errors = vec![
        (
            "nosuchgroup".to_owned(),
            "X1".to_owned(),
            "ValidationError".to_owned(),
        ),
        (
            "nosuchgroup".to_owned(),
            "X2".to_owned(),
            "ValidationError".to_owned(),
        ),
    ];
    assert_eq!(
        errors, expected_errors,
        "case \"Errors\": expected {expected_errors:?}, got {errors:?}"
    );
}

#[tokio::test]
async fn a_feature_group_arn_and_its_name_reach_one_online_store() {
    let emulator = Emulator::new();
    // The `FeatureGroupNameOrArn` pattern makes the ARN prefix optional, so both forms
    // name one feature group. On the wire the ARN is one `@httpLabel` segment, so its
    // `:` and `/` arrive percent-encoded.
    let arn = "arn:aws:sagemaker:us-east-1:123456789012:feature-group/customers";
    let arn_on_the_wire =
        "arn%3Aaws%3Asagemaker%3Aus-east-1%3A123456789012%3Afeature-group%2Fcustomers";
    emulator
        .put_record(
            arn_on_the_wire,
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z"),
                feature("city", "Seattle")
            ] }),
        )
        .await;

    for (case, group) in [("bare name", "customers"), ("ARN", arn_on_the_wire)] {
        let (status, body) = emulator
            .get_record(group, "RecordIdentifierValueAsString=C1")
            .await;
        assert_eq!(status, 200, "case {case:?}: expected GetRecord 200, got {status}");
        let actual = record_pairs(&body);
        assert_eq!(
            actual.len(),
            3,
            "case {case:?}: expected the record written through the other form, got {actual:?}"
        );
    }

    let (_, body) = emulator.list_records(arn_on_the_wire, json!({})).await;
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!(["C1"]),
        "case \"ListRecords by ARN\" ({arn}): expected the identifier written by name, \
         got {listed:?}"
    );
}

#[tokio::test]
async fn a_feature_group_that_does_not_resolve_is_refused_and_an_absent_record_is_not() {
    let emulator = Emulator::new();

    // A `FeatureGroupName` that names no feature group is a request defect: every
    // operation of this data plane refuses it with the modelled ValidationError (400),
    // never with ResourceNotFound (404), which would say a record is missing from a
    // feature group that exists. The table that spans this rule over every operation and
    // both name spellings is `tests/all/feature_group_resolution.rs`.
    let (status, body) = emulator
        .get_record("nosuchgroup", "RecordIdentifierValueAsString=C1")
        .await;
    assert_eq!(status, 400, "case \"GetRecord\": expected 400, got {status}");
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"GetRecord\": expected the ValidationError error code, got {code:?}"
    );

    let (status, body) = emulator.list_records("nosuchgroup", json!({})).await;
    assert_eq!(status, 400, "case \"ListRecords\": expected 400, got {status}");
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"ListRecords\": expected the ValidationError error code, got {code:?}"
    );

    // An identifier that was never written to a group that DOES exist is an empty
    // result, not an error: "If no Record with RecordIdentifierValue is found, then an
    // empty result is returned."
    emulator
        .put_record(
            "customers",
            json!({ "Record": [feature("customer_id", "C1")] }),
        )
        .await;
    let (status, body) = emulator
        .get_record("customers", "RecordIdentifierValueAsString=C-missing")
        .await;
    assert_eq!(
        status, 200,
        "case \"unknown identifier\": expected 200, got {status}"
    );
    let actual = body.get("Record").cloned().unwrap_or(Value::Null);
    assert!(
        actual.is_null(),
        "case \"unknown identifier\": expected no Record, got {actual:?}"
    );

    // Nor is a group that exists but holds no record yet: it lists nothing and serves
    // nothing, with no error at all.
    emulator.create_feature_group("emptygroup", "customer_id", "event_time", true);
    let (status, body) = emulator.list_records("emptygroup", json!({})).await;
    assert_eq!(
        status, 200,
        "case \"a group with no record yet\": expected ListRecords 200, got {status}"
    );
    let listed = body["RecordIdentifiers"].clone();
    assert_eq!(
        listed,
        json!([]),
        "case \"a group with no record yet\": expected no identifiers, got {listed:?}"
    );

    // DeleteRecord is refused for the same reason the others are: the delete marker it
    // documents belongs to a feature group that exists.
    let (status, body) = emulator
        .delete_record(
            "nosuchgroup",
            "RecordIdentifierValueAsString=C1&EventTime=2024-01-01T00:00:00Z",
        )
        .await;
    assert_eq!(status, 400, "case \"DeleteRecord\": expected 400, got {status}");
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"DeleteRecord\": expected the ValidationError error code, got {code:?}"
    );
}

#[tokio::test]
async fn a_request_outside_the_models_constraints_is_refused_with_400() {
    let emulator = Emulator::new();

    // Each row breaks one modelled constraint of the member it names.
    let too_long = "a".repeat(151); // FeatureGroupName: len 1..150

    for (case, group) in [
        ("FeatureGroupName longer than 150", too_long.as_str()),
        ("FeatureGroupName outside the pattern", "bad!name"),
    ] {
        let (status, _) = emulator
            .put_record(group, json!({ "Record": [feature("customer_id", "C1")] }))
            .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}");
    }

    // `Record` is `len 1..`.
    let (status, _) = emulator
        .put_record("customers", json!({ "Record": [] }))
        .await;
    assert_eq!(status, 400, "case \"empty Record\": expected 400, got {status}");

    // `Identifiers` is `len 1..100`.
    let (status, _) = emulator
        .batch_get_record(json!({ "Identifiers": [] }))
        .await;
    assert_eq!(
        status, 400,
        "case \"empty Identifiers\": expected 400, got {status}"
    );

    // `MaxResults` is `range 1..100`.
    for (case, max_results) in [("below the range", json!(0)), ("above the range", json!(101))] {
        let (status, _) = emulator
            .list_records("customers", json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected MaxResults={max_results} refused with 400, got {status}"
        );
    }

    // A record that does not carry the group's record-identifier feature cannot be
    // stored, and is refused rather than silently dropped.
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z")
            ] }),
        )
        .await;
    let (status, body) = emulator
        .put_record(
            "customers",
            json!({ "Record": [feature("city", "Seattle")] }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"record without the identifier feature\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ValidationError",
        "case \"record without the identifier feature\": expected the modelled \
         ValidationError error code, got {code:?}"
    );
}

#[tokio::test]
async fn a_value_outside_an_enum_or_a_fabricated_page_token_is_refused_with_400() {
    let emulator = Emulator::new();
    emulator
        .put_record(
            "customers",
            json!({ "Record": [
                feature("customer_id", "C1"),
                feature("event_time", "2024-01-01T00:00:00Z")
            ] }),
        )
        .await;

    // Every enum-typed member of this service, with a value the model's value list
    // does not declare — a typo must not be read as "not the value I compare against"
    // and silently change what the call does. Sources: `TARGET_STORE_VALUES`,
    // `DELETION_MODE_VALUES`, `EXPIRATION_TIME_RESPONSE_VALUES` in
    // `src/ops/types/shared_types.rs`, rendered from the model's enums. The refusal is
    // a 400 whether the boundary validator catches the value (`ValidationException`)
    // or the handler does (the modelled `ValidationError`).
    let (status, body) = emulator
        .put_record(
            "customers",
            json!({
                "Record": [feature("customer_id", "C1")],
                "TargetStores": ["Online"]
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"PutRecord TargetStores typo\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert!(
        is_client_refusal(&code),
        "case \"PutRecord TargetStores typo\": expected a validation error code, got {code:?}"
    );

    for (case, query) in [
        (
            "DeleteRecord DeletionMode typo",
            "RecordIdentifierValueAsString=C1&EventTime=2024-01-02T00:00:00Z&DeletionMode=Hard",
        ),
        (
            "DeleteRecord TargetStores typo",
            "RecordIdentifierValueAsString=C1&EventTime=2024-01-02T00:00:00Z&TargetStores=Online",
        ),
        (
            "DeleteRecord EventTime that is no timestamp",
            "RecordIdentifierValueAsString=C1&EventTime=yesterday",
        ),
    ] {
        let (status, body) = emulator.delete_record("customers", query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}");
        let code = error_code(&body);
        assert!(
            is_client_refusal(&code),
            "case {case:?}: expected a validation error code, got {code:?}"
        );
    }

    let (status, body) = emulator
        .get_record(
            "customers",
            "RecordIdentifierValueAsString=C1&ExpirationTimeResponse=On",
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"GetRecord ExpirationTimeResponse typo\": expected 400, got {status}"
    );
    let code = error_code(&body);
    assert!(
        is_client_refusal(&code),
        "case \"GetRecord ExpirationTimeResponse typo\": expected a validation error code, \
         got {code:?}"
    );

    let (status, _) = emulator
        .batch_get_record(json!({
            "Identifiers": [
                { "FeatureGroupName": "customers", "RecordIdentifiersValueAsString": ["C1"] }
            ],
            "ExpirationTimeResponse": "On"
        }))
        .await;
    assert_eq!(
        status, 400,
        "case \"BatchGetRecord ExpirationTimeResponse typo\": expected 400, got {status}"
    );

    // A page token the caller fabricated is refused even when it is well-formed hex:
    // only a token this operation issued resumes a page.
    let fabricated = hex::encode("C1".as_bytes());
    for (case, token) in [
        ("not hex at all", "not-a-token".to_owned()),
        ("valid hex, never issued", fabricated),
    ] {
        let (status, body) = emulator
            .list_records("customers", json!({ "NextToken": token }))
            .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}");
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected ValidationError, got {code:?}"
        );
    }
}

#[tokio::test]
async fn the_online_store_refuses_a_write_past_its_capacity_bound() {
    use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::state::MAX_FEATURE_GROUPS;

    let emulator = Emulator::new();
    let record = json!({ "Record": [
        feature("customer_id", "C1"),
        feature("event_time", "2024-01-01T00:00:00Z")
    ] });

    // The store is process memory, so it holds a bounded number of feature groups. Each
    // one has to exist in the control plane before a record can reach it.
    for index in 0..MAX_FEATURE_GROUPS {
        emulator.create_feature_group(
            &format!("group{index}"),
            "customer_id",
            "event_time",
            true,
        );
    }
    emulator.create_feature_group("onegrouptoomany", "customer_id", "event_time", true);
    for index in 0..MAX_FEATURE_GROUPS {
        let (status, _) = emulator
            .put_record(&format!("group{index}"), record.clone())
            .await;
        assert_eq!(
            status, 200,
            "case \"group {index} of {MAX_FEATURE_GROUPS}\": expected PutRecord 200, got {status}"
        );
    }

    // One past the bound is refused as a server-side capacity limit (503), not as the
    // caller's mistake (400) — the request is well-formed.
    let (status, body) = emulator
        .put_record("onegrouptoomany", record.clone())
        .await;
    assert_eq!(
        status, 503,
        "case \"one group past the bound\": expected 503, got {status}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ServiceUnavailable",
        "case \"one group past the bound\": expected ServiceUnavailable, got {code:?}"
    );

    // A group already in the store still takes writes.
    let (status, _) = emulator.put_record("group0", record).await;
    assert_eq!(
        status, 200,
        "case \"an existing group at the bound\": expected PutRecord 200, got {status}"
    );
}

#[tokio::test]
async fn every_operation_of_this_slice_answers_from_the_store() {
    let emulator = Emulator::new();

    // Every operation of this data plane is implemented, so none of them answers the
    // stub's 501 any more — `BatchWriteRecord` last (see
    // `tests/all/batch_write_record_bulk_ingestion.rs` for its own rules).
    let (status, body) = emulator
        .send(
            "POST",
            "/BatchWriteRecord",
            Some(json!({ "Entries": [
                { "FeatureGroupName": "customers",
                  "Record": [feature("customer_id", "C1")] }
            ] })),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (200, ""),
        "case \"BatchWriteRecord\": expected HTTP 200, got {status} with body {body:?}"
    );
    assert_eq!(
        body["Errors"],
        json!([]),
        "case \"BatchWriteRecord\": expected no per-entry error, got {body:?}"
    );

    // And what it wrote reads back through `GetRecord`.
    let (status, body) = emulator
        .get_record("customers", "RecordIdentifierValueAsString=C1")
        .await;
    assert_eq!(
        (status, record_pairs(&body)),
        (200, vec![("customer_id".to_owned(), json!("C1"))]),
        "case \"GetRecord after BatchWriteRecord\": expected the written record, got {status} \
         with body {body:?}"
    );
}
