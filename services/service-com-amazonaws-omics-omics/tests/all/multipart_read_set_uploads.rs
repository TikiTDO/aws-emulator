// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the multipart read set upload slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns; only the upload helpers live here.
//!
//! The rules under test:
//!
//! * initiating a multipart read set upload creates a resource of its own, stored once
//!   inside the sequence store the request names: whatever the request carried is what
//!   the response reports, each member taken from that one record, nothing invented for a
//!   member the request left out, and the record is really in the service's state
//!   afterwards (which is what the parts a caller uploads next are collected against);
//!   * the `uploadId` is the ten-digit decimal the modelled `UploadId` length admits,
//!     minted per upload and never reused;
//!   * a replayed `clientToken` answers with the upload the first call initiated rather
//!     than initiating a second one, while a request that presents no token is a new
//!     upload every time, and the token is namespaced to the store it was presented to;
//! * an upload belongs to the store it was initiated against: two stores keep their
//!   uploads apart;
//! * a sequence store id no store holds is the modelled 404, however the id is shaped.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_omics_omics::state::{
    service_state, MultipartReadSetUpload, FIRST_SEQUENCE_STORE_ID, MAX_TAGS, UPLOAD_ID_OFFSET,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::private_workflow_runs::{
    assert_absent, assert_members, assert_timestamp, call, error_code, tag_map,
};

/// Every value the modelled `FileType` enum admits.
/// Source: the `sourceFileType` member of `CreateMultipartReadSetUploadRequest` ("one of:
/// BAM | CRAM | FASTQ | UBAM").
pub(crate) const FILE_TYPES: &[&str] = &["BAM", "CRAM", "FASTQ", "UBAM"];

/// The source formats HealthOmics aligns against a reference genome.
/// Source: `CreateSequenceStore`'s own documentation ("For aligned formats (BAM and
/// CRAM), a sequence store can only use one reference genome. For unaligned formats
/// (FASTQ and uBAM), a reference genome is not required.").
const ALIGNED_FILE_TYPES: &[&str] = &["BAM", "CRAM"];

/// A reference ARN satisfying the modelled `ReferenceArn` (len 1..127, `^arn:.+$`).
pub(crate) const REFERENCE_ARN: &str =
    "arn:aws:omics:us-west-2:000000000000:referenceStore/1000000000/reference/1000000001";

/// The published bounds of the modelled `UploadId` length (10..36).
/// Source: the `uploadId` member of `CreateMultipartReadSetUploadResponse`.
const UPLOAD_ID_WIDTH: (usize, usize) = (10, 36);

/// The published bounds of the modelled `SequenceStoreId` length (10..36).
/// Source: the `sequenceStoreId` member of `CreateMultipartReadSetUploadRequest`.
const SEQUENCE_STORE_ID_WIDTH: (usize, usize) = (10, 36);

/// The published upper bound of the modelled `ReadSetName` length (1..127).
/// Source: the `name` member of `CreateMultipartReadSetUploadRequest`.
const READ_SET_NAME_MAX: usize = 127;

/// The published upper bound of the modelled `ReadSetDescription` length (1..255).
/// Source: the `description` member of `CreateMultipartReadSetUploadRequest`.
const READ_SET_DESCRIPTION_MAX: usize = 255;

/// One emulator with an empty store, mounted exactly as the router mounts it, with the
/// shared engine kept beside it.
///
/// The suites that only drive the wire use the harness's own `emulator()`; this slice
/// also asserts that an initiated upload is really IN the service's state afterwards —
/// the record `UploadReadSetPart` and the upload listings read — so it needs the one
/// engine the service was mounted on.
pub(crate) fn emulator_holding_its_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_omics_omics::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// `POST /sequencestore` — the store every upload below is initiated against.
pub(crate) async fn create_store(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/sequencestore", Some(&json!({ "name": name }))).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected 200 from CreateSequenceStore, got {body:?}"
    );
    body.get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: CreateSequenceStore carried no id, got {body:?}"))
        .to_string()
}

