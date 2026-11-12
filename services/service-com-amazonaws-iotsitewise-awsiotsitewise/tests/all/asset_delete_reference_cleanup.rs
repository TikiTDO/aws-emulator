// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `DeleteAsset` takes with the asset it removes,
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: removing a resource removes every record this
//! service holds only as a reference to it, so no later read reports a resource
//! the service no longer holds. An action and its execution each reference an
//! asset in two places — the resource the action was taken ON and the detailed
//! resource it RESOLVES TO — and the write that records them resolves both
//! references against the store first, so a record naming either cannot outlive
//! the asset: the read of one refuses it, the listings over them stop reporting
//! it, and the resolved-resource listing of every computation model stops naming
//! it. A record that names a DIFFERENT asset, or none, is left exactly as it
//! was.
//!
//! The wire harness (`call`, `router_with_engine`, `error_code`, the asset
//! creates) is `asset_lifecycle`'s, the action-side harness (`Target`,
//! `target_query`, `list_actions`, `seed_action`) is `actions`', the
//! computation-model create is `computation_model_execution_summary`'s, the
//! single-action read is `execute_action`'s and the message reader is
//! `hierarchies_etags_and_refusals`' — re-used rather than copied.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TargetResource;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    asset_not_found_message, not_found_message, service_state, ExecutionRecord,
    ACTION_EXECUTION_DELAY, ACTIVATION_DELAY, EXECUTION_STATES, EXECUTION_STATE_COMPLETED,
    RESOURCE_ACTION, RESOURCE_EXECUTION,
};

use crate::actions::{list_actions, seed_action, target_query, Target};
use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_catalog, router_with_engine,
};
use crate::computation_model_execution_summary::create_computation_model;
use crate::computation_model_lifecycle::id_of;
use crate::execute_action::describe_action;
use crate::hierarchies_etags_and_refusals::error_message;

/// How many assets the fixture creates, so a row can name one that is deleted
/// and one that is not.
const ASSETS: usize = 4;

/// How many computation models the fixture creates, so the cleanup is seen to be
/// scoped to the ASSET rather than to one model's records.
const MODELS: usize = 2;

/// The assets the suite deletes, by their index in the fixture: the first while
/// the runs that name it are still in flight, the second once they are over.
const DELETED_WHILE_RUNNING: usize = 0;
const DELETED_AFTER_THE_RUN: usize = 2;

/// Long enough that a transition the clock owes has certainly fallen due.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(250)
}

/// Long enough that an execution's run has certainly fallen due.
fn run_over() -> Duration {
    ACTION_EXECUTION_DELAY + Duration::from_millis(250)
}

/// Which kind of record a row of the registry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Action,
    Execution,
}

impl Kind {
    /// The resource kind this record's not-found refusal names.
    fn resource(self) -> &'static str {
        match self {
            Kind::Action => RESOURCE_ACTION,
            Kind::Execution => RESOURCE_EXECUTION,
        }
    }

    /// The members of this record a delete of an asset it does not name must
    /// leave alone: an action's whole read form, and of an execution everything
    /// its run does not derive from the clock (its state and its end time move
    /// as the run falls due, which `describe_execution_reads` is the suite for).
    fn stable_members(self) -> &'static [&'static str] {
        match self {
            Kind::Action => &[
                "actionId",
                "actionDefinitionId",
                "targetResource",
                "actionPayload",
                "executionTime",
                "resolveTo",
            ],
            Kind::Execution => &[
                "executionId",
                "actionType",
                "targetResource",
                "targetResourceVersion",
                "resolveTo",
                "executionStartTime",
            ],
        }
    }
}

/// One record the fixture leaves in the store, and the references it carries.
#[derive(Debug, Clone)]
struct Record {
    /// What this record is in the suite, named in every assertion.
    case: String,
    kind: Kind,
    /// The id the record is read back by.
    id: String,
    /// The computation model the action was taken on, when it was taken on one.
    model: Option<String>,
    /// The asset the action was taken on, when it was taken on one.
    target_asset: Option<String>,
    /// The asset the record resolves to, when it resolves to one.
    resolve_to: Option<String>,
}

