// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `BatchWriteRecord`: the bulk ingestion of records into one
//! or more feature groups.
//!
//! The rules asserted here are the ones the operation documents
//! (`smithy_query(query="doc", shape="BatchWriteRecord")` and
//! `smithy_query(query="shape", shape="BatchWriteRecordEntry")`):
//!
//! * every entry of the batch is ingested into the feature group it names, by the same
//!   rules one `PutRecord` of the same record follows — the record's identifier is the
//!   value of the group's own `RecordIdentifierFeatureName`, the write is a full
//!   over-write, and a historic entry leaves the stored record standing;
//! * an entry this data plane cannot write is reported in `Errors` as that ONE entry's
//!   failure and leaves every other entry of the batch written;
//! * "A request level TtlDuration applies to all entries that do not specify their own
//!   TtlDuration", an entry's own `TtlDuration` overrides it, and `ExpiresAt = EventTime
//!   + TtlDuration`;
//! * `TargetStores` decides whether the OnlineStore this service serves takes the entry
//!   at all.
//!
//! Each test is a TABLE spanning the class its rule generalises over — how many entries
//! a batch carries, how many feature groups they name, which spelling names a group,
//! the kind of defect an entry carries, where its TTL comes from, every declared TTL
//! unit, and the two accepted event-time formats — and the recorded wire case is one row.
//! Every case goes through the real front door: the multi-service router, the restJson1
//! protocol and the modelled constraint validation, so what is asserted is the wire
//! behaviour a client sees rather than a handler's return value.

use std::sync::Arc;

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The feature group every row writes to, and its two key feature names as its
/// definition fixes them.
const GROUP: &str = "customers";
const IDENTIFIER_FEATURE: &str = "customer_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// A second feature group, with a DIFFERENT record-identifier feature, so a batch
/// spanning two groups cannot be served by reading one group's definition twice.
const OTHER_GROUP: &str = "orders";
const OTHER_IDENTIFIER_FEATURE: &str = "order_id";

/// A feature group created with no `OnlineStore` — the documented default of
/// `EnableOnlineStore`.
const OFFLINE_GROUP: &str = "offlineonly";

/// An event time well in the future, so no row's expiry is already due when the record
/// is read back.
const EVENT_TIME: &str = "2035-01-01T00:00:00Z";

/// The prefix of every feature-group ARN this emulator mints: the account id
/// (`engine::EMULATOR_ACCOUNT_ID`) and the region the suite's requests are signed for.
const ARN_PREFIX: &str = "arn:aws:sagemaker:us-east-1:000000000000:feature-group/";

/// One `(FeatureName, Value)` pair of a record, as [`record_pairs`] reports it.
type Pair = (String, Value);

/// One row of the bulk-ingestion table: the case name, the `Entries` the batch carries,
/// and the records the OnlineStore must hold afterwards as
/// `(FeatureGroupName, RecordIdentifierValueAsString, the record's pairs)`.
type BulkCase<'a> = (&'a str, Value, Vec<(&'a str, String, Vec<Pair>)>);

/// One row of the per-entry failure table: the case name, the defective entry, the
/// `ErrorCode` it must be reported with and the part of the refusal its `ErrorMessage`
/// must state.
type FailureCase<'a> = (&'a str, Value, &'a str, String);

/// One row of the TTL table: the case name, the request-level `TtlDuration`, the
/// `TtlDuration` the FIRST entry names, and the seconds after the event time `ExpiresAt`
/// must fall for that entry and for a second entry that never names one of its own
/// (`None` for a record that must carry no `ExpiresAt` at all).
type TtlCase<'a> = (&'a str, Value, Value, Option<f64>, Option<f64>);