/// `POST /sequencestore/{id}/upload`.
pub(crate) async fn create_upload(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/sequencestore/{sequence_store_id}/upload"),
        Some(body),
    )
    .await
}

/// The `uploadId` of a 200 create response, after asserting the status and its shape.
pub(crate) fn created_upload_id(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
    let id = body
        .get("uploadId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no uploadId, got {body:?}"))
        .to_string();
    let (min, max) = UPLOAD_ID_WIDTH;
    assert!(
        id.len() >= min && id.len() <= max && id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a {min}..{max} digit decimal uploadId, got {id:?}"
    );
    let value: u64 = id
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: uploadId {id:?} does not parse as a number"));
    let floor = FIRST_SEQUENCE_STORE_ID + UPLOAD_ID_OFFSET;
    assert!(
        value >= floor,
        "case {case:?}: expected an uploadId at or above {floor}, got {value}"
    );
    id
}

/// The record the service holds for `upload_id` inside `sequence_store_id`.
pub(crate) fn stored_upload(
    engine: &Arc<engine::InMemoryEngine>,
    sequence_store_id: &str,
    upload_id: &str,
) -> MultipartReadSetUpload {
    let state = service_state(&**engine).expect("the omics service state");
    let registry = state.lock();
    registry
        .multipart_read_set_upload(sequence_store_id, upload_id)
        .cloned()
        .unwrap_or_else(|| {
            panic!("no upload {upload_id} is stored for sequence store {sequence_store_id}")
        })
}

/// A row of the create table: the request body, the members the response must report for
/// it, and the members it must not carry.
struct UploadCase {
    /// How the row is reported in a failure, and the upload's `name` and `clientToken`.
    case: &'static str,
    /// The request body, without the `name`/`clientToken`/`sourceFileType` the row fills
    /// in around it.
    request: Value,
    /// The `sourceFileType` the row uploads.
    source_file_type: &'static str,
    /// Every member the response must carry with exactly this value.
    expected: Value,
    /// Members the response must NOT carry: nothing is invented for what the request left
    /// out.
    absent: Vec<&'static str>,
}

