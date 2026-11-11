// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request member outside a modelled `@length` or `@range`.
//!
//! RULE: a value outside a size or numeric bound the model declares is refused before any
//! handler runs, with HTTP 400 and `x-amzn-errortype: ValidationException`, carrying the
//! constraint-violation message BUILT FROM THE REQUEST — `N validation error(s) detected:
//! Value '<what was sent>' at '<the member path>' failed to satisfy constraint: Member
//! must have length|value greater|less than or equal to <the bound the model declares>`,
//! where `N` is the number of violations reported — while a value AT the bound is inside
//! it and reaches the handler, which then judges the request on its own rules.
//!
//! The recorded case the rule was filed from is ONE row of the table below: a
//! `BatchGetRecord` carrying 101 `Identifiers`. The rows differ along what the rule
//! generalises over: which operation carries the member, whether the bound is a `@length`
//! on a list or a string or a `@range` on an integer, whether the member sits at the top
//! of the request or nested inside a list or a structure, where in a list the offending
//! element sits, which side of the bound the value falls on, and how far past it.
//!
//! The catalog of bounds is the model's own, read with
//! `smithy_query(query="operation", operation="BatchGetRecord,BatchWriteRecord,\
//! PutRecord,UpdateRecord,DeleteRecord,ListRecords,GetRecord")` and
//! `smithy_query(query="shape", shape="BatchGetRecordIdentifier,FeatureNames,\
//! ListRecordsNextToken,ListRecordsMaxResults,TtlDuration,TtlDurationValue")` on this
//! crate's `smithy-model/`:
//!
//! ```text
//! BatchGetRecordRequest$Identifiers                        @length 1..100  (list)
//! BatchGetRecordIdentifier$RecordIdentifiersValueAsString  @length 1..100  (list)
//! BatchGetRecordIdentifier$FeatureNames                    @length 1..     (list)
//! BatchWriteRecordRequest$Entries                          @length 1..25   (list)
//! PutRecordRequest$Record, UpdateRecordRequest$Features    @length 1..     (list)
//! PutRecord/UpdateRecord/DeleteRecord $TargetStores        @length 1..2    (list)
//! TtlDuration$Value                                        @range  1..     (integer)
//! ListRecordsRequest$MaxResults                            @range  1..100  (integer)
//! ListRecordsRequest$NextToken                             @length 1..8192 (string)
//! ```
//!
//! The three remaining constrained shapes — `FeatureGroupNameOrArn` (`@length` 1..150),
//! `FeatureName` (1..64) and `ValueAsString` (0..358400) — are left to
//! `pattern_refusal_rendering`: every value long enough to cross the first two maxima
//! also fails their `@pattern` (whose repetition admits at most 64 characters), so no
//! bound can be observed alone on them, and `ValueAsString`'s minimum of 0 excludes
//! nothing while its maximum states the same `MaxLength` clause three rows here pin.
//!
//! Every case goes through the real front door — the multi-service router, the restJson1
//! protocol, the modelled constraint validation — so what is asserted is the wire
//! behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The feature group every row names, and the two feature names its definition fixes.
const GROUP: &str = "customers";
const IDENTIFIER_FEATURE: &str = "record_id";
const EVENT_TIME_FEATURE: &str = "event_time";

/// The record seeded into that group, so a row inside every bound reaches a handler that
/// has something to serve.
const RECORD: &str = "R1";
const EVENT_TIME: &str = "2024-01-01T00:00:00Z";

/// The error code the framework answers a constraint violation with, before any handler
/// runs. A handler's own refusal carries the MODELLED `ValidationError` instead, which is
/// how a row inside the bound is told apart from one outside it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A booted emulator holding this service only, with one feature group and one record.
struct Emulator {
    router: MultiServiceRouter,
}

/// One answer as the wire carries it.
struct Reply {
    status: u16,
    /// The wire error code: restJson1 sets `x-amzn-errortype` and also carries it as the
    /// body's `__type`, so the header is read first and the body is the fallback.
    errortype: String,
    /// The error body's message. The framework's own refusal spells it `message`; a
    /// modelled error of this service declares a `Message` member, so both are read.
    message: String,
    /// The parsed response body, so a row can read what a successful call served.
    body: Value,
}

