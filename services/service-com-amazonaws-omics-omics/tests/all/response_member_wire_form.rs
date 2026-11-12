// SPDX-License-Identifier: CC0-1.0

//! The wire form of two classes of response member of this service: the instants it
//! reports, and a member whose modelled shape admits no value standing for "none".
//!
//! RULE 1 — a response member the model binds to `@timestampFormat("date-time")` travels
//! on the restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING naming the instant the
//! service holds, never as the epoch-seconds NUMBER the protocol defaults to. Every
//! timestamp member of this model declares that format, which the first test below reads
//! off the shipped Smithy model rather than off a copied list, so the rule reaches every
//! shape this service reports an instant in.
//!
//! RULE 2 — a response member the service has no value for is left OUT of the body; it is
//! never reported as the empty string, which the member's own modelled `@length`/`@pattern`
//! would refuse. `CreateMultipartReadSetUpload`'s `referenceArn` is the member this is
//! asserted over: the request's own `referenceArn` is optional (HealthOmics needs a
//! reference genome only for the aligned source formats) while the response's is
//! `@required`, and `ReferenceArn` is `@length(1..127)` `@pattern("^arn:.+$")`.
//!
//! Both rules are asserted over the WIRE — the production `MultiServiceRouter`, restJson1
//! serialization and the real handlers — so what is asserted is what a signed AWS SDK
//! request observes. The HTTP harness is the one `private_workflow_runs` owns and the
//! upload helpers are `multipart_read_set_uploads`'; only the rows live here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::multipart_read_set_uploads::{
    create_store, create_upload, created_upload_id, emulator_holding_its_engine, stored_upload,
    FILE_TYPES, REFERENCE_ARN,
};
use crate::private_workflow_runs::{assert_timestamp, call, created_id, items, start_run,
    start_run_body, workflow_for_runs};

/// This service's Smithy namespace: the prefix of every shape id in its model.
const NAMESPACE: &str = "com.amazonaws.omics";

/// How far a reported instant may sit from this test's own clock reading, in seconds.
///
/// The point of the assertion is that the member names the instant the service wrote
/// rather than zero, a fixed literal or a parse of its own text, so the slack is generous
/// enough that a loaded machine cannot turn it into a timing failure.
const INSTANT_SLACK_SECONDS: f64 = 300.0;

/// The published upper bound of the modelled `ReferenceArn` length.
/// Source: the `referenceArn` member of `CreateMultipartReadSetUploadRequest`
/// (`ReferenceArn`, len 1..127, pattern `^arn:.+$`).
const REFERENCE_ARN_MAX: usize = 127;

// ---------------------------------------------------------------------------
// RULE 1, on the model: every timestamp this service declares is a `date-time`
// ---------------------------------------------------------------------------

/// This service's own Smithy model, as shipped in this crate.
fn model() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-omics-omics.json"
    );
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

/// The `@timestampFormat` a member-like node declares, on the node itself or on the shape
/// it targets.
fn declared_format<'model>(
    shapes: &'model serde_json::Map<String, Value>,
    node: &'model Value,
) -> Option<&'model str> {
    let on_node = node
        .get("traits")
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str);
    let on_target = node
        .get("target")
        .and_then(Value::as_str)
        .and_then(|target| shapes.get(target))
        .and_then(|shape| shape.get("traits"))
        .and_then(|traits| traits.get("smithy.api#timestampFormat"))
        .and_then(Value::as_str);
    on_node.or(on_target)
}

/// Whether a member-like node targets a timestamp: the Smithy prelude's own `Timestamp`,
/// or a model shape declared `"type": "timestamp"`.
fn targets_a_timestamp(shapes: &serde_json::Map<String, Value>, node: &Value) -> bool {
    match node.get("target").and_then(Value::as_str) {
        Some("smithy.api#Timestamp") => true,
        Some(target) => {
            shapes.get(target).and_then(|shape| shape.get("type")) == Some(&json!("timestamp"))
        }
        None => false,
    }
}

