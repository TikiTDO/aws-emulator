// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE final-snapshot rule every operation that takes a final
//! snapshot reaches, driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! The rule: a request whose required `skipSnapshot` is `false` takes a FINAL snapshot
//! of the graph AS THE CALL FOUND IT — named after that graph, carrying its id, its
//! encryption key, the configuration it is provisioned with and the data it holds, and
//! reported `CREATING` until the clock makes its move to `AVAILABLE` due — and when the
//! snapshot store has no room for it the request is refused instead, in one wording,
//! with nothing minted and nothing emptied.
//!
//! Both operations that take one (`ResetGraph`, before the data is emptied, and
//! `DeleteGraph`, before the graph is torn down) go through the same store path, so each
//! table below drives BOTH of them over the same graph shape and asserts they agree:
//! the rows differ along what the rule generalises over — how much data the graph holds,
//! whether it is encrypted with a customer managed key, and the configuration it is
//! provisioned with (memory, replicas, public connectivity, vector search) — rather than
//! re-running one request.
//!
//! The data is seeded through the store's own writers (`crate::state::GraphData`), which
//! is where a bulk load and a restore write it: the openCypher engine behind
//! `ExecuteQuery` evaluates read-only queries, so a query cannot write it. The id of a
//! snapshot a
//! call minted is read out of the store, which is what proves exactly one was minted;
//! every assertion about that snapshot then goes over the wire through
//! `GetGraphSnapshot`, and what it captured of the graph is read back through
//! `RestoreGraphFromSnapshot`.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphEdge, GraphNode, FINAL_SNAPSHOT_SUFFIX, MAX_SNAPSHOTS, PROVISIONING_SPAN,
    SERVICE_OWNED_KMS_KEY,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The replica count a restored graph reports when the restore request carries none,
/// from the member's own documentation: "The number of replicas in other AZs. Min =0,
/// Max = 2, Default =1"
/// (`smithy_query(query="doc",shape="RestoreGraphFromSnapshotInput",
/// member="replicaCount")`).
const DOCUMENTED_DEFAULT_REPLICA_COUNT: i64 = 1;

/// One node of a seeded graph: its id, its labels, and the property keys set on it.
type SeedNode<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// One edge of a seeded graph: its id, its label, the ids of the nodes it leaves and
/// enters, and the property keys set on it.
type SeedEdge<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [&'a str]);

/// One row of the final-snapshot table: the graph shape both operations that take a
/// final snapshot are driven over.
struct FinalSnapshotCase<'a> {
    /// What this row is about, named in every assertion of it.
    case: &'a str,
    /// The stem both graphs of this row are named from.
    stem: &'a str,
    nodes: &'a [SeedNode<'a>],
    edges: &'a [SeedEdge<'a>],
    /// The customer managed key the graph is created with, or `None` for a graph on the
    /// service-owned key.
    kms_key: Option<&'a str>,
    /// `provisionedMemory`, within the modelled `range 16..24576`.
    provisioned_memory: i64,
    /// `replicaCount`, within the modelled `range 0..2`.
    replica_count: i64,
    public_connectivity: bool,
    /// `vectorSearchConfiguration.dimension`, within the documented maximum of 65535
    /// (`smithy_query(query="doc",shape="VectorSearchConfiguration")`), or `None` for a
    /// graph that indexes no embeddings.
    vector_dimension: Option<i64>,
}

/// An emulator holding only this service, with the engine it was built on, so a test can
/// seed a graph's data the way the data plane writes it and read the store for the id of
/// a resource no implemented list operation reports yet.
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

/// `GET /graphs/{id}`.
async fn get_graph(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
}

/// `PUT /graphs/{id}` with the required `skipSnapshot` in the body.
async fn reset_graph(router: &MultiServiceRouter, id: &str, skip_snapshot: bool) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/graphs/{id}"),
        Some(json!({"skipSnapshot": skip_snapshot})),
    )
    .await
}

/// `DELETE /graphs/{id}?skipSnapshot=<skip_snapshot>`.
async fn delete_graph(router: &MultiServiceRouter, id: &str, skip_snapshot: bool) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/graphs/{id}?skipSnapshot={skip_snapshot}"),
        None,
    )
    .await
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

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The id of a graph (or snapshot) record a response reported.
fn id_of(record: &Value) -> String {
    record["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {record}"))
        .to_string()
}

