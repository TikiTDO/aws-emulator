// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EMPTINESS precondition `StartImportTask` states on the
//! graph it loads into, driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! The rule every assertion here is about: an import into an existing graph is accepted
//! only while that graph holds NO data — "The graph needs to be empty and in the
//! AVAILABLE state" (`smithy_query(query="doc",shape="StartImportTask")`) — so a graph
//! holding any node or edge is refused `ConflictException` (409) and nothing is
//! persisted: no task is minted, the graph keeps its status and keeps its data. The
//! decision is taken on the data the graph holds AT THE TIME OF THE CALL, however it
//! came to hold it, and it is a fresh decision each time: emptying the graph makes it
//! importable again.
//!
//! So the tables below span what that rule generalises over: how much data the graph
//! holds (none, one unlabelled element, several nodes, nodes joined by edges), what the
//! elements carry (labels and properties, or neither), how the data got there (written
//! into a stored graph, or brought up with a graph restored from a snapshot that
//! captured it), and what emptying it again does.
//!
//! The data is seeded through the store's own writers (`crate::state::GraphData`), which
//! is where a bulk load and a restore write it: the openCypher engine behind
//! `ExecuteQuery` evaluates read-only queries, so a query cannot write it, and the
//! precondition must
//! hold for the data the store holds however it got there. Every other step — the
//! create, the snapshot, the restore, the reset, the import, the summary and the task
//! listing — goes over the wire.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphEdge, GraphNode, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

/// The `source` every import request here names; nothing reads it.
const SOURCE: &str = "s3://example-bucket/data/";

/// One node of a seeded graph: its id, its labels, and the property keys set on it.
type SeedNode<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// One edge of a seeded graph: its id, its label, the ids of the nodes it leaves and
/// enters, and the property keys set on it.
type SeedEdge<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [&'a str]);

/// One row of an emptiness table: the case, the nodes and the edges the target graph
/// holds when the import arrives, and whether the import is accepted.
type EmptinessCase<'a> = (&'a str, &'a [SeedNode<'a>], &'a [SeedEdge<'a>], bool);

/// The whole message the emptiness refusal must carry, written out here rather than
/// reached from the service's own helper so the test states the expectation
/// independently of the code under test.
const NOT_EMPTY_MESSAGE: &str = "Unable to start the import task because the graph is not empty.";