/// The table: one row per way a create can differ, so the rule is asserted over the class
/// and not over one request. The rows vary along the source format, the members present,
/// the size of the tag map and the length of the caller-sized strings.
fn upload_cases() -> Vec<UploadCase> {
    let mut cases = vec![
        UploadCase {
            case: "only the required members",
            request: json!({ "subjectId": "subject-a", "sampleId": "sample-a" }),
            source_file_type: "FASTQ",
            expected: json!({
                "subjectId": "subject-a",
                "sampleId": "sample-a",
                "sourceFileType": "FASTQ",
            }),
            absent: vec!["description", "generatedFrom", "tags"],
        },
        UploadCase {
            case: "a described upload",
            request: json!({
                "subjectId": "subject-b",
                "sampleId": "sample-b",
                "description": "reads for the autumn cohort",
            }),
            source_file_type: "FASTQ",
            expected: json!({
                "subjectId": "subject-b",
                "sampleId": "sample-b",
                "description": "reads for the autumn cohort",
            }),
            absent: vec!["generatedFrom", "tags"],
        },
        UploadCase {
            case: "a named source",
            request: json!({
                "subjectId": "subject-c",
                "sampleId": "sample-c",
                "generatedFrom": "1000 Genomes",
            }),
            source_file_type: "UBAM",
            expected: json!({
                "subjectId": "subject-c",
                "sampleId": "sample-c",
                "generatedFrom": "1000 Genomes",
                "sourceFileType": "UBAM",
            }),
            absent: vec!["description", "tags"],
        },
        UploadCase {
            case: "an aligned upload with a reference",
            request: json!({
                "subjectId": "subject-d",
                "sampleId": "sample-d",
                "referenceArn": REFERENCE_ARN,
            }),
            source_file_type: "CRAM",
            expected: json!({
                "subjectId": "subject-d",
                "sampleId": "sample-d",
                "sourceFileType": "CRAM",
                "referenceArn": REFERENCE_ARN,
            }),
            absent: vec!["description", "generatedFrom", "tags"],
        },
        UploadCase {
            case: "no tags at all",
            request: json!({
                "subjectId": "subject-e",
                "sampleId": "sample-e",
                "tags": tag_map(0),
            }),
            source_file_type: "FASTQ",
            expected: json!({ "subjectId": "subject-e", "sampleId": "sample-e" }),
            absent: vec!["description", "generatedFrom", "tags"],
        },
        UploadCase {
            case: "one tag",
            request: json!({
                "subjectId": "subject-f",
                "sampleId": "sample-f",
                "tags": tag_map(1),
            }),
            source_file_type: "FASTQ",
            expected: json!({
                "subjectId": "subject-f",
                "sampleId": "sample-f",
                "tags": tag_map(1),
            }),
            absent: vec!["description", "generatedFrom"],
        },
        UploadCase {
            case: "tags at the documented bound",
            request: json!({
                "subjectId": "subject-g",
                "sampleId": "sample-g",
                "tags": tag_map(MAX_TAGS),
            }),
            source_file_type: "BAM",
            expected: json!({
                "subjectId": "subject-g",
                "sampleId": "sample-g",
                "sourceFileType": "BAM",
                "tags": tag_map(MAX_TAGS),
            }),
            absent: vec!["description", "generatedFrom"],
        },
        UploadCase {
            case: "a description at the published bound",
            request: json!({
                "subjectId": "subject-h",
                "sampleId": "sample-h",
                "description": "d".repeat(READ_SET_DESCRIPTION_MAX),
            }),
            source_file_type: "FASTQ",
            expected: json!({
                "subjectId": "subject-h",
                "sampleId": "sample-h",
                "description": "d".repeat(READ_SET_DESCRIPTION_MAX),
            }),
            absent: vec!["generatedFrom", "tags"],
        },
        UploadCase {
            case: "every optional member at once",
            request: json!({
                "subjectId": "subject-i",
                "sampleId": "sample-i",
                "description": "everything named",
                "generatedFrom": "an EC2 instance",
                "referenceArn": REFERENCE_ARN,
                "tags": tag_map(3),
            }),
            source_file_type: "BAM",
            expected: json!({
                "subjectId": "subject-i",
                "sampleId": "sample-i",
                "description": "everything named",
                "generatedFrom": "an EC2 instance",
                "referenceArn": REFERENCE_ARN,
                "sourceFileType": "BAM",
                "tags": tag_map(3),
            }),
            absent: vec![],
        },
    ];
    // One row per value the modelled `FileType` enum admits, so the whole catalog is
    // exercised and not only the formats the rows above happen to name. An aligned format
    // carries a reference; an unaligned one does not.
    for file_type in FILE_TYPES {
        let aligned = ALIGNED_FILE_TYPES.contains(file_type);
        cases.push(UploadCase {
            case: file_type,
            request: if aligned {
                json!({
                    "subjectId": format!("subject-{file_type}"),
                    "sampleId": format!("sample-{file_type}"),
                    "referenceArn": REFERENCE_ARN,
                })
            } else {
                json!({
                    "subjectId": format!("subject-{file_type}"),
                    "sampleId": format!("sample-{file_type}"),
                })
            },
            source_file_type: file_type,
            expected: json!({
                "subjectId": format!("subject-{file_type}"),
                "sampleId": format!("sample-{file_type}"),
                "sourceFileType": file_type,
            }),
            absent: vec!["description", "generatedFrom", "tags"],
        });
    }
    cases
}

/// The request body for a row: its own members plus the `name`, `clientToken` and
/// `sourceFileType` the row is named by.
fn request_for(case: &UploadCase) -> Value {
    let mut body = case.request.as_object().cloned().unwrap_or_default();
    body.insert("name".to_string(), Value::String(case.case.to_string()));
    body.insert(
        "clientToken".to_string(),
        Value::String(format!("token-{}", case.case)),
    );
    body.insert(
        "sourceFileType".to_string(),
        Value::String(case.source_file_type.to_string()),
    );
    Value::Object(body)
}

