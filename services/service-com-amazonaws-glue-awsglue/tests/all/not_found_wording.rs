// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of the refusal a Glue operation answers when the
//! resource a request addressed does not exist.
//!
//! The rule under test: a refusal's `Message` is the sentence the service itself words
//! that error class with — its fixed text reproduced exactly, carrying ONLY the variable
//! parts that sentence names (and the account the emulator serves, where the sentence
//! names one) — so a client that matches on the documented text matches, and a refusal
//! whose sentence has no variable part never varies with the request. The status (400) and
//! the error code (`EntityNotFoundException`, "a specified entity does not exist" —
//! `smithy_query operation=GetBlueprint,GetTrigger,GetWorkflow,DeleteConnection,
//! GetStatement,GetWorkflowRunProperties,GetTableVersion,BatchDeleteTableVersion`) are the
//! same for every sentence below, which is exactly why the sentence is what distinguishes
//! them.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! requested name or id's value, length, casing and character set, which operation of the
//! family asks, and — where the sentence has a variable part — the value that part takes.
//! A sentence with no variable part is asserted to carry no part of the request at all, so
//! a template creeping back in fails here. Where ONE family words two misses two ways (the
//! version read's), both are rows of the same suite. The expected strings are written out
//! as literals (not read from the service's own helpers) so the test pins the wording
//! rather than restating it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, ERROR_DETAIL_TAG};

/// The error code every refusal in this suite carries.
const NOT_FOUND: &str = "EntityNotFoundException";

/// Names that span the axis these templates generalise over: a plain name, a single
/// character, a mixed-case name, one carrying the punctuation the strictest member here
/// admits, and one at that member's ceiling.
///
/// The spellings stay inside `OrchestrationNameString` ("len 1..128", pattern
/// `^[\.\-_A-Za-z0-9]+$` — `smithy_query operation=StartBlueprintRun`), which is the
/// narrowest of the members addressed below, so one axis serves every row and no row is
/// refused at the request boundary instead of by the handler.
fn absent_names() -> Vec<(String, String)> {
    vec![
        ("a name nothing created".to_string(), "no-such-entity".to_string()),
        ("a single character".to_string(), "x".to_string()),
        ("mixed case".to_string(), "MissingEntity".to_string()),
        (
            "punctuation and digits".to_string(),
            "team.data_lake-2026.raw".to_string(),
        ),
        ("at the 128 ceiling".to_string(), "n".repeat(128)),
    ]
}

/// Assert one refusal: HTTP 400, `EntityNotFoundException`, and `expected` verbatim.
async fn assert_miss(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    expected: &str,
) {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?} via {operation}: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with(NOT_FOUND),
        "case {case:?} via {operation}: expected {NOT_FOUND}, got {}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        expected,
        "case {case:?} via {operation}: expected Message {expected:?}, got {:?}",
        error_message(&body)
    );
}

/// Assert that `expected` is the whole refusal AND that it discloses nothing of the
/// request — the half of the rule a sentence with no variable part carries.
pub(crate) async fn assert_fixed_miss(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    addressed: &str,
    expected: &str,
) {
    assert_miss(router, case, operation, request, expected).await;
    assert!(
        !expected.contains(addressed),
        "case {case:?} via {operation}: a fixed sentence must not carry the requested \
         {addressed:?}, got {expected:?}"
    );
}

/// A `CreateConnection` body for `name`.
fn connection_body(name: &str) -> Value {
    json!({
        "ConnectionInput": {
            "Name": name,
            "ConnectionType": "JDBC",
            "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://host:3306/db" },
        },
    })
}

/// Register the job a workflow's entry-point trigger runs, so a trigger bound to a
/// workflow resolves and the workflow can be started.
async fn register_entry_point(router: &MultiServiceRouter, workflow: &str, trigger: &str) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": format!("{workflow}-job"),
            "Role": "arn:aws:iam::000000000000:role/GlueJobRole",
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    ok(router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    ok(
        router,
        "CreateTrigger",
        &json!({
            "Name": trigger,
            "Type": "ON_DEMAND",
            "WorkflowName": workflow,
            "Actions": [{ "JobName": format!("{workflow}-job") }],
        }),
    )
    .await;
}

/// Write a table carrying four versions, so a version id past the newest is a miss of a
/// table that exists.
async fn write_versioned_table(router: &MultiServiceRouter, database: &str, table: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": { "Name": table } }),
    )
    .await;
    for retention in 1..4 {
        ok(
            router,
            "UpdateTable",
            &json!({
                "DatabaseName": database,
                "TableInput": { "Name": table, "Retention": retention },
            }),
        )
        .await;
    }
}

