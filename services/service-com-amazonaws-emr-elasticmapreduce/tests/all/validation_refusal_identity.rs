// SPDX-License-Identifier: CC0-1.0

//! The IDENTITY an Amazon EMR input-validation refusal travels under, end to end over
//! the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends.
//!
//! THE RULE every table below asserts: a request Amazon EMR refuses on its own input is
//! answered with the identity that operation and that class carry — the error TYPE
//! Amazon EMR raises (its modelled client error, or the request-validation envelope where
//! the refusal is a judgement about the request's shape rather than about a target it
//! resolved), the `ErrorCode` that operation publishes for the class where it publishes
//! one, and the class's own sentence: its fixed text exactly, with only the parts Amazon
//! EMR interpolates taken from the request. Nothing the emulator authored reaches the
//! wire — no parser diagnostic, no range-checker prose, and no sentence borrowed from
//! another operation's class.
//!
//! The recorded instance behind each rule is ONE row; every table spans the class —
//! several forms of the offending value, and the operations beside the recorded one that
//! answer the same class — and each table carries the control that shows the refusal is a
//! judgement rather than a blanket answer.

use serde_json::{json, Value};

use crate::cluster_id_refusal_code::{INVALID_CLUSTER_ID, MALFORMED_CLUSTER_ID};
use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, minimal_request, Emr};
use crate::malformed_input_refusals::INVALID_RELEASE_LABEL;
use crate::notebook_executions::{engine_sentence, REQUEST_VALIDATION};
use crate::refusal_messages::message;
use crate::refusal_wording::{
    MALFORMED_SESSION_IDS, UNKNOWN_CLUSTER_IDS, WELL_FORMED_SESSION_IDS,
};
use crate::resource_tags::error_code;

/// The service's own client error, which carries `ErrorCode` beside `Message`.
const SERVICE_ERROR: &str = "InvalidRequestException";

/// The discriminator the release-DESCRIBE read publishes for a label naming none of the
/// catalog's releases.
const INVALID_INPUT_PARAMETER: &str = "InvalidInputParameter";

/// The discriminator `CreateSecurityConfiguration` publishes for a document it will not
/// store.
const INVALID_SECURITY_CONFIGURATION: &str = "INVALID_SECURITY_CONFIGURATION";

/// Release labels the catalog offers none of. Rows differ along WHY the label is
/// unusable: a version the catalog does not carry, a label of no release form at all,
/// one differently cased, and one missing a component.
const UNOFFERED_RELEASE_LABELS: &[&str] = &["emr-6.99.0", "6.15.0", "EMR-7.5.0", "emr-7.5", "x"];

/// A release the catalog does offer, so each table can show its refusals are lookups.
const CATALOGUED_RELEASE: &str = "emr-7.5.0";

// ---------------------------------------------------------------------------
// The session surface: the shape of a session id is judged before its resources
// ---------------------------------------------------------------------------