/// A booted emulator holding this service only.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A booted emulator with the feature groups this suite writes to already created.
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
        emulator.create_feature_group(GROUP, IDENTIFIER_FEATURE, true);
        emulator.create_feature_group(OTHER_GROUP, OTHER_IDENTIFIER_FEATURE, true);
        emulator.create_feature_group(OFFLINE_GROUP, IDENTIFIER_FEATURE, false);
        emulator
    }

    /// Create a feature group the way the SageMaker control plane's `CreateFeatureGroup`
    /// does: by registering its definition — its two key feature names and whether its
    /// `OnlineStore` is on — in the ONE shared feature-group directory this data plane
    /// resolves names against.
    fn create_feature_group(
        &self,
        name: &str,
        record_identifier_feature_name: &str,
        online_store_enabled: bool,
    ) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!("arn:aws:sagemaker:us-east-1:123456789012:feature-group/{name}"),
                record_identifier_feature_name: record_identifier_feature_name.to_owned(),
                event_time_feature_name: EVENT_TIME_FEATURE.to_owned(),
                online_store_enabled,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
            });
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
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

    async fn batch_write_record(&self, body: Value) -> (u16, Value) {
        self.send("POST", "/BatchWriteRecord", Some(body)).await
    }

    async fn get_record(&self, group: &str, identifier: &str) -> (u16, Value) {
        self.send(
            "GET",
            &format!(
                "/FeatureGroup/{group}?RecordIdentifierValueAsString={identifier}\
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

/// A `BatchWriteRecordEntry` for `group`, whose record carries the group's identifier
/// feature, its event-time feature and one ordinary feature.
fn entry(group: &str, identifier_feature: &str, identifier: &str, city: &str) -> Value {
    json!({ "FeatureGroupName": group, "Record": [
        feature(identifier_feature, identifier),
        feature(EVENT_TIME_FEATURE, EVENT_TIME),
        feature("city", city)
    ] })
}

/// The pairs an [`entry`]'s record reads back as.
fn entry_pairs(identifier_feature: &str, identifier: &str, city: &str) -> Vec<Pair> {
    vec![
        (identifier_feature.to_owned(), json!(identifier)),
        (EVENT_TIME_FEATURE.to_owned(), json!(EVENT_TIME)),
        ("city".to_owned(), json!(city)),
    ]
}

/// The `Record` member of a `GetRecord` response, as `(FeatureName, Value)` pairs where
/// the value is the scalar or the list the response carried.
fn record_pairs(body: &Value) -> Vec<Pair> {
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

/// The `Errors` a `BatchWriteRecord` response reports, as
/// `(ErrorCode, ErrorMessage, the Entry it blames)`.
fn reported_errors(body: &Value) -> Vec<(String, String, Value)> {
    body.get("Errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|error| {
                    (
                        error["ErrorCode"].as_str().unwrap_or_default().to_owned(),
                        error["ErrorMessage"].as_str().unwrap_or_default().to_owned(),
                        error.get("Entry").cloned().unwrap_or(Value::Null),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An event time, as the instant an ISO-8601 or epoch-seconds value denotes.
fn secs(iso_or_epoch: &str) -> f64 {
    DateTime::from_str(iso_or_epoch, Format::DateTime)
        .map(|parsed| parsed.as_secs_f64())
        .unwrap_or_else(|_| iso_or_epoch.parse::<f64>().expect("a parsable event time"))
}

/// `secs` as the ISO-8601 string an event-time feature of type `String` carries.
fn iso(at: f64) -> String {
    DateTime::from_secs_f64(at)
        .fmt(Format::DateTime)
        .expect("format an ISO-8601 event time")
}

#[tokio::test]
async fn every_entry_of_a_batch_is_ingested_into_the_feature_group_it_names() {
    // The class the rule spans: how many entries a batch carries, how many feature
    // groups they name, which modelled spelling names a group, and the shape of the
    // values a record carries. The recorded bulk-ingestion case is the "several entries"
    // row.
    let arn = format!("{ARN_PREFIX}{GROUP}");
    let mut cases: Vec<BulkCase> = vec![
        (
            "one entry",
            json!({ "Entries": [entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle")] }),
            vec![(
                GROUP,
                "C1".to_owned(),
                entry_pairs(IDENTIFIER_FEATURE, "C1", "Seattle"),
            )],
        ),
        (
            "three entries into one feature group",
            json!({ "Entries": [
                entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle"),
                entry(GROUP, IDENTIFIER_FEATURE, "C2", "Boston"),
                entry(GROUP, IDENTIFIER_FEATURE, "C3", "Austin")
            ] }),
            vec![
                (
                    GROUP,
                    "C1".to_owned(),
                    entry_pairs(IDENTIFIER_FEATURE, "C1", "Seattle"),
                ),
                (
                    GROUP,
                    "C2".to_owned(),
                    entry_pairs(IDENTIFIER_FEATURE, "C2", "Boston"),
                ),
                (
                    GROUP,
                    "C3".to_owned(),
                    entry_pairs(IDENTIFIER_FEATURE, "C3", "Austin"),
                ),
            ],
        ),
        (
            // Each group's record identifier is ITS OWN `RecordIdentifierFeatureName`,
            // so a batch spanning two groups is read against two definitions.
            "entries spread across two feature groups",
            json!({ "Entries": [
                entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle"),
                entry(OTHER_GROUP, OTHER_IDENTIFIER_FEATURE, "O1", "Denver")
            ] }),
            vec![
                (
                    GROUP,
                    "C1".to_owned(),
                    entry_pairs(IDENTIFIER_FEATURE, "C1", "Seattle"),
                ),
                (
                    OTHER_GROUP,
                    "O1".to_owned(),
                    entry_pairs(OTHER_IDENTIFIER_FEATURE, "O1", "Denver"),
                ),
            ],
        ),
        (
            // The ARN and the bare name denote one feature group, so the entry reaches
            // the store the bare name reads.
            "an entry naming its feature group by the ARN the control plane minted",
            json!({ "Entries": [entry(&arn, IDENTIFIER_FEATURE, "C9", "Reno")] }),
            vec![(
                GROUP,
                "C9".to_owned(),
                entry_pairs(IDENTIFIER_FEATURE, "C9", "Reno"),
            )],
        ),
        (
            // "This will be a full over-write": a collection-valued feature round-trips
            // as the list it was written as.
            "an entry whose record carries a list-valued feature",
            json!({ "Entries": [{ "FeatureGroupName": GROUP, "Record": [
                feature(IDENTIFIER_FEATURE, "C4"),
                feature(EVENT_TIME_FEATURE, EVENT_TIME),
                { "FeatureName": "tags", "ValueAsStringList": ["a", "b", "c"] }
            ] }] }),
            vec![(
                GROUP,
                "C4".to_owned(),
                vec![
                    (IDENTIFIER_FEATURE.to_owned(), json!("C4")),
                    (EVENT_TIME_FEATURE.to_owned(), json!(EVENT_TIME)),
                    ("tags".to_owned(), json!(["a", "b", "c"])),
                ],
            )],
        ),
        (
            // A record with no value for the group's event-time feature has no event
            // time to order against, and is stored all the same.
            "an entry whose record carries no event time",
            json!({ "Entries": [{ "FeatureGroupName": GROUP, "Record": [
                feature(IDENTIFIER_FEATURE, "C5"),
                feature("city", "Tulsa")
            ] }] }),
            vec![(
                GROUP,
                "C5".to_owned(),
                vec![
                    (IDENTIFIER_FEATURE.to_owned(), json!("C5")),
                    ("city".to_owned(), json!("Tulsa")),
                ],
            )],
        ),
    ];

    // `Entries` is modelled `len 1..25`, so the largest batch the API admits is its own
    // row: every one of the 25 entries is ingested, not just the first.
    let widest: Vec<Value> = (0..25)
        .map(|index| {
            entry(
                GROUP,
                IDENTIFIER_FEATURE,
                &format!("W{index}"),
                &format!("city-{index}"),
            )
        })
        .collect();
    let widest_expected: Vec<(&str, String, Vec<Pair>)> = (0..25)
        .map(|index| {
            let identifier = format!("W{index}");
            let pairs = entry_pairs(IDENTIFIER_FEATURE, &identifier, &format!("city-{index}"));
            (GROUP, identifier, pairs)
        })
        .collect();
    cases.push((
        "25 entries, the modelled maximum",
        json!({ "Entries": widest }),
        widest_expected,
    ));

    for (case, body, expected) in cases {
        let emulator = Emulator::new();
        let (status, reply) = emulator.batch_write_record(body).await;
        assert_eq!(
            (status, error_code(&reply).as_str()),
            (200, ""),
            "case {case:?}: expected HTTP 200, got {status} with body {reply:?}"
        );
        let errors = reported_errors(&reply);
        assert!(
            errors.is_empty(),
            "case {case:?}: expected no per-entry error, got {errors:?}"
        );
        assert_eq!(
            reply["UnprocessedEntries"],
            json!([]),
            "case {case:?}: expected every entry to be processed, got {reply:?}"
        );

        for (group, identifier, pairs) in expected {
            let (status, body) = emulator.get_record(group, &identifier).await;
            let served = record_pairs(&body);
            assert_eq!(
                (status, served.clone()),
                (200, pairs.clone()),
                "case {case:?}: expected {group}/{identifier} to read back as {pairs:?}, got \
                 {status} with {served:?} in {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_entry_that_cannot_be_written_is_reported_in_errors_and_the_rest_of_the_batch_is_written()
{
    // The class the rule spans: every kind of defect ONE entry of a batch can carry.
    // Each row puts the defective entry BETWEEN two sound ones, so the rule asserted is
    // that the batch goes on either side of it. The recorded partial-failure case is the
    // unresolvable-feature-group row.
    let cases: Vec<FailureCase> = vec![
        (
            // A feature group exists only because `CreateFeatureGroup` created one, so an
            // entry naming one that was never created is reported as that entry's own
            // failure, stating that the feature group could not be found and naming it by
            // the ARN a group of that name would carry.
            "an entry naming a feature group that was never created",
            entry("nosuchgroup", IDENTIFIER_FEATURE, "X1", "Nowhere"),
            "ValidationError",
            format!(
                "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name \
                 [{ARN_PREFIX}nosuchgroup]."
            ),
        ),
        (
            // The ARN and the bare name denote one feature group, so an entry naming the
            // missing group either way is reported the same way.
            "an entry naming, as an ARN, a feature group that was never created",
            entry(
                &format!("{ARN_PREFIX}nosuchgroup"),
                IDENTIFIER_FEATURE,
                "X1",
                "Nowhere",
            ),
            "ValidationError",
            format!(
                "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name \
                 [{ARN_PREFIX}nosuchgroup]."
            ),
        ),
        (
            // The record's identifier is the value of the group's own
            // `RecordIdentifierFeatureName`, so a record carrying no value for it
            // identifies nothing to write.
            "an entry whose record omits the group's record-identifier feature",
            json!({ "FeatureGroupName": GROUP, "Record": [
                feature(EVENT_TIME_FEATURE, EVENT_TIME),
                feature("city", "Nowhere")
            ] }),
            "ValidationError",
            IDENTIFIER_FEATURE.to_owned(),
        ),
        (
            "an entry whose record-identifier feature carries a list",
            json!({ "FeatureGroupName": GROUP, "Record": [
                { "FeatureName": IDENTIFIER_FEATURE, "ValueAsStringList": ["X1"] },
                feature(EVENT_TIME_FEATURE, EVENT_TIME),
                feature("city", "Nowhere")
            ] }),
            "ValidationError",
            "ValueAsString".to_owned(),
        ),
        (
            // `ExpiresAt` and the historic-record rule are both computed from the event
            // time, so a value in neither accepted format is no event time at all.
            "an entry whose event time is in neither accepted format",
            json!({ "FeatureGroupName": GROUP, "Record": [
                feature(IDENTIFIER_FEATURE, "X1"),
                feature(EVENT_TIME_FEATURE, "last Tuesday"),
                feature("city", "Nowhere")
            ] }),
            "ValidationError",
            "EventTime".to_owned(),
        ),
    ];

    for (case, defective, expected_code, expected_message) in cases {
        let emulator = Emulator::new();
        let (status, reply) = emulator
            .batch_write_record(json!({ "Entries": [
                entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle"),
                defective.clone(),
                entry(OTHER_GROUP, OTHER_IDENTIFIER_FEATURE, "O1", "Denver")
            ] }))
            .await;

        // The batch itself succeeds: a failure that concerns one entry is reported in
        // `Errors`, never as the status of the call.
        assert_eq!(
            (status, error_code(&reply).as_str()),
            (200, ""),
            "case {case:?}: expected HTTP 200 with the failure reported per entry, got \
             {status} with body {reply:?}"
        );
        let errors = reported_errors(&reply);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected exactly one reported error, got {errors:?}"
        );
        let (code, message, blamed) = errors.into_iter().next().expect("one reported error");
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the {expected_code:?} error code, got {code:?} with \
             message {message:?}"
        );
        assert!(
            message.contains(&expected_message),
            "case {case:?}: expected the ErrorMessage to state {expected_message:?}, got \
             {message:?}"
        );
        assert_eq!(
            blamed, defective,
            "case {case:?}: expected the reported Entry to be the entry that failed, got \
             {blamed:?}"
        );
        assert_eq!(
            reply["UnprocessedEntries"],
            json!([]),
            "case {case:?}: expected no unprocessed entry, got {reply:?}"
        );

        // The entries on either side of the defective one are written all the same.
        for (group, identifier_feature, identifier, city) in [
            (GROUP, IDENTIFIER_FEATURE, "C1", "Seattle"),
            (OTHER_GROUP, OTHER_IDENTIFIER_FEATURE, "O1", "Denver"),
        ] {
            let (status, body) = emulator.get_record(group, identifier).await;
            let served = record_pairs(&body);
            let expected = entry_pairs(identifier_feature, identifier, city);
            assert_eq!(
                (status, served.clone()),
                (200, expected.clone()),
                "case {case:?}: expected the sound entry {group}/{identifier} to be written \
                 as {expected:?}, got {status} with {served:?} in {body:?}"
            );
        }

        // And nothing was written for the defective entry.
        let (status, body) = emulator.get_record(GROUP, "X1").await;
        let served = record_pairs(&body);
        assert_eq!(
            (status, served.clone()),
            (200, Vec::new()),
            "case {case:?}: expected the refused entry to store nothing, got {status} with \
             {served:?} in {body:?}"
        );
    }
}

#[tokio::test]
async fn the_request_level_ttl_applies_to_every_entry_that_names_none_of_its_own() {
    // The class the rule spans: where an entry's TTL comes from, crossed with every unit
    // the model's `TtlDurationUnit` declares (Seconds, Minutes, Hours, Days, Weeks — the
    // list `smithy_query(query="shape", shape="TtlDurationUnit")` reports), including
    // units no recorded case carries.
    let cases: Vec<TtlCase> = vec![
        ("no TtlDuration at all", Value::Null, Value::Null, None, None),
        (
            "a request-level TtlDuration only, in seconds",
            json!({ "Unit": "Seconds", "Value": 1 }),
            Value::Null,
            Some(1.0),
            Some(1.0),
        ),
        (
            "a request-level TtlDuration only, in minutes",
            json!({ "Unit": "Minutes", "Value": 90 }),
            Value::Null,
            Some(5_400.0),
            Some(5_400.0),
        ),
        (
            "a request-level TtlDuration only, in hours",
            json!({ "Unit": "Hours", "Value": 36 }),
            Value::Null,
            Some(129_600.0),
            Some(129_600.0),
        ),
        (
            // An entry-level duration applies to THAT entry only, so the entry beside it
            // carries no expiry at all.
            "an entry-level TtlDuration only, in days",
            Value::Null,
            json!({ "Unit": "Days", "Value": 2 }),
            Some(172_800.0),
            None,
        ),
        (
            "an entry-level TtlDuration only, in weeks",
            Value::Null,
            json!({ "Unit": "Weeks", "Value": 52 }),
            Some(31_449_600.0),
            None,
        ),
        (
            // "This overrides the request level TtlDuration" — for the entry that names
            // one; the entry that does not still takes the request's.
            "both, so the entry's own duration is the one that applies to it",
            json!({ "Unit": "Weeks", "Value": 1 }),
            json!({ "Unit": "Hours", "Value": 1 }),
            Some(3_600.0),
            Some(604_800.0),
        ),
    ];

    for (case, request_ttl, entry_ttl, expected_first, expected_second) in cases {
        let emulator = Emulator::new();
        let mut first = entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle");
        if let Some(ttl) = entry_ttl.as_object() {
            first["TtlDuration"] = Value::Object(ttl.clone());
        }
        // A SECOND entry, never carrying a TTL of its own, so the request-level duration
        // is asserted to apply to "all entries that do not specify their own".
        let second = entry(GROUP, IDENTIFIER_FEATURE, "C2", "Boston");
        let mut body = json!({ "Entries": [first, second] });
        if let Some(ttl) = request_ttl.as_object() {
            body["TtlDuration"] = Value::Object(ttl.clone());
        }

        let (status, reply) = emulator.batch_write_record(body).await;
        assert_eq!(
            (status, error_code(&reply).as_str()),
            (200, ""),
            "case {case:?}: expected HTTP 200, got {status} with body {reply:?}"
        );

        // The entry that may carry its own duration, and the one that never does.
        for (identifier, seconds) in [("C1", expected_first), ("C2", expected_second)] {
            let (status, body) = emulator.get_record(GROUP, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{identifier}: expected the record to read back, got {status} \
                 with body {body:?}"
            );
            let expires_at = body.get("ExpiresAt").and_then(Value::as_str).map(str::to_owned);
            match seconds {
                None => assert!(
                    expires_at.is_none(),
                    "case {case:?}/{identifier}: expected no ExpiresAt, got {expires_at:?}"
                ),
                Some(seconds) => {
                    let at = expires_at.clone().unwrap_or_default();
                    let delta = secs(&at) - secs(EVENT_TIME);
                    assert!(
                        (delta - seconds).abs() < 0.001,
                        "case {case:?}/{identifier}: expected ExpiresAt {seconds} s after the \
                         event time, got {delta} s (ExpiresAt {expires_at:?})"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_historic_entry_leaves_the_stored_record_standing() {
    // The class the rule spans: how the entry's event time compares with the stored
    // record's, in both accepted event-time formats — ISO-8601 and seconds since the
    // epoch, which must order against each other exactly as two values of one format do.
    //
    // (case, the second write's event time, the city the record must hold afterwards)
    let stored = secs(EVENT_TIME);
    let cases: Vec<(&str, String, &str)> = vec![
        ("an earlier event time", iso(stored - 86_400.0), "first"),
        ("the same event time", EVENT_TIME.to_owned(), "first"),
        ("a later event time", iso(stored + 86_400.0), "second"),
        (
            "an earlier event time, as seconds since the epoch",
            format!("{}", stored - 86_400.0),
            "first",
        ),
        (
            "a later event time, as seconds since the epoch",
            format!("{}", stored + 86_400.0),
            "second",
        ),
    ];

    for (case, event_time, expected_city) in cases {
        let emulator = Emulator::new();
        let (status, reply) = emulator
            .batch_write_record(
                json!({ "Entries": [entry(GROUP, IDENTIFIER_FEATURE, "C1", "first")] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the seeding batch to succeed, got {status} with body \
             {reply:?}"
        );

        let (status, reply) = emulator
            .batch_write_record(json!({ "Entries": [{
                "FeatureGroupName": GROUP,
                "Record": [
                    feature(IDENTIFIER_FEATURE, "C1"),
                    feature(EVENT_TIME_FEATURE, &event_time),
                    feature("city", "second")
                ]
            }] }))
            .await;
        // A historic entry is written only to the OfflineStore, which this emulator does
        // not model: it is not a failure of the entry, so it is reported in neither
        // `Errors` nor `UnprocessedEntries`.
        assert_eq!(
            (status, error_code(&reply).as_str()),
            (200, ""),
            "case {case:?}: expected HTTP 200, got {status} with body {reply:?}"
        );
        assert_eq!(
            (reply["Errors"].clone(), reply["UnprocessedEntries"].clone()),
            (json!([]), json!([])),
            "case {case:?}: expected no reported failure, got {reply:?}"
        );

        let (_, body) = emulator.get_record(GROUP, "C1").await;
        let served = record_pairs(&body);
        let city = served
            .iter()
            .find(|(name, _)| name == "city")
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            city,
            json!(expected_city),
            "case {case:?}: expected the record to hold city {expected_city:?}, got {city:?} \
             in {body:?}"
        );
    }
}

#[tokio::test]
async fn target_stores_decides_whether_the_online_store_takes_the_entry() {
    // "A list of stores to which you're adding the record. By default, Feature Store
    // adds the record to all of the stores that you're using for the FeatureGroup." This
    // service is the OnlineStore data plane, so the class the rule spans is every
    // `TargetStores` list the modelled `len 1..2` admits, plus its absence.
    //
    // (case, the TargetStores the entry names, whether the OnlineStore must hold it)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("no TargetStores at all", Value::Null, true),
        ("the OnlineStore alone", json!(["OnlineStore"]), true),
        (
            "both stores",
            json!(["OnlineStore", "OfflineStore"]),
            true,
        ),
        (
            "both stores, the offline one first",
            json!(["OfflineStore", "OnlineStore"]),
            true,
        ),
        ("the OfflineStore alone", json!(["OfflineStore"]), false),
    ];

    for (case, target_stores, online) in cases {
        let emulator = Emulator::new();
        let mut written = entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle");
        if !target_stores.is_null() {
            written["TargetStores"] = target_stores.clone();
        }
        let (status, reply) = emulator
            .batch_write_record(json!({ "Entries": [written] }))
            .await;
        assert_eq!(
            (status, error_code(&reply).as_str()),
            (200, ""),
            "case {case:?}: expected HTTP 200, got {status} with body {reply:?}"
        );
        assert_eq!(
            (reply["Errors"].clone(), reply["UnprocessedEntries"].clone()),
            (json!([]), json!([])),
            "case {case:?}: expected no reported failure, got {reply:?}"
        );

        let (status, body) = emulator.get_record(GROUP, "C1").await;
        let served = record_pairs(&body);
        let expected = if online {
            entry_pairs(IDENTIFIER_FEATURE, "C1", "Seattle")
        } else {
            Vec::new()
        };
        assert_eq!(
            (status, served.clone()),
            (200, expected.clone()),
            "case {case:?}: expected the OnlineStore to hold {expected:?}, got {status} with \
             {served:?} in {body:?}"
        );
    }
}

#[tokio::test]
async fn an_entry_written_to_a_feature_group_with_no_online_store_is_not_a_failure() {
    // A write to a group that HAS no `OnlineStore` is accepted and replicated to the
    // group's OfflineStore where it has one (this group has none) — so it leaves the online
    // store alone and is reported in neither `Errors` nor `UnprocessedEntries`, exactly
    // as an entry naming only the OfflineStore is.
    let emulator = Emulator::new();
    let (status, reply) = emulator
        .batch_write_record(json!({ "Entries": [
            entry(OFFLINE_GROUP, IDENTIFIER_FEATURE, "C1", "Seattle"),
            entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle")
        ] }))
        .await;
    assert_eq!(
        (status, error_code(&reply).as_str()),
        (200, ""),
        "case \"a group with no OnlineStore\": expected HTTP 200, got {status} with body \
         {reply:?}"
    );
    assert_eq!(
        (reply["Errors"].clone(), reply["UnprocessedEntries"].clone()),
        (json!([]), json!([])),
        "case \"a group with no OnlineStore\": expected no reported failure, got {reply:?}"
    );

    // There is no online store for a READ of that group to serve, so the read is refused
    // — which is how this suite observes that the entry stored nothing online.
    let (status, body) = emulator.get_record(OFFLINE_GROUP, "C1").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationError"),
        "case \"a group with no OnlineStore\": expected the read to be refused, got {status} \
         with body {body:?}"
    );

    // And the entry beside it, for a group that HAS an online store, was written.
    let (status, body) = emulator.get_record(GROUP, "C1").await;
    let served = record_pairs(&body);
    let expected = entry_pairs(IDENTIFIER_FEATURE, "C1", "Seattle");
    assert_eq!(
        (status, served.clone()),
        (200, expected.clone()),
        "case \"a group with no OnlineStore\": expected the entry beside it to be written as \
         {expected:?}, got {status} with {served:?} in {body:?}"
    );
}

#[tokio::test]
async fn a_request_level_ttl_duration_in_no_declared_unit_refuses_the_whole_request() {
    // The request-level `TtlDuration` "applies to all entries that do not specify their
    // own TtlDuration", so a unit outside the modelled `TtlDurationUnit` is a defect of
    // the REQUEST and not of one entry: it is refused with the modelled `ValidationError`
    // rather than reported per entry, and nothing of the batch is written.
    //
    // The modelled enum is refused at the boundary for a string member; this asserts the
    // handler's own guard answers the same classification.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .batch_write_record(json!({
            "Entries": [entry(GROUP, IDENTIFIER_FEATURE, "C1", "Seattle")],
            "TtlDuration": { "Unit": "Fortnights", "Value": 2 }
        }))
        .await;
    let code = error_code(&body);
    // Either layer may be the one that refuses it — the boundary validator for the
    // modelled enum, or the handler's own guard — and both state a 400 the caller reads
    // as its own mistake.
    assert!(
        status == 400 && (code == "ValidationException" || code == "ValidationError"),
        "case \"a request-level TtlDuration in no declared unit\": expected HTTP 400 with a \
         validation error code, got {status} {code:?} with body {body:?}"
    );

    let (status, body) = emulator.get_record(GROUP, "C1").await;
    let served = record_pairs(&body);
    assert_eq!(
        (status, served.clone()),
        (200, Vec::new()),
        "case \"a request-level TtlDuration in no declared unit\": expected the refused \
         request to store nothing, got {status} with {served:?} in {body:?}"
    );
}