/// Every member-like node of one shape, as `(member path, node)`: a structure's or union's
/// members, a list's element, and a map's key and value — the four places a model can bind
/// a timestamp.
fn member_nodes(shape_id: &str, shape: &Value) -> Vec<(String, Value)> {
    let mut nodes = Vec::new();
    if let Some(members) = shape.get("members").and_then(Value::as_object) {
        for (name, node) in members {
            nodes.push((format!("{shape_id}${name}"), node.clone()));
        }
    }
    for single in ["member", "key", "value"] {
        if let Some(node) = shape.get(single) {
            nodes.push((format!("{shape_id}${single}"), node.clone()));
        }
    }
    nodes
}

/// RULE 1 on the model: every member of this model that targets a timestamp declares
/// `@timestampFormat("date-time")`.
///
/// This is what makes the service-wide date-time codec (`service.rs`'s
/// `override_protocol`) the model's own rule rather than a guess: a member that declared
/// `epoch-seconds` or `http-date` would be carried in the wrong form, and would fail here
/// instead. Read off the shipped model, so a model refresh that added such a member is
/// reported rather than silently served.
#[test]
fn every_timestamp_member_of_the_model_declares_the_date_time_format() {
    let model = model();
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model must declare shapes");

    let mut timestamp_members = 0_usize;
    let mut wrong: Vec<String> = Vec::new();
    for (shape_id, shape) in shapes {
        if !shape_id.starts_with(NAMESPACE) {
            continue;
        }
        for (path, node) in member_nodes(shape_id, shape) {
            if !targets_a_timestamp(shapes, &node) {
                continue;
            }
            timestamp_members += 1;
            let format = declared_format(shapes, &node);
            if format != Some("date-time") {
                wrong.push(format!("{path}: {format:?}"));
            }
        }
    }

    assert!(
        wrong.is_empty(),
        "every timestamp member must declare the date-time format, got {} that do not: {wrong:?}",
        wrong.len()
    );
    // A floor, so a walk that found nothing cannot pass as agreement.
    assert!(
        timestamp_members >= 50,
        "the walk must have reached this model's timestamp members, got {timestamp_members}"
    );
}

// ---------------------------------------------------------------------------
// RULE 1, on the wire: every instant this service reports is a date-time string
// ---------------------------------------------------------------------------

/// One document a path answered, and the members of it the rule covers.
struct Reported {
    case: String,
    document: Value,
    /// Members that must be present AND a date-time string.
    required: Vec<&'static str>,
    /// Members that are a date-time string whenever the document carries them (an instant
    /// the record does not hold yet is absent, not zero).
    optional: Vec<&'static str>,
}

/// This test's clock reading, in epoch seconds.
fn now_seconds() -> f64 {
    aws_smithy_types::DateTime::from(std::time::SystemTime::now()).as_secs_f64()
}

/// The `id` a create answered with. `created_id` asserts the 201 the run, workflow, run
/// group and run cache creates answer; `CreateSequenceStore` models a 200, so its status is
/// asserted by the caller.
fn id_of(case: &str, body: &Value) -> String {
    body.get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no id, got {body:?}"))
        .to_string()
}

