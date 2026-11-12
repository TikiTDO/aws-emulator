// SPDX-License-Identifier: CC0-1.0

//! The ANNOTATION SET of an Automated Reasoning policy build workflow end-to-end:
//! `StartAutomatedReasoningPolicyBuildWorkflow` records the corrections a build carries and
//! `GetAutomatedReasoningPolicyAnnotations` reads them back, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **The set round-trips**: a read reports the policy the workflow builds, the name that
//!    policy was created with, the workflow's own id, the annotations the start carried —
//!    member for member, in the order they were sent — a hash one
//!    `AutomatedReasoningPolicyHash` admits and the instant the set was written. The rows
//!    span every form `sourceContent` takes (documents, the fidelity report's documents,
//!    the iterative refinement's documents, a policy definition only, and repair
//!    annotations), every member `AutomatedReasoningPolicyAnnotation` admits, and set sizes
//!    0, 1, 2, 7 and the modelled maximum of 10.
//! 2. **The hash is a function of the SET**: two workflows started with the same
//!    annotations report ONE hash, and any change to the set — a different annotation, a
//!    different value inside one, the same annotations in another order, one more
//!    annotation — reports a different one.
//! 3. **A read reports, it does not write**: two reads of one workflow, and a read taken
//!    after the build has moved on through its progression, report the same set, the same
//!    hash and the same instant.
//! 4. **The set is addressed by the policy AND the workflow**: an ARN naming no policy of
//!    this account, an id nothing started, and one policy's workflow id under another
//!    policy are each refused `ResourceNotFoundException` with this service's wording for
//!    the half that is missing.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="GetAutomatedReasoningPolicyAnnotations")` /
//! `query="shape"`). The requests go through `crate::support` — the ONE harness every suite
//! in this crate drives the emulator with.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::support::{
    age_build_workflow, emulator, label, wall_clock_seconds, Emulator, Reply, ACCOUNT, REGION,
};

/// A source document's bytes on the wire: `AutomatedReasoningPolicyBuildDocumentBlob` is a
/// `blob` (`@length(1, 5000000)`), which this protocol carries base64-encoded — this is
/// `policy rules`.
const DOCUMENT_BLOB: &str = "cG9saWN5IHJ1bGVz";

/// The `ResourceNotFoundException` message a request addressing an Automated Reasoning
/// policy this account holds none of is answered with — a FIXED text, naming neither the
/// ARN the request carried nor the account.
const POLICY_NOT_FOUND_MESSAGE: &str = "Policy does not exist";

/// The `ResourceNotFoundException` message a request addressed BY a `buildWorkflowId` this
/// policy holds no workflow under is answered with — fixed text like
/// [`POLICY_NOT_FOUND_MESSAGE`].
const WORKFLOW_NOT_FOUND_MESSAGE: &str = "Build WorkflowExecution not found";

/// A `buildWorkflowId` nothing has minted that is still one the member's pattern admits
/// (`^[a-f0-9]{8}-?[a-f0-9]{4}-?4[a-f0-9]{3}-?[89ab][a-f0-9]{3}-?[a-f0-9]{12}$`), so the
/// request is refused for the resource being absent rather than for the id being malformed.
const UNMINTED_WORKFLOW_ID: &str = "00000000-0000-4000-8000-000000000000";

/// How far a started workflow must be aged for the clock to have carried it past a stage of
/// its build, so a read taken later in the progression is a read of a workflow whose own
/// `updatedAt` has moved while its annotations have not.
const ONE_STAGE: Duration = Duration::from_secs(4);

