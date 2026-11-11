// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue blueprint DELETE slice: `DeleteBlueprint` and the
//! `GetBlueprint`, `GetBlueprintRun` and `ListBlueprints` reads that observe what it
//! removed.
//!
//! RULE: a delete addressed by a blueprint name removes the WHOLE record this account
//! holds under that name — the registration and the runs started of it — and answers with
//! the name it was addressed by; after it the name reads as one nothing is registered
//! under and is free to register again, every other blueprint is left as it was, and the
//! workflows those runs created outlive the blueprint. Because the operation models no
//! entity-miss failure, a name holding no blueprint is answered the same way instead of
//! being refused, and neither the status of the registration nor a run of it in flight is
//! a reason to refuse.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: which optional members the
//! registration carried, the form and the length of the name, which status the
//! registration is in when the delete lands, how many runs of it the store holds, and how
//! a requested name misses.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, BLUEPRINT_SETTLES_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::blueprint_runs::{emulator_with_engine, rewind, start, stored_runs};
use crate::wire::{call, error_code, error_message, ok};

/// The longest `Name` a `DeleteBlueprint` may carry: `NameString`, "len 1..255"
/// (`smithy_query operation=DeleteBlueprint`). Longer than the longest name a create may
/// register (`OrchestrationNameString`, "len 1..128",
/// `smithy_query operation=CreateBlueprint`), so a name of this length can only ever be a
/// miss.
const MAX_DELETE_NAME_LENGTH: usize = 255;

/// The longest `Name` a `CreateBlueprint` may register: `OrchestrationNameString`,
/// "len 1..128" (`smithy_query operation=CreateBlueprint`).
const MAX_CREATE_NAME_LENGTH: usize = 128;

/// The `BlueprintStatus` a registration is in until the clock owes it the next one: the
/// modelled value "Creating — The blueprint registration is in progress"
/// (`smithy_query shape=Blueprint`, member `Status`).
const STATUS_CREATING: &str = "CREATING";

/// The role every run in this suite is started under, as `OrchestrationIAMRoleArn`'s pattern
/// `^arn:aws[^:]*:iam::[0-9]*:role/.+$` requires. Shared, so one spelling serves every suite
/// that starts a blueprint run.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GlueBlueprintRole";

/// Delete the blueprint `name` and return the name the answer reports, if any.
pub(crate) async fn delete(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let deleted = ok(router, "DeleteBlueprint", &json!({ "Name": name })).await;
    deleted
        .get("Name")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every blueprint name `ListBlueprints` reports.
async fn listed(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListBlueprints", &json!({})).await;
    page["Blueprints"]
        .as_array()
        .expect("Blueprints is a list")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Whether the store itself holds a blueprint under `name`.
fn stored(engine: &Arc<engine::InMemoryEngine>, name: &str) -> bool {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store.blueprint(name).is_some()
}

/// Assert that `GetBlueprint` reports `name` as a blueprint this account does not hold.
async fn assert_missing(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "GetBlueprint", &json!({ "Name": name })).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted {name:?}, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        messages::BLUEPRINT_NOT_FOUND,
        "case {case}: the refusal is the recorded sentence for a blueprint miss, got {body}"
    );
}

/// One row of the delete table: a registration to write, how far the clock has carried it,
/// and how many runs of it the store holds when the delete lands.
struct Row {
    case: &'static str,
    request: Value,
    /// Whether the registration is carried past the span the clock owed it
    /// ([`BLUEPRINT_SETTLES_AFTER`]) before the delete, i.e. is ACTIVE rather than
    /// CREATING. A run can only be started of a finished registration.
    settled: bool,
    /// How many runs of the blueprint to start before the delete.
    runs: usize,
}

// ---------------------------------------------------------------------------
// Delete, then observe what went
// ---------------------------------------------------------------------------

