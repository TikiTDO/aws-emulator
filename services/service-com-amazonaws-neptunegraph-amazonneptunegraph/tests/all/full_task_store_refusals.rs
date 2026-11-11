// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how this service reports a FULL store, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: an operation that mints a record and models
//! no quota error reports a store with no room left through the `ConflictException`
//! (409) it does model — the request conflicts with what the store already holds — with
//! nothing minted, and never as a `ValidationException`, because every member of such a
//! request is well formed and was accepted. Both task namespaces mint through such an
//! operation (`StartExportTask` and `StartImportTask` model
//! `[ConflictException 409, InternalServerException 500, ResourceNotFoundException 404,
//! ThrottlingException 429, ValidationException 400]`, and no quota error:
//! `smithy_query(query="operation",operation="StartExportTask,StartImportTask")`), so
//! each table below drives BOTH of them and asserts they agree.
//!
//! The rows differ along what the rule generalises over — which task namespace is full,
//! and the members of the refused request (the export format and its parquet type,
//! whether a filter or a tag map is supplied, the destination's bucket and prefix, the
//! IAM role's path form, the import source and format, `failOnError`, blank-node
//! handling) — rather than re-running one request, because a full store is a fact about
//! the STORE and the refusal may not depend on the shape of the request that met it.
//! The ceiling itself is tested at its boundary: one record below it the identical
//! request is accepted, and at it the next one is refused.
//!
//! The filler records are written through the store's own writers, which is where an
//! accepted call writes them: driving `StartImportTask` to its ceiling over the wire is
//! not possible in one AVAILABLE graph (each accepted import takes the graph through the
//! transient `IMPORTING` status), so neither namespace is filled that way, and the one
//! call per namespace that must be ACCEPTED goes over the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, ExportTask, ExportTaskStatus, ImportLoad, ImportTask, ImportTaskStatus,
    ARN_NAMESPACE, MAX_EXPORT_TASKS, MAX_IMPORT_TASKS, PROVISIONING_SPAN,
    SOURCE_UNREADABLE_REASON,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneTaskRole";

/// The same pattern's `role/service-role` form, so the table spans both role paths.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/NeptuneTask";

/// Which task namespace a row is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskStore {
    Import,
    Export,
}

impl TaskStore {
    /// The operation a row drives, named in every assertion of it.
    fn operation(self) -> &'static str {
        match self {
            Self::Import => "StartImportTask",
            Self::Export => "StartExportTask",
        }
    }

    /// The ceiling this namespace keeps.
    fn ceiling(self) -> usize {
        match self {
            Self::Import => MAX_IMPORT_TASKS,
            Self::Export => MAX_EXPORT_TASKS,
        }
    }

    /// The plural the refusal names this namespace's records with.
    fn records(self) -> &'static str {
        match self {
            Self::Import => "import tasks",
            Self::Export => "export tasks",
        }
    }
}

/// One row of the full-store table: the namespace whose store is full, and the
/// well-formed request that meets it.
struct CeilingCase<'a> {
    /// What this row is about, named in every assertion of it.
    case: &'a str,
    store: TaskStore,
    /// The request body, built from the id of the graph the call names.
    body: fn(&str) -> Value,
}

/// An emulator holding only this service, with the engine it was built on, so a test can
/// fill a store the way an accepted call fills it and count what it holds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Start one task of `store` against `graph_id`, over the wire.
async fn start_task(
    router: &MultiServiceRouter,
    store: TaskStore,
    graph_id: &str,
    body: Value,
) -> (u16, Value) {
    match store {
        TaskStore::Export => call(router, "POST", "/exporttasks", Some(body)).await,
        TaskStore::Import => {
            call(
                router,
                "POST",
                &format!("/graphs/{graph_id}/importtasks"),
                Some(body),
            )
            .await
        }
    }
}

/// The plainest well-formed request of each namespace: what the boundary rows send, and
/// what every refused row varies from.
fn plain_request(store: TaskStore, graph_id: &str) -> Value {
    match store {
        TaskStore::Export => json!({
            "graphIdentifier": graph_id,
            "roleArn": ROLE_ARN,
            "format": "CSV",
            "destination": "s3://export-bucket/plain/",
            "kmsKeyIdentifier": KMS_KEY_ARN,
        }),
        TaskStore::Import => json!({
            "source": "s3://import-bucket/plain/",
            "roleArn": ROLE_ARN,
        }),
    }
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, "POST", "/graphs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {created}"))
        .to_string();
    for _ in 0..40 {
        let (code, graph) = call(router, "GET", &format!("/graphs/{id}"), None).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected 200 from GetGraph, got {code}: {graph}"
        );
        if graph["status"] == json!("AVAILABLE") {
            return id;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {case:?}: graph {id} never became AVAILABLE");
}

/// How many records each task store holds.
fn stored_counts(engine: &Arc<engine::InMemoryEngine>) -> (usize, usize) {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let store = state.lock();
    (store.import_tasks.len(), store.export_tasks.len())
}