/// Assert the stored record carries what the request named, member for member.
fn assert_stored(case: &UploadCase, request: &Value, upload: &MultipartReadSetUpload) {
    let member = |name: &str| request.get(name).and_then(Value::as_str).map(str::to_string);
    assert_eq!(
        Some(upload.name.clone()),
        member("name"),
        "case {:?}: the stored name must be the one the request carried, got {upload:?}",
        case.case
    );
    assert_eq!(
        Some(upload.subject_id.clone()),
        member("subjectId"),
        "case {:?}: the stored subjectId must be the one the request carried, got {upload:?}",
        case.case
    );
    assert_eq!(
        Some(upload.sample_id.clone()),
        member("sampleId"),
        "case {:?}: the stored sampleId must be the one the request carried, got {upload:?}",
        case.case
    );
    assert_eq!(
        Some(upload.source_file_type.clone()),
        member("sourceFileType"),
        "case {:?}: the stored sourceFileType must be the one the request carried, got {upload:?}",
        case.case
    );
    assert_eq!(
        upload.description, member("description"),
        "case {:?}: the stored description must be the one the request carried, got {upload:?}",
        case.case
    );
    assert_eq!(
        upload.generated_from,
        member("generatedFrom"),
        "case {:?}: the stored generatedFrom must be the one the request carried, got {upload:?}",
        case.case
    );
    assert_eq!(
        upload.reference_arn,
        member("referenceArn"),
        "case {:?}: the stored referenceArn must be the one the request carried, got {upload:?}",
        case.case
    );
    let expected_tags = request
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| tags.len())
        .unwrap_or(0);
    assert_eq!(
        upload.tags.len(),
        expected_tags,
        "case {:?}: expected {expected_tags} stored tags, got {upload:?}",
        case.case
    );
}

/// Assert the response reports `referenceArn` exactly when the request named one.
///
/// The modelled `ReferenceArn` is `@length(1..127)` `@pattern("^arn:.+$")`, so there is no
/// value standing for "this upload has no reference": an upload initiated without one
/// leaves the member out of the body rather than reporting the empty string.
fn assert_reference_arn(case: &str, request: &Value, body: &Value) {
    let named = request.get("referenceArn");
    assert_eq!(
        body.get("referenceArn"),
        named,
        "case {case:?}: expected referenceArn {named:?}, got {:?} in {body:?}",
        body.get("referenceArn")
    );
}