/// An emulator holding only this service, with the engine it was built on, so a test can
/// seed a graph's data the way the data plane writes it. Each test starts from an empty
/// store.
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
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
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

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
fn is_task_id(value: &str) -> bool {
    value.strip_prefix("t-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// `POST /graphs/{graphIdentifier}/importtasks` with the required members.
async fn start_import(router: &MultiServiceRouter, graph_id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        &[],
        Some(json!({"source": SOURCE, "roleArn": ROLE_ARN})),
    )
    .await
}

/// The task ids `ListImportTasks` reports for one graph
/// (`GET /importtasks?graphIdentifier=...`).
async fn listed_task_ids(router: &MultiServiceRouter, graph_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/importtasks?graphIdentifier={graph_id}"),
        &[],
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the tasks of graph {graph_id}: expected 200 from ListImportTasks, got \
         {status}: {body}"
    );
    body["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case the tasks of graph {graph_id}: no tasks list in {body}"))
        .iter()
        .map(|task| task["taskId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `GET /summary` for one graph, which names the graph in the `graphIdentifier` HEADER
/// (`smithy_query(query="operation",operation="GetGraphSummary")` -> input).
async fn reported_counts(router: &MultiServiceRouter, case: &str, graph_id: &str) -> (i64, i64) {
    let (status, body) = call(
        router,
        "GET",
        "/summary",
        &[("graphIdentifier", graph_id)],
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetGraphSummary, got {status}: {body}"
    );
    let reported = &body["graphSummary"];
    (
        reported["numNodes"].as_i64().unwrap_or(-1),
        reported["numEdges"].as_i64().unwrap_or(-1),
    )
}

/// Poll one resource until it reports `expected`, then return its record. No handler
/// ever sleeps; only this test waits, because it is asking about a status the clock
/// reaches on its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, &[], None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, &[], None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// Create a graph, wait the provisioning the clock owes it out, and return its id.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        &[],
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
    id
}

/// Write `nodes` and `edges` into the graph's stored data, the way the data plane writes
/// them.
fn seed(
    engine: &engine::InMemoryEngine,
    graph_id: &str,
    nodes: &[SeedNode<'_>],
    edges: &[SeedEdge<'_>],
) {
    let state = service_state(engine).expect("the neptune-graph store is mounted");
    let mut store = state.lock();
    let graph = store
        .graphs
        .get_mut(graph_id)
        .unwrap_or_else(|| panic!("graph {graph_id} was created"));
    let written_at = std::time::SystemTime::now();
    for (id, labels, property_keys) in nodes {
        graph
            .data
            .insert_node(
                id,
                GraphNode {
                    labels: labels.iter().map(|label| (*label).to_string()).collect(),
                    properties: property_keys
                        .iter()
                        .map(|key| ((*key).to_string(), json!("a value")))
                        .collect(),
                },
                written_at,
            )
            .unwrap_or_else(|reason| panic!("node {id}: refused with {reason}"));
    }
    for (id, label, from_node, to_node, property_keys) in edges {
        graph
            .data
            .insert_edge(
                id,
                GraphEdge {
                    label: (*label).to_string(),
                    from_node: (*from_node).to_string(),
                    to_node: (*to_node).to_string(),
                    properties: property_keys
                        .iter()
                        .map(|key| ((*key).to_string(), json!(7)))
                        .collect(),
                },
                written_at,
            )
            .unwrap_or_else(|reason| panic!("edge {id}: refused with {reason}"));
    }
}

/// Assert one ACCEPTED import: the modelled success status, a minted task id, the graph
/// the task loads into, and the transient status the graph moved to.
async fn assert_accepted(router: &MultiServiceRouter, case: &str, graph_id: &str, body: &Value) {
    let task_id = body["taskId"].as_str().unwrap_or_default();
    assert!(
        is_task_id(task_id),
        "case {case:?}: expected a minted ^t-[a-z0-9]{{10}}$ task id, got {task_id:?}"
    );
    assert_eq!(
        body["graphId"],
        json!(graph_id),
        "case {case:?}: expected the task to name graph {graph_id:?}, got {}",
        body["graphId"]
    );
    assert_eq!(
        body["status"],
        json!("INITIALIZING"),
        "case {case:?}: expected INITIALIZING, got {}",
        body["status"]
    );
    let listed = listed_task_ids(router, graph_id).await;
    assert!(
        listed.iter().any(|id| id == task_id),
        "case {case:?}: expected the minted task {task_id:?} among the graph's tasks, \
         got {listed:?}"
    );
}

/// Assert one REFUSED import: the modelled status and code, the whole message the rule
/// words, that the refusal fronts no identifier, and that nothing was persisted — no
/// task was minted and the graph keeps the status and the data it had.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    code: u16,
    body: &Value,
    counts_before: (i64, i64),
) {
    assert_eq!(code, 409, "case {case:?}: expected 409, got {code}: {body}");
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual, NOT_EMPTY_MESSAGE,
        "case {case:?}: expected message {NOT_EMPTY_MESSAGE:?}, got {actual:?}"
    );
    assert!(
        !actual.contains(graph_id),
        "case {case:?}: the refusal must not front the graph identifier, got {actual:?}"
    );

    let listed = listed_task_ids(router, graph_id).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: a refused import must mint no task, got {listed:?}"
    );
    let (read_code, read) = call(router, "GET", &format!("/graphs/{graph_id}"), &[], None).await;
    assert_eq!(
        read_code, 200,
        "case {case:?}: expected 200 from GetGraph, got {read_code}: {read}"
    );
    assert_eq!(
        read["status"],
        json!("AVAILABLE"),
        "case {case:?}: a refused import must leave the graph serving, got {}",
        read["status"]
    );
    let counts_after = reported_counts(router, case, graph_id).await;
    assert_eq!(
        counts_after, counts_before,
        "case {case:?}: a refused import must leave the data alone, expected \
         {counts_before:?} (nodes, edges), got {counts_after:?}"
    );
}

#[tokio::test]
async fn an_import_is_accepted_only_into_a_graph_that_holds_no_data() {
    let (router, engine) = emulator();

    // The rows differ along what the rule generalises over: how much data the graph
    // holds when the import arrives, and what its elements carry — an element with
    // neither labels nor properties still makes the graph non-empty.
    let cases: &[EmptinessCase<'_>] = &[
        ("a graph created empty and never written to", &[], &[], true),
        (
            "a graph holding one unlabelled node with no properties",
            &[("n1", &[], &[])],
            &[],
            false,
        ),
        (
            "a graph holding one labelled node carrying properties",
            &[("n1", &["Person"], &["name", "age"])],
            &[],
            false,
        ),
        (
            "a graph holding several nodes and no edges",
            &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person", "Author"], &[]),
                ("n3", &["Book"], &["title"]),
            ],
            &[],
            false,
        ),
        (
            "a graph holding nodes joined by edges",
            &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person"], &[]),
                ("n3", &["City"], &[]),
            ],
            &[
                ("e1", "KNOWS", "n1", "n2", &["since"]),
                ("e2", "LIVES_IN", "n2", "n3", &[]),
            ],
            false,
        ),
    ];

    for (index, (case, nodes, edges, accepted)) in cases.iter().enumerate() {
        // A graph per row, so a row is about the DATA its graph holds and not about a
        // transition another row left outstanding on a shared graph.
        let graph_id = graph_in_service(&router, case, &format!("empty-gate-{index}")).await;
        seed(&engine, &graph_id, nodes, edges);
        let counts_before = reported_counts(&router, case, &graph_id).await;
        assert_eq!(
            counts_before,
            (nodes.len() as i64, edges.len() as i64),
            "case {case:?}: the seeded graph must report what it holds, got \
             {counts_before:?}"
        );

        let (code, body) = start_import(&router, &graph_id).await;
        if *accepted {
            assert_eq!(
                code, 201,
                "case {case:?}: expected 201, got {code}: {body}"
            );
            assert_accepted(&router, case, &graph_id, &body).await;
        } else {
            assert_refused(&router, case, &graph_id, code, &body, counts_before).await;
        }
    }
}