/// A delete removes the whole record stored under the name it addresses and answers with
/// that name: afterwards the blueprint, and every run of it, read as missing, the name is
/// no longer listed and is free to register again, while the workflows its runs created and
/// every other blueprint stay exactly as they were.
#[tokio::test]
async fn deleting_a_blueprint_removes_the_record_stored_under_its_name() {
    let long_name = "n".repeat(MAX_CREATE_NAME_LENGTH);

    let rows = vec![
        Row {
            // The shape a lifecycle client sends: the two required members alone, deleted
            // while the registration is still in flight.
            case: "a registration still in flight",
            request: json!({
                "Name": "in-flight",
                "BlueprintLocation": "s3://blueprint-bucket/in-flight.zip",
            }),
            settled: false,
            runs: 0,
        },
        Row {
            case: "a finished registration with no runs",
            request: json!({
                "Name": "idle",
                "BlueprintLocation": "s3://blueprint-bucket/idle.zip",
                "Description": "Partitioned ingest of one table.",
            }),
            settled: true,
            runs: 0,
        },
        Row {
            case: "a registration carrying tags",
            request: json!({
                "Name": "tagged",
                "BlueprintLocation": "s3://blueprint-bucket/tagged.zip",
                "Tags": { "team": "data", "env": "prod" },
            }),
            settled: true,
            runs: 0,
        },
        Row {
            case: "one run of the blueprint",
            request: json!({
                "Name": "one-run",
                "BlueprintLocation": "s3://blueprint-bucket/one-run.zip",
            }),
            settled: true,
            runs: 1,
        },
        Row {
            // More than one run, so what goes with the record is the whole run history and
            // not just its latest entry.
            case: "several runs of the blueprint",
            request: json!({
                "Name": "Mixed.Case-blueprint_42",
                "BlueprintLocation": "s3://blueprint-bucket/mixed.zip",
                "Description": "Mixed-case name.",
            }),
            settled: true,
            runs: 3,
        },
        Row {
            case: "a name at the longest a create may register",
            request: json!({
                "Name": long_name,
                "BlueprintLocation": "s3://blueprint-bucket/long-name.zip",
            }),
            settled: true,
            runs: 1,
        },
    ];

    for row in &rows {
        let (router, engine) = emulator_with_engine();
        let case = row.case;
        let name = row.request["Name"].as_str().expect("the row carries a Name");

        // A second blueprint nothing in this row addresses: the delete must leave it alone.
        ok(
            &router,
            "CreateBlueprint",
            &json!({
                "Name": "bystander",
                "BlueprintLocation": "s3://blueprint-bucket/bystander.zip",
            }),
        )
        .await;

        ok(&router, "CreateBlueprint", &row.request).await;
        if row.settled {
            rewind(&engine, name, BLUEPRINT_SETTLES_AFTER);
        }

        // The runs — each laying out its own workflow, so one run never clashes with
        // another's.
        let mut run_ids = Vec::new();
        let mut workflows = Vec::new();
        for index in 0..row.runs {
            let workflow = format!("wf-{index}");
            run_ids.push(
                start(
                    &router,
                    &json!({
                        "BlueprintName": name,
                        "RoleArn": ROLE_ARN,
                        "Parameters": json!({ "WorkflowName": &workflow }).to_string(),
                    }),
                )
                .await,
            );
            workflows.push(workflow);
        }
        assert_eq!(
            stored_runs(&engine, name),
            row.runs,
            "case {case}: the store must hold the runs that were started"
        );

        // "Returns the name of the blueprint that was deleted."
        assert_eq!(
            delete(&router, name).await.as_deref(),
            Some(name),
            "case {case}: DeleteBlueprint must report the name it deleted"
        );

        // The record is gone from the store, so every read of it is a miss.
        assert!(
            !stored(&engine, name),
            "case {case}: the store must hold no blueprint under {name:?} after the delete"
        );
        assert_missing(&router, case, name).await;
        for run_id in &run_ids {
            let (status, body) = call(
                &router,
                "GetBlueprintRun",
                &json!({ "BlueprintName": name, "RunId": run_id }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: a run of a deleted blueprint must not be readable, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                messages::BLUEPRINT_NOT_FOUND,
                "case {case}: the run read misses on the blueprint itself, got {body}"
            );
        }

        // The listing reports the blueprints the account still holds, and only those.
        let names = listed(&router).await;
        assert_eq!(
            names,
            vec!["bystander".to_string()],
            "case {case}: expected only the untouched blueprint to be listed, got {names:?}"
        );

        // A workflow outlives the run that created it, so a blueprint delete leaves the
        // workflows its runs laid out where they are.
        for workflow in &workflows {
            let read = ok(&router, "GetWorkflow", &json!({ "Name": workflow })).await;
            assert_eq!(
                read["Workflow"]["Name"].as_str(),
                Some(workflow.as_str()),
                "case {case}: the workflow {workflow:?} the run created must survive, got \
                 {read}"
            );
        }

        // The name the delete freed can be registered again, and what it then holds is a
        // FRESH registration: in flight once more, with none of the deleted record's runs.
        ok(&router, "CreateBlueprint", &row.request).await;
        let reread = ok(&router, "GetBlueprint", &json!({ "Name": name })).await;
        assert_eq!(
            reread["Blueprint"]["Status"].as_str(),
            Some(STATUS_CREATING),
            "case {case}: a re-registered name holds a fresh registration, got {reread}"
        );
        assert_eq!(
            stored_runs(&engine, name),
            0,
            "case {case}: the runs of the deleted blueprint must not survive it"
        );

        // The blueprint the row never addressed still reads back as its own create wrote it.
        let bystander = ok(&router, "GetBlueprint", &json!({ "Name": "bystander" })).await;
        assert_eq!(
            bystander["Blueprint"]["BlueprintLocation"].as_str(),
            Some("s3://blueprint-bucket/bystander.zip"),
            "case {case}: the delete must leave every other blueprint alone, got {bystander}"
        );
    }
}

// ---------------------------------------------------------------------------
// A name holding no blueprint
// ---------------------------------------------------------------------------

/// `DeleteBlueprint` models no entity-miss failure, so a name this account holds no
/// blueprint under is not refused: the call is served, reports the name it was addressed
/// by, and leaves the store exactly as it found it — which makes the delete idempotent and
/// makes a differently-cased or partial name a miss rather than a match.
#[tokio::test]
async fn deleting_a_name_that_holds_no_blueprint_is_served_and_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    ok(
        &router,
        "CreateBlueprint",
        &json!({
            "Name": "StoredBlueprint",
            "BlueprintLocation": "s3://blueprint-bucket/stored.zip",
        }),
    )
    .await;
    // A blueprint that IS deleted, so the second delete of it is a miss too.
    ok(
        &router,
        "CreateBlueprint",
        &json!({
            "Name": "already-deleted",
            "BlueprintLocation": "s3://blueprint-bucket/gone.zip",
        }),
    )
    .await;
    assert_eq!(
        delete(&router, "already-deleted").await.as_deref(),
        Some("already-deleted"),
        "the first delete of a stored blueprint reports its name"
    );

    let longest = "n".repeat(MAX_DELETE_NAME_LENGTH);
    let misses: Vec<(&str, &str)> = vec![
        ("never registered", "no-such-blueprint"),
        ("already deleted, so deleted twice", "already-deleted"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Blueprint"),
        ("a stored name lowercased", "storedblueprint"),
        ("a stored name uppercased", "STOREDBLUEPRINT"),
        ("a name carrying dots, dashes and underscores", "a.b-c_d"),
        ("the shortest name the model accepts", "x"),
        ("a name at the longest the model accepts", &longest),
    ];

    for (case, name) in &misses {
        // Served, not refused — and it reports the name it was addressed by, since the
        // answer's only member names the blueprint the delete was for.
        assert_eq!(
            delete(&router, name).await.as_deref(),
            Some(*name),
            "case {case}: a delete of {name:?} must be served and report that name"
        );
        assert!(
            !stored(&engine, name),
            "case {case}: {name:?} must still hold no blueprint"
        );
        // Nothing else went with it: the one stored blueprint is still readable and listed.
        let names = listed(&router).await;
        assert_eq!(
            names,
            vec!["StoredBlueprint".to_string()],
            "case {case}: a miss must leave the namespace as it was, got {names:?}"
        );
        let read = ok(&router, "GetBlueprint", &json!({ "Name": "StoredBlueprint" })).await;
        assert_eq!(
            read["Blueprint"]["BlueprintLocation"].as_str(),
            Some("s3://blueprint-bucket/stored.zip"),
            "case {case}: the stored blueprint must be untouched, got {read}"
        );
    }

    // The stored name itself deletes, so the misses above are misses and not a broken
    // lookup.
    assert_eq!(
        delete(&router, "StoredBlueprint").await.as_deref(),
        Some("StoredBlueprint"),
        "case the stored name: the delete reports the name it removed"
    );
    assert_missing(&router, "the stored name", "StoredBlueprint").await;
}