/// Create a policy and hand back the `policyArn` it minted.
async fn policy(emu: &Emulator, name: &str) -> String {
    let created = emu
        .call(
            "POST",
            "/automated-reasoning-policies",
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    created.text("policyArn")
}

/// `StartAutomatedReasoningPolicyBuildWorkflow`: the `sourceContent` is the request's
/// `@httpPayload`, and the policy ARN and the workflow type are path labels.
async fn start(emu: &Emulator, policy_arn: &str, workflow_type: &str, source: Value) -> String {
    let path = format!(
        "/automated-reasoning-policies/{}/build-workflows/{workflow_type}/start",
        label(policy_arn)
    );
    let started = emu.call("POST", &path, Some(source)).await;
    assert_eq!(
        started.status, 200,
        "StartAutomatedReasoningPolicyBuildWorkflow must answer 200, got {} {:?}",
        started.status, started.body
    );
    started.text("buildWorkflowId")
}

/// `GetAutomatedReasoningPolicyAnnotations`: the policy ARN and the workflow id are path
/// labels and the request carries no body at all.
async fn get_annotations(emu: &Emulator, policy_arn: &str, build_workflow_id: &str) -> Reply {
    emu.call(
        "GET",
        &format!(
            "/automated-reasoning-policies/{}/build-workflows/{build_workflow_id}/annotations",
            label(policy_arn)
        ),
        None,
    )
    .await
}

/// One source document, as `AutomatedReasoningPolicyBuildWorkflowDocument` spells it.
fn document(name: &str, content_type: &str) -> Value {
    json!({
        "document": DOCUMENT_BLOB,
        "documentContentType": content_type,
        "documentName": name,
    })
}

/// `sourceContent` carrying repair annotations rather than a document
/// (`AutomatedReasoningPolicyBuildWorkflowRepairContent$annotations`).
fn repair_source(annotations: &[Value]) -> Value {
    json!({
        "workflowContent": { "policyRepairAssets": { "annotations": annotations } },
    })
}

/// Every member `AutomatedReasoningPolicyAnnotation` admits
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyAnnotation")`), each with
/// every member of its own target filled, so no row is keyed to one correction's shape.
///
/// The list is split into two rows below because
/// `AutomatedReasoningPolicyAnnotationList` is `@length(min: 0, max: 10)`, so the thirteen
/// members cannot travel in one request.
fn every_annotation() -> Vec<Value> {
    vec![
        json!({
            "addType": {
                "name": "Order status",
                "description": "where an order has got to",
                "values": [
                    { "value": "placed", "description": "the order is in" },
                    { "value": "shipped" },
                ],
            },
        }),
        json!({
            "updateType": {
                "name": "Order status",
                "newName": "Fulfilment status",
                "description": "where a fulfilment has got to",
                "values": [
                    { "addTypeValue": { "value": "packed", "description": "boxed, not sent" } },
                    { "updateTypeValue": { "value": "shipped", "newValue": "dispatched" } },
                    { "deleteTypeValue": { "value": "placed" } },
                ],
            },
        }),
        json!({ "deleteType": { "name": "Order status" } }),
        json!({
            "addVariable": {
                "name": "days_since_delivery",
                "type": "number",
                "description": "how long ago the order arrived",
            },
        }),
        json!({
            "updateVariable": {
                "name": "days_since_delivery",
                "newName": "days_since_receipt",
                "description": "how long ago the customer got it",
            },
        }),
        json!({ "deleteVariable": { "name": "days_since_receipt" } }),
        json!({ "addRule": { "expression": "days_since_receipt <= 30" } }),
        json!({
            "updateRule": { "ruleId": "R00000000001", "expression": "days_since_receipt <= 45" },
        }),
        json!({ "deleteRule": { "ruleId": "R00000000002" } }),
        json!({
            "addRuleFromNaturalLanguage": {
                "naturalLanguage": "returns close thirty days after delivery",
            },
        }),
        json!({
            "updateFromRulesFeedback": {
                "ruleIds": ["R00000000001", "R00000000002"],
                "feedback": "the window is measured from receipt, not from dispatch",
            },
        }),
        json!({
            "updateFromScenarioFeedback": {
                "ruleIds": ["R00000000001"],
                "scenarioExpression": "days_since_receipt = 31",
                "feedback": "this scenario must be refused",
            },
        }),
        json!({ "ingestContent": { "content": "Refunds follow the returns window." } }),
    ]
}

/// `count` `addRule` annotations, each with its own expression, so a row differs from
/// another in the SIZE of the set and in nothing else.
fn rule_annotations(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({ "addRule": { "expression": format!("rule_{index} = true") } }))
        .collect()
}