/// THE RULE for the session read: a `SessionId` outside the form Amazon EMR mints one in
/// names no session on any cluster, so it is judged as a bad REQUEST — the
/// request-validation envelope, HTTP 400, stating the session's qualified path exactly as
/// the request spelled it — BEFORE the cluster beside it is resolved; a well-formed id
/// instead reaches the lookups, where the cluster surface's own sentence answers for an
/// id no cluster is held under.
#[tokio::test]
async fn a_session_id_outside_the_minted_form_is_judged_before_the_cluster() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    // Rows pair every unmintable id with both kinds of cluster beside it: the id's shape
    // is what answers either way, which is what shows the gate runs first.
    for session_id in MALFORMED_SESSION_IDS {
        for (label, cluster_id) in [
            ("a cluster that exists", live.clone()),
            ("a cluster that does not", "j-ZZZZZZZZZZZZZ".to_string()),
        ] {
            let case = format!("GetSession({label}, {session_id:?})");
            let (status, payload) = emr
                .call(
                    "GetSession",
                    json!({ "ClusterId": &cluster_id, "SessionId": session_id }),
                )
                .await;
            assert_refused(&case, status, &payload, REQUEST_VALIDATION);
            let expected = format!("sessionId '{cluster_id}/session/{session_id}' is not valid.");
            assert_eq!(
                message(&case, &payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
            assert!(
                at(&payload, &["Session"]).is_null(),
                "case {case:?}: a refusal carries no Session, got {payload}"
            );
        }
    }

    // The other half: an id of the minted form is a target rather than a bad request, so
    // the cluster it names is resolved and that surface's refusal is the answer.
    for session_id in WELL_FORMED_SESSION_IDS {
        for unresolvable in UNKNOWN_CLUSTER_IDS {
            let case = format!("GetSession({unresolvable:?}, {session_id:?})");
            let (status, payload) = emr
                .call(
                    "GetSession",
                    json!({ "ClusterId": unresolvable, "SessionId": session_id }),
                )
                .await;
            assert_refused(&case, status, &payload, SERVICE_ERROR);
            let expected = format!("Cluster id '{unresolvable}' is not valid.");
            assert_eq!(
                message(&case, &payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
            assert_eq!(
                error_code(&payload).as_deref(),
                Some(MALFORMED_CLUSTER_ID),
                "case {case:?}: expected ErrorCode {MALFORMED_CLUSTER_ID}, got {:?} in \
                 {payload}",
                error_code(&payload)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetClusterSessionCredentials: its own code for an unresolvable cluster
// ---------------------------------------------------------------------------

/// THE RULE: the code an unresolvable cluster id is refused under is the one the
/// OPERATION publishes for the class — the session-credentials read publishes its own,
/// beside the sentence it states without the id, while the cluster reads publish theirs
/// beside the sentence that quotes the id. Rows span every form of unresolvable id and
/// both surfaces, so neither code can stand in for the other.
#[tokio::test]
async fn the_session_credentials_read_publishes_its_own_cluster_id_code() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    for unresolvable in UNKNOWN_CLUSTER_IDS {
        let case = format!("GetClusterSessionCredentials with {unresolvable:?}");
        let (status, payload) = emr
            .call(
                "GetClusterSessionCredentials",
                json!({
                    "ClusterId": unresolvable,
                    "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
                }),
            )
            .await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(INVALID_CLUSTER_ID),
            "case {case:?}: expected ErrorCode {INVALID_CLUSTER_ID}, got {:?} in {payload}",
            error_code(&payload)
        );
        let said = message(&case, &payload);
        assert_eq!(
            said, "Cluster id is not valid.",
            "case {case:?}: unexpected message, got {payload}"
        );

        // The same id on a CLUSTER read answers the cluster reads' code, so the two
        // surfaces are told apart by the discriminator alone.
        let case = format!("DescribeCluster with {unresolvable:?}");
        let (_, payload) = emr
            .call("DescribeCluster", json!({ "ClusterId": unresolvable }))
            .await;
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(MALFORMED_CLUSTER_ID),
            "case {case:?}: expected ErrorCode {MALFORMED_CLUSTER_ID}, got {:?} in {payload}",
            error_code(&payload)
        );
    }

    // The control: on a cluster that EXISTS the refusal is a different class entirely —
    // it names the security configuration rather than the cluster id — so the code above
    // is a statement about the lookup rather than about the operation.
    let case = "GetClusterSessionCredentials on a cluster that exists";
    let (status, payload) = emr
        .call(
            "GetClusterSessionCredentials",
            json!({
                "ClusterId": &live,
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
            }),
        )
        .await;
    assert_refused(case, status, &payload, SERVICE_ERROR);
    assert_ne!(
        error_code(&payload).as_deref(),
        Some(INVALID_CLUSTER_ID),
        "case {case:?}: a resolved cluster is not refused as an unusable cluster id, got \
         {payload}"
    );
}

// ---------------------------------------------------------------------------
// StartNotebookExecution: an execution engine that resolves no cluster
// ---------------------------------------------------------------------------

/// THE RULE: an `ExecutionEngine` that resolves no usable Amazon EMR cluster is judged as
/// a bad REQUEST — the request-validation envelope, HTTP 400 — with the class's own
/// sentence, which names the cluster RESOLUTION produced: the absent-value rendering
/// where it produced none, which is every refusal of this class, and so never the id the
/// request sent.
///
/// The per-row assertion is built from what the engine resolved to rather than from a
/// literal, and each row additionally asserts the id it sent is NOT in the sentence —
/// which is the assertion that tells this rule apart from the other candidate (echoing
/// `ExecutionEngine.Id`) and that the recorded instance of the class settles: a recorded
/// request carrying an id is refused with the slot absent.
#[tokio::test]
async fn an_unusable_execution_engine_is_refused_with_the_engine_sentence() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    // Rows differ along the form of the engine id, including one that is not a cluster id
    // at all and the empty id the member's length admits.
    for engine_id in UNKNOWN_CLUSTER_IDS {
        let case = format!("StartNotebookExecution on engine {engine_id:?}");
        let (status, payload) = emr
            .call(
                "StartNotebookExecution",
                json!({
                    "NotebookExecutionName": "probe",
                    "RelativePath": "notebooks/demo.ipynb",
                    "ExecutionEngine": { "Id": engine_id, "Type": "EMR" },
                    "ServiceRole": "EMR_Notebooks_DefaultRole",
                }),
            )
            .await;
        assert_refused(&case, status, &payload, REQUEST_VALIDATION);
        // Nothing resolved, so the sentence names no cluster.
        let expected = engine_sentence(None);
        let said = message(&case, &payload);
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );
        if !engine_id.is_empty() {
            assert!(
                !said.contains(engine_id),
                "case {case:?}: the sentence names what resolution produced, not the id \
                 the request sent, got {said:?}"
            );
        }
        assert!(
            at(&payload, &["NotebookExecutionId"]).is_null(),
            "case {case:?}: a refusal mints no execution id, got {payload}"
        );
    }

    // The control: the same request against a cluster this service holds is recorded, and
    // the execution reads back — so the refusals above are lookups of persisted state.
    let started = emr
        .ok(
            "StartNotebookExecution",
            json!({
                "NotebookExecutionName": "admitted",
                "RelativePath": "notebooks/demo.ipynb",
                "ExecutionEngine": { "Id": &live, "Type": "EMR" },
                "ServiceRole": "EMR_Notebooks_DefaultRole",
            }),
        )
        .await;
    let id = started["NotebookExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("an admitted start answers an id, got {started}"))
        .to_string();
    let described = emr
        .ok(
            "DescribeNotebookExecution",
            json!({ "NotebookExecutionId": &id }),
        )
        .await;
    assert_eq!(
        at(&described, &["NotebookExecution", "NotebookExecutionId"]).as_str(),
        Some(id.as_str()),
        "the admitted execution must read back, got {described}"
    );
}

// ---------------------------------------------------------------------------
// The release-label surface: one class, two operations, two identities
// ---------------------------------------------------------------------------

/// THE RULE: each operation that resolves a release label refuses an unoffered one under
/// its OWN code and its OWN sentence — the DESCRIBE read with a constant sentence that
/// echoes nothing, the instance-type listing with one that names the label the request
/// sent — so neither identity stands in for the other. Rows span the forms of unusable
/// label.
#[tokio::test]
async fn each_release_label_surface_publishes_its_own_code_and_sentence() {
    let emr = Emr::new();

    for label in UNOFFERED_RELEASE_LABELS {
        let case = format!("DescribeReleaseLabel with {label:?}");
        let (status, payload) = emr
            .call("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
            .await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(INVALID_INPUT_PARAMETER),
            "case {case:?}: expected ErrorCode {INVALID_INPUT_PARAMETER}, got {:?} in \
             {payload}",
            error_code(&payload)
        );
        let said = message(&case, &payload);
        assert_eq!(
            said, "The release label isn't valid.",
            "case {case:?}: unexpected message, got {payload}"
        );
        assert!(
            !said.contains(label),
            "case {case:?}: the sentence echoes no label, got {said:?}"
        );

        let case = format!("ListSupportedInstanceTypes with {label:?}");
        let (status, payload) = emr
            .call(
                "ListSupportedInstanceTypes",
                json!({ "ReleaseLabel": label }),
            )
            .await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(INVALID_RELEASE_LABEL),
            "case {case:?}: expected ErrorCode {INVALID_RELEASE_LABEL}, got {:?} in {payload}",
            error_code(&payload)
        );
        let expected = format!("The supplied release label is invalid: {label}.");
        assert_eq!(
            message(&case, &payload),
            expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );
    }

    // The control: a label the catalog DOES offer is served by both operations, so the
    // refusals above are lookups of the catalog.
    let described = emr
        .ok(
            "DescribeReleaseLabel",
            json!({ "ReleaseLabel": CATALOGUED_RELEASE }),
        )
        .await;
    assert_eq!(
        described["ReleaseLabel"].as_str(),
        Some(CATALOGUED_RELEASE),
        "case {CATALOGUED_RELEASE:?}: an offered release must be described, got {described}"
    );
    let listed = emr
        .ok(
            "ListSupportedInstanceTypes",
            json!({ "ReleaseLabel": CATALOGUED_RELEASE }),
        )
        .await;
    assert!(
        listed["SupportedInstanceTypes"]
            .as_array()
            .is_some_and(|types| !types.is_empty()),
        "case {CATALOGUED_RELEASE:?}: an offered release must report instance types, got \
         {listed}"
    );
}

// ---------------------------------------------------------------------------
// StepConcurrencyLevel: one band, one sentence, every operation that takes it
// ---------------------------------------------------------------------------

/// The documented band a step concurrency level lies in: "The default value is 1. The
/// maximum value is 256." (`RunJobFlow$StepConcurrencyLevel` doc) / "a minimum of 1 step
/// and a maximum of 256 steps" (`ModifyCluster$StepConcurrencyLevel` doc).
const STEP_CONCURRENCY_BAND: (i64, i64) = (1, 256);

/// The widest ceiling any step execution mode raises the band to, exported by the service
/// as `state::step_execution_mode::DISTRIBUTED_MAX_STEP_CONCURRENCY_LEVEL`: a level past
/// it is one no mode of the service could take.
const WIDEST_STEP_CONCURRENCY_CEILING: i64 = 6400;

/// The sentence a level NO mode could take is refused with: the band the cluster's own mode
/// allows, and nothing of the request.
const BEYOND_EVERY_BAND_SENTENCE: &str =
    "Invalid step concurrency level. Allowed level is from 1 to 256";

/// The sentence a level above the cluster's own ceiling but inside the widest one a mode
/// raises it to is refused with: the level it could not take, the ceiling and the mode that
/// ceiling belongs to, then the mode that would take it and that mode's own ceiling.
fn above_the_band_sentence(level: i64) -> String {
    format!(
        "StepConcurrencyLevel {level} exceeds the maximum (256) for StepExecutionMode \
         STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps."
    )
}

/// THE RULE: a `StepConcurrencyLevel` outside the band the cluster's step execution mode
/// allows is refused with the class's own sentence for WHICH class it falls into — the
/// level, the ceiling it exceeded and the mode that would take it for a level another mode
/// would take, and the cluster's own band for one no mode could take — on every operation
/// that takes the member; a level inside the band is stored and read back. Rows span either
/// side of each published bound and far past both.
#[tokio::test]
async fn a_step_concurrency_level_outside_the_band_is_refused_per_side_of_the_band() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("concurrency")).await;
    let (low, high) = STEP_CONCURRENCY_BAND;
    let widest = WIDEST_STEP_CONCURRENCY_CEILING;

    let out_of_band = [
        (low - 1, BEYOND_EVERY_BAND_SENTENCE.to_string()),
        (low - 2, BEYOND_EVERY_BAND_SENTENCE.to_string()),
        (-1, BEYOND_EVERY_BAND_SENTENCE.to_string()),
        (high + 1, above_the_band_sentence(high + 1)),
        (high + 2, above_the_band_sentence(high + 2)),
        (widest, above_the_band_sentence(widest)),
        (widest + 1, BEYOND_EVERY_BAND_SENTENCE.to_string()),
        (10_000, BEYOND_EVERY_BAND_SENTENCE.to_string()),
        (i64::from(i32::MAX), BEYOND_EVERY_BAND_SENTENCE.to_string()),
    ];

    for (level, sentence) in out_of_band {
        let case = format!("RunJobFlow with StepConcurrencyLevel {level}");
        let mut request = minimal_request("out of band");
        request["StepConcurrencyLevel"] = json!(level);
        let (status, payload) = emr.call("RunJobFlow", request).await;
        assert_refused(&case, status, &payload, REQUEST_VALIDATION);
        assert_eq!(
            message(&case, &payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );

        let case = format!("ModifyCluster with StepConcurrencyLevel {level}");
        let (status, payload) = emr
            .call(
                "ModifyCluster",
                json!({ "ClusterId": &cluster, "StepConcurrencyLevel": level }),
            )
            .await;
        assert_refused(&case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            message(&case, &payload),
            sentence,
            "case {case:?}: expected {sentence:?}, got {payload}"
        );
    }

    // Both published bounds, and one between them, are inside the band: each is stored
    // and reported, so the refusals above are the band rather than the member.
    for level in [low, high, (low + high) / 2] {
        let case = format!("ModifyCluster with StepConcurrencyLevel {level}");
        emr.ok(
            "ModifyCluster",
            json!({ "ClusterId": &cluster, "StepConcurrencyLevel": level }),
        )
        .await;
        let described = emr.describe_cluster(&cluster).await;
        assert_eq!(
            described["StepConcurrencyLevel"].as_i64(),
            Some(level),
            "case {case:?}: the stored level must read back, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateSecurityConfiguration: the document class carries its own code
// ---------------------------------------------------------------------------

/// The sentence a document that is not JSON is refused with: constant, naming neither the
/// parser, nor where it stopped, nor anything the document carried.
const MALFORMED_DOCUMENT_SENTENCE: &str = "Invalid security configuration; Malformed Json string.";

/// A security configuration document the service stores.
const STORED_DOCUMENT: &str = r#"{"EncryptionConfiguration":{"EnableInTransitEncryption":false,"EnableAtRestEncryption":false}}"#;

/// THE RULE: a `SecurityConfiguration` document the service will not store is refused
/// under the code Amazon EMR publishes for that class, beside the class's constant
/// sentence — the same identity for every document that fails to parse, whatever the
/// parser would have said about it. Rows span the ways a document fails to parse, each of
/// which stops a parser somewhere else.
#[tokio::test]
async fn a_document_that_is_not_json_carries_the_document_code_and_sentence() {
    let emr = Emr::new();

    let documents = [
        ("bare text", "this-is-not-json"),
        ("an unquoted key", "{not valid json"),
        ("an unterminated object", "{\"EncryptionConfiguration\":"),
        ("an unterminated array", "["),
        ("an empty document", ""),
        ("a missing value", "{\"EncryptionConfiguration\":}"),
        ("a trailing comma", "{\"a\": 1,}"),
        ("single-quoted JSON", "{'EncryptionConfiguration': {}}"),
        ("an XML document", "<EncryptionConfiguration/>"),
        ("a bare number with a stray tail", "12 34"),
    ];

    for (case, document) in documents {
        let name = format!("malformed-{case}");
        let (status, payload) = emr
            .call(
                "CreateSecurityConfiguration",
                json!({ "Name": &name, "SecurityConfiguration": document }),
            )
            .await;
        assert_refused(case, status, &payload, SERVICE_ERROR);
        assert_eq!(
            error_code(&payload).as_deref(),
            Some(INVALID_SECURITY_CONFIGURATION),
            "case {case:?}: expected ErrorCode {INVALID_SECURITY_CONFIGURATION}, got {:?} in \
             {payload}",
            error_code(&payload)
        );
        assert_eq!(
            message(case, &payload),
            MALFORMED_DOCUMENT_SENTENCE,
            "case {case:?}: expected {MALFORMED_DOCUMENT_SENTENCE:?}, got {payload}"
        );

        // A refused create stored nothing under the name it carried.
        let (status, payload) = emr
            .call("DescribeSecurityConfiguration", json!({ "Name": &name }))
            .await;
        assert_refused(case, status, &payload, SERVICE_ERROR);
    }

    // The control: a document that IS JSON is stored and reads back, and the conflict a
    // second, different document under that name raises is a DIFFERENT class — one Amazon
    // EMR publishes no code for — so the document code means exactly one thing.
    emr.ok(
        "CreateSecurityConfiguration",
        json!({ "Name": "stored", "SecurityConfiguration": STORED_DOCUMENT }),
    )
    .await;
    let described = emr
        .ok("DescribeSecurityConfiguration", json!({ "Name": "stored" }))
        .await;
    assert_eq!(
        described["SecurityConfiguration"].as_str(),
        Some(STORED_DOCUMENT),
        "the stored document must read back, got {described}"
    );

    let case = "a name already taken by a different document";
    let (status, payload) = emr
        .call(
            "CreateSecurityConfiguration",
            json!({ "Name": "stored", "SecurityConfiguration": "{}" }),
        )
        .await;
    assert_refused(case, status, &payload, SERVICE_ERROR);
    assert_eq!(
        error_code(&payload),
        None,
        "case {case:?}: a conflict carries no document code, got {:?} in {payload}",
        error_code(&payload)
    );
}

/// A refusal carries no success member of the operation it refused: the tables above
/// assert the identity, and this one asserts that nothing was recorded for a request that
/// was refused on its input. One row per operation this suite refuses.
#[tokio::test]
async fn a_refused_request_records_nothing() {
    let emr = Emr::new();

    let refusals: Vec<(&str, Value)> = vec![
        (
            "GetSession",
            json!({ "ClusterId": "j-ZZZZZZZZZZZZZ", "SessionId": "e-3ABCDEFGHIJKL" }),
        ),
        (
            "GetClusterSessionCredentials",
            json!({ "ClusterId": "j-ZZZZZZZZZZZZZ" }),
        ),
        (
            "StartNotebookExecution",
            json!({
                "NotebookExecutionName": "refused",
                "RelativePath": "notebooks/demo.ipynb",
                "ExecutionEngine": { "Id": "j-ZZZZZZZZZZZZZ", "Type": "EMR" },
                "ServiceRole": "EMR_Notebooks_DefaultRole",
            }),
        ),
        (
            "CreateSecurityConfiguration",
            json!({ "Name": "refused", "SecurityConfiguration": "{not json" }),
        ),
    ];
    for (operation, body) in refusals {
        let (status, payload) = emr.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected HTTP 400, got {status} with {payload}"
        );
    }

    // Nothing above reached state: no cluster, no execution and no security
    // configuration is held.
    let clusters = emr.ok("ListClusters", json!({})).await;
    assert_eq!(
        at(&clusters, &["Clusters"]).as_array().map(Vec::len),
        Some(0),
        "a refused request launches no cluster, got {clusters}"
    );
    let executions = emr.ok("ListNotebookExecutions", json!({})).await;
    assert_eq!(
        at(&executions, &["NotebookExecutions"])
            .as_array()
            .map(Vec::len),
        Some(0),
        "a refused start records no execution, got {executions}"
    );
    let configurations = emr.ok("ListSecurityConfigurations", json!({})).await;
    assert_eq!(
        at(&configurations, &["SecurityConfigurations"])
            .as_array()
            .map(Vec::len),
        Some(0),
        "a refused create stores no security configuration, got {configurations}"
    );
}
