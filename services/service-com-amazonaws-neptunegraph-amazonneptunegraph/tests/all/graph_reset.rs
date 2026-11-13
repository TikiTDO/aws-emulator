// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ResetGraph`, driven over the wire (`restJson1`) through the
//! same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A reset of a SERVING graph is accepted and empties that graph's DATA: the call
//!   answers 200 with the graph reporting the modelled `RESETTING` status, the graph
//!   keeps everything that is not data (its id, name, ARN, configuration and encryption
//!   key), and once the transition the clock owes falls due it is serving again over
//!   empty data.
//! * `skipSnapshot` decides whether a FINAL snapshot of the data is taken before it is
//!   emptied: `false` mints one that holds the data as the reset found it, `true` mints
//!   nothing. Neither value changes the answer or the status the reset reports.
//! * A reset the service cannot carry out changes nothing: no snapshot is minted and
//!   the graph keeps the data it held.
//!
//! So each table below spans what its rule generalises over: both values of the
//! required `skipSnapshot`, graphs holding nothing through graphs holding several nodes
//! and edges, encrypted and unencrypted graphs, and every state a reset is refused
//! from — rather than re-running one recorded request.
//!
//! The data is seeded through the store's own writers (`crate::state::GraphData`),
//! which is where a bulk load and a restore write it: the openCypher engine behind
//! `ExecuteQuery` evaluates read-only queries, so a query cannot write the data, and a
//! reset
//! must empty the data the store holds however it got there. `ListGraphSnapshots` is a
//! scaffold stub too, so the id of a snapshot the reset minted is read out of the store
//! (which is also what proves the reset minted exactly one); every assertion about that
//! snapshot then goes over the wire.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphEdge, GraphNode, FINAL_SNAPSHOT_SUFFIX, PROVISIONING_SPAN,
    SERVICE_OWNED_KMS_KEY,
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

/// One node of a seeded graph: its id, its labels, and the property keys set on it.
type SeedNode<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// One edge of a seeded graph: its id, its label, the ids of the nodes it leaves and
/// enters, and the property keys set on it.
type SeedEdge<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [&'a str]);

/// One row of the reset table: the case, the graph's name, the nodes and edges it holds
/// when the reset arrives, the `skipSnapshot` the reset carries, and the KMS key the
/// graph was created with (`None` for a graph on the service-owned key).
type ResetCase<'a> = (
    &'a str,
    &'a str,
    &'a [SeedNode<'a>],
    &'a [SeedEdge<'a>],
    bool,
    Option<&'a str>,
);

/// An emulator holding only this service, with the engine it was built on, so a test
/// can seed a graph's data the way the data plane writes it and read the store for the
/// id of a resource no implemented list operation reports yet.
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

/// `PUT /graphs/{id}` with the required `skipSnapshot` in the body
/// (`smithy_query(query="operation",operation="ResetGraph")` binds it to neither a
/// label nor a query string).
async fn reset_graph(
    router: &MultiServiceRouter,
    id: &str,
    skip_snapshot: bool,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/graphs/{id}"),
        &[],
        Some(json!({"skipSnapshot": skip_snapshot})),
    )
    .await
}

/// `GET /summary` for one graph, which names the graph in the `graphIdentifier` HEADER.
async fn summary(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", "/summary", &[("graphIdentifier", id)], None).await
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

/// Create a graph (optionally encrypted, optionally delete-protected) and wait it into
/// service, returning the record `CreateGraph` reported.
async fn graph_in_service(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    kms_key: Option<&str>,
    deletion_protection: bool,
) -> Value {
    let mut body = json!({
        "graphName": name,
        "provisionedMemory": MIN_PROVISIONED_MEMORY,
        "deletionProtection": deletion_protection,
    });
    if let Some(key) = kms_key {
        body["kmsKeyIdentifier"] = json!(key);
    }
    let (status, created) = call(router, "POST", "/graphs", &[], Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph of {body}, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
    created
}

/// The id of a record `CreateGraph` (or any graph read) reported.
fn id_of(record: &Value) -> String {
    record["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {record}"))
        .to_string()
}

/// Write `nodes` and `edges` into the graph's stored data, the way the data plane
/// writes them.
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

/// The snapshot ids the store holds.
fn stored_snapshot_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the neptune-graph store is mounted");
    let ids: Vec<String> = state.lock().snapshots.keys().cloned().collect();
    ids
}

/// How many nodes and edges a graph's summary reports.
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