/// How many records the store of `store` holds.
fn stored_count(engine: &Arc<engine::InMemoryEngine>, store: TaskStore) -> usize {
    let (imports, exports) = stored_counts(engine);
    match store {
        TaskStore::Import => imports,
        TaskStore::Export => exports,
    }
}

/// Fill the store of `store` to `upto` records, through the store's own writers — where
/// an accepted call writes them.
fn fill_to(engine: &Arc<engine::InMemoryEngine>, store: TaskStore, graph_id: &str, upto: usize) {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let mut held = state.lock();
    let now = Instant::now();
    loop {
        let filled = match store {
            TaskStore::Import => held.import_tasks.len(),
            TaskStore::Export => held.export_tasks.len(),
        };
        if filled >= upto {
            return;
        }
        match store {
            TaskStore::Export => {
                let id = held.next_export_task_id();
                held.insert_export_task(ExportTask {
                    arn: engine::emulator_arn(
                        ARN_NAMESPACE,
                        REGION,
                        &format!("export-task/{id}"),
                    ),
                    region: REGION.to_string(),
                    id,
                    source_graph_id: graph_id.to_string(),
                    role_arn: ROLE_ARN.to_string(),
                    format: "CSV".to_string(),
                    parquet_type: None,
                    destination: "s3://export-bucket/filler/".to_string(),
                    kms_key_identifier: KMS_KEY_ARN.to_string(),
                    export_filter: None,
                    status: ExportTaskStatus::Initializing,
                    status_reason: None,
                    exported_vertices: 0,
                    exported_edges: 0,
                    cancelled_progress: None,
                    tags: BTreeMap::new(),
                    start_time: SystemTime::now(),
                    started_at: now,
                    settles_at: None,
                });
            }
            TaskStore::Import => {
                let id = held.next_import_task_id();
                held.insert_import_task(ImportTask {
                    id,
                    region: REGION.to_string(),
                    graph_id: graph_id.to_string(),
                    source: "s3://import-bucket/filler/".to_string(),
                    format: None,
                    parquet_type: None,
                    blank_node_handling: None,
                    fail_on_error: true,
                    role_arn: ROLE_ARN.to_string(),
                    import_options: None,
                    status: ImportTaskStatus::Initializing,
                    status_reason: None,
                    attempt_number: 1,
                    // A filler record: nothing read a source for it, so it loads
                    // nothing and stops where it is.
                    load: ImportLoad::unread(SOURCE_UNREADABLE_REASON),
                    delivered: false,
                    start_time: SystemTime::now(),
                    started_at: Instant::now(),
                    stopped_progress: None,
                    settles_at: None,
                });
            }
        }
    }
}

/// The ceiling refusal both namespaces word: what the emulator holds at most, and of
/// what.
fn expected_refusal(store: TaskStore) -> String {
    format!(
        "this emulator holds at most {} {}",
        store.ceiling(),
        store.records()
    )
}

#[tokio::test]
async fn a_full_task_store_is_a_conflict_whatever_the_request_looked_like() {
    let (router, engine) = emulator();
    let graph_id = graph_in_service(&router, "a full task store", "full-store").await;

    // Both namespaces are filled right up to their ceilings, so what every row below
    // meets is a store with no room left rather than anything about its own request.
    for store in [TaskStore::Import, TaskStore::Export] {
        fill_to(&engine, store, &graph_id, store.ceiling());
        assert_eq!(
            stored_count(&engine, store),
            store.ceiling(),
            "case {:?}: expected the store filled to its ceiling, got {}",
            store.operation(),
            stored_count(&engine, store)
        );
    }

    let rows = [
        CeilingCase {
            case: "export, the plainest request",
            store: TaskStore::Export,
            body: |graph_id| plain_request(TaskStore::Export, graph_id),
        },
        CeilingCase {
            case: "export, PARQUET under a service role, with tags",
            store: TaskStore::Export,
            body: |graph_id| {
                json!({
                    "graphIdentifier": graph_id,
                    "roleArn": SERVICE_ROLE_ARN,
                    "format": "PARQUET",
                    "parquetType": "COLUMNAR",
                    "destination": "s3://another-bucket/deep/prefix/path/",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                    "tags": {"team": "graph", "env": "test"},
                })
            },
        },
        CeilingCase {
            case: "export, filtered on a vertex label",
            store: TaskStore::Export,
            body: |graph_id| {
                json!({
                    "graphIdentifier": graph_id,
                    "roleArn": ROLE_ARN,
                    "format": "CSV",
                    "destination": "s3://export-bucket/filtered",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                    "exportFilter": {
                        "vertexFilter": {"Person": {"properties": {"name": {"sourcePropertyName": "name"}}}},
                    },
                })
            },
        },
        CeilingCase {
            case: "import, the plainest request",
            store: TaskStore::Import,
            body: |graph_id| plain_request(TaskStore::Import, graph_id),
        },
        CeilingCase {
            case: "import, PARQUET that does not halt on an error",
            store: TaskStore::Import,
            body: |_| {
                json!({
                    "source": "s3://another-import-bucket/deep/prefix/",
                    "roleArn": SERVICE_ROLE_ARN,
                    "format": "PARQUET",
                    "parquetType": "COLUMNAR",
                    "failOnError": false,
                })
            },
        },
        CeilingCase {
            case: "import, NTRIPLES converting blank nodes",
            store: TaskStore::Import,
            body: |_| {
                json!({
                    "source": "s3://import-bucket/rdf/",
                    "roleArn": ROLE_ARN,
                    "format": "NTRIPLES",
                    "blankNodeHandling": "convertToIri",
                })
            },
        },
    ];

    for row in rows {
        let case = row.case;
        let operation = row.store.operation();
        let before = stored_count(&engine, row.store);
        let (status, body) = start_task(&router, row.store, &graph_id, (row.body)(&graph_id)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 from {operation} against a full store, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected a ConflictException from {operation}, got {body}"
        );
        let refusal = message(&body);
        assert_eq!(
            refusal,
            expected_refusal(row.store),
            "case {case:?}: unexpected refusal wording from {operation}, got {refusal:?}"
        );
        // A full store is not a bad input, so nothing names a reason that would tell the
        // caller to change a request with nothing wrong with it.
        assert_eq!(
            body["reason"],
            Value::Null,
            "case {case:?}: expected no reason member from {operation}, got {body}"
        );
        assert_eq!(
            stored_count(&engine, row.store),
            before,
            "case {case:?}: expected {operation} to mint nothing against a full store, \
             got {} records where it held {before}",
            stored_count(&engine, row.store)
        );
    }
}