/// Whether `value` matches the modelled `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
fn is_snapshot_id(value: &str) -> bool {
    value.strip_prefix("gs-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
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

/// Create a graph with the row's configuration, wait it into service, and return the
/// record `CreateGraph` reported.
async fn graph_in_service(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    row: &FinalSnapshotCase<'_>,
) -> Value {
    let mut body = json!({
        "graphName": name,
        "provisionedMemory": row.provisioned_memory,
        "replicaCount": row.replica_count,
        "publicConnectivity": row.public_connectivity,
        "deletionProtection": false,
    });
    if let Some(key) = row.kms_key {
        body["kmsKeyIdentifier"] = json!(key);
    }
    if let Some(dimension) = row.vector_dimension {
        body["vectorSearchConfiguration"] = json!({"dimension": dimension});
    }
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph of {body}, got {status}: {created}"
    );
    let id = id_of(&created);
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
    created
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

/// The snapshot ids the store holds.
fn stored_snapshot_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the neptune-graph store is mounted");
    let ids: Vec<String> = state.lock().snapshots.keys().cloned().collect();
    ids
}

/// The one snapshot id `operation` added to the store, asserting it added exactly one.
fn one_minted_snapshot(
    case: &str,
    operation: &str,
    before: &[String],
    after: &[String],
) -> String {
    let minted: Vec<&String> = after.iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(
        minted.len(),
        1,
        "case {case:?}: expected {operation} to mint exactly one final snapshot, \
         got {minted:?}"
    );
    minted[0].clone()
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

/// Restore `snapshot_id` into a graph named `name`, wait it into service, and return the
/// record a read of it reports.
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
        "case {case:?}: expected 201 from RestoreGraphFromSnapshot of {snapshot_id}, \
         got {status}: {restored}"
    );
    let id = id_of(&restored);
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await
}

/// The members of a snapshot record that are the snapshot's OWN identity (so two
/// snapshots of two graphs differ in them), dropped so what is left is what the rule
/// says both final-snapshot paths capture alike.
fn without_own_identity(record: &Value) -> Value {
    let mut compared = record.clone();
    for member in ["id", "arn", "name", "sourceGraphId", "snapshotCreateTime"] {
        compared
            .as_object_mut()
            .expect("a snapshot record is a JSON object")
            .remove(member);
    }
    compared
}

