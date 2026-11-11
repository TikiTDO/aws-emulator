// SPDX-License-Identifier: CC0-1.0

//! How this data plane resolves the `FeatureGroupName` every one of its operations
//! carries, and what it answers when that name resolves to no feature group.
//!
//! RULE: a SageMaker Feature Store runtime operation serves a `FeatureGroupName` only
//! when it resolves to a feature group the SageMaker control plane holds; a name that
//! resolves to none is a request defect, refused with the modelled `ValidationError`
//! (HTTP 400) — never with `ResourceNotFound` (404), which says a RECORD is missing from
//! a feature group that exists, and never accepted, which would report an ingestion no
//! later read could find. A group that resolves with NO `OnlineStore` is refused the same
//! way on the READS, which have nothing to serve from it, while a WRITE to it leaves the
//! online store alone and succeeds: AWS replicates such a write to the OfflineStore,
//! which this emulator does not model.
//!
//! The runtime API carries no feature-group definition, so the resolution goes through
//! the shared feature-group directory `CreateFeatureGroup` writes
//! (`service_common::feature_store_groups`). These tests create a group exactly as that
//! handler does and then drive the real front door — the multi-service router, the
//! restJson1 protocol, the modelled constraint validation — so what is asserted is the
//! wire behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The record identifier every row writes and reads.
const RECORD: &str = "R1";

/// The feature group's own `RecordIdentifierFeatureName`, as its definition fixes it.
const IDENTIFIER_FEATURE: &str = "record_id";

/// The feature group's own `EventTimeFeatureName`.
const EVENT_TIME_FEATURE: &str = "event_time";

/// The event time the seeded record carries, and a later one for a delete.
const EVENT_TIME: &str = "2024-01-01T00:00:00Z";
const LATER_EVENT_TIME: &str = "2024-06-01T00:00:00Z";

/// One operation of this data plane, named by the `FeatureGroupName` it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    PutRecord,
    BatchWriteRecord,
    GetRecord,
    UpdateRecord,
    ListRecords,
    BatchGetRecord,
    DeleteRecord,
}

/// Every operation that names a feature group, in an order that leaves the delete last
/// so a row's earlier calls still see the record.
const EVERY_CALL: [Call; 7] = [
    Call::PutRecord,
    Call::BatchWriteRecord,
    Call::GetRecord,
    Call::UpdateRecord,
    Call::ListRecords,
    Call::BatchGetRecord,
    Call::DeleteRecord,
];