/// The rows rule 1 and rule 2 are asserted over: a case name, the `buildWorkflowType` the
/// start names, its `sourceContent`, and the annotation set a read of that workflow must
/// report.
fn rows() -> Vec<(&'static str, &'static str, Value, Vec<Value>)> {
    let every = every_annotation();
    vec![
        (
            "a document ingestion start carries no annotations",
            "INGEST_CONTENT",
            json!({ "workflowContent": { "documents": [document("returns.txt", "txt")] } }),
            Vec::new(),
        ),
        (
            "a fidelity report start carries no annotations",
            "GENERATE_FIDELITY_REPORT",
            json!({
                "workflowContent": {
                    "generateFidelityReportContent": {
                        "documents": [document("returns.pdf", "pdf")],
                    },
                },
            }),
            Vec::new(),
        ),
        (
            "an iterative refinement start carries no annotations",
            "ITERATIVELY_REFINE_POLICY",
            json!({
                "workflowContent": {
                    "iterativeRefinementContent": {
                        "documents": [document("returns.txt", "txt")],
                        "feedback": "tighten the window",
                    },
                },
            }),
            Vec::new(),
        ),
        (
            "a start carrying only the policy definition it begins from",
            "IMPORT_POLICY",
            json!({
                "policyDefinition": {
                    "version": "1",
                    "types": [],
                    "variables": [],
                    "rules": [],
                },
            }),
            Vec::new(),
        ),
        (
            "a repair carrying the empty set the list's minimum admits",
            "REFINE_POLICY",
            repair_source(&[]),
            Vec::new(),
        ),
        (
            "a repair carrying one annotation",
            "REFINE_POLICY",
            repair_source(&every[8..9]),
            every[8..9].to_vec(),
        ),
        (
            "a repair carrying two annotations",
            "REFINE_POLICY",
            repair_source(&every[8..10]),
            every[8..10].to_vec(),
        ),
        (
            "a repair carrying the first seven members of the union",
            "RESOLVE_POLICY_AMBIGUITIES",
            repair_source(&every[..7]),
            every[..7].to_vec(),
        ),
        (
            "a repair carrying the last six members of the union",
            "REFINE_POLICY",
            repair_source(&every[7..]),
            every[7..].to_vec(),
        ),
        (
            "a repair carrying the ten annotations the list's maximum admits",
            "GENERATE_POLICY_SCENARIOS",
            repair_source(&rule_annotations(10)),
            rule_annotations(10),
        ),
    ]
}

/// Whether `hash` is one `AutomatedReasoningPolicyHash` admits: 128 digits of `[0-9a-z]`
/// (`smithy_query(query="shape",shape="AutomatedReasoningPolicyHash")`).
fn is_policy_hash(hash: &str) -> bool {
    hash.len() == 128
        && hash
            .chars()
            .all(|digit| digit.is_ascii_digit() || digit.is_ascii_lowercase())
}

#[tokio::test]
async fn a_started_build_workflows_annotation_set_reads_back() {
    let emu = emulator();
    for (index, (case, workflow_type, source, expected)) in rows().into_iter().enumerate() {
        // One policy per row: a policy a workflow is still building admits no second
        // start, so each row's workflow is a workflow of its own policy.
        let name = format!("annotated policy {index}");
        let policy_arn = policy(&emu, &name).await;

        let before = wall_clock_seconds();
        let id = start(&emu, &policy_arn, workflow_type, source).await;
        let after = wall_clock_seconds();

        let read = get_annotations(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("policyArn"),
            policy_arn,
            "case {case:?}: the read must report the policy it was addressed with, got {:?}",
            read.get("policyArn")
        );
        assert_eq!(
            read.text("name"),
            name,
            "case {case:?}: the read must report the name the policy was created with, got {:?}",
            read.get("name")
        );
        assert_eq!(
            read.text("buildWorkflowId"),
            id,
            "case {case:?}: the read must report the workflow the start minted, got {:?}",
            read.get("buildWorkflowId")
        );
        assert_eq!(
            read.get("annotations"),
            Value::Array(expected.clone()),
            "case {case:?}: the read must report the annotations the start carried, got {:?}",
            read.get("annotations")
        );

        let hash = read.text("annotationSetHash");
        assert!(
            is_policy_hash(&hash),
            "case {case:?}: annotationSetHash must be 128 digits of [0-9a-z], got {hash:?}"
        );

        // `updatedAt` is modelled `@timestampFormat("date-time")`, so `instant` refuses
        // anything but the RFC 3339 string the wire requires.
        let updated_at = read.instant("updatedAt");
        assert!(
            updated_at >= before - 1.0 && updated_at <= after + 1.0,
            "case {case:?}: updatedAt must be the instant the set was written, between \
             {before} and {after}, got {updated_at}"
        );
    }
}