#[tokio::test]
async fn an_initiated_multipart_read_set_upload_is_reported_from_its_stored_record() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the upload destination").await;
    let cases = upload_cases();

    // Initiate them all first, so each assertion also proves the store keeps the rows
    // apart rather than answering with the most recent one.
    let mut created = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for upload in &cases {
        let request = request_for(upload);
        let (status, body) = create_upload(&router, &store, &request).await;
        let upload_id = created_upload_id(upload.case, status, &body);
        assert_members(
            upload.case,
            "CreateMultipartReadSetUpload",
            &upload.expected,
            &body,
        );
        assert_members(
            upload.case,
            "CreateMultipartReadSetUpload",
            &json!({ "sequenceStoreId": store, "name": upload.case }),
            &body,
        );
        assert_absent(upload.case, &upload.absent, &body);
        assert_reference_arn(upload.case, &request, &body);
        assert_timestamp(upload.case, "creationTime", &body);
        assert!(
            ids.insert(upload_id.clone()),
            "case {:?}: uploadId {upload_id} was already handed out",
            upload.case
        );
        created.push((upload_id, request, body));
    }

    // Nothing above was a shallow success: every upload is really in the service's state,
    // under the store it was initiated against, with the members the request carried.
    for (upload, (upload_id, request, body)) in cases.iter().zip(&created) {
        let stored = stored_upload(&engine, &store, upload_id);
        assert_eq!(
            stored.sequence_store_id, store,
            "case {:?}: the stored upload must name the store it was initiated against, \
             got {stored:?}",
            upload.case
        );
        assert_eq!(
            stored.upload_id, *upload_id,
            "case {:?}: the stored record must carry the uploadId the response reported, \
             got {stored:?}",
            upload.case
        );
        assert_stored(upload, request, &stored);
        assert_eq!(
            body.get("creationTime"),
            create_upload(&router, &store, request)
                .await
                .1
                .get("creationTime"),
            "case {:?}: a replay must report the creation time the record holds, got \
             {stored:?}",
            upload.case
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_returns_the_same_upload() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the replay destination").await;
    // The rule generalises over the token's text and over how much of the rest of the
    // request changes on the replay: the token alone decides.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "identical replay",
            json!({
                "clientToken": "upload-1",
                "name": "alpha",
                "subjectId": "s1",
                "sampleId": "p1",
                "sourceFileType": "FASTQ",
            }),
            json!({
                "clientToken": "upload-1",
                "name": "alpha",
                "subjectId": "s1",
                "sampleId": "p1",
                "sourceFileType": "FASTQ",
            }),
        ),
        (
            "replay with different members",
            json!({
                "clientToken": "upload-2",
                "name": "beta",
                "subjectId": "s2",
                "sampleId": "p2",
                "sourceFileType": "FASTQ",
                "description": "first",
            }),
            json!({
                "clientToken": "upload-2",
                "name": "beta-renamed",
                "subjectId": "s2-other",
                "sampleId": "p2-other",
                "sourceFileType": "BAM",
                "referenceArn": REFERENCE_ARN,
            }),
        ),
        (
            "replay of a token that is all punctuation",
            json!({
                "clientToken": "::upload-3::",
                "name": "gamma",
                "subjectId": "s3",
                "sampleId": "p3",
                "sourceFileType": "UBAM",
            }),
            json!({
                "clientToken": "::upload-3::",
                "name": "gamma",
                "subjectId": "s3",
                "sampleId": "p3",
                "sourceFileType": "UBAM",
            }),
        ),
    ];

    let mut first = Vec::new();
    for (case, body, _) in &rows {
        let (status, response) = create_upload(&router, &store, body).await;
        let id = created_upload_id(case, status, &response);
        first.push((id, response));
    }

    for ((case, _, replay), (expected_id, expected_body)) in rows.iter().zip(&first) {
        let (status, body) = create_upload(&router, &store, replay).await;
        let id = created_upload_id(case, status, &body);
        assert_eq!(
            &id, expected_id,
            "case {case:?}: a replayed clientToken must answer with the original uploadId, \
             got {body:?}"
        );
        for member in [
            "name",
            "subjectId",
            "sampleId",
            "sourceFileType",
            "description",
            "referenceArn",
            "creationTime",
        ] {
            assert_eq!(
                body.get(member),
                expected_body.get(member),
                "case {case:?}: a replayed clientToken must answer with the original \
                 {member}, got {body:?}"
            );
        }
    }

    // A request that presents no token is deduped against nothing: each one is its own
    // upload, even when every other member is identical.
    let untokened = json!({
        "name": "no token at all",
        "subjectId": "s4",
        "sampleId": "p4",
        "sourceFileType": "FASTQ",
    });
    let (status, body) = create_upload(&router, &store, &untokened).await;
    let one = created_upload_id("no token, first call", status, &body);
    let (status, body) = create_upload(&router, &store, &untokened).await;
    let two = created_upload_id("no token, second call", status, &body);
    assert_ne!(
        one, two,
        "case \"no token at all\": two requests carrying no clientToken must be two \
         uploads, got {one} twice"
    );
}