impl Reply {
    /// The `ValueAsString` the served `Record` carries for the feature `name`, or the
    /// empty string when the response serves no such feature.
    fn served_feature(&self, name: &str) -> String {
        self.body
            .get("Record")
            .and_then(Value::as_array)
            .and_then(|features| {
                features
                    .iter()
                    .find(|held| held["FeatureName"] == json!(name))
            })
            .and_then(|held| held["ValueAsString"].as_str())
            .unwrap_or_default()
            .to_owned()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // Create the feature group the way the SageMaker control plane's
        // `CreateFeatureGroup` does: by registering its definition in the ONE shared
        // directory this data plane resolves against.
        engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: GROUP.to_owned(),
                arn: format!("arn:aws:sagemaker:us-east-1:123456789012:feature-group/{GROUP}"),
                record_identifier_feature_name: IDENTIFIER_FEATURE.to_owned(),
                event_time_feature_name: EVENT_TIME_FEATURE.to_owned(),
                online_store_enabled: true,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
            });
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
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
        let header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
        let errortype = if header.is_empty() {
            parsed
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .to_owned()
        } else {
            header
        };
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Reply {
            status,
            errortype,
            message,
            body: parsed,
        }
    }

    /// Ingest the record the reads work on.
    async fn seed(&self) {
        let reply = self
            .send(
                "PUT",
                &format!("/FeatureGroup/{GROUP}"),
                Some(json!({ "Record": a_record() })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"seeding PutRecord\": expected 200, got {} with {:?}",
            reply.status, reply.message
        );
    }
}

/// One `FeatureValue` with a scalar value.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// A record that satisfies every rule of the group's definition, so the only thing a row
/// violates is the bound it is about.
fn a_record() -> Value {
    json!([
        feature(IDENTIFIER_FEATURE, RECORD),
        feature(EVENT_TIME_FEATURE, EVENT_TIME),
        feature("feature_a", "original")
    ])
}

/// A `BatchGetRecordIdentifier` naming the one feature group and `count` record
/// identifiers.
fn batch_entry(count: usize) -> Value {
    let identifiers: Vec<String> = (0..count).map(|index| format!("id-{index}")).collect();
    json!({
        "FeatureGroupName": GROUP,
        "RecordIdentifiersValueAsString": identifiers
    })
}

/// A `BatchWriteRecordEntry` for the one feature group.
fn write_entry() -> Value {
    json!({ "FeatureGroupName": GROUP, "Record": a_record() })
}