#[tokio::test]
async fn both_final_snapshot_paths_capture_the_graph_the_same_way() {
    let (router, engine) = emulator();

    // The rows differ along what the rule generalises over: how much data the graph
    // holds when the call arrives (none, one node, several nodes joined by edges),
    // whether it is encrypted with a customer managed key, and the configuration it is
    // provisioned with — each published bound of `provisionedMemory` (`range 16..24576`)
    // and `replicaCount` (`range 0..2`) included.
    let rows: &[FinalSnapshotCase<'_>] = &[
        FinalSnapshotCase {
            case: "an empty graph at the smallest size",
            stem: "empty",
            nodes: &[],
            edges: &[],
            kms_key: None,
            provisioned_memory: 16,
            replica_count: 0,
            public_connectivity: false,
            vector_dimension: None,
        },
        FinalSnapshotCase {
            case: "one node, public, one replica",
            stem: "one-node",
            nodes: &[("n1", &["Person"], &["name"])],
            edges: &[],
            kms_key: None,
            provisioned_memory: 32,
            replica_count: 1,
            public_connectivity: true,
            vector_dimension: None,
        },
        FinalSnapshotCase {
            case: "three nodes joined by two edges, vector search, two replicas",
            stem: "three-nodes",
            nodes: &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person", "Author"], &[]),
                ("n3", &["Book"], &["title", "isbn"]),
            ],
            edges: &[
                ("e1", "WROTE", "n2", "n3", &["year"]),
                ("e2", "KNOWS", "n1", "n2", &[]),
            ],
            kms_key: None,
            provisioned_memory: 128,
            replica_count: 2,
            public_connectivity: false,
            vector_dimension: Some(1),
        },
        FinalSnapshotCase {
            case: "an encrypted graph holding data at the largest size",
            stem: "encrypted",
            nodes: &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person"], &["name"]),
            ],
            edges: &[("e1", "KNOWS", "n1", "n2", &[])],
            kms_key: Some(KMS_KEY_ARN),
            provisioned_memory: 24576,
            replica_count: 0,
            public_connectivity: true,
            vector_dimension: Some(65535),
        },
    ];

    for row in rows {
        let case = row.case;
        // Two graphs of the SAME shape, one per operation that takes a final snapshot,
        // so the two snapshots can be compared member for member.
        let reset_name = format!("{}-reset", row.stem);
        let delete_name = format!("{}-deleted", row.stem);
        let reset_record = graph_in_service(&router, case, &reset_name, row).await;
        let delete_record = graph_in_service(&router, case, &delete_name, row).await;
        let reset_id = id_of(&reset_record);
        let delete_id = id_of(&delete_record);
        seed(&engine, &reset_id, row.nodes, row.edges);
        seed(&engine, &delete_id, row.nodes, row.edges);
        let seeded_nodes = i64::try_from(row.nodes.len()).expect("a node count fits an i64");
        let seeded_edges = i64::try_from(row.edges.len()).expect("an edge count fits an i64");
        let before_counts = reported_counts(&router, case, &reset_id).await;
        assert_eq!(
            before_counts,
            (seeded_nodes, seeded_edges),
            "case {case:?}: expected the seeded data to be reported before the reset, \
             got {before_counts:?}"
        );

        // Each operation is asked for a final snapshot, and each is accepted.
        let before_reset = stored_snapshot_ids(&engine);
        let (reset_status, reset) = reset_graph(&router, &reset_id, false).await;
        assert_eq!(
            reset_status, 200,
            "case {case:?}: expected 200 from ResetGraph, got {reset_status}: {reset}"
        );
        assert_eq!(
            reset["status"],
            json!("RESETTING"),
            "case {case:?}: expected RESETTING from ResetGraph, got {}",
            reset["status"]
        );
        let after_reset = stored_snapshot_ids(&engine);
        let from_reset_id = one_minted_snapshot(case, "ResetGraph", &before_reset, &after_reset);

        let (delete_status, deleted) = delete_graph(&router, &delete_id, false).await;
        assert_eq!(
            delete_status, 200,
            "case {case:?}: expected 200 from DeleteGraph, got {delete_status}: {deleted}"
        );
        assert_eq!(
            deleted["status"],
            json!("DELETING"),
            "case {case:?}: expected DELETING from DeleteGraph, got {}",
            deleted["status"]
        );
        let after_delete = stored_snapshot_ids(&engine);
        let from_delete_id = one_minted_snapshot(case, "DeleteGraph", &after_reset, &after_delete);

        // Each minted snapshot becomes restorable on its own, a fixed span after the
        // call that took it.
        let from_reset = wait_for_status(
            &router,
            case,
            &format!("/snapshots/{from_reset_id}"),
            "AVAILABLE",
        )
        .await;
        let from_delete = wait_for_status(
            &router,
            case,
            &format!("/snapshots/{from_delete_id}"),
            "AVAILABLE",
        )
        .await;

        // Every member each snapshot carries of the graph it was taken of: its id, the
        // name derived from the graph's, the graph's own encryption key, and an ARN that
        // is the graph's with the snapshot as the resource.
        let expected_key = row
            .kms_key
            .map_or_else(|| json!(SERVICE_OWNED_KMS_KEY), |key| json!(key));
        for (operation, graph, graph_name, snapshot_id, snapshot) in [
            (
                "ResetGraph",
                &reset_record,
                &reset_name,
                &from_reset_id,
                &from_reset,
            ),
            (
                "DeleteGraph",
                &delete_record,
                &delete_name,
                &from_delete_id,
                &from_delete,
            ),
        ] {
            let graph_id = id_of(graph);
            assert!(
                is_snapshot_id(snapshot_id),
                "case {case:?}: expected {operation} to mint a ^gs-[a-z0-9]{{10}}$ id, \
                 got {snapshot_id:?}"
            );
            assert_eq!(
                snapshot["id"],
                json!(snapshot_id),
                "case {case:?}: expected {operation}'s snapshot to answer to its own id, \
                 got {}",
                snapshot["id"]
            );
            assert_eq!(
                snapshot["name"],
                json!(format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}")),
                "case {case:?}: expected {operation}'s snapshot named after the graph, \
                 got {}",
                snapshot["name"]
            );
            assert_eq!(
                snapshot["sourceGraphId"],
                json!(graph_id),
                "case {case:?}: expected {operation}'s snapshot to name its source graph, \
                 got {}",
                snapshot["sourceGraphId"]
            );
            assert_eq!(
                snapshot["kmsKeyIdentifier"], expected_key,
                "case {case:?}: expected {operation}'s snapshot on the graph's own key, \
                 got {}",
                snapshot["kmsKeyIdentifier"]
            );
            let graph_arn = graph["arn"].as_str().unwrap_or_default();
            let expected_arn = graph_arn.replace(
                &format!("graph/{graph_id}"),
                &format!("graph-snapshot/{snapshot_id}"),
            );
            assert_eq!(
                snapshot["arn"],
                json!(expected_arn),
                "case {case:?}: expected {operation}'s snapshot ARN in the graph's \
                 account and Region, got {}",
                snapshot["arn"]
            );
            assert!(
                snapshot["snapshotCreateTime"].is_number(),
                "case {case:?}: expected {operation}'s snapshot to report when it was \
                 taken, got {}",
                snapshot["snapshotCreateTime"]
            );
        }

        // Beyond their own identity the two records are the SAME record: one rule minted
        // both.
        assert_eq!(
            without_own_identity(&from_reset),
            without_own_identity(&from_delete),
            "case {case:?}: expected the reset's and the delete's final snapshots to \
             agree, got {from_reset} and {from_delete}"
        );

        // What each snapshot captured of its graph, read back by restoring it: the
        // configuration the graph was provisioned with and the data it held when the
        // call found it.
        let from_reset_graph = restored_graph(
            &router,
            case,
            &from_reset_id,
            &format!("{}-from-reset", row.stem),
        )
        .await;
        let from_delete_graph = restored_graph(
            &router,
            case,
            &from_delete_id,
            &format!("{}-from-delete", row.stem),
        )
        .await;
        for (operation, source, restored, snapshot_id) in [
            ("ResetGraph", &reset_record, &from_reset_graph, &from_reset_id),
            (
                "DeleteGraph",
                &delete_record,
                &from_delete_graph,
                &from_delete_id,
            ),
        ] {
            for member in [
                "provisionedMemory",
                "publicConnectivity",
                "deletionProtection",
                "kmsKeyIdentifier",
                "vectorSearchConfiguration",
            ] {
                assert_eq!(
                    restored[member], source[member],
                    "case {case:?}: expected {operation}'s final snapshot to capture \
                     {member} as the call found it, got {} (the graph had {})",
                    restored[member], source[member]
                );
            }
            // `replicaCount` is the one member of a restore input whose own model
            // documentation states a default for an omitted value ("Min =0, Max = 2,
            // Default =1"), and this restore omits it, so the documented default
            // answers for it rather than the count the snapshot captured
            // (`documented_member_defaults.rs` spans that rule).
            assert_eq!(
                restored["replicaCount"],
                json!(DOCUMENTED_DEFAULT_REPLICA_COUNT),
                "case {case:?}: expected the graph restored from {operation}'s final \
                 snapshot to report the documented default \
                 {DOCUMENTED_DEFAULT_REPLICA_COUNT} replicas for the member the restore \
                 omitted, got {}",
                restored["replicaCount"]
            );
            assert_eq!(
                restored["sourceSnapshotId"],
                json!(snapshot_id),
                "case {case:?}: expected the restored graph to name {operation}'s \
                 snapshot, got {}",
                restored["sourceSnapshotId"]
            );
            let restored_counts = reported_counts(&router, case, &id_of(restored)).await;
            assert_eq!(
                restored_counts,
                (seeded_nodes, seeded_edges),
                "case {case:?}: expected {operation}'s final snapshot to hold the data \
                 the graph had, got {restored_counts:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_full_snapshot_store_refuses_every_final_snapshot_alike() {
    let (router, engine) = emulator();
    let row = FinalSnapshotCase {
        case: "a full snapshot store",
        stem: "full",
        nodes: &[
            ("n1", &["Person"], &["name"]),
            ("n2", &["Person"], &["name"]),
        ],
        edges: &[("e1", "KNOWS", "n1", "n2", &["since"])],
        kms_key: None,
        provisioned_memory: 16,
        replica_count: 0,
        public_connectivity: false,
        vector_dimension: None,
    };
    let case = row.case;

    let filler = graph_in_service(&router, case, "filler", &row).await;
    let reset_record = graph_in_service(&router, case, "full-reset", &row).await;
    let delete_record = graph_in_service(&router, case, "full-deleted", &row).await;
    let reset_id = id_of(&reset_record);
    let delete_id = id_of(&delete_record);
    seed(&engine, &reset_id, row.nodes, row.edges);
    seed(&engine, &delete_id, row.nodes, row.edges);

    // Fill the snapshot store to its ceiling through the store's own final-snapshot
    // path, so what the two operations below meet is a store with no room left.
    {
        let state = service_state(&*engine).expect("the neptune-graph store is mounted");
        let mut store = state.lock();
        let filler_graph = store
            .graphs
            .get(&id_of(&filler))
            .expect("the filler graph was created")
            .clone();
        let now = Instant::now();
        while store.snapshots.len() < MAX_SNAPSHOTS {
            store
                .mint_final_snapshot(&filler_graph, REGION, now)
                .expect("a store under its ceiling has room for one more snapshot");
        }
    }
    let full = stored_snapshot_ids(&engine);
    assert_eq!(
        full.len(),
        MAX_SNAPSHOTS,
        "case {case:?}: expected the store filled to its ceiling, got {} snapshots",
        full.len()
    );

    // Neither operation can take the final snapshot it was asked for, and each reports
    // the one refusal the rule states — through its own modelled error channel, naming
    // its own graph and the ceiling it met.
    let (reset_status, refused_reset) = reset_graph(&router, &reset_id, false).await;
    let (delete_status, refused_delete) = delete_graph(&router, &delete_id, false).await;
    for (operation, status, body, graph_id) in [
        ("ResetGraph", reset_status, &refused_reset, &reset_id),
        ("DeleteGraph", delete_status, &refused_delete, &delete_id),
    ] {
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 from {operation}, got {status}: {body}"
        );
        assert_eq!(
            error_code(body),
            "ConflictException",
            "case {case:?}: unexpected error code from {operation} in {body}"
        );
        let refusal = message(body);
        assert!(
            refusal.contains(graph_id.as_str()),
            "case {case:?}: expected {operation}'s refusal to name graph {graph_id}, \
             got {refusal:?}"
        );
        assert!(
            refusal.contains(&MAX_SNAPSHOTS.to_string()),
            "case {case:?}: expected {operation}'s refusal to name the ceiling \
             {MAX_SNAPSHOTS}, got {refusal:?}"
        );
    }
    let reset_refusal = message(&refused_reset).replace(reset_id.as_str(), "<graph>");
    let delete_refusal = message(&refused_delete).replace(delete_id.as_str(), "<graph>");
    assert_eq!(
        reset_refusal, delete_refusal,
        "case {case:?}: expected one wording for a final snapshot there is no room for, \
         got {reset_refusal:?} and {delete_refusal:?}"
    );

    // A refused call changed nothing: no snapshot was minted, and both graphs are still
    // serving the data they held.
    let after = stored_snapshot_ids(&engine);
    assert_eq!(
        after, full,
        "case {case:?}: expected a refused final snapshot to mint nothing, got {} \
         snapshots",
        after.len()
    );
    for (operation, graph_id) in [("ResetGraph", &reset_id), ("DeleteGraph", &delete_id)] {
        let (status, body) = get_graph(&router, graph_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading the graph {operation} refused, \
             got {status}: {body}"
        );
        assert_eq!(
            body["status"],
            json!("AVAILABLE"),
            "case {case:?}: expected the graph {operation} refused to be serving still, \
             got {}",
            body["status"]
        );
        let counts = reported_counts(&router, case, graph_id).await;
        assert_eq!(
            counts,
            (2, 1),
            "case {case:?}: expected the graph {operation} refused to keep its data, \
             got {counts:?}"
        );
    }

    // The refusal is about the snapshot, not the operation: the same request that skips
    // the final snapshot is accepted against the same full store.
    let (skipped_reset_status, skipped_reset) = reset_graph(&router, &reset_id, true).await;
    assert_eq!(
        skipped_reset_status, 200,
        "case {case:?}: expected 200 from ResetGraph with skipSnapshot=true, \
         got {skipped_reset_status}: {skipped_reset}"
    );
    let (skipped_delete_status, skipped_delete) = delete_graph(&router, &delete_id, true).await;
    assert_eq!(
        skipped_delete_status, 200,
        "case {case:?}: expected 200 from DeleteGraph with skipSnapshot=true, \
         got {skipped_delete_status}: {skipped_delete}"
    );
    assert_eq!(
        stored_snapshot_ids(&engine),
        full,
        "case {case:?}: expected a skipped final snapshot to mint nothing"
    );
}