impl Record {
    /// `true` when this record NAMES `asset_id` — in either of the two places a
    /// record of an action references an asset.
    fn names(&self, asset_id: &str) -> bool {
        self.target_asset.as_deref() == Some(asset_id)
            || self.resolve_to.as_deref() == Some(asset_id)
    }

    /// The key this record's snapshot is held under.
    fn key(&self) -> (Kind, String) {
        (self.kind, self.id.clone())
    }
}

/// One execute the fixture takes through the API.
#[derive(Debug, Clone, Copy)]
struct ExecuteRow {
    case: &'static str,
    /// The computation model the action is taken on, by index.
    model: usize,
    /// The asset the action resolves to, by index, when it resolves to one.
    resolve_to: Option<usize>,
}

/// The executes, spanning the dimensions the rule generalises over: which
/// computation model the action was taken on, whether it resolved to an asset,
/// whether that asset is one the suite deletes, whether two executes resolve to
/// the SAME asset, and whether two models resolve to the same one.
fn execute_table() -> Vec<ExecuteRow> {
    vec![
        ExecuteRow {
            case: "model 0, resolving to the asset deleted while running",
            model: 0,
            resolve_to: Some(DELETED_WHILE_RUNNING),
        },
        ExecuteRow {
            case: "model 0, a second execute resolving to that same asset",
            model: 0,
            resolve_to: Some(DELETED_WHILE_RUNNING),
        },
        ExecuteRow {
            case: "model 0, resolving to an asset the suite keeps",
            model: 0,
            resolve_to: Some(1),
        },
        ExecuteRow {
            case: "model 0, resolving to nothing at all",
            model: 0,
            resolve_to: None,
        },
        ExecuteRow {
            case: "model 0, resolving to the asset deleted after the run",
            model: 0,
            resolve_to: Some(DELETED_AFTER_THE_RUN),
        },
        ExecuteRow {
            case: "model 1, resolving to the asset deleted while running",
            model: 1,
            resolve_to: Some(DELETED_WHILE_RUNNING),
        },
        ExecuteRow {
            case: "model 1, resolving to an asset the suite keeps",
            model: 1,
            resolve_to: Some(3),
        },
        ExecuteRow {
            case: "model 1, resolving to nothing at all",
            model: 1,
            resolve_to: None,
        },
    ]
}

/// One action-and-execution pair the fixture records against an ASSET target.
#[derive(Debug, Clone, Copy)]
struct SeedRow {
    case: &'static str,
    /// The id the action is stored under.
    action_id: &'static str,
    /// The id the execution is stored under.
    execution_id: &'static str,
    /// The asset the action was taken on, by index.
    target_asset: usize,
}

/// The records taken on an ASSET: the other place an action references an asset.
///
/// Seeded directly in the shared store, as `seed_action` documents:
/// `ExecuteAction` resolves an `actionDefinitionId` against the actions the
/// target resource publishes, and this store materialises those only for a
/// computation model's computation type, so an action taken on an asset cannot
/// be recorded through the API at all.
fn seed_table() -> Vec<SeedRow> {
    vec![
        SeedRow {
            case: "an action taken on the asset deleted while running",
            action_id: "aaaaaaa1-2222-4000-8000-000000000001",
            execution_id: "aaaaaaa1-1111-4000-8000-000000000001",
            target_asset: DELETED_WHILE_RUNNING,
        },
        SeedRow {
            case: "an action taken on the asset deleted after the run",
            action_id: "aaaaaaa1-2222-4000-8000-000000000002",
            execution_id: "aaaaaaa1-1111-4000-8000-000000000002",
            target_asset: DELETED_AFTER_THE_RUN,
        },
        SeedRow {
            case: "an action taken on an asset the suite keeps",
            action_id: "aaaaaaa1-2222-4000-8000-000000000003",
            execution_id: "aaaaaaa1-1111-4000-8000-000000000003",
            target_asset: 1,
        },
    ]
}

/// The router, the resources the table names, and every record the fixture left
/// in the store.
struct Fixture {
    router: MultiServiceRouter,
    assets: Vec<String>,
    models: Vec<String>,
    records: Vec<Record>,
}

