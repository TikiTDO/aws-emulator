// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH graphs can be captured into a snapshot, driven over the
//! wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a snapshot is a copy of the graph's STORED
//! data, so every call that captures a graph is served for a graph AT REST — whether it
//! rests serving (`AVAILABLE`) or stopped (`STOPPED`), since a stopped graph keeps its
//! data and its configuration — and refused `ValidationException` (400) only while a
//! transition of that graph is still in flight and its data is in motion. A capture does
//! not start or stop the graph, so the source graph goes through the modelled
//! `SNAPSHOTTING` status and settles back into the status the call found it resting in.
//!
//! So the tables below span what that rule generalises over rather than re-running one
//! request: each at-rest status crossed with how much data the graph holds when the call
//! arrives (none, one node, several nodes joined by an edge), and the two calls that
//! capture the same graph (`CreateGraphSnapshot` and `DeleteGraph`'s final snapshot),
//! whose captures must agree. The in-flight side of the rule is driven on one graph
//! across its own `STOPPING` -> `STOPPED` boundary, so only the stored status differs
//! between the refusal and the acceptance.
//!
//! The data is seeded through the store's own writers (`crate::state::GraphData`), which
//! is where the data plane writes. What a snapshot captured is then read back over the
//! wire through `RestoreGraphFromSnapshot` and the restored graph's own summary.

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
/// most of these graphs need.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// One node of a seeded graph: its id and the labels it carries.
type SeedNode<'a> = (&'a str, &'a [&'a str]);

/// One edge of a seeded graph: its id, its label, and the ids of the nodes it joins.
type SeedEdge<'a> = (&'a str, &'a str, &'a str, &'a str);

/// One row of the at-rest table: the status the graph is resting in when the capture
/// arrives, and the data it holds by then.
struct AtRestCase<'a> {
    /// What this row is about, named in every assertion of it.
    case: &'a str,
    /// The stem the row's graph and snapshot are named from.
    stem: &'a str,
    /// The at-rest status the graph is left in: `AVAILABLE` or `STOPPED`.
    resting: &'a str,
    nodes: &'a [SeedNode<'a>],
    edges: &'a [SeedEdge<'a>],
}

/// An emulator holding only this service, with the engine it was built on, so a test can
/// seed a graph's data the way the data plane writes it and read the store for the id of
/// a snapshot no request named. Each test starts from an empty store.
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

/// `GET /summary` for one graph, which names the graph in the `graphIdentifier` HEADER —
/// so it cannot go through [`call`], which signs no extra header.
async fn summary(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri("/summary")
        .header("content-type", "application/json")
        .header("graphIdentifier", id)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
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

/// The id of a record a response reported.
fn id_of(case: &str, record: &Value) -> String {
    record["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {record}"))
        .to_string()
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
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph of {body}, got {status}: {created}"
    );
    let id = id_of(case, &created);
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
    id
}

/// Write `nodes` and `edges` into the graph's stored data, the way the data plane writes
/// them.
fn seed(
    engine: &engine::InMemoryEngine,
    case: &str,
    graph_id: &str,
    nodes: &[SeedNode<'_>],
    edges: &[SeedEdge<'_>],
) {
    let state = service_state(engine).expect("the neptune-graph store is mounted");
    let mut store = state.lock();
    let graph = store
        .graphs
        .get_mut(graph_id)
        .unwrap_or_else(|| panic!("case {case:?}: graph {graph_id} was created"));
    let written_at = std::time::SystemTime::now();
    for (id, labels) in nodes {
        graph
            .data
            .insert_node(
                id,
                GraphNode {
                    labels: labels.iter().map(|label| (*label).to_string()).collect(),
                    properties: [("name".to_string(), json!("a value"))]
                        .into_iter()
                        .collect(),
                },
                written_at,
            )
            .unwrap_or_else(|reason| panic!("case {case:?}: node {id} refused with {reason}"));
    }
    for (id, label, from_node, to_node) in edges {
        graph
            .data
            .insert_edge(
                id,
                GraphEdge {
                    label: (*label).to_string(),
                    from_node: (*from_node).to_string(),
                    to_node: (*to_node).to_string(),
                    properties: std::collections::BTreeMap::new(),
                },
                written_at,
            )
            .unwrap_or_else(|reason| panic!("case {case:?}: edge {id} refused with {reason}"));
    }
}

/// The snapshot ids the store holds, for reading the id of a snapshot no request named.
fn stored_snapshot_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the neptune-graph store is mounted");
    let ids: Vec<String> = state.lock().snapshots.keys().cloned().collect();
    ids
}

/// Stop a graph and wait it into `STOPPED`.
async fn stop_and_wait(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, stopping) = call(router, "POST", &format!("/graphs/{id}/stop"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from StopGraph, got {status}: {stopping}"
    );
    wait_for_status(router, case, &format!("/graphs/{id}"), "STOPPED").await;
}

/// Leave a graph resting in `resting`, which is either of the two at-rest statuses.
async fn rest_in(router: &MultiServiceRouter, case: &str, id: &str, resting: &str) {
    match resting {
        "AVAILABLE" => {}
        "STOPPED" => stop_and_wait(router, case, id).await,
        other => panic!("case {case:?}: {other} is not a status a graph rests in"),
    }
}

/// `POST /snapshots` of one graph under `snapshot_name`.
async fn create_snapshot(
    router: &MultiServiceRouter,
    graph_id: &str,
    snapshot_name: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": snapshot_name})),
    )
    .await
}