/// What a row expects of the answer.
enum Expect {
    /// Outside the bound: refused at the boundary with this constraint requirement, which
    /// the message must state for the row's member path.
    Outside(&'static str),
    /// Inside the bound: the request reached the handler, so it answers this status and
    /// this error code (`""` for a success) and the answer states no constraint on the
    /// row's member.
    Inside(u16, &'static str),
}

/// One row: a request, the member whose bound it is about, and what it expects.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The member path a constraint clause names, as the walk reports it (a list step is
    /// `<1-based index>.member`).
    path: &'static str,
    expect: Expect,
}

/// `N` in the message's envelope, when it opens with the AWS constraint-violation
/// envelope: `1 validation error detected: …` / `N validation errors detected: …`.
fn envelope_count(message: &str) -> Option<usize> {
    let (count, rest) = message.split_once(" validation error")?;
    rest.starts_with(" detected: ")
        .then_some(())
        .or_else(|| rest.starts_with("s detected: ").then_some(()))?;
    count.parse().ok()
}

/// How many constraint clauses the message carries.
fn clause_count(message: &str) -> usize {
    message.matches(" failed to satisfy constraint: ").count()
}

fn rows() -> Vec<Row> {
    // A `NextToken` is bounded at 8192 characters; the two rows around that bound build
    // it from a character the modelled shape admits anywhere (it carries no `@pattern`).
    let token_at_the_maximum = "t".repeat(8192);
    let token_past_the_maximum = "t".repeat(8193);
    vec![
        // ---- BatchGetRecordRequest$Identifiers: @length 1..100 -------------------
        Row {
            case: "BatchGetRecord with 101 identifiers (the recorded case)",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": vec![batch_entry(1); 101] })),
            path: "identifiers",
            expect: Expect::Outside("Member must have length less than or equal to 100"),
        },
        Row {
            case: "BatchGetRecord two and a half times past the identifier maximum",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": vec![batch_entry(1); 250] })),
            path: "identifiers",
            expect: Expect::Outside("Member must have length less than or equal to 100"),
        },
        Row {
            case: "BatchGetRecord with exactly the identifier maximum",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": vec![batch_entry(1); 100] })),
            path: "identifiers",
            expect: Expect::Inside(200, ""),
        },
        Row {
            case: "BatchGetRecord with no identifiers at all",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": [] })),
            path: "identifiers",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        // ---- BatchGetRecordIdentifier$RecordIdentifiersValueAsString: 1..100 -----
        Row {
            case: "BatchGetRecord, one entry past the record-identifier maximum",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": [batch_entry(101)] })),
            path: "identifiers.1.member.recordIdentifiersValueAsString",
            expect: Expect::Outside("Member must have length less than or equal to 100"),
        },
        Row {
            case: "BatchGetRecord, one entry at exactly the record-identifier maximum",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": [batch_entry(100)] })),
            path: "identifiers.1.member.recordIdentifiersValueAsString",
            expect: Expect::Inside(200, ""),
        },
        Row {
            case: "BatchGetRecord, an entry naming no record identifier",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": [batch_entry(0)] })),
            path: "identifiers.1.member.recordIdentifiersValueAsString",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        Row {
            // The clause names the offending element's own position, so the SECOND entry
            // is reported at `identifiers.2`, not at the first.
            case: "BatchGetRecord, the SECOND entry past the record-identifier maximum",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": [batch_entry(1), batch_entry(101)] })),
            path: "identifiers.2.member.recordIdentifiersValueAsString",
            expect: Expect::Outside("Member must have length less than or equal to 100"),
        },
        // ---- BatchGetRecordIdentifier$FeatureNames: @length min 1 ----------------
        Row {
            case: "BatchGetRecord, an entry naming an empty FeatureNames",
            method: "POST",
            uri: "/BatchGetRecord".to_string(),
            body: Some(json!({ "Identifiers": [{
                "FeatureGroupName": GROUP,
                "RecordIdentifiersValueAsString": [RECORD],
                "FeatureNames": []
            }] })),
            path: "identifiers.1.member.featureNames",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        // ---- BatchWriteRecordRequest$Entries: @length 1..25 ----------------------
        Row {
            case: "BatchWriteRecord with one entry past the maximum",
            method: "POST",
            uri: "/BatchWriteRecord".to_string(),
            body: Some(json!({ "Entries": vec![write_entry(); 26] })),
            path: "entries",
            expect: Expect::Outside("Member must have length less than or equal to 25"),
        },
        Row {
            // Inside the bound, so the request reaches the operation, which writes all
            // 25 entries and reports no per-entry error.
            case: "BatchWriteRecord with exactly the maximum number of entries",
            method: "POST",
            uri: "/BatchWriteRecord".to_string(),
            body: Some(json!({ "Entries": vec![write_entry(); 25] })),
            path: "entries",
            expect: Expect::Inside(200, ""),
        },
        Row {
            case: "BatchWriteRecord with no entries at all",
            method: "POST",
            uri: "/BatchWriteRecord".to_string(),
            body: Some(json!({ "Entries": [] })),
            path: "entries",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        // ---- PutRecordRequest$Record / UpdateRecordRequest$Features: min 1 -------
        Row {
            case: "PutRecord with an empty Record",
            method: "PUT",
            uri: format!("/FeatureGroup/{GROUP}"),
            body: Some(json!({ "Record": [] })),
            path: "record",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        Row {
            case: "UpdateRecord with empty Features",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/Record"),
            body: Some(json!({
                "RecordIdentifierValueAsString": RECORD,
                "Features": []
            })),
            path: "features",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        // ---- TargetStores: @length 1..2 -----------------------------------------
        Row {
            // Three values the `TargetStore` enum declares: only the list's length is
            // outside a bound, so the clause states that and nothing else.
            case: "PutRecord naming three TargetStores",
            method: "PUT",
            uri: format!("/FeatureGroup/{GROUP}"),
            body: Some(json!({
                "Record": a_record(),
                "TargetStores": ["OnlineStore", "OfflineStore", "OnlineStore"]
            })),
            path: "targetStores",
            expect: Expect::Outside("Member must have length less than or equal to 2"),
        },
        Row {
            case: "PutRecord naming both TargetStores",
            method: "PUT",
            uri: format!("/FeatureGroup/{GROUP}"),
            body: Some(json!({
                "Record": a_record(),
                "TargetStores": ["OnlineStore", "OfflineStore"]
            })),
            path: "targetStores",
            expect: Expect::Inside(200, ""),
        },
        Row {
            case: "PutRecord naming an empty TargetStores",
            method: "PUT",
            uri: format!("/FeatureGroup/{GROUP}"),
            body: Some(json!({ "Record": a_record(), "TargetStores": [] })),
            path: "targetStores",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        Row {
            // The same bound on the same shape, carried as an `@httpQuery` list rather
            // than a body member.
            case: "DeleteRecord naming three TargetStores in the query string",
            method: "DELETE",
            uri: format!(
                "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={RECORD}\
                 &EventTime={EVENT_TIME}&TargetStores=OnlineStore&TargetStores=OfflineStore\
                 &TargetStores=OnlineStore"
            ),
            body: None,
            path: "targetStores",
            expect: Expect::Outside("Member must have length less than or equal to 2"),
        },
        // ---- TtlDuration$Value: @range min 1 ------------------------------------
        Row {
            case: "PutRecord with a TtlDuration of zero",
            method: "PUT",
            uri: format!("/FeatureGroup/{GROUP}"),
            body: Some(json!({
                "Record": a_record(),
                "TtlDuration": { "Unit": "Seconds", "Value": 0 }
            })),
            path: "ttlDuration.value",
            expect: Expect::Outside("Member must have value greater than or equal to 1"),
        },
        Row {
            case: "PutRecord with a TtlDuration of one week",
            method: "PUT",
            uri: format!("/FeatureGroup/{GROUP}"),
            body: Some(json!({
                "Record": a_record(),
                "TtlDuration": { "Unit": "Weeks", "Value": 1 }
            })),
            path: "ttlDuration.value",
            expect: Expect::Inside(200, ""),
        },
        // ---- ListRecordsRequest$MaxResults: @range 1..100 -----------------------
        Row {
            case: "ListRecords with MaxResults one past the maximum",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "MaxResults": 101 })),
            path: "maxResults",
            expect: Expect::Outside("Member must have value less than or equal to 100"),
        },
        Row {
            case: "ListRecords with MaxResults at the maximum",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "MaxResults": 100 })),
            path: "maxResults",
            expect: Expect::Inside(200, ""),
        },
        Row {
            case: "ListRecords with MaxResults at the minimum",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "MaxResults": 1 })),
            path: "maxResults",
            expect: Expect::Inside(200, ""),
        },
        Row {
            case: "ListRecords with MaxResults of zero",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "MaxResults": 0 })),
            path: "maxResults",
            expect: Expect::Outside("Member must have value greater than or equal to 1"),
        },
        Row {
            case: "ListRecords with a negative MaxResults",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "MaxResults": -5 })),
            path: "maxResults",
            expect: Expect::Outside("Member must have value greater than or equal to 1"),
        },
        // ---- ListRecordsRequest$NextToken: @length 1..8192 ----------------------
        Row {
            case: "ListRecords with an empty NextToken",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "NextToken": "" })),
            path: "nextToken",
            expect: Expect::Outside("Member must have length greater than or equal to 1"),
        },
        Row {
            case: "ListRecords with a NextToken one character past the maximum",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "NextToken": token_past_the_maximum })),
            path: "nextToken",
            expect: Expect::Outside("Member must have length less than or equal to 8192"),
        },
        Row {
            // Inside the bound, so the handler judges it — and refuses it with the
            // MODELLED `ValidationError` because it is no pagination token this service
            // ever issued, which is a different refusal from the bound's.
            case: "ListRecords with a NextToken at exactly the maximum",
            method: "POST",
            uri: format!("/FeatureGroup/{GROUP}/ListRecords"),
            body: Some(json!({ "NextToken": token_at_the_maximum })),
            path: "nextToken",
            expect: Expect::Inside(400, "ValidationError"),
        },
    ]
}