impl Fixture {
    /// The ids of the records of `kind` taken on `model` that survive the
    /// removal of `removed`, in ascending id order.
    fn surviving(&self, kind: Kind, model: &str, removed: &[String]) -> Vec<String> {
        let mut ids: Vec<String> = self
            .records
            .iter()
            .filter(|record| record.kind == kind && record.model.as_deref() == Some(model))
            .filter(|record| !removed.iter().any(|asset| record.names(asset)))
            .map(|record| record.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The DISTINCT assets the surviving records of `model` resolve to, in
    /// ascending id order — what that model's resolved-resource listing owes.
    fn surviving_resolutions(&self, model: &str, removed: &[String]) -> Vec<String> {
        let mut ids: Vec<String> = self
            .records
            .iter()
            .filter(|record| record.model.as_deref() == Some(model))
            .filter(|record| !removed.iter().any(|asset| record.names(asset)))
            .filter_map(|record| record.resolve_to.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

/// Build the fixture: the assets and computation models through the API, the
/// executes of the table through `ExecuteAction`, and the asset-targeted rows in
/// the shared store.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let asset_model = create_model(&router, "Boiler", property_catalog()).await;
    let mut assets = Vec::with_capacity(ASSETS);
    for at in 0..ASSETS {
        assets.push(create_asset(&router, &format!("Boiler-{at}"), &asset_model).await);
    }
    let mut models = Vec::with_capacity(MODELS);
    for at in 0..MODELS {
        models.push(create_computation_model(&router, &format!("anomaly-{at}")).await);
    }

    // An action is taken only on a settled computation model, so wait for the
    // transition each create owes before taking any.
    tokio::time::sleep(settled()).await;
    let mut definitions = Vec::with_capacity(MODELS);
    for model in &models {
        definitions.push(first_definition(&router, model).await);
    }

    let mut records: Vec<Record> = Vec::new();
    for row in execute_table() {
        let model = models[row.model].clone();
        let resolve_to = row.resolve_to.map(|at| assets[at].clone());
        let mut body = json!({
            "targetResource": { "computationModelId": model },
            "actionDefinitionId": definitions[row.model],
            "actionPayload": { "stringValue": "{\"run\":\"once\"}" },
        });
        if let Some(asset_id) = resolve_to.as_ref() {
            body["resolveTo"] = json!({ "assetId": asset_id });
        }
        let (status, taken) = call(&router, "POST", "/actions", Some(body)).await;
        assert_eq!(
            status, 202,
            "case {:?}: ExecuteAction expected 202, got {status} {taken}",
            row.case
        );
        let action_id = taken["actionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no actionId in {taken}", row.case))
            .to_string();
        records.push(Record {
            case: format!("the action of {}", row.case),
            kind: Kind::Action,
            id: action_id,
            model: Some(model),
            target_asset: None,
            resolve_to,
        });
    }

    // Each execute started exactly one run, and an execution carries no
    // `actionId`, so the runs are picked up from the listing of the model they
    // were taken on rather than minted here.
    for (at, model) in models.iter().enumerate() {
        let (status, listed) =
            list_executions(&router, &target_query(Target::ComputationModel, model)).await;
        assert_eq!(
            status, 200,
            "model {at}: ListExecutions expected 200, got {status} {listed}"
        );
        let summaries = listed["executionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("model {at}: no executionSummaries in {listed}"));
        let owed = execute_table().iter().filter(|row| row.model == at).count();
        assert_eq!(
            summaries.len(),
            owed,
            "model {at}: each execute starts one execution, so {owed} were owed, got {listed}"
        );
        for summary in summaries {
            let execution_id = summary["executionId"]
                .as_str()
                .unwrap_or_else(|| panic!("model {at}: no executionId in {summary}"))
                .to_string();
            let resolve_to = summary["resolveTo"]["assetId"].as_str().map(str::to_string);
            records.push(Record {
                case: format!(
                    "the execution of model {at} resolving to {}",
                    resolve_to.as_deref().unwrap_or("nothing")
                ),
                kind: Kind::Execution,
                id: execution_id,
                model: Some(model.clone()),
                target_asset: None,
                resolve_to,
            });
        }
    }

    for row in seed_table() {
        let target = assets[row.target_asset].clone();
        seed_action(
            &engine,
            row.action_id,
            &id_of('4'),
            Target::AssetA,
            &target,
            "{}",
            None,
            SystemTime::now(),
        );
        seed_execution(&engine, row.execution_id, &target);
        for (kind, id) in [
            (Kind::Action, row.action_id),
            (Kind::Execution, row.execution_id),
        ] {
            records.push(Record {
                case: format!("{kind:?} of {}", row.case),
                kind,
                id: id.to_string(),
                model: None,
                target_asset: Some(target.clone()),
                resolve_to: None,
            });
        }
    }

    Fixture {
        router,
        assets,
        models,
        records,
    }
}

/// Record ONE execution against an ASSET target in the shared store, as
/// `ExecuteAction` records the run of an action — the run of the action
/// `seed_action` records beside it, which the API cannot take for an asset.
fn seed_execution(engine: &Arc<engine::InMemoryEngine>, execution_id: &str, target_asset: &str) {
    let state = service_state(&**engine).expect("the iotsitewise store");
    let mut store = state.lock();
    let started = SystemTime::now();
    store.executions.insert(
        execution_id.to_string(),
        ExecutionRecord {
            execution_id: execution_id.to_string(),
            action_type: Some("Amazon Web Services/ANOMALY_DETECTION_TRAINING".to_string()),
            target_resource: TargetResource {
                asset_id: Some(target_asset.to_string()),
                computation_model_id: None,
            },
            target_resource_version: "1".to_string(),
            resolve_to: None,
            execution_start_time: started,
            settles_at: started,
            outcome: EXECUTION_STATE_COMPLETED.to_string(),
            execution_entity_version: None,
        },
    );
}

/// The `actionDefinitionId` of the first action `computation_model_id`
/// publishes, read off `DescribeComputationModel` rather than written out here.
async fn first_definition(router: &MultiServiceRouter, computation_model_id: &str) -> String {
    let (status, described) = call(
        router,
        "GET",
        &format!("/computation-models/{computation_model_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "describe {computation_model_id}: got {status} {described}"
    );
    described["actionDefinitions"]
        .as_array()
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition["actionDefinitionId"].as_str())
        .unwrap_or_else(|| panic!("no actionDefinitions in {described}"))
        .to_string()
}

/// `GET /executions?...` — the runs recorded against one target resource.
async fn list_executions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/executions?{query}"), None).await
}

/// `GET /computation-models/{id}/resolve-to-resources`.
async fn list_resolutions(router: &MultiServiceRouter, computation_model_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/computation-models/{computation_model_id}/resolve-to-resources"),
        None,
    )
    .await
}

/// `GET /computation-models/{id}/execution-summary`.
async fn execution_summary(router: &MultiServiceRouter, computation_model_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/computation-models/{computation_model_id}/execution-summary"),
        None,
    )
    .await
}

/// `DELETE /assets/{assetId}`.
async fn delete_asset(router: &MultiServiceRouter, asset_id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/assets/{asset_id}"), None).await
}

/// Read ONE record back through the operation that addresses it.
async fn read(router: &MultiServiceRouter, record: &Record) -> (u16, Value) {
    match record.kind {
        Kind::Action => describe_action(router, &record.id).await,
        Kind::Execution => {
            call(router, "GET", &format!("/executions/{}", record.id), None).await
        }
    }
}

/// The ids a listing body reports, in ascending order.
fn listed_ids(case: &str, body: &Value, list: &str, member: &str) -> Vec<String> {
    let mut ids: Vec<String> = body[list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {list} list, got {body}"))
        .iter()
        .map(|summary| {
            summary[member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no {member} in {summary}"))
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The resolved asset ids one resolved-resource listing reports.
fn reported_resolutions(case: &str, body: &Value) -> Vec<String> {
    body["computationModelResolveToResourceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {body}"))
        .iter()
        .map(|summary| {
            summary["resolveTo"]["assetId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no resolved resource in {summary}"))
                .to_string()
        })
        .collect()
}

/// How many executions one execution summary counts, over every state the model
/// declares.
fn summarised_total(case: &str, body: &Value) -> u64 {
    let counts = body["computationModelExecutionSummary"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary map, got {body}"));
    EXECUTION_STATES
        .iter()
        .map(|state| {
            counts
                .get(*state)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no {state} count in {body}"))
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("case {case:?}: a non-decimal count in {body}"))
        })
        .sum()
}

/// Every record of the fixture read back, keyed as its snapshot.
async fn snapshot(fixture: &Fixture) -> BTreeMap<(Kind, String), Value> {
    let mut snapshots = BTreeMap::new();
    for record in &fixture.records {
        let (status, body) = read(&fixture.router, record).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture's record must read back, got {status} {body}",
            record.case
        );
        snapshots.insert(record.key(), body);
    }
    snapshots
}

/// Assert the whole store follows the rule once `removed` have been deleted:
/// every record that names one of them is gone, every other record is untouched,
/// and every listing over them reports exactly the survivors.
async fn assert_store_follows(
    fixture: &Fixture,
    snapshots: &BTreeMap<(Kind, String), Value>,
    removed: &[String],
    stage: &str,
) {
    for record in &fixture.records {
        let case = format!("{stage}: {}", record.case);
        let (status, body) = read(&fixture.router, record).await;
        if removed.iter().any(|asset| record.names(asset)) {
            assert_eq!(
                status, 404,
                "case {case:?}: a record naming a removed asset must be gone, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            let expected = not_found_message(record.kind.resource(), &record.id);
            assert_eq!(
                error_message(&body),
                expected.as_str(),
                "case {case:?}: expected {expected:?}, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a record naming no removed asset must still read back, got \
                 {status} {body}"
            );
            let before = snapshots
                .get(&record.key())
                .unwrap_or_else(|| panic!("case {case:?}: the fixture took no snapshot of it"));
            for member in record.kind.stable_members() {
                assert_eq!(
                    body[member], before[member],
                    "case {case:?}: {member} must be untouched by the delete, expected {}, got {}",
                    before[member], body[member]
                );
            }
        }
    }

    for (at, model) in fixture.models.iter().enumerate() {
        let case = format!("{stage}: model {at}");
        let query = target_query(Target::ComputationModel, model);

        let (status, listed) = list_actions(&fixture.router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListActions expected 200, got {status} {listed}"
        );
        let expected = fixture.surviving(Kind::Action, model, removed);
        let got = listed_ids(&case, &listed, "actionSummaries", "actionId");
        assert_eq!(
            got, expected,
            "case {case:?}: expected the surviving actions {expected:?}, got {got:?}"
        );

        let (status, listed) = list_executions(&fixture.router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListExecutions expected 200, got {status} {listed}"
        );
        let expected = fixture.surviving(Kind::Execution, model, removed);
        let got = listed_ids(&case, &listed, "executionSummaries", "executionId");
        assert_eq!(
            got, expected,
            "case {case:?}: expected the surviving executions {expected:?}, got {got:?}"
        );

        let (status, listed) = list_resolutions(&fixture.router, model).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the resolved-resource listing expected 200, got {status} {listed}"
        );
        let expected = fixture.surviving_resolutions(model, removed);
        let got = reported_resolutions(&case, &listed);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the resolutions {expected:?} of the surviving records, got \
             {got:?}"
        );

        let (status, summary) = execution_summary(&fixture.router, model).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the execution summary expected 200, got {status} {summary}"
        );
        let owed = fixture.surviving(Kind::Execution, model, removed).len() as u64;
        let counted = summarised_total(&case, &summary);
        assert_eq!(
            counted, owed,
            "case {case:?}: the summary must count the {owed} surviving executions, got {counted} \
             in {summary}"
        );
    }

    // The delete took the asset and what names it, and nothing else: every other
    // asset of the fixture still reads back.
    for asset in &fixture.assets {
        let (status, body) = call(&fixture.router, "GET", &format!("/assets/{asset}"), None).await;
        let (owed, what) = if removed.contains(asset) {
            (404, "a deleted asset must be gone")
        } else {
            (200, "an asset nothing deleted must still read back")
        };
        assert_eq!(
            status, owed,
            "case {stage:?}: {what}, got {status} {body} for {asset}"
        );
    }
}

#[tokio::test]
async fn deleting_an_asset_drops_every_action_and_execution_that_names_it() {
    let fixture = fixture().await;
    let snapshots = snapshot(&fixture).await;

    // Nothing is deleted yet, so every record stands and every listing reports
    // all of them: the baseline the deletes below are a difference from.
    assert_store_follows(&fixture, &snapshots, &[], "before any delete").await;

    // The first delete lands while the runs that name the asset are still in
    // flight, so the cleanup cannot be waiting on them.
    let doomed = fixture.assets[DELETED_WHILE_RUNNING].clone();
    let (status, deleted) = delete_asset(&fixture.router, &doomed).await;
    assert_eq!(
        status, 202,
        "DeleteAsset {doomed}: expected 202, got {status} {deleted}"
    );
    assert_eq!(
        deleted["assetId"].as_str(),
        Some(doomed.as_str()),
        "DeleteAsset {doomed}: unexpected assetId in {deleted}"
    );
    let mut removed = vec![doomed];
    assert_store_follows(
        &fixture,
        &snapshots,
        &removed,
        "after the delete of the asset whose runs are in flight",
    )
    .await;

    // The second lands once every run is over: a finished run is no more a
    // reason to keep naming an asset the service no longer holds.
    tokio::time::sleep(run_over()).await;
    let doomed = fixture.assets[DELETED_AFTER_THE_RUN].clone();
    let (status, deleted) = delete_asset(&fixture.router, &doomed).await;
    assert_eq!(
        status, 202,
        "DeleteAsset {doomed}: expected 202, got {status} {deleted}"
    );
    removed.push(doomed);
    assert_store_follows(
        &fixture,
        &snapshots,
        &removed,
        "after the delete of the asset whose runs are over",
    )
    .await;

    // A second delete of an asset already gone removes nothing further.
    let gone = removed[0].clone();
    let (status, refused) = delete_asset(&fixture.router, &gone).await;
    assert_eq!(
        status, 404,
        "DeleteAsset {gone} again: expected 404, got {status} {refused}"
    );
    assert_store_follows(&fixture, &snapshots, &removed, "after the repeated delete").await;
}

#[tokio::test]
async fn a_deleted_asset_is_no_longer_a_reference_any_call_resolves() {
    let fixture = fixture().await;
    let doomed = fixture.assets[DELETED_WHILE_RUNNING].clone();
    let model = fixture.models[0].clone();
    let definition = first_definition(&fixture.router, &model).await;
    let expected = asset_not_found_message(&doomed);

    let (status, deleted) = delete_asset(&fixture.router, &doomed).await;
    assert_eq!(
        status, 202,
        "DeleteAsset {doomed}: expected 202, got {status} {deleted}"
    );

    // Every call that takes the same reference now refuses it, whichever side it
    // reads or writes from: the removed asset resolves to nothing anywhere.
    let scoped = format!(
        "{}&resolveToResourceType=ASSET&resolveToResourceId={doomed}",
        target_query(Target::ComputationModel, &model)
    );
    let reads: Vec<(&str, (u16, Value))> = vec![
        ("ListActions scoped to it", list_actions(&fixture.router, &scoped).await),
        (
            "ListExecutions scoped to it",
            list_executions(&fixture.router, &scoped).await,
        ),
        (
            "the execution summary scoped to it",
            call(
                &fixture.router,
                "GET",
                &format!(
                    "/computation-models/{model}/execution-summary\
                     ?resolveToResourceType=ASSET&resolveToResourceId={doomed}"
                ),
                None,
            )
            .await,
        ),
        (
            "ExecuteAction resolving to it",
            call(
                &fixture.router,
                "POST",
                "/actions",
                Some(json!({
                    "targetResource": { "computationModelId": model },
                    "actionDefinitionId": definition,
                    "actionPayload": { "stringValue": "{}" },
                    "resolveTo": { "assetId": doomed },
                })),
            )
            .await,
        ),
    ];
    for (case, (status, body)) in reads {
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected.as_str(),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // And the write that was refused recorded nothing: the model's resolved
    // resources are the survivors, with no entry for the removed asset.
    let (status, listed) = list_resolutions(&fixture.router, &model).await;
    assert_eq!(
        status, 200,
        "the resolved-resource listing: expected 200, got {status} {listed}"
    );
    let got = reported_resolutions("after the refusals", &listed);
    assert!(
        !got.contains(&doomed),
        "a removed asset must be reported by no resolved-resource listing, got {got:?}"
    );
}