/// Every document an implemented path of this service reports an instant in: one row per
/// resource family per path, which is what the rule generalises over.
async fn reported_documents(router: &MultiServiceRouter) -> Vec<Reported> {
    let mut rows: Vec<Reported> = Vec::new();

    // A sequence store, and the upload it is the destination of.
    let (status, created_store) = call(
        router,
        "POST",
        "/sequencestore",
        Some(&json!({ "name": "the instant destination" })),
    )
    .await;
    assert_eq!(status, 200, "CreateSequenceStore must answer 200, got {created_store:?}");
    let store_id = id_of("sequence store", &created_store);
    rows.push(Reported {
        case: "CreateSequenceStore".to_string(),
        document: created_store,
        required: vec!["creationTime"],
        optional: vec!["updateTime"],
    });
    let (status, read_store) = call(router, "GET", &format!("/sequencestore/{store_id}"), None).await;
    assert_eq!(status, 200, "GetSequenceStore must answer 200, got {read_store:?}");
    rows.push(Reported {
        case: "GetSequenceStore".to_string(),
        document: read_store,
        required: vec!["creationTime"],
        optional: vec!["updateTime"],
    });

    let (status, upload) = create_upload(
        router,
        &store_id,
        &json!({
            "name": "an instant of an upload",
            "subjectId": "subject",
            "sampleId": "sample",
            "sourceFileType": "FASTQ",
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateMultipartReadSetUpload must answer 200, got {upload:?}");
    rows.push(Reported {
        case: "CreateMultipartReadSetUpload".to_string(),
        document: upload,
        required: vec!["creationTime"],
        optional: vec![],
    });

    // A workflow, a version of it, and both listings.
    let workflow_id = workflow_for_runs(router, "instants").await;
    let (status, workflow) = call(router, "GET", &format!("/workflow/{workflow_id}"), None).await;
    assert_eq!(status, 200, "GetWorkflow must answer 200, got {workflow:?}");
    rows.push(Reported {
        case: "GetWorkflow".to_string(),
        document: workflow,
        required: vec!["creationTime"],
        optional: vec![],
    });
    let (status, listed) = call(router, "GET", "/workflow", None).await;
    for (index, item) in items("ListWorkflows", status, &listed).into_iter().enumerate() {
        rows.push(Reported {
            case: format!("ListWorkflows item {index}"),
            document: item,
            required: vec!["creationTime"],
            optional: vec![],
        });
    }

    let (status, version) = call(
        router,
        "POST",
        &format!("/workflow/{workflow_id}/version"),
        Some(&json!({ "versionName": "1.0.0", "requestId": "instants-version" })),
    )
    .await;
    assert_eq!(status, 201, "CreateWorkflowVersion must answer 201, got {version:?}");
    let (status, version) = call(
        router,
        "GET",
        &format!("/workflow/{workflow_id}/version/1.0.0"),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetWorkflowVersion must answer 200, got {version:?}");
    rows.push(Reported {
        case: "GetWorkflowVersion".to_string(),
        document: version,
        required: vec!["creationTime"],
        optional: vec![],
    });
    let (status, listed) = call(
        router,
        "GET",
        &format!("/workflow/{workflow_id}/version"),
        None,
    )
    .await;
    for (index, item) in items("ListWorkflowVersions", status, &listed)
        .into_iter()
        .enumerate()
    {
        rows.push(Reported {
            case: format!("ListWorkflowVersions item {index}"),
            document: item,
            required: vec!["creationTime"],
            optional: vec![],
        });
    }

    // A run group and a run cache, each read back and listed.
    let (status, group) = call(
        router,
        "POST",
        "/runGroup",
        Some(&json!({ "requestId": "instants-group", "name": "instants" })),
    )
    .await;
    let group_id = created_id("run group", status, &group);
    let (status, group) = call(router, "GET", &format!("/runGroup/{group_id}"), None).await;
    assert_eq!(status, 200, "GetRunGroup must answer 200, got {group:?}");
    rows.push(Reported {
        case: "GetRunGroup".to_string(),
        document: group,
        required: vec!["creationTime"],
        optional: vec![],
    });
    let (status, listed) = call(router, "GET", "/runGroup", None).await;
    for (index, item) in items("ListRunGroups", status, &listed).into_iter().enumerate() {
        rows.push(Reported {
            case: format!("ListRunGroups item {index}"),
            document: item,
            required: vec!["creationTime"],
            optional: vec![],
        });
    }

    let (status, cache) = call(
        router,
        "POST",
        "/runCache",
        Some(&json!({
            "requestId": "instants-cache",
            "cacheS3Location": "s3://omics-emulator-cache/instants/",
        })),
    )
    .await;
    let cache_id = created_id("run cache", status, &cache);
    let (status, cache) = call(router, "GET", &format!("/runCache/{cache_id}"), None).await;
    assert_eq!(status, 200, "GetRunCache must answer 200, got {cache:?}");
    rows.push(Reported {
        case: "GetRunCache".to_string(),
        document: cache,
        required: vec!["creationTime"],
        optional: vec![],
    });

    // A run, read back and listed: its `startTime` and `stopTime` are instants the clock
    // owes, so they are date-time strings whenever they are there at all.
    let (status, run) = start_run(
        router,
        &start_run_body(&workflow_id, "instants-run", &json!({})),
    )
    .await;
    let run_id = created_id("run", status, &run);
    let (status, run) = call(router, "GET", &format!("/run/{run_id}"), None).await;
    assert_eq!(status, 200, "GetRun must answer 200, got {run:?}");
    rows.push(Reported {
        case: "GetRun".to_string(),
        document: run,
        required: vec!["creationTime"],
        optional: vec!["startTime", "stopTime"],
    });
    let (status, listed) = call(router, "GET", "/run", None).await;
    for (index, item) in items("ListRuns", status, &listed).into_iter().enumerate() {
        rows.push(Reported {
            case: format!("ListRuns item {index}"),
            document: item,
            required: vec!["creationTime"],
            optional: vec!["startTime", "stopTime"],
        });
    }

    rows
}

/// RULE 1 on the wire: every instant every implemented path of this service reports is an
/// RFC 3339 date-time string naming the moment the service wrote.
#[tokio::test]
async fn every_reported_instant_is_a_date_time_string() {
    let (router, _engine) = emulator_holding_its_engine();
    let before = now_seconds();
    let rows = reported_documents(&router).await;
    let after = now_seconds();

    assert!(
        rows.len() >= 12,
        "the table must span every implemented path that reports an instant, got {} rows",
        rows.len()
    );

    for row in &rows {
        for member in &row.required {
            let seconds = assert_timestamp(&row.case, member, &row.document);
            assert!(
                seconds >= before - INSTANT_SLACK_SECONDS
                    && seconds <= after + INSTANT_SLACK_SECONDS,
                "case {:?}: expected {member} within {INSTANT_SLACK_SECONDS}s of this run \
                 ({before}..{after}), got {seconds}",
                row.case
            );
        }
        for member in &row.optional {
            if row.document.get(*member).is_some() {
                assert_timestamp(&row.case, member, &row.document);
            }
        }
    }
}

/// RULE 1 on the instant itself: the date-time string an upload reports names exactly the
/// instant the service stored for it, sub-second digits included.
///
/// Read against the record in the service's own state, so a well-formed string naming
/// another moment — or one rounded to the second — fails too.
#[tokio::test]
async fn a_reported_instant_names_the_stored_one_to_the_millisecond() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the stored instant").await;

    // One row per source format, so the rule is asserted over the class rather than over
    // one upload, and each row is its own record with its own instant.
    for file_type in FILE_TYPES {
        let (status, body) = create_upload(
            &router,
            &store,
            &json!({
                "name": format!("instant of {file_type}"),
                "subjectId": "subject",
                "sampleId": "sample",
                "sourceFileType": file_type,
            }),
        )
        .await;
        let upload_id = created_upload_id(file_type, status, &body);
        let stored = stored_upload(&engine, &store, &upload_id);
        let reported = assert_timestamp(file_type, "creationTime", &body);
        let reported_millis = (reported * 1_000.0).round() as i64;
        assert_eq!(
            reported_millis, stored.creation_time_ms,
            "case {file_type:?}: expected the stored instant {} ms, got {reported_millis} ms \
             from {body:?}",
            stored.creation_time_ms
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a reference is reported only when the upload named one
// ---------------------------------------------------------------------------

/// A row of the reference table: the `sourceFileType` the upload carries and the
/// `referenceArn` it names, if any.
struct ReferenceCase {
    case: &'static str,
    source_file_type: &'static str,
    reference_arn: Option<String>,
}

/// The table. The rule generalises over the source format (every value the modelled
/// `FileType` admits, aligned and unaligned alike, since the request's `referenceArn` is
/// optional for all of them) and over the ARN's own text across the published
/// `ReferenceArn` length 1..127.
fn reference_cases() -> Vec<ReferenceCase> {
    let mut cases = vec![
        ReferenceCase {
            case: "an aligned upload naming a reference",
            source_file_type: "BAM",
            reference_arn: Some(REFERENCE_ARN.to_string()),
        },
        ReferenceCase {
            case: "an aligned upload naming none",
            source_file_type: "CRAM",
            reference_arn: None,
        },
        ReferenceCase {
            case: "an unaligned upload naming a reference",
            source_file_type: "FASTQ",
            reference_arn: Some(REFERENCE_ARN.to_string()),
        },
        ReferenceCase {
            case: "an unaligned upload naming none",
            source_file_type: "UBAM",
            reference_arn: None,
        },
        ReferenceCase {
            case: "the shortest ARN the pattern admits",
            source_file_type: "BAM",
            // `^arn:.+$` with one character after the prefix: the floor of the pattern.
            reference_arn: Some("arn:x".to_string()),
        },
        ReferenceCase {
            case: "an ARN at the published length bound",
            source_file_type: "CRAM",
            reference_arn: Some(format!("arn:{}", "r".repeat(REFERENCE_ARN_MAX - 4))),
        },
    ];
    // And one row per source format carrying no reference, so no value of the catalog is
    // left reporting an empty string.
    for file_type in FILE_TYPES {
        cases.push(ReferenceCase {
            case: file_type,
            source_file_type: file_type,
            reference_arn: None,
        });
    }
    cases
}

/// Assert the body reports `referenceArn` exactly when `expected` is `Some`, and that an
/// absent reference is an absent MEMBER rather than an empty or null value.
fn assert_reference(case: &str, expected: Option<&str>, body: &Value) {
    match expected {
        Some(arn) => assert_eq!(
            body.get("referenceArn").and_then(Value::as_str),
            Some(arn),
            "case {case:?}: expected referenceArn {arn:?}, got {:?} in {body:?}",
            body.get("referenceArn")
        ),
        None => assert!(
            body.get("referenceArn").is_none(),
            "case {case:?}: an upload that named no reference must carry no referenceArn \
             member at all, got {:?} in {body:?}",
            body.get("referenceArn")
        ),
    }
}

/// RULE 2: `referenceArn` is the ARN the request named, or nothing at all.
#[tokio::test]
async fn a_reference_is_reported_only_when_the_upload_named_one() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the reference destination").await;

    for row in reference_cases() {
        let mut request = json!({
            "name": format!("reference case {}", row.case),
            "clientToken": format!("reference-{}-{}", row.case, row.source_file_type),
            "subjectId": "subject",
            "sampleId": "sample",
            "sourceFileType": row.source_file_type,
        });
        if let Some(arn) = &row.reference_arn {
            request["referenceArn"] = json!(arn);
        }

        let (status, body) = create_upload(&router, &store, &request).await;
        let upload_id = created_upload_id(row.case, status, &body);
        assert_reference(row.case, row.reference_arn.as_deref(), &body);

        // The reported member is the stored record's, not the request's: the upload really
        // holds the reference it reports, and holds none when it reports none.
        let stored = stored_upload(&engine, &store, &upload_id);
        assert_eq!(
            stored.reference_arn, row.reference_arn,
            "case {:?}: expected the stored referenceArn {:?}, got {:?}",
            row.case, row.reference_arn, stored.reference_arn
        );

        // A replayed idempotency token answers with that same record, so the member is
        // absent there too rather than appearing as a default on the second answer.
        let (status, replay) = create_upload(&router, &store, &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: a replay must answer 200, got {replay:?}",
            row.case
        );
        assert_reference(row.case, row.reference_arn.as_deref(), &replay);
    }
}