#[tokio::test]
async fn a_value_outside_a_modeled_bound_is_refused_with_the_constraint_clause() {
    let emulator = Emulator::new();
    emulator.seed().await;

    for row in rows() {
        let Row {
            case,
            method,
            uri,
            body,
            path,
            expect,
        } = row;
        let reply = emulator.send(method, &uri, body).await;
        let clause = format!("at '{path}' failed to satisfy constraint: ");

        match expect {
            Expect::Outside(requirement) => {
                assert_eq!(
                    (reply.status, reply.errortype.as_str()),
                    (400, VALIDATION_EXCEPTION),
                    "case {case:?}: expected HTTP 400 {VALIDATION_EXCEPTION} for a value \
                     outside the bound on {path:?}, got {} {:?} with message {:?}",
                    reply.status,
                    reply.errortype,
                    reply.message
                );
                // The message is built from the request: the envelope counts the
                // violations, and the clause names the member and the constraint.
                let expected = format!("{clause}{requirement}");
                assert!(
                    reply.message.contains(&expected),
                    "case {case:?}: expected the message to state {expected:?}, got {:?}",
                    reply.message
                );
                assert!(
                    reply.message.starts_with("1 validation error detected: Value "),
                    "case {case:?}: expected the AWS constraint-violation envelope stating \
                     one violation and echoing the value, got {:?}",
                    reply.message
                );
                let counted = envelope_count(&reply.message);
                assert_eq!(
                    counted,
                    Some(clause_count(&reply.message)),
                    "case {case:?}: expected the envelope's count to be the number of \
                     clauses the message carries, got {counted:?} for {:?}",
                    reply.message
                );
            }
            Expect::Inside(status, errortype) => {
                assert_eq!(
                    (reply.status, reply.errortype.as_str()),
                    (status, errortype),
                    "case {case:?}: expected HTTP {status} {errortype:?} for a value inside \
                     the bound on {path:?}, got {} {:?} with message {:?}",
                    reply.status,
                    reply.errortype,
                    reply.message
                );
                assert!(
                    !reply.message.contains(&clause),
                    "case {case:?}: expected no constraint clause on {path:?}, got {:?}",
                    reply.message
                );
            }
        }
    }
}