#[tokio::test]
async fn the_annotation_set_hash_follows_the_set() {
    let every = every_annotation();
    // Rows that differ ONLY in their annotation set, so what the hash answers can depend on
    // nothing else. The two rows sharing a set name it in the same words.
    let sets: Vec<(&str, Vec<Value>)> = vec![
        ("the empty set", Vec::new()),
        ("one rule deletion", every[8..9].to_vec()),
        ("one rule deletion, again", every[8..9].to_vec()),
        ("two annotations", every[8..10].to_vec()),
        (
            "the same two annotations in the other order",
            vec![every[9].clone(), every[8].clone()],
        ),
        (
            "one of those two annotations with a different value",
            vec![json!({ "deleteRule": { "ruleId": "R00000000003" } })],
        ),
        ("three annotations", every[7..10].to_vec()),
        ("ten annotations", rule_annotations(10)),
    ];

    let emu = emulator();
    // The hash each set was reported with, keyed by the set itself: a set seen twice must
    // answer with the hash it answered with the first time, and two different sets must
    // never answer with one hash.
    let mut by_set: BTreeMap<String, String> = BTreeMap::new();
    let mut by_hash: BTreeMap<String, String> = BTreeMap::new();
    for (index, (case, annotations)) in sets.into_iter().enumerate() {
        let policy_arn = policy(&emu, &format!("hashed policy {index}")).await;
        let id = start(
            &emu,
            &policy_arn,
            "REFINE_POLICY",
            repair_source(&annotations),
        )
        .await;
        let read = get_annotations(&emu, &policy_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read must answer 200, got {} {:?}",
            read.status, read.body
        );
        let hash = read.text("annotationSetHash");
        let set = Value::Array(annotations).to_string();

        if let Some(seen) = by_set.get(&set) {
            assert_eq!(
                &hash, seen,
                "case {case:?}: one annotation set must report one hash, got {hash:?} after \
                 {seen:?}"
            );
        }
        if let Some(other) = by_hash.get(&hash) {
            assert_eq!(
                other, &set,
                "case {case:?}: two different annotation sets must not report one hash \
                 ({hash:?} is also {other})"
            );
        }
        by_set.insert(set.clone(), hash.clone());
        by_hash.insert(hash, set);
    }
}

#[tokio::test]
async fn reading_an_annotation_set_changes_nothing() {
    let every = every_annotation();
    let rows: Vec<(&str, Vec<Value>)> = vec![
        ("the empty set", Vec::new()),
        ("a set of seven annotations", every[..7].to_vec()),
    ];

    let emu = emulator();
    for (index, (case, annotations)) in rows.into_iter().enumerate() {
        let policy_arn = policy(&emu, &format!("reread policy {index}")).await;
        let id = start(
            &emu,
            &policy_arn,
            "REFINE_POLICY",
            repair_source(&annotations),
        )
        .await;

        let first = get_annotations(&emu, &policy_arn, &id).await;
        let second = get_annotations(&emu, &policy_arn, &id).await;
        assert_eq!(
            second.body, first.body,
            "case {case:?}: two reads of one workflow must report one answer, got {:?} after \
             {:?}",
            second.body, first.body
        );

        // The workflow's own clock moves its status on while the annotations stay as they
        // were written, so the set a later read reports is still the set the start carried.
        age_build_workflow(&emu, &policy_arn, &id, ONE_STAGE);
        let later = get_annotations(&emu, &policy_arn, &id).await;
        assert_eq!(
            later.body, first.body,
            "case {case:?}: a read taken further into the build must report the same set, \
             got {:?} after {:?}",
            later.body, first.body
        );
    }
}

#[tokio::test]
async fn an_annotation_set_is_addressed_by_its_policy_and_its_workflow() {
    let emu = emulator();
    let policy_arn = policy(&emu, "addressed policy").await;
    let other_arn = policy(&emu, "other policy").await;
    let id = start(
        &emu,
        &policy_arn,
        "REFINE_POLICY",
        repair_source(&every_annotation()[8..9]),
    )
    .await;

    // Every row is a pair naming no stored annotation set, with the sentence the half that
    // is missing is reported in.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "an ARN naming no policy of this account",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/zzzzzzzzzzzz"
            ),
            id.clone(),
            POLICY_NOT_FOUND_MESSAGE,
        ),
        (
            "a version of a policy that has cut none",
            format!("{policy_arn}:1"),
            id.clone(),
            POLICY_NOT_FOUND_MESSAGE,
        ),
        (
            "an id nothing started",
            policy_arn.clone(),
            UNMINTED_WORKFLOW_ID.to_string(),
            WORKFLOW_NOT_FOUND_MESSAGE,
        ),
        (
            "one policy's workflow id under another policy",
            other_arn.clone(),
            id.clone(),
            WORKFLOW_NOT_FOUND_MESSAGE,
        ),
    ];

    for (case, arn, workflow_id, expected) in rows {
        let read = get_annotations(&emu, &arn, &workflow_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the read must answer 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: the read must be refused ResourceNotFoundException, got {:?}",
            read.error
        );
        assert_eq!(
            read.text("message"),
            expected,
            "case {case:?}: the refusal must carry this service's wording, got {:?}",
            read.get("message")
        );
    }

    // The pair that DOES name a set still answers it, so the rows above were refused for
    // the resource they addressed and not because the operation refuses everything.
    let read = get_annotations(&emu, &policy_arn, &id).await;
    assert_eq!(
        read.status, 200,
        "the stored pair must still answer 200, got {} {:?}",
        read.status, read.body
    );
}