/// How many nodes and edges a graph's own summary reports.
async fn reported_counts(router: &MultiServiceRouter, case: &str, id: &str) -> (i64, i64) {
    let (status, body) = summary(router, id).await;
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

/// Restore `snapshot_id` into a graph named `name`, wait it into service, and return the
/// record a read of the restored graph reports.
async fn restored_graph(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
    name: &str,
) -> Value {
    let (status, restored) = call(
        router,
        "POST",
        &format!("/snapshots/{snapshot_id}/restore"),
        Some(json!({"graphName": name})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from RestoreGraphFromSnapshot of {snapshot_id}, got \
         {status}: {restored}"
    );
    let id = id_of(case, &restored);
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await
}

/// The members of a graph record that are that graph's OWN identity, dropped so what is
/// left is the configuration and data its snapshot captured.
fn without_own_identity(record: &Value) -> Value {
    let mut compared = record.clone();
    for member in [
        "id",
        "name",
        "arn",
        "endpoint",
        "createTime",
        "sourceSnapshotId",
    ] {
        compared
            .as_object_mut()
            .expect("a graph record is a JSON object")
            .remove(member);
    }
    compared
}

#[tokio::test]
async fn a_graph_at_rest_is_snapshottable_and_settles_back_into_the_status_it_rested_in() {
    let (router, engine) = emulator();

    // The rows differ along what the rule generalises over: which status the graph is
    // resting in when the capture arrives (the two at-rest statuses, `AVAILABLE` and
    // `STOPPED`, from the modelled `GraphStatus` enum
    // `smithy_query(query="shape",shape="GraphStatus")`), crossed with how much data it
    // holds by then (none, one node, several nodes joined by an edge).
    let rows: &[AtRestCase<'_>] = &[
        AtRestCase {
            case: "a serving graph holding nothing",
            stem: "serving-empty",
            resting: "AVAILABLE",
            nodes: &[],
            edges: &[],
        },
        AtRestCase {
            case: "a serving graph holding data",
            stem: "serving-filled",
            resting: "AVAILABLE",
            nodes: &[("n-1", &["Person"]), ("n-2", &["Person", "Author"])],
            edges: &[("e-1", "KNOWS", "n-1", "n-2")],
        },
        AtRestCase {
            case: "a stopped graph holding nothing",
            stem: "stopped-empty",
            resting: "STOPPED",
            nodes: &[],
            edges: &[],
        },
        AtRestCase {
            case: "a stopped graph holding one node",
            stem: "stopped-one",
            resting: "STOPPED",
            nodes: &[("n-1", &["Person"])],
            edges: &[],
        },
        AtRestCase {
            case: "a stopped graph holding data",
            stem: "stopped-filled",
            resting: "STOPPED",
            nodes: &[
                ("n-1", &["Person"]),
                ("n-2", &["Person"]),
                ("n-3", &["City"]),
            ],
            edges: &[("e-1", "KNOWS", "n-1", "n-2"), ("e-2", "LIVES_IN", "n-2", "n-3")],
        },
    ];

    for row in rows {
        let case = row.case;
        let graph_id = graph_in_service(
            &router,
            case,
            json!({
                "graphName": format!("{}-source", row.stem),
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
            }),
        )
        .await;
        // The data is written while the graph is serving, which is the only state its
        // data plane writes in; the graph then rests where the row says.
        seed(&engine, case, &graph_id, row.nodes, row.edges);
        rest_in(&router, case, &graph_id, row.resting).await;

        let snapshot_name = format!("{}-snap", row.stem);
        let (status, created) = create_snapshot(&router, &graph_id, &snapshot_name).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
        );
        let snapshot_id = id_of(case, &created);
        assert_eq!(
            created["sourceGraphId"],
            json!(graph_id),
            "case {case:?}: expected the source graph in the snapshot record, got {}",
            created["sourceGraphId"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: expected CREATING straight after the call, got {}",
            created["status"]
        );

        // The capture takes the source graph through the modelled transient status, and
        // the transition is owed at a future instant, so this read still sees it.
        let (read_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {read_status}: {graph}"
        );
        assert_eq!(
            graph["status"],
            json!("SNAPSHOTTING"),
            "case {case:?}: expected SNAPSHOTTING straight after the call, got {}",
            graph["status"]
        );

        // Both settle on their own: the snapshot becomes restorable, and the graph goes
        // back to resting WHERE IT WAS — a capture neither starts nor stops a graph.
        wait_for_status(
            &router,
            case,
            &format!("/snapshots/{snapshot_id}"),
            "AVAILABLE",
        )
        .await;
        let settled = wait_for_status(&router, case, &format!("/graphs/{graph_id}"), row.resting)
            .await;
        assert_eq!(
            settled["status"],
            json!(row.resting),
            "case {case:?}: expected the graph back in {}, got {}",
            row.resting,
            settled["status"]
        );

        // The listing reports the stored snapshot of that graph, so the capture was
        // persisted rather than only answered.
        let (listed_status, listed) = call(
            &router,
            "GET",
            &format!("/snapshots?graphIdentifier={graph_id}"),
            None,
        )
        .await;
        assert_eq!(
            listed_status, 200,
            "case {case:?}: expected 200 from ListGraphSnapshots, got {listed_status}: {listed}"
        );
        let listed_ids: Vec<String> = listed["graphSnapshots"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            listed_ids.contains(&snapshot_id),
            "case {case:?}: expected {snapshot_id} among that graph's snapshots, got \
             {listed_ids:?}"
        );

        // What it captured is read back through a restore: the restored graph holds the
        // data the source graph held when the capture arrived, so the snapshot of a
        // stopped graph is a real copy of its data.
        let restored = restored_graph(
            &router,
            case,
            &snapshot_id,
            &format!("{}-restored", row.stem),
        )
        .await;
        let restored_id = id_of(case, &restored);
        let counts = reported_counts(&router, case, &restored_id).await;
        assert_eq!(
            counts,
            (row.nodes.len() as i64, row.edges.len() as i64),
            "case {case:?}: expected the restored graph to hold the captured data, got \
             {counts:?}"
        );
    }
}

#[tokio::test]
async fn the_two_calls_that_capture_a_stopped_graph_capture_it_the_same_way() {
    let (router, engine) = emulator();

    // Two graphs provisioned alike and holding the same data, both left resting STOPPED:
    // one captured by `CreateGraphSnapshot`, the other by `DeleteGraph`'s final snapshot.
    let nodes: &[SeedNode<'_>] = &[("n-1", &["Person"]), ("n-2", &["City"])];
    let edges: &[SeedEdge<'_>] = &[("e-1", "LIVES_IN", "n-1", "n-2")];
    let configuration = json!({
        "provisionedMemory": 32,
        "replicaCount": 1,
        "publicConnectivity": true,
        "deletionProtection": false,
    });

    let case = "the two captures of one stopped graph shape";
    let mut created_graphs: Vec<String> = Vec::new();
    for stem in ["named-capture", "final-capture"] {
        let mut body = configuration.clone();
        body["graphName"] = json!(format!("{stem}-source"));
        let id = graph_in_service(&router, case, body).await;
        seed(&engine, case, &id, nodes, edges);
        stop_and_wait(&router, case, &id).await;
        created_graphs.push(id);
    }
    let (named_source, final_source) = (&created_graphs[0], &created_graphs[1]);

    // `CreateGraphSnapshot` of the first stopped graph.
    let (named_status, named) = create_snapshot(&router, named_source, "named-capture-snap").await;
    assert_eq!(
        named_status, 201,
        "case {case:?}: expected 201 from CreateGraphSnapshot of a stopped graph, got \
         {named_status}: {named}"
    );
    let named_snapshot = id_of(case, &named);

    // `DeleteGraph` with the required `skipSnapshot: false` of the second: the final
    // snapshot carries no name in the request, so its id is the one the store gained.
    let before = stored_snapshot_ids(&engine);
    let (deleted_status, deleted) = call(
        &router,
        "DELETE",
        &format!("/graphs/{final_source}?skipSnapshot=false"),
        None,
    )
    .await;
    assert_eq!(
        deleted_status, 200,
        "case {case:?}: expected 200 from DeleteGraph of a stopped graph, got \
         {deleted_status}: {deleted}"
    );
    let after = stored_snapshot_ids(&engine);
    let minted: Vec<&String> = after.iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(
        minted.len(),
        1,
        "case {case:?}: expected DeleteGraph to mint exactly one final snapshot, got \
         {minted:?}"
    );
    let final_snapshot = minted[0].clone();

    // Both become restorable on their own, and restore to the same graph: the same
    // configuration, the same encryption key and the same data.
    for snapshot in [&named_snapshot, &final_snapshot] {
        wait_for_status(&router, case, &format!("/snapshots/{snapshot}"), "AVAILABLE").await;
    }
    let from_named = restored_graph(&router, case, &named_snapshot, "named-capture-restored").await;
    let from_final = restored_graph(&router, case, &final_snapshot, "final-capture-restored").await;
    assert_eq!(
        without_own_identity(&from_named),
        without_own_identity(&from_final),
        "case {case:?}: expected the two captures of one stopped graph shape to restore \
         alike, got {from_named} and {from_final}"
    );
    let named_counts = reported_counts(&router, case, &id_of(case, &from_named)).await;
    let final_counts = reported_counts(&router, case, &id_of(case, &from_final)).await;
    assert_eq!(
        named_counts,
        (nodes.len() as i64, edges.len() as i64),
        "case {case:?}: expected the named capture to hold the stopped graph's data, got \
         {named_counts:?}"
    );
    assert_eq!(
        final_counts, named_counts,
        "case {case:?}: expected both captures to hold the same data, got {final_counts:?} \
         and {named_counts:?}"
    );
}

#[tokio::test]
async fn a_capture_is_refused_only_while_the_graphs_transition_is_in_flight() {
    let (router, engine) = emulator();

    let case = "one graph across the STOPPING -> STOPPED boundary";
    let graph_id = graph_in_service(
        &router,
        case,
        json!({
            "graphName": "boundary-source",
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
        }),
    )
    .await;
    seed(&engine, case, &graph_id, &[("n-1", &["Person"])], &[]);

    // The stop is accepted and its transition is in flight, so the graph's data is in
    // motion: the capture is refused, and the refusal names the status that held it back.
    let (stopping_status, stopping) =
        call(&router, "POST", &format!("/graphs/{graph_id}/stop"), None).await;
    assert_eq!(
        stopping_status, 200,
        "case {case:?}: expected 200 from StopGraph, got {stopping_status}: {stopping}"
    );
    let (refused_status, refused) = create_snapshot(&router, &graph_id, "boundary-snap").await;
    assert_eq!(
        refused_status, 400,
        "case {case:?} while STOPPING: expected 400, got {refused_status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case {case:?} while STOPPING: unexpected error code in {refused}"
    );
    assert_eq!(
        refused["message"],
        json!(format!(
            "Graph [{graph_id}] is not available for snapshot. You can only create a \
             snapshot for an AVAILABLE Graph."
        )),
        "case {case:?} while STOPPING: unexpected message in {refused}"
    );

    // Once the stop comes due the graph RESTS, and the SAME call is served — so the
    // refusal was about the transition being in flight and nothing else, and it
    // persisted nothing, not even the name it asked for.
    wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "STOPPED").await;
    let (accepted_status, accepted) = create_snapshot(&router, &graph_id, "boundary-snap").await;
    assert_eq!(
        accepted_status, 201,
        "case {case:?} once STOPPED: expected 201, got {accepted_status}: {accepted}"
    );
    assert_eq!(
        accepted["name"],
        json!("boundary-snap"),
        "case {case:?} once STOPPED: expected the requested name, got {}",
        accepted["name"]
    );
    let snapshot_id = id_of(case, &accepted);
    let read = wait_for_status(
        &router,
        case,
        &format!("/snapshots/{snapshot_id}"),
        "AVAILABLE",
    )
    .await;
    assert_eq!(
        read["sourceGraphId"],
        json!(graph_id),
        "case {case:?} once STOPPED: expected the stored snapshot to name its source \
         graph, got {read}"
    );
    // The graph it was taken of is stopped still: nothing about the capture started it.
    let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
    assert_eq!(
        graph_status, 200,
        "case {case:?}: expected 200 from GetGraph, got {graph_status}: {graph}"
    );
    assert_eq!(
        graph["status"],
        json!("STOPPED"),
        "case {case:?}: expected the graph still STOPPED, got {}",
        graph["status"]
    );
}