/// The bounds are enforced on the REQUEST, before any handler runs — so a refused request
/// changes nothing in the store, whatever the operation would have done with it.
#[tokio::test]
async fn a_request_refused_for_a_bound_leaves_the_store_as_it_was() {
    let emulator = Emulator::new();
    emulator.seed().await;

    // (case, method, uri, body) — each one a write whose bound is violated, so none of
    // them may reach the store.
    let refused: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "PutRecord with an empty Record",
            "PUT",
            format!("/FeatureGroup/{GROUP}"),
            Some(json!({ "Record": [] })),
        ),
        (
            "PutRecord naming three TargetStores",
            "PUT",
            format!("/FeatureGroup/{GROUP}"),
            Some(json!({
                "Record": [
                    feature(IDENTIFIER_FEATURE, RECORD),
                    feature(EVENT_TIME_FEATURE, "2030-01-01T00:00:00Z"),
                    feature("feature_a", "overwritten")
                ],
                "TargetStores": ["OnlineStore", "OfflineStore", "OnlineStore"]
            })),
        ),
        (
            "UpdateRecord with empty Features",
            "POST",
            format!("/FeatureGroup/{GROUP}/Record"),
            Some(json!({
                "RecordIdentifierValueAsString": RECORD,
                "Features": []
            })),
        ),
        (
            "UpdateRecord with a TtlDuration of zero",
            "POST",
            format!("/FeatureGroup/{GROUP}/Record"),
            Some(json!({
                "RecordIdentifierValueAsString": RECORD,
                "Features": [
                    feature("feature_a", "overwritten"),
                    feature(EVENT_TIME_FEATURE, "2030-01-01T00:00:00Z")
                ],
                "TtlDuration": { "Unit": "Seconds", "Value": 0 }
            })),
        ),
        (
            "DeleteRecord naming three TargetStores",
            "DELETE",
            format!(
                "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={RECORD}\
                 &EventTime=2030-01-01T00:00:00Z&TargetStores=OnlineStore\
                 &TargetStores=OfflineStore&TargetStores=OnlineStore"
            ),
            None,
        ),
    ];

    for (case, method, uri, body) in refused {
        let reply = emulator.send(method, &uri, body).await;
        assert_eq!(
            (reply.status, reply.errortype.as_str()),
            (400, VALIDATION_EXCEPTION),
            "case {case:?}: expected HTTP 400 {VALIDATION_EXCEPTION}, got {} {:?} with \
             message {:?}",
            reply.status,
            reply.errortype,
            reply.message
        );

        // The seeded record is still exactly what the seeding `PutRecord` wrote: not
        // over-written, not updated, not deleted, and carrying no expiry.
        let reply = emulator
            .send(
                "GET",
                &format!(
                    "/FeatureGroup/{GROUP}?RecordIdentifierValueAsString={RECORD}\
                     &ExpirationTimeResponse=Enabled"
                ),
                None,
            )
            .await;
        let event_time = reply.served_feature(EVENT_TIME_FEATURE);
        let feature_a = reply.served_feature("feature_a");
        let expires_at = reply.body.get("ExpiresAt").cloned();
        assert_eq!(
            (
                reply.status,
                event_time.as_str(),
                feature_a.as_str(),
                expires_at.clone()
            ),
            (200, EVENT_TIME, "original", None),
            "case {case:?}: expected the seeded record to be served unchanged and with no \
             ExpiresAt, got {} with body {}",
            reply.status,
            reply.body
        );
    }
}