impl Call {
    /// Whether this operation READS the OnlineStore (rather than writing it), which is
    /// what decides how it answers a feature group that exists with no `OnlineStore`:
    /// there is nothing for a read to serve, while a write to such a group is the
    /// OfflineStore's business and leaves the online store alone.
    fn reads(self) -> bool {
        matches!(
            self,
            Call::GetRecord | Call::ListRecords | Call::BatchGetRecord
        )
    }
}

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
        Self { engine, router }
    }

    /// Create a feature group the way the SageMaker control plane's `CreateFeatureGroup`
    /// does: by registering its definition — its two key feature names and whether its
    /// `OnlineStore` is on — in the ONE shared directory this data plane resolves against.
    fn create_feature_group(&self, name: &str, online_store_enabled: bool) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!("arn:aws:sagemaker:us-east-1:123456789012:feature-group/{name}"),
                record_identifier_feature_name: IDENTIFIER_FEATURE.to_owned(),
                event_time_feature_name: EVENT_TIME_FEATURE.to_owned(),
                online_store_enabled,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
            });
    }

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

    /// Perform `call` against the feature group named by `group`, as it travels on the
    /// wire (an `@httpLabel` carries an ARN percent-encoded).
    async fn perform(&self, call: Call, group: &str) -> (u16, Value) {
        match call {
            Call::PutRecord => {
                self.send(
                    "PUT",
                    &format!("/FeatureGroup/{group}"),
                    Some(json!({ "Record": [
                        feature(IDENTIFIER_FEATURE, RECORD),
                        feature(EVENT_TIME_FEATURE, EVENT_TIME),
                        feature("feature_a", "original")
                    ] })),
                )
                .await
            }
            Call::BatchWriteRecord => {
                self.send(
                    "POST",
                    "/BatchWriteRecord",
                    Some(json!({ "Entries": [{
                        "FeatureGroupName": percent_decoded(group),
                        "Record": [
                            feature(IDENTIFIER_FEATURE, RECORD),
                            feature(EVENT_TIME_FEATURE, EVENT_TIME),
                            feature("feature_a", "original")
                        ]
                    }] })),
                )
                .await
            }
            Call::GetRecord => {
                self.send(
                    "GET",
                    &format!("/FeatureGroup/{group}?RecordIdentifierValueAsString={RECORD}"),
                    None,
                )
                .await
            }
            Call::UpdateRecord => {
                self.send(
                    "POST",
                    &format!("/FeatureGroup/{group}/Record"),
                    Some(json!({
                        "RecordIdentifierValueAsString": RECORD,
                        "Features": [feature("feature_a", "updated")]
                    })),
                )
                .await
            }
            Call::ListRecords => {
                self.send(
                    "POST",
                    &format!("/FeatureGroup/{group}/ListRecords"),
                    Some(json!({})),
                )
                .await
            }
            Call::BatchGetRecord => {
                self.send(
                    "POST",
                    "/BatchGetRecord",
                    Some(json!({ "Identifiers": [{
                        "FeatureGroupName": percent_decoded(group),
                        "RecordIdentifiersValueAsString": [RECORD]
                    }] })),
                )
                .await
            }
            Call::DeleteRecord => {
                self.send(
                    "DELETE",
                    &format!(
                        "/FeatureGroup/{group}?RecordIdentifierValueAsString={RECORD}\
                         &EventTime={LATER_EVENT_TIME}"
                    ),
                    None,
                )
                .await
            }
        }
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// The name as a BODY member carries it: `BatchGetRecordIdentifier$FeatureGroupName` is
/// no `@httpLabel`, so it is not percent-encoded on the wire.
fn percent_decoded(group: &str) -> String {
    group.replace("%3A", ":").replace("%2F", "/")
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

/// The per-entry `ErrorCode`s a batch response reports — `BatchGetRecord` per
/// identifier, `BatchWriteRecord` per entry.
fn batch_error_codes(body: &Value) -> Vec<String> {
    body.get("Errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|error| {
                    error["ErrorCode"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How a feature group was (or was not) created before the row ran.
#[derive(Debug, Clone, Copy)]
enum Group {
    /// Created with `OnlineStoreConfig.EnableOnlineStore` true.
    WithOnlineStore,
    /// Created with no `OnlineStore` — the documented default of `EnableOnlineStore`.
    WithoutOnlineStore,
    /// Never created.
    Absent,
}

#[tokio::test]
async fn every_operation_refuses_a_feature_group_name_that_resolves_to_no_feature_group() {
    // The class the rule spans: how the group was created, crossed with every spelling
    // of the name the modelled `FeatureGroupNameOrArn` admits. Each row is run against
    // EVERY operation that names one feature group.
    //
    // (case, how the group was created, the group's name, the spelling the call carries)
    let rows: Vec<(&str, Group, &str, String)> = vec![
        (
            "a created group, by bare name",
            Group::WithOnlineStore,
            "customers",
            "customers".to_owned(),
        ),
        (
            "a created group, by the ARN the control plane minted",
            Group::WithOnlineStore,
            "orders",
            "arn%3Aaws%3Asagemaker%3Aus-east-1%3A123456789012%3Afeature-group%2Forders".to_owned(),
        ),
        (
            "a created group, by an ARN of another Region and account",
            Group::WithOnlineStore,
            "events",
            "arn%3Aaws%3Asagemaker%3Aeu-west-1%3A000000000000%3Afeature-group%2Fevents".to_owned(),
        ),
        (
            "a name never created",
            Group::Absent,
            "nosuchgroup",
            "nosuchgroup".to_owned(),
        ),
        (
            "a name never created, as an ARN",
            Group::Absent,
            "alsonosuchgroup",
            "arn%3Aaws%3Asagemaker%3Aus-east-1%3A123456789012%3Afeature-group%2Falsonosuchgroup"
                .to_owned(),
        ),
        (
            "a name that differs from a created one by one character",
            Group::Absent,
            "customer",
            "customer".to_owned(),
        ),
        (
            "a group created without an OnlineStore",
            Group::WithoutOnlineStore,
            "offlineonly",
            "offlineonly".to_owned(),
        ),
    ];

    for (case, group, name, spelling) in rows {
        let emulator = Emulator::new();
        // Every row has one OTHER group created, so no row's verdict can come from the
        // directory being empty.
        emulator.create_feature_group("customers", true);
        match group {
            Group::WithOnlineStore => emulator.create_feature_group(name, true),
            Group::WithoutOnlineStore => emulator.create_feature_group(name, false),
            Group::Absent => {}
        }
        let resolves = matches!(group, Group::WithOnlineStore);
        if resolves {
            // Seed the record the reads and the update of this row work on, through the
            // bare name, so the row's own spelling is the only thing that varies.
            let (status, body) = emulator.perform(Call::PutRecord, name).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (200, ""),
                "case {case:?}: expected the seeding PutRecord to be accepted, got {status} \
                 with body {body:?}"
            );
        }

        for call in EVERY_CALL {
            // What each class of group answers. A name that resolves to NO feature group
            // is refused everywhere. A group that resolves but has no `OnlineStore` is
            // refused on the READS — there is no online store for this data plane to
            // serve — while a WRITE to it is accepted and leaves the online store alone,
            // because AWS replicates such a write to the OfflineStore and this emulator
            // does not model that store.
            let refused = match group {
                Group::WithOnlineStore => false,
                Group::Absent => true,
                Group::WithoutOnlineStore => call.reads(),
            };
            let (status, body) = emulator.perform(call, &spelling).await;
            let code = error_code(&body);
            if matches!(call, Call::BatchGetRecord | Call::BatchWriteRecord) {
                // A batch operation reports a failure that concerns one identifier or
                // one entry in `Errors` rather than failing the batch, so the same
                // classification arrives as that element's own `ErrorCode`.
                let reported = batch_error_codes(&body);
                let expected: Vec<String> = if refused {
                    vec!["ValidationError".to_owned()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    (status, reported.clone()),
                    (200, expected.clone()),
                    "case {case:?}/{call:?}: expected HTTP 200 with per-identifier errors \
                     {expected:?}, got {status} with {reported:?} in {body:?}"
                );
                continue;
            }
            if refused {
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationError"),
                    "case {case:?}/{call:?}: expected HTTP 400 ValidationError for a name \
                     with no online store to serve, got {status} {code:?} with body {body:?}"
                );
            } else {
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {case:?}/{call:?}: expected HTTP 200, got {status} {code:?} with \
                     body {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_record_written_to_a_name_that_does_not_resolve_is_never_stored() {
    let emulator = Emulator::new();
    emulator.create_feature_group("customers", true);

    // The refusal is what keeps the silent data-loss path shut: the write is refused,
    // and the name stays unreadable afterwards rather than holding a record no feature
    // group accounts for.
    let (status, body) = emulator.perform(Call::PutRecord, "nosuchgroup").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationError"),
        "case \"PutRecord against an unresolvable name\": expected HTTP 400 ValidationError, \
         got {status} with body {body:?}"
    );

    // And once the group IS created, the same write is accepted and reads back.
    emulator.create_feature_group("nosuchgroup", true);
    let (status, _) = emulator.perform(Call::PutRecord, "nosuchgroup").await;
    assert_eq!(
        status, 200,
        "case \"PutRecord after the group is created\": expected 200, got {status}"
    );
    let (status, body) = emulator.perform(Call::GetRecord, "nosuchgroup").await;
    let served: Vec<String> = body["Record"]
        .as_array()
        .map(|features| {
            features
                .iter()
                .map(|f| f["FeatureName"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        (status, served.clone()),
        (
            200,
            vec![
                IDENTIFIER_FEATURE.to_owned(),
                EVENT_TIME_FEATURE.to_owned(),
                "feature_a".to_owned()
            ]
        ),
        "case \"GetRecord after the group is created\": expected the written features, got \
         {status} with {served:?} in {body:?}"
    );
}

#[tokio::test]
async fn the_records_identifier_is_the_groups_record_identifier_feature_whatever_order_it_arrives_in()
{
    let emulator = Emulator::new();
    emulator.create_feature_group("customers", true);

    // RULE: the record's identifier is the value of the group's own
    // `RecordIdentifierFeatureName`, not of whichever feature the request happens to
    // list first. Rows: the identifier first, last, in the middle, and absent.
    //
    // (case, the posted Record, the identifier the write must store)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the identifier first",
            json!([
                feature(IDENTIFIER_FEATURE, "first"),
                feature(EVENT_TIME_FEATURE, EVENT_TIME),
                feature("feature_a", "a")
            ]),
            Some("first"),
        ),
        (
            "the identifier last",
            json!([
                feature(EVENT_TIME_FEATURE, EVENT_TIME),
                feature("feature_a", "a"),
                feature(IDENTIFIER_FEATURE, "last")
            ]),
            Some("last"),
        ),
        (
            "the identifier in the middle",
            json!([
                feature("feature_a", "a"),
                feature(IDENTIFIER_FEATURE, "middle"),
                feature(EVENT_TIME_FEATURE, EVENT_TIME)
            ]),
            Some("middle"),
        ),
        (
            "no identifier feature at all",
            json!([
                feature("feature_a", "a"),
                feature(EVENT_TIME_FEATURE, EVENT_TIME)
            ]),
            None,
        ),
    ];

    for (case, record, stored) in rows {
        let (status, body) = emulator
            .send(
                "PUT",
                "/FeatureGroup/customers",
                Some(json!({ "Record": record })),
            )
            .await;
        match stored {
            Some(identifier) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected PutRecord 200, got {status} with body {body:?}"
                );
                let (status, body) = emulator
                    .send(
                        "GET",
                        &format!(
                            "/FeatureGroup/customers?RecordIdentifierValueAsString={identifier}"
                        ),
                        None,
                    )
                    .await;
                let served = body.get("Record").cloned().unwrap_or(Value::Null);
                assert!(
                    status == 200 && served.is_array(),
                    "case {case:?}: expected the record to be stored under {identifier:?}, \
                     got {status} with body {body:?}"
                );
            }
            None => {
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationError"),
                    "case {case:?}: expected HTTP 400 ValidationError for a record carrying \
                     no value for the group's RecordIdentifierFeatureName, got {status} \
                     {code:?} with body {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_write_to_a_group_without_an_online_store_is_still_judged_by_the_requests_own_rules() {
    let emulator = Emulator::new();
    emulator.create_feature_group("customers", true);
    emulator.create_feature_group("offlineonly", false);

    // RULE: whether a feature group has an `OnlineStore` decides only whether the WRITE
    // reaches this store, never whether the request is well-formed — so the same record
    // is judged the same way in both groups, and only a well-formed one differs (stored
    // online, or accepted with nothing stored here).
    //
    // (case, the group to write to, the posted Record, expected status)
    let rows: Vec<(&str, &str, Value, u16)> = vec![
        (
            "an empty Record, to a group with an OnlineStore",
            "customers",
            json!([]),
            400,
        ),
        (
            "an empty Record, to a group without one",
            "offlineonly",
            json!([]),
            400,
        ),
        (
            "a Record missing the group's identifier feature, with an OnlineStore",
            "customers",
            json!([feature("feature_a", "a")]),
            400,
        ),
        (
            "a Record missing the group's identifier feature, without one",
            "offlineonly",
            json!([feature("feature_a", "a")]),
            400,
        ),
        (
            "an event time that is no timestamp, without an OnlineStore",
            "offlineonly",
            json!([
                feature(IDENTIFIER_FEATURE, RECORD),
                feature(EVENT_TIME_FEATURE, "yesterday")
            ]),
            400,
        ),
        (
            "a well-formed Record, to a group without an OnlineStore",
            "offlineonly",
            json!([
                feature(IDENTIFIER_FEATURE, RECORD),
                feature(EVENT_TIME_FEATURE, EVENT_TIME)
            ]),
            200,
        ),
        (
            "a well-formed Record, to a group with an OnlineStore",
            "customers",
            json!([
                feature(IDENTIFIER_FEATURE, RECORD),
                feature(EVENT_TIME_FEATURE, EVENT_TIME)
            ]),
            200,
        ),
    ];

    for (case, group, record, expected_status) in rows {
        let (status, body) = emulator
            .send(
                "PUT",
                &format!("/FeatureGroup/{group}"),
                Some(json!({ "Record": record })),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected PutRecord {expected_status}, got {status} with body \
             {body:?}"
        );
    }

    // The accepted write to the offline-only group stored nothing HERE, and the read of
    // that group is refused rather than answering an empty record.
    let (status, body) = emulator.perform(Call::GetRecord, "offlineonly").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationError"),
        "case \"GetRecord of the offline-only group\": expected HTTP 400 ValidationError, \
         got {status} with body {body:?}"
    );
}