#[tokio::test]
async fn emptying_a_graph_makes_it_importable_again() {
    let (router, engine) = emulator();
    let case = "a graph emptied between two identical import requests";
    let graph_id = graph_in_service(&router, case, "reset-then-import").await;
    seed(
        &engine,
        &graph_id,
        &[("n1", &["Person"], &["name"]), ("n2", &["City"], &[])],
        &[("e1", "LIVES_IN", "n1", "n2", &["since"])],
    );

    // The same request is refused while the graph holds the data...
    let counts_before = reported_counts(&router, case, &graph_id).await;
    let (code, body) = start_import(&router, &graph_id).await;
    assert_refused(&router, case, &graph_id, code, &body, counts_before).await;

    // ...and accepted once a reset has carried that data away: the precondition is
    // decided on what the graph holds now, not on what it ever held.
    let (reset_code, reset) = call(
        &router,
        "PUT",
        &format!("/graphs/{graph_id}"),
        &[],
        Some(json!({"skipSnapshot": true})),
    )
    .await;
    assert_eq!(
        reset_code, 200,
        "case {case:?}: expected 200 from ResetGraph, got {reset_code}: {reset}"
    );
    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let counts_after_reset = reported_counts(&router, case, &graph_id).await;
    assert_eq!(
        counts_after_reset,
        (0, 0),
        "case {case:?}: a settled reset leaves the graph empty, got {counts_after_reset:?}"
    );

    let (code, body) = start_import(&router, &graph_id).await;
    assert_eq!(
        code, 201,
        "case {case:?}: expected 201 once the graph is empty, got {code}: {body}"
    );
    assert_accepted(&router, case, &graph_id, &body).await;
}

#[tokio::test]
async fn a_graph_restored_from_a_snapshot_takes_an_import_only_when_the_capture_was_empty() {
    let (router, engine) = emulator();

    // The rows differ along what the capture held: a restored graph comes up holding
    // the snapshot's data, so the import it accepts is decided by that data and not by
    // the graph being newly minted.
    let cases: &[EmptinessCase<'_>] = &[
        ("a snapshot of an empty graph", &[], &[], true),
        (
            "a snapshot of a graph holding nodes joined by an edge",
            &[("n1", &["Person"], &["name"]), ("n2", &["Book"], &[])],
            &[("e1", "WROTE", "n1", "n2", &["year"])],
            false,
        ),
    ];

    for (index, (case, nodes, edges, accepted)) in cases.iter().enumerate() {
        let source_id = graph_in_service(&router, case, &format!("capture-source-{index}")).await;
        seed(&engine, &source_id, nodes, edges);

        let (snapshot_code, snapshot) = call(
            &router,
            "POST",
            "/snapshots",
            &[],
            Some(json!({
                "graphIdentifier": source_id,
                "snapshotName": format!("capture-{index}"),
            })),
        )
        .await;
        assert_eq!(
            snapshot_code, 201,
            "case {case:?}: expected 201 from CreateGraphSnapshot, got {snapshot_code}: \
             {snapshot}"
        );
        let snapshot_id = snapshot["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {snapshot}"))
            .to_string();
        wait_for_status(&router, case, &format!("/snapshots/{snapshot_id}"), "AVAILABLE").await;

        let (restore_code, restored) = call(
            &router,
            "POST",
            &format!("/snapshots/{snapshot_id}/restore"),
            &[],
            Some(json!({"graphName": format!("restored-{index}")})),
        )
        .await;
        assert_eq!(
            restore_code, 201,
            "case {case:?}: expected 201 from RestoreGraphFromSnapshot, got \
             {restore_code}: {restored}"
        );
        let restored_id = restored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {restored}"))
            .to_string();
        wait_for_status(&router, case, &format!("/graphs/{restored_id}"), "AVAILABLE").await;

        // The restored graph came up holding what the snapshot captured, which is what
        // the import precondition is then decided on.
        let counts_before = reported_counts(&router, case, &restored_id).await;
        assert_eq!(
            counts_before,
            (nodes.len() as i64, edges.len() as i64),
            "case {case:?}: the restored graph must hold the capture, got {counts_before:?}"
        );

        let (code, body) = start_import(&router, &restored_id).await;
        if *accepted {
            assert_eq!(
                code, 201,
                "case {case:?}: expected 201, got {code}: {body}"
            );
            assert_accepted(&router, case, &restored_id, &body).await;
        } else {
            assert_refused(&router, case, &restored_id, code, &body, counts_before).await;
        }
    }
}