#[tokio::test]
async fn a_reset_empties_the_graphs_data_and_keeps_everything_else() {
    let (router, engine) = emulator();

    // The rows differ along what the rule generalises over: how much data the graph
    // holds when the reset arrives (none, one node, several nodes joined by edges),
    // both values of the required `skipSnapshot`, and whether the graph is encrypted
    // with a customer managed key (which a final snapshot inherits).
    let cases: &[ResetCase<'_>] = &[
        ("an empty graph, snapshot skipped", "empty-skip", &[], &[], true, None),
        (
            "an empty graph, final snapshot",
            "empty-snap",
            &[],
            &[],
            false,
            None,
        ),
        (
            "one node, snapshot skipped",
            "one-node-skip",
            &[("n1", &["Person"], &["name"])],
            &[],
            true,
            None,
        ),
        (
            "three nodes joined by two edges, final snapshot",
            "three-nodes-snap",
            &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person", "Author"], &[]),
                ("n3", &["Book"], &["title", "isbn"]),
            ],
            &[
                ("e1", "WROTE", "n2", "n3", &["year"]),
                ("e2", "KNOWS", "n1", "n2", &[]),
            ],
            false,
            None,
        ),
        (
            "an encrypted graph holding data, final snapshot",
            "encrypted-snap",
            &[("n1", &["Person"], &["name"]), ("n2", &["Person"], &["name"])],
            &[("e1", "KNOWS", "n1", "n2", &[])],
            false,
            Some(KMS_KEY_ARN),
        ),
    ];

    for (case, graph_name, nodes, edges, skip_snapshot, kms_key) in cases {
        let created = graph_in_service(&router, case, graph_name, *kms_key, false).await;
        let id = id_of(&created);
        seed(&engine, &id, nodes, edges);

        // What the graph holds before the reset, so the emptying below is a change this
        // call made rather than a graph that was empty all along.
        let seeded_nodes = i64::try_from(nodes.len()).expect("a seeded node count fits an i64");
        let seeded_edges = i64::try_from(edges.len()).expect("a seeded edge count fits an i64");
        let before_counts = reported_counts(&router, case, &id).await;
        assert_eq!(
            before_counts,
            (seeded_nodes, seeded_edges),
            "case {case:?}: expected the seeded data to be reported before the reset, \
             got {before_counts:?}"
        );
        let snapshots_before = stored_snapshot_ids(&engine);

        // The reset is accepted whichever value `skipSnapshot` carries, and reports the
        // graph coming back up on empty data.
        let (status, reset) = reset_graph(&router, &id, *skip_snapshot).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {reset}");
        assert_eq!(
            reset["status"],
            json!("RESETTING"),
            "case {case:?}: expected RESETTING, got {}",
            reset["status"]
        );
        // Everything that is not DATA survives the reset: the response is the same
        // record `CreateGraph` reported, bar the status it is now in.
        for member in [
            "id",
            "name",
            "arn",
            "createTime",
            "provisionedMemory",
            "endpoint",
            "publicConnectivity",
            "replicaCount",
            "kmsKeyIdentifier",
            "deletionProtection",
            "buildNumber",
        ] {
            assert_eq!(
                reset[member], created[member],
                "case {case:?}: expected {member} unchanged by the reset, got {} (was {})",
                reset[member], created[member]
            );
        }

        // A final snapshot is minted exactly when the request asked for one.
        let snapshots_after = stored_snapshot_ids(&engine);
        let minted: Vec<String> = snapshots_after
            .iter()
            .filter(|snapshot| !snapshots_before.contains(snapshot))
            .cloned()
            .collect();
        let expected_mints = usize::from(!*skip_snapshot);
        assert_eq!(
            minted.len(),
            expected_mints,
            "case {case:?}: expected {expected_mints} snapshot(s) minted, got {minted:?}"
        );

        // While the reset is in flight the graph has no serving data plane, so a summary
        // of it is refused, and a second reset conflicts with the one already running.
        let (summary_status, refused_summary) = summary(&router, &id).await;
        assert_eq!(
            summary_status, 400,
            "case {case:?}: a resetting graph has no data plane to summarise, \
             got {summary_status}: {refused_summary}"
        );
        assert_eq!(
            error_code(&refused_summary),
            "ValidationException",
            "case {case:?}: unexpected error code in {refused_summary}"
        );
        let (again_status, again) = reset_graph(&router, &id, true).await;
        assert_eq!(
            again_status, 409,
            "case {case:?}: a second reset of a resetting graph must conflict, \
             got {again_status}: {again}"
        );
        assert_eq!(
            error_code(&again),
            "ConflictException",
            "case {case:?}: unexpected error code in {again}"
        );

        // Once the transition the clock owes falls due the graph is serving again — and
        // it is serving NO data.
        let settled = wait_for_status(&router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
        assert_eq!(
            settled["id"],
            json!(id.clone()),
            "case {case:?}: expected the same graph after the reset, got {settled}"
        );
        assert_eq!(
            settled["name"], created["name"],
            "case {case:?}: expected the graph to keep its name, got {}",
            settled["name"]
        );
        let after_counts = reported_counts(&router, case, &id).await;
        assert_eq!(
            after_counts,
            (0, 0),
            "case {case:?}: expected no nodes and no edges after the reset, \
             got {after_counts:?}"
        );
        let (_, emptied) = summary(&router, &id).await;
        for (member, empty) in [
            ("nodeLabels", json!([])),
            ("edgeLabels", json!([])),
            ("nodeProperties", json!([])),
            ("edgeProperties", json!([])),
            ("numNodeLabels", json!(0)),
            ("numEdgeLabels", json!(0)),
            ("totalNodePropertyValues", json!(0)),
            ("totalEdgePropertyValues", json!(0)),
        ] {
            assert_eq!(
                emptied["graphSummary"][member], empty,
                "case {case:?}: expected {member} empty after the reset, got {}",
                emptied["graphSummary"][member]
            );
        }

        // The final snapshot holds the data as the reset FOUND it: it is named after the
        // graph, carries the graph's own key, and a graph restored from it comes up
        // holding the data the reset emptied.
        if let Some(snapshot_id) = minted.first() {
            let snapshot = wait_for_status(
                &router,
                case,
                &format!("/snapshots/{snapshot_id}"),
                "AVAILABLE",
            )
            .await;
            assert_eq!(
                snapshot["sourceGraphId"],
                json!(id.clone()),
                "case {case:?}: expected the reset graph as the source, got {}",
                snapshot["sourceGraphId"]
            );
            assert_eq!(
                snapshot["name"],
                json!(format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}")),
                "case {case:?}: expected the final snapshot named after the graph, got {}",
                snapshot["name"]
            );
            assert_eq!(
                snapshot["kmsKeyIdentifier"],
                kms_key.map_or_else(|| json!(SERVICE_OWNED_KMS_KEY), |key| json!(key)),
                "case {case:?}: expected the graph's own key on the snapshot, got {}",
                snapshot["kmsKeyIdentifier"]
            );
            let restored_name = format!("{graph_name}-restored");
            let (restore_status, restored) = call(
                &router,
                "POST",
                &format!("/snapshots/{snapshot_id}/restore"),
                &[],
                Some(json!({"graphName": restored_name})),
            )
            .await;
            assert_eq!(
                restore_status, 201,
                "case {case:?}: expected 201 from RestoreGraphFromSnapshot, \
                 got {restore_status}: {restored}"
            );
            let restored_id = id_of(&restored);
            wait_for_status(&router, case, &format!("/graphs/{restored_id}"), "AVAILABLE").await;
            let restored_counts = reported_counts(&router, case, &restored_id).await;
            assert_eq!(
                restored_counts,
                (seeded_nodes, seeded_edges),
                "case {case:?}: expected the final snapshot to hold the data the reset \
                 emptied, got {restored_counts:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_reset_takes_no_snapshot_and_leaves_the_data_alone() {
    let (router, engine) = emulator();

    // The data every graph below holds, so a refused reset can be shown to have emptied
    // nothing.
    let nodes: &[SeedNode<'_>] = &[
        ("n1", &["Person"], &["name"]),
        ("n2", &["Person"], &["name"]),
    ];
    let edges: &[SeedEdge<'_>] = &[("e1", "KNOWS", "n1", "n2", &["since"])];

    // A graph at rest but NOT serving: a stopped graph has no data plane to empty.
    let stopped = graph_in_service(&router, "stopped", "stopped-graph", None, false).await;
    let stopped_id = id_of(&stopped);
    seed(&engine, &stopped_id, nodes, edges);
    let (stop_status, stopping) =
        call(&router, "POST", &format!("/graphs/{stopped_id}/stop"), &[], None).await;
    assert_eq!(
        stop_status, 200,
        "case stopped: expected 200 from StopGraph, got {stop_status}: {stopping}"
    );
    wait_for_status(&router, "stopped", &format!("/graphs/{stopped_id}"), "STOPPED").await;

    // A graph that is still provisioning: a transition is in flight, so nothing may
    // start another. Created LAST, so its provisioning is still outstanding when the
    // refusals below run — the clock settles it on its own a fixed span later.
    let (creating_status, creating) = call(
        &router,
        "POST",
        "/graphs",
        &[],
        Some(json!({"graphName": "still-creating-graph", "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        creating_status, 201,
        "case still creating: expected 201, got {creating_status}: {creating}"
    );
    let creating_id = id_of(&creating);
    seed(&engine, &creating_id, nodes, edges);

    // (case, the graph the reset names, the status it must be refused with, the error
    // code it must be refused as). The rows differ along WHY the reset is refused.
    let cases: &[(&str, &str, u16, &str)] = &[
        (
            "a transition is still in flight",
            creating_id.as_str(),
            409,
            "ConflictException",
        ),
        (
            "the graph is stopped, so nothing serves its data",
            stopped_id.as_str(),
            409,
            "ConflictException",
        ),
        (
            "an identifier no graph holds",
            "g-0000000000",
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, identifier, expected_status, expected_code) in cases {
        let before = stored_snapshot_ids(&engine);
        // A refused reset that asked for a final snapshot must not leave one behind:
        // nothing was emptied, so nothing was snapshotted.
        let (status, body) = reset_graph(&router, identifier, false).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            stored_snapshot_ids(&engine),
            before,
            "case {case:?}: a refused reset must mint no snapshot"
        );
    }

    // And the data the refused resets named is untouched: the provisioning graph still
    // holds it once it is serving, and so does the stopped graph once it is started.
    wait_for_status(
        &router,
        "a transition is still in flight",
        &format!("/graphs/{creating_id}"),
        "AVAILABLE",
    )
    .await;
    let creating_counts =
        reported_counts(&router, "a transition is still in flight", &creating_id).await;
    assert_eq!(
        creating_counts,
        (2, 1),
        "case \"a transition is still in flight\": expected the seeded data to survive \
         the refused reset, got {creating_counts:?}"
    );

    let (start_status, starting) =
        call(&router, "POST", &format!("/graphs/{stopped_id}/start"), &[], None).await;
    assert_eq!(
        start_status, 200,
        "case stopped: expected 200 from StartGraph, got {start_status}: {starting}"
    );
    wait_for_status(&router, "stopped", &format!("/graphs/{stopped_id}"), "AVAILABLE").await;
    let stopped_counts = reported_counts(&router, "stopped", &stopped_id).await;
    assert_eq!(
        stopped_counts,
        (2, 1),
        "case \"stopped\": expected the seeded data to survive the refused reset, \
         got {stopped_counts:?}"
    );
}

#[tokio::test]
async fn deletion_protection_holds_back_a_delete_but_not_a_reset() {
    let (router, engine) = emulator();

    // Deletion protection is about the graph going AWAY: a reset keeps the graph and
    // empties only its data, so the flag does not hold it back — while the delete it is
    // about stays refused.
    let created = graph_in_service(&router, "protected", "protected-graph", None, true).await;
    let id = id_of(&created);
    seed(
        &engine,
        &id,
        &[("n1", &["Person"], &["name"])],
        &[] as &[SeedEdge<'_>],
    );

    let (status, reset) = reset_graph(&router, &id, true).await;
    assert_eq!(
        status, 200,
        "case protected: a reset of a delete-protected graph must be accepted, \
         got {status}: {reset}"
    );
    assert_eq!(
        reset["status"],
        json!("RESETTING"),
        "case protected: expected RESETTING, got {}",
        reset["status"]
    );
    assert_eq!(
        reset["deletionProtection"],
        json!(true),
        "case protected: expected the graph to keep its deletion protection, got {}",
        reset["deletionProtection"]
    );

    wait_for_status(&router, "protected", &format!("/graphs/{id}"), "AVAILABLE").await;
    let counts = reported_counts(&router, "protected", &id).await;
    assert_eq!(
        counts,
        (0, 0),
        "case protected: expected the data emptied by the reset, got {counts:?}"
    );

    // The flag still does what it is for.
    let (delete_status, refused) = call(
        &router,
        "DELETE",
        &format!("/graphs/{id}?skipSnapshot=true"),
        &[],
        None,
    )
    .await;
    assert_eq!(
        delete_status, 400,
        "case protected: expected 400 from DeleteGraph, got {delete_status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case protected: unexpected error code in {refused}"
    );

    // The graph is still there, resting in the status the reset's own transition settles
    // into — which is the condition that proves nothing else fell due behind it.
    let still_there = wait_for_status(&router, "protected", &format!("/graphs/{id}"), "AVAILABLE").await;
    assert_eq!(
        still_there["id"],
        json!(id),
        "case protected: expected the graph to survive, got {still_there}"
    );
    assert_eq!(
        still_there["status"],
        json!("AVAILABLE"),
        "case protected: expected AVAILABLE, got {}",
        still_there["status"]
    );
}