#[tokio::test]
async fn uploads_are_kept_per_sequence_store() {
    let (router, engine) = emulator_holding_its_engine();
    let first = create_store(&router, "the first store").await;
    let second = create_store(&router, "the second store").await;
    assert_ne!(first, second, "the two stores must be two resources");

    // The same clientToken presented to two stores initiates an upload in each: an upload
    // belongs to the store it was initiated against, so the token resumes nothing across
    // them.
    let body = json!({
        "clientToken": "shared-token",
        "name": "shared name",
        "subjectId": "shared-subject",
        "sampleId": "shared-sample",
        "sourceFileType": "FASTQ",
    });
    let mut ids = Vec::new();
    for (case, store) in [("first store", &first), ("second store", &second)] {
        let (status, response) = create_upload(&router, store, &body).await;
        let id = created_upload_id(case, status, &response);
        assert_members(
            case,
            "CreateMultipartReadSetUpload",
            &json!({ "sequenceStoreId": store }),
            &response,
        );
        ids.push((case, store.clone(), id));
    }
    assert_ne!(
        ids[0].2, ids[1].2,
        "an upload in each store must be its own resource, got {} twice",
        ids[0].2
    );

    for (case, store, id) in &ids {
        let stored = stored_upload(&engine, store, id);
        assert_eq!(
            &stored.sequence_store_id, store,
            "case {case:?}: the stored upload must name its own store, got {stored:?}"
        );
        // And it is not reachable through the other store: an upload id is resolved
        // inside the store the request names.
        let other = if store == &first { &second } else { &first };
        let state = service_state(&*engine).expect("the omics service state");
        let registry = state.lock();
        assert!(
            registry.multipart_read_set_upload(other, id).is_none(),
            "case {case:?}: upload {id} must not resolve under store {other}"
        );
    }
}

#[tokio::test]
async fn an_upload_into_an_unknown_sequence_store_is_not_found() {
    let (router, _engine) = emulator_holding_its_engine();
    // One real store exists, so the 404s below are about the id, not about an empty
    // registry.
    let existing = create_store(&router, "the only store").await;

    // The rule generalises over the id's width and value, across the whole modelled
    // `SequenceStoreId` length (10..36): anything the registry does not hold is not
    // found, however it is shaped.
    let (min, max) = SEQUENCE_STORE_ID_WIDTH;
    let ids = [
        "0".repeat(min),
        "9".repeat(min),
        "1234567890".to_string(),
        "1".repeat(max),
        "9".repeat(max),
    ];
    let body = json!({
        "name": "an upload with nowhere to go",
        "subjectId": "subject",
        "sampleId": "sample",
        "sourceFileType": "FASTQ",
    });
    for id in &ids {
        assert_ne!(
            id, &existing,
            "the fixture must not collide with the real id"
        );
        let (status, response) = create_upload(&router, id, &body).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from CreateMultipartReadSetUpload, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn requests_outside_the_published_bounds_are_refused() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the bounds destination").await;

    // Each row is a caller-sized member at its published bound and one character past it:
    // the bound is accepted and anything beyond it is the modelled 400. `tags` carries no
    // modelled length, so its bound is the documented 50.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a name at the published bound",
            json!({ "name": "n".repeat(READ_SET_NAME_MAX) }),
            true,
        ),
        (
            "a name one past the published bound",
            json!({ "name": "n".repeat(READ_SET_NAME_MAX + 1) }),
            false,
        ),
        (
            "a description at the published bound",
            json!({
                "name": "described",
                "description": "d".repeat(READ_SET_DESCRIPTION_MAX),
            }),
            true,
        ),
        (
            "a description one past the published bound",
            json!({
                "name": "over-described",
                "description": "d".repeat(READ_SET_DESCRIPTION_MAX + 1),
            }),
            false,
        ),
        (
            "tags at the documented bound",
            json!({ "name": "tagged", "tags": tag_map(MAX_TAGS) }),
            true,
        ),
        (
            "tags one past the documented bound",
            json!({ "name": "over-tagged", "tags": tag_map(MAX_TAGS + 1) }),
            false,
        ),
    ];

    for (case, row, expect_ok) in &rows {
        let mut body = row.as_object().cloned().unwrap_or_default();
        body.insert("subjectId".to_string(), Value::String("subject".to_string()));
        body.insert("sampleId".to_string(), Value::String("sample".to_string()));
        body.insert(
            "sourceFileType".to_string(),
            Value::String("FASTQ".to_string()),
        );
        let (status, response) = create_upload(&router, &store, &Value::Object(body)).await;
        if *expect_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {response:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
        }
    }
}