#[tokio::test]
async fn one_record_below_the_ceiling_the_same_request_is_accepted() {
    for store in [TaskStore::Import, TaskStore::Export] {
        let operation = store.operation();
        let case = format!("{operation} one below its ceiling");
        let (router, engine) = emulator();
        let graph_id = graph_in_service(&router, &case, "boundary").await;

        // One record below the ceiling the store still has room, so the identical
        // request the full store refuses is accepted here: the refusal is the BOUND
        // being met, not a blanket refusal of the request.
        fill_to(&engine, store, &graph_id, store.ceiling() - 1);
        let (status, accepted) =
            start_task(&router, store, &graph_id, plain_request(store, &graph_id)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from {operation} one record below its ceiling, \
             got {status}: {accepted}"
        );
        let task_id = accepted["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {accepted}"))
            .to_string();
        assert_eq!(
            stored_count(&engine, store),
            store.ceiling(),
            "case {case:?}: expected the accepted call to fill the store to its ceiling, \
             got {}",
            stored_count(&engine, store)
        );

        // The accepted record round-trips through the read of its own namespace, so what
        // filled the last slot is a stored task and not a reported one.
        let read_path = match store {
            TaskStore::Export => format!("/exporttasks/{task_id}"),
            TaskStore::Import => format!("/importtasks/{task_id}"),
        };
        let (read_status, read) = call(&router, "GET", &read_path, None).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 reading back {task_id}, got {read_status}: {read}"
        );
        assert_eq!(
            read["taskId"], accepted["taskId"],
            "case {case:?}: expected the read to answer task {task_id}, got {read}"
        );

        // And the next identical request, now that the ceiling is met, is the conflict.
        let (refused_status, refused) =
            start_task(&router, store, &graph_id, plain_request(store, &graph_id)).await;
        assert_eq!(
            refused_status, 409,
            "case {case:?}: expected 409 from the call past the ceiling, got \
             {refused_status}: {refused}"
        );
        assert_eq!(
            message(&refused),
            expected_refusal(store),
            "case {case:?}: unexpected refusal wording, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_full_store_still_reports_an_invalid_member_as_a_validation_failure() {
    let (router, engine) = emulator();
    let graph_id = graph_in_service(&router, "a full store with a bad member", "full-bad").await;
    fill_to(&engine, TaskStore::Export, &graph_id, MAX_EXPORT_TASKS);

    // A full store does not swallow an input violation, and an input violation is not
    // what a full store answers: the two channels stay distinct, so a request carrying a
    // `destination` that is no Amazon S3 URI is still the 400 that names the member.
    for (case, destination) in [
        ("a destination with no scheme", "export-bucket/prefix/"),
        ("a destination of another scheme", "https://example.com/x"),
        ("an empty bucket", "s3:///prefix/"),
    ] {
        let mut body = plain_request(TaskStore::Export, &graph_id);
        body["destination"] = json!(destination);
        let (status, refused) = start_task(&router, TaskStore::Export, &graph_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from StartExportTask, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused}"
        );
        let refusal = message(&refused);
        assert!(
            refusal.contains("destination"),
            "case {case:?}: expected the refusal to name the member, got {refusal:?}"
        );
        assert!(
            !refusal.contains(&MAX_EXPORT_TASKS.to_string()),
            "case {case:?}: expected the refusal to be about the member and not the \
             store's ceiling, got {refusal:?}"
        );
    }
}