/// Every blueprint operation that addresses one blueprint by name answers the service's
/// FIXED blueprint miss — the sentence states that the name resolved to no blueprint and
/// carries no part of the request.
#[tokio::test]
async fn the_blueprint_miss_is_the_services_fixed_sentence() {
    let router = emulator();
    // A blueprint that DOES exist, so every refusal below is about the name it asked for.
    ok(
        &router,
        "CreateBlueprint",
        &json!({
            "Name": "StoredBlueprint",
            "BlueprintLocation": "s3://blueprint-bucket/layout.zip",
        }),
    )
    .await;

    let expected = "Blueprint with given name not found.";
    let mut cases = absent_names();
    // A blueprint name is matched exactly as sent, so a name differing from a stored one
    // only in case is a miss too.
    cases.push((
        "differs only in case from a stored blueprint".to_string(),
        "storedblueprint".to_string(),
    ));

    for (case, name) in &cases {
        for (operation, request) in [
            ("GetBlueprint", json!({ "Name": name })),
            (
                "StartBlueprintRun",
                json!({
                    "BlueprintName": name,
                    "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                }),
            ),
            (
                "GetBlueprintRun",
                json!({ "BlueprintName": name, "RunId": "br_0000000000000000" }),
            ),
            ("GetBlueprintRuns", json!({ "BlueprintName": name })),
        ] {
            assert_fixed_miss(&router, case, operation, &request, name, expected).await;
        }
    }

    // The registration that exists is still served, so the refusals above are about the
    // requested name alone.
    let read = ok(
        &router,
        "GetBlueprint",
        &json!({ "Name": "StoredBlueprint" }),
    )
    .await;
    assert_eq!(
        read["Blueprint"]["Name"].as_str(),
        Some("StoredBlueprint"),
        "the stored registration must still be reported, got {read}"
    );
}

/// Every trigger operation that addresses one trigger by name answers the service's FIXED
/// trigger miss.
#[tokio::test]
async fn the_trigger_miss_is_the_services_fixed_sentence() {
    let router = emulator();
    register_entry_point(&router, "trigger-miss-workflow", "StoredTrigger").await;

    let expected = "Trigger not found.";
    let mut cases = absent_names();
    cases.push((
        "differs only in case from a stored trigger".to_string(),
        "storedtrigger".to_string(),
    ));

    for (case, name) in &cases {
        for operation in ["GetTrigger", "StartTrigger", "StopTrigger"] {
            assert_fixed_miss(
                &router,
                case,
                operation,
                &json!({ "Name": name }),
                name,
                expected,
            )
            .await;
        }
    }

    // The trigger that exists is still served.
    let read = ok(&router, "GetTrigger", &json!({ "Name": "StoredTrigger" })).await;
    assert_eq!(
        read["Trigger"]["Name"].as_str(),
        Some("StoredTrigger"),
        "the stored trigger must still be reported, got {read}"
    );
}

/// The workflow surface answers one GENERIC entity sentence for every miss of its own: a
/// name no workflow has, and a run id a workflow that EXISTS holds no run under.
#[tokio::test]
async fn the_workflow_surface_answers_the_generic_entity_miss() {
    let router = emulator();
    register_entry_point(&router, "StoredWorkflow", "StoredWorkflowTrigger").await;
    let started = ok(
        &router,
        "StartWorkflowRun",
        &json!({ "Name": "StoredWorkflow" }),
    )
    .await;
    let run_id = started["RunId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartWorkflowRun must answer a RunId, got {started}"))
        .to_string();

    let expected = "Entity not found";
    let mut cases = absent_names();
    cases.push((
        "differs only in case from a stored workflow".to_string(),
        "storedworkflow".to_string(),
    ));

    // The workflow half of the address resolves to nothing.
    for (case, name) in &cases {
        for (operation, request) in [
            ("GetWorkflow", json!({ "Name": name })),
            ("GetWorkflowRuns", json!({ "Name": name })),
            ("StartWorkflowRun", json!({ "Name": name })),
            (
                "GetWorkflowRun",
                json!({ "Name": name, "RunId": run_id.clone() }),
            ),
            (
                "GetWorkflowRunProperties",
                json!({ "Name": name, "RunId": run_id.clone() }),
            ),
            (
                "PutWorkflowRunProperties",
                json!({
                    "Name": name,
                    "RunId": run_id.clone(),
                    "RunProperties": { "stage": "load" },
                }),
            ),
        ] {
            assert_fixed_miss(&router, case, operation, &request, name, expected).await;
        }
    }

    // The RUN half resolves to nothing, on the workflow that exists: ids spanning a value
    // never minted, a prefix of the minted one, the minted one without its prefix, and the
    // minted one in another case.
    let run_cases = vec![
        ("a run id never minted".to_string(), "wr_0000000000000000".to_string()),
        ("a prefix of the minted id".to_string(), run_id[..8].to_string()),
        (
            "the minted id uppercased".to_string(),
            run_id.to_uppercase(),
        ),
        ("a single character".to_string(), "r".to_string()),
    ];
    for (case, candidate) in &run_cases {
        for (operation, request) in [
            (
                "GetWorkflowRun",
                json!({ "Name": "StoredWorkflow", "RunId": candidate }),
            ),
            (
                "GetWorkflowRunProperties",
                json!({ "Name": "StoredWorkflow", "RunId": candidate }),
            ),
            (
                "PutWorkflowRunProperties",
                json!({
                    "Name": "StoredWorkflow",
                    "RunId": candidate,
                    "RunProperties": { "stage": "load" },
                }),
            ),
        ] {
            assert_fixed_miss(&router, case, operation, &request, candidate, expected).await;
        }
    }

    // The run that WAS minted is still served, so the refusals above are about the id.
    let read = ok(
        &router,
        "GetWorkflowRun",
        &json!({ "Name": "StoredWorkflow", "RunId": run_id.clone() }),
    )
    .await;
    assert_eq!(
        read["Run"]["WorkflowRunId"].as_str(),
        Some(run_id.as_str()),
        "the started run must still be readable, got {read}"
    );
}

/// The DELETE of a connection nothing holds states the resource kind alone, where the READ
/// of the same miss names the connection and the account and the UPDATE names the
/// connection: three operations, three sentences, so a client can tell which one refused.
#[tokio::test]
async fn the_connection_delete_miss_states_the_resource_kind_alone() {
    let router = emulator();
    ok(&router, "CreateConnection", &connection_body("present-conn")).await;

    let expected = "Connection not found";
    let mut cases = absent_names();
    cases.push((
        "differs only in case from a stored connection".to_string(),
        "PRESENT-CONN".to_string(),
    ));

    for (case, name) in &cases {
        for named_catalog in [false, true] {
            let label = format!("{case} (CatalogId named: {named_catalog})");
            let mut request = json!({ "ConnectionName": name });
            if named_catalog {
                request["CatalogId"] = json!(ACCOUNT_ID);
            }
            assert_fixed_miss(
                &router,
                &label,
                "DeleteConnection",
                &request,
                name,
                expected,
            )
            .await;
        }
    }

    // The same miss through the read and the update is worded differently, each its own
    // way: the delete's sentence is not borrowed for either.
    let (_, read) = call(&router, "GetConnection", &json!({ "Name": "no-such-conn" })).await;
    let (_, updated) = call(
        &router,
        "UpdateConnection",
        &json!({
            "Name": "no-such-conn",
            "ConnectionInput": {
                "Name": "no-such-conn",
                "ConnectionType": "JDBC",
                "ConnectionProperties": { "JDBC_CONNECTION_URL": "jdbc:mysql://host:3306/db" },
            },
        }),
    )
    .await;
    assert_eq!(
        error_message(&read),
        format!("Connection with name no-such-conn in account {ACCOUNT_ID} is not found"),
        "the read keeps its own account-scoped sentence, got {read}"
    );
    assert_eq!(
        error_message(&updated),
        "Connection not found: no-such-conn",
        "the update keeps its own sentence, got {updated}"
    );

    // A second delete of a connection that DID exist is the same miss as a name that never
    // existed, so the delete is refused for absence and not for the spelling.
    ok(
        &router,
        "DeleteConnection",
        &json!({ "ConnectionName": "present-conn" }),
    )
    .await;
    assert_miss(
        &router,
        "a connection deleted twice",
        "DeleteConnection",
        &json!({ "ConnectionName": "present-conn" }),
        expected,
    )
    .await;
}

/// An operation that reaches a session's STATEMENTS through its `SessionId` names the
/// member that resolved to nothing, where the session READ names the session: the two
/// sentences differ, so one is never borrowed for the other.
#[tokio::test]
async fn the_statement_surface_names_the_session_id_that_resolved_to_nothing() {
    let router = emulator();

    let mut cases = absent_names();
    cases.push((
        "an id spelled like a minted session id".to_string(),
        "sim2real-glue-session-0000".to_string(),
    ));

    for (case, id) in &cases {
        let expected = format!("Session ID {id} not found");
        for (operation, request) in [
            ("GetStatement", json!({ "SessionId": id, "Id": 1 })),
            ("ListStatements", json!({ "SessionId": id })),
        ] {
            assert_miss(&router, case, operation, &request, &expected).await;
        }
        // The session read's own miss is a DIFFERENT sentence for the same absent id.
        assert_miss(
            &router,
            case,
            "GetSession",
            &json!({ "Id": id }),
            &format!("Session {id} not found."),
        )
        .await;
    }
}

/// A version the addressed table does not hold is worded by WHICH miss it is: an id the
/// table never minted is no version of it at all, an id it minted and whose definition the
/// history no longer holds is a version that has gone, and the BATCH delete answers 200 and
/// reports the failure per item with the `VersionId` that item sent.
#[tokio::test]
async fn the_table_version_miss_is_worded_per_operation() {
    let router = emulator();
    let database = "version_wording_db";
    let table = "events";
    write_versioned_table(&router, database, table).await;

    // Ids past the newest version the table has minted (its live definition is version 3):
    // the next counter, a far one, the same counter zero-padded, and a large one. "Each
    // version is incremented by 1" (`GetTableVersionRequest$VersionId`), so none of these
    // has ever stood for a definition of this table.
    let never_minted = ["4", "7", "0007", "12345"];
    for version_id in never_minted {
        for operation in ["GetTableVersion", "DeleteTableVersion"] {
            assert_fixed_miss(
                &router,
                version_id,
                operation,
                &json!({
                    "DatabaseName": database,
                    "TableName": table,
                    "VersionId": version_id,
                }),
                version_id,
                "Version not found.",
            )
            .await;
        }
    }

    // A version the table DID mint, deleted and then addressed again: the same two
    // operations word this miss the service's other way.
    ok(
        &router,
        "DeleteTableVersion",
        &json!({ "DatabaseName": database, "TableName": table, "VersionId": "1" }),
    )
    .await;
    for (case, version_id) in [("the deleted version", "1"), ("padded", "01")] {
        for operation in ["GetTableVersion", "DeleteTableVersion"] {
            assert_fixed_miss(
                &router,
                case,
                operation,
                &json!({
                    "DatabaseName": database,
                    "TableName": table,
                    "VersionId": version_id,
                }),
                version_id,
                "Table version not found.",
            )
            .await;
        }
    }

    // The batch reports the same failure inside `Errors`, one entry per item, each naming
    // the id that item sent in its own sentence. Version 0 is archived and goes; version 1
    // was deleted above and 2 is addressed twice, so the batch both deletes and reports.
    let batch = ok(
        &router,
        "BatchDeleteTableVersion",
        &json!({
            "DatabaseName": database,
            "TableName": table,
            "VersionIds": ["0", "1", "2", "2"],
        }),
    )
    .await;
    let errors = batch["Errors"]
        .as_array()
        .unwrap_or_else(|| panic!("the batch must report an Errors list, got {batch}"));
    let reported: Vec<(&str, &str, &str)> = errors
        .iter()
        .map(|entry| {
            (
                entry["VersionId"].as_str().unwrap_or("<no VersionId>"),
                entry["ErrorDetail"]["ErrorCode"]
                    .as_str()
                    .unwrap_or("<no ErrorCode>"),
                entry["ErrorDetail"]["ErrorMessage"]
                    .as_str()
                    .unwrap_or("<no ErrorMessage>"),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![
            ("1", NOT_FOUND, "Table version (1) not found"),
            ("2", NOT_FOUND, "Table version (2) not found"),
        ],
        "every item that missed must be reported under the id it sent, got {batch}"
    );
    for entry in errors {
        assert_eq!(
            entry["ErrorDetail"]["__type"].as_str(),
            Some(ERROR_DETAIL_TAG),
            "every entry carries the ErrorDetail type tag, got {entry}"
        );
    }

    // A version the table still holds is served, so the misses above are misses.
    let read = ok(
        &router,
        "GetTableVersion",
        &json!({ "DatabaseName": database, "TableName": table, "VersionId": "3" }),
    )
    .await;
    assert_eq!(
        read["TableVersion"]["VersionId"].as_str(),
        Some("3"),
        "the stored version must still be readable, got {read}"
    );
}

/// The catalog miss and the data quality ruleset miss keep the two shapes this rule
/// admits: a sentence with NO variable part, and one carrying the account searched plus
/// the name the request sent.
#[tokio::test]
async fn the_catalog_and_ruleset_misses_keep_their_own_shapes() {
    let router = emulator();

    // Catalog names admit lowercase letters, digits, underscores and hyphens
    // (`CATALOG_NAME_PATTERN`, from `Catalog$Name`), so the axis stays inside them.
    for (case, catalog_id) in [
        ("a catalog nothing created", "not-yet-created"),
        ("a single character", "c"),
        ("underscores and digits", "lake_catalog_2026"),
    ] {
        assert_fixed_miss(
            &router,
            case,
            "GetCatalog",
            &json!({ "CatalogId": catalog_id }),
            catalog_id,
            "Catalog not found.",
        )
        .await;
    }

    for (case, name) in &absent_names() {
        assert_miss(
            &router,
            case,
            "GetDataQualityRuleset",
            &json!({ "Name": name }),
            &format!("Cannot find Data Quality Ruleset in account {ACCOUNT_ID} with name {name}"),
        )
        .await;
    }
}
