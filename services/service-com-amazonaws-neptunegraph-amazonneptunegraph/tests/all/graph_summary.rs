// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetGraphSummary`, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! Every assertion is about a RULE rather than one recorded request: the summary of
//! a graph is COMPUTED from the nodes and edges that graph holds, so the table below
//! spans graphs that differ along everything the computation generalises over — the
//! number of nodes and edges, how many labels they carry, which property keys they
//! set, how often each key is used, and how those shapes group — and asserts the
//! whole report for each. `DETAILED` adds the two structure members the model marks
//! as `DETAILED`-only, and a graph whose data plane is not serving is refused.
//!
//! The data is seeded through the store's own writers (`crate::state::GraphData`),
//! which is where a bulk load and a restore write it: the openCypher engine behind
//! `ExecuteQuery` evaluates read-only queries, so a query cannot write it, and a
//! summary must report the data the store holds however it got there.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphEdge, GraphNode, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// One node of a seeded graph: its id, its labels, and the property keys set on it.
type SeedNode<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// One edge of a seeded graph: its id, its label, the ids of the nodes it leaves
/// and enters, and the property keys set on it.
type SeedEdge<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [&'a str]);

/// One row of the `DETAILED` table: the case, the nodes and the edges to seed, and
/// the `nodeStructures` and `edgeStructures` the report must then carry.
type DetailedCase<'a> = (&'a str, &'a [SeedNode<'a>], &'a [SeedEdge<'a>], Value, Value);

/// An emulator holding only this service, on its own engine, so each test starts
/// from an empty store. The engine is handed back as well, so a test can seed a
/// graph's data the way the data plane writes it.
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

/// `GET /summary` for one graph, with `mode` when a case asks for one. The graph is
/// named in the `graphIdentifier` HEADER
/// (`smithy_query(query="operation",operation="GetGraphSummary")` -> input).
async fn summary(router: &MultiServiceRouter, graph_id: &str, mode: Option<&str>) -> (u16, Value) {
    let path = match mode {
        Some(mode) => format!("/summary?mode={mode}"),
        None => "/summary".to_string(),
    };
    call(router, "GET", &path, &[("graphIdentifier", graph_id)], None).await
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

/// Create a graph, wait out the provisioning the clock owes it, and return its id.
async fn available_graph(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        &[],
        Some(json!({"graphName": name, "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(status, 201, "case {name}: expected 201, got {status}: {created}");
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no id in {created}"))
        .to_string();
    for _ in 0..20 {
        let (status, graph) = call(router, "GET", &format!("/graphs/{id}"), &[], None).await;
        assert_eq!(status, 200, "case {name}: expected 200 from GetGraph, got {status}: {graph}");
        if graph["status"] == json!("AVAILABLE") {
            return id;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {name}: the graph never became AVAILABLE");
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

#[tokio::test]
async fn basic_summary_is_computed_from_the_nodes_and_edges_the_graph_holds() {
    // (case, nodes, edges, the whole graphSummary a BASIC request must report).
    // The rows differ along what the computation generalises over: the counts, the
    // number of labels one element carries, which property keys are set, and how
    // many elements share a key.
    let cases: &[(&str, &[SeedNode<'_>], &[SeedEdge<'_>], Value)] = &[
        (
            "an empty graph",
            &[],
            &[],
            json!({
                "numNodes": 0,
                "numEdges": 0,
                "numNodeLabels": 0,
                "numEdgeLabels": 0,
                "nodeLabels": [],
                "edgeLabels": [],
                "numNodeProperties": 0,
                "numEdgeProperties": 0,
                "nodeProperties": [],
                "edgeProperties": [],
                "totalNodePropertyValues": 0,
                "totalEdgePropertyValues": 0,
            }),
        ),
        (
            "one unlabelled node with no properties",
            &[("n1", &[], &[])],
            &[],
            json!({
                "numNodes": 1,
                "numEdges": 0,
                "numNodeLabels": 0,
                "numEdgeLabels": 0,
                "nodeLabels": [],
                "edgeLabels": [],
                "numNodeProperties": 0,
                "numEdgeProperties": 0,
                "nodeProperties": [],
                "edgeProperties": [],
                "totalNodePropertyValues": 0,
                "totalEdgePropertyValues": 0,
            }),
        ),
        (
            "two nodes sharing a label, one of them carrying two labels",
            &[
                ("n1", &["Person"], &["name", "age"]),
                ("n2", &["Person", "Author"], &["name"]),
            ],
            &[],
            json!({
                "numNodes": 2,
                "numEdges": 0,
                "numNodeLabels": 2,
                "numEdgeLabels": 0,
                "nodeLabels": ["Author", "Person"],
                "edgeLabels": [],
                "numNodeProperties": 2,
                "numEdgeProperties": 0,
                "nodeProperties": [{"age": 1}, {"name": 2}],
                "edgeProperties": [],
                "totalNodePropertyValues": 3,
                "totalEdgePropertyValues": 0,
            }),
        ),
        (
            "three nodes joined by edges of two labels",
            &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person"], &["name"]),
                ("n3", &["City"], &[]),
            ],
            &[
                ("e1", "KNOWS", "n1", "n2", &["since"]),
                ("e2", "LIVES_IN", "n1", "n3", &[]),
                ("e3", "LIVES_IN", "n2", "n3", &["since"]),
            ],
            json!({
                "numNodes": 3,
                "numEdges": 3,
                "numNodeLabels": 2,
                "numEdgeLabels": 2,
                "nodeLabels": ["City", "Person"],
                "edgeLabels": ["KNOWS", "LIVES_IN"],
                "numNodeProperties": 1,
                "numEdgeProperties": 1,
                "nodeProperties": [{"name": 2}],
                "edgeProperties": [{"since": 2}],
                "totalNodePropertyValues": 2,
                "totalEdgePropertyValues": 2,
            }),
        ),
        (
            "a graph whose every element carries every property",
            &[
                ("n1", &["Thing"], &["a", "b"]),
                ("n2", &["Thing"], &["a", "b"]),
                ("n3", &["Thing"], &["a", "b"]),
            ],
            &[
                ("e1", "LINKS", "n1", "n2", &["w"]),
                ("e2", "LINKS", "n2", "n3", &["w"]),
            ],
            json!({
                "numNodes": 3,
                "numEdges": 2,
                "numNodeLabels": 1,
                "numEdgeLabels": 1,
                "nodeLabels": ["Thing"],
                "edgeLabels": ["LINKS"],
                "numNodeProperties": 2,
                "numEdgeProperties": 1,
                "nodeProperties": [{"a": 3}, {"b": 3}],
                "edgeProperties": [{"w": 2}],
                "totalNodePropertyValues": 6,
                "totalEdgePropertyValues": 2,
            }),
        ),
    ];

    for (index, (case, nodes, edges, expected)) in cases.iter().enumerate() {
        let (router, engine) = emulator();
        let graph_id = available_graph(&router, &format!("summary-graph-{index}")).await;
        seed(&engine, &graph_id, nodes, edges);

        let (status, body) = summary(&router, &graph_id, None).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            &body["graphSummary"], expected,
            "case {case:?}: expected {expected}, got {}",
            body["graphSummary"]
        );
        // BASIC is the default mode, so naming it explicitly reports the same thing.
        let (status, explicit) = summary(&router, &graph_id, Some("BASIC")).await;
        assert_eq!(status, 200, "case {case:?} as BASIC: expected 200, got {status}: {explicit}");
        assert_eq!(
            explicit["graphSummary"], body["graphSummary"],
            "case {case:?}: BASIC must report what no mode reports, got {} and {}",
            explicit["graphSummary"], body["graphSummary"]
        );
        // The structures belong to DETAILED only, so neither member is present.
        for member in ["nodeStructures", "edgeStructures"] {
            assert!(
                body["graphSummary"].get(member).is_none(),
                "case {case:?}: BASIC must not report {member}, got {}",
                body["graphSummary"]
            );
        }
    }
}

#[tokio::test]
async fn detailed_summary_adds_the_structures_the_elements_group_into() {
    // (case, nodes, edges, the nodeStructures and the edgeStructures DETAILED must
    // report). The rows differ along how the elements group: one shape per element,
    // several elements sharing a shape, and a graph with no elements to group.
    let cases: &[DetailedCase<'_>] = &[
        ("an empty graph", &[], &[], json!([]), json!([])),
        (
            "two nodes sharing a shape and one on its own",
            &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person"], &["name"]),
                ("n3", &["City"], &[]),
            ],
            &[
                ("e1", "LIVES_IN", "n1", "n3", &[]),
                ("e2", "LIVES_IN", "n2", "n3", &[]),
            ],
            // Commonest shape first: both Person nodes carry `name` and have one
            // outgoing LIVES_IN edge; the City node carries nothing and has none.
            json!([
                {"count": 2, "nodeProperties": ["name"], "distinctOutgoingEdgeLabels": ["LIVES_IN"]},
                {"count": 1, "nodeProperties": [], "distinctOutgoingEdgeLabels": []},
            ]),
            json!([{"count": 2, "edgeProperties": []}]),
        ),
        (
            "nodes with different outgoing edge labels",
            &[
                ("n1", &["Person"], &["name"]),
                ("n2", &["Person"], &["name"]),
                ("n3", &["City"], &[]),
            ],
            &[
                ("e1", "KNOWS", "n1", "n2", &["since"]),
                ("e2", "LIVES_IN", "n1", "n3", &[]),
                ("e3", "LIVES_IN", "n2", "n3", &["since"]),
            ],
            // Every node is its own shape, so the order is the shape order: the
            // node with no properties first, then the two `name` nodes by their
            // outgoing labels.
            json!([
                {"count": 1, "nodeProperties": [], "distinctOutgoingEdgeLabels": []},
                {
                    "count": 1,
                    "nodeProperties": ["name"],
                    "distinctOutgoingEdgeLabels": ["KNOWS", "LIVES_IN"],
                },
                {"count": 1, "nodeProperties": ["name"], "distinctOutgoingEdgeLabels": ["LIVES_IN"]},
            ]),
            json!([
                {"count": 2, "edgeProperties": ["since"]},
                {"count": 1, "edgeProperties": []},
            ]),
        ),
    ];

    for (index, (case, nodes, edges, node_structures, edge_structures)) in
        cases.iter().enumerate()
    {
        let (router, engine) = emulator();
        let graph_id = available_graph(&router, &format!("detailed-graph-{index}")).await;
        seed(&engine, &graph_id, nodes, edges);

        let (status, body) = summary(&router, &graph_id, Some("DETAILED")).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
        assert_eq!(
            &body["graphSummary"]["nodeStructures"], node_structures,
            "case {case:?}: expected nodeStructures {node_structures}, got {}",
            body["graphSummary"]["nodeStructures"]
        );
        assert_eq!(
            &body["graphSummary"]["edgeStructures"], edge_structures,
            "case {case:?}: expected edgeStructures {edge_structures}, got {}",
            body["graphSummary"]["edgeStructures"]
        );

        // DETAILED adds the structures to what BASIC reports and changes nothing
        // else.
        let (status, basic) = summary(&router, &graph_id, Some("BASIC")).await;
        assert_eq!(status, 200, "case {case:?} as BASIC: expected 200, got {status}: {basic}");
        let mut detailed_without_structures = body["graphSummary"].clone();
        for member in ["nodeStructures", "edgeStructures"] {
            detailed_without_structures
                .as_object_mut()
                .expect("the summary is a JSON object")
                .remove(member);
        }
        assert_eq!(
            detailed_without_structures, basic["graphSummary"],
            "case {case:?}: expected DETAILED to differ from BASIC only in the structures, \
             got {detailed_without_structures} and {}",
            basic["graphSummary"]
        );
    }
}

#[tokio::test]
async fn the_statistics_computation_time_moves_with_the_data_and_not_with_a_read() {
    let (router, engine) = emulator();
    let graph_id = available_graph(&router, "statistics-graph").await;

    let (status, first) = summary(&router, &graph_id, None).await;
    assert_eq!(status, 200, "case a fresh graph: expected 200, got {status}: {first}");
    // The framework renders every modelled `Timestamp` of this service the same way
    // (`GetGraph`'s `createTime` included), so what is asserted here is the VALUE:
    // the instant the statistics over the graph's data were computed.
    let as_created = first["lastStatisticsComputationTime"].clone();
    let created_at = as_created
        .as_f64()
        .unwrap_or_else(|| panic!("case a fresh graph: expected a timestamp, got {as_created}"));
    assert!(
        created_at > 0.0,
        "case a fresh graph: expected an instant past the epoch, got {created_at}"
    );
    assert!(
        first["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "case a fresh graph: expected the summary report version, got {}",
        first["version"]
    );

    // A read reports the same statistics: nothing about the graph changed.
    let (_, again) = summary(&router, &graph_id, Some("DETAILED")).await;
    assert_eq!(
        again["lastStatisticsComputationTime"], as_created,
        "case read twice: expected {as_created}, got {}",
        again["lastStatisticsComputationTime"]
    );

    // A write to the data recomputes them, so the reported time moves forward.
    seed(&engine, &graph_id, &[("n1", &["Person"], &["name"])], &[]);
    let (_, after_write) = summary(&router, &graph_id, None).await;
    let after = after_write["lastStatisticsComputationTime"].clone();
    let recomputed_at = after
        .as_f64()
        .unwrap_or_else(|| panic!("case after a write: expected a timestamp, got {after}"));
    assert!(
        recomputed_at > created_at,
        "case after a write: expected a time past {created_at}, got {recomputed_at}"
    );
    assert_eq!(
        after_write["graphSummary"]["numNodes"],
        json!(1),
        "case after a write: expected the written node to be counted, got {}",
        after_write["graphSummary"]
    );
}

#[tokio::test]
async fn a_summary_is_refused_for_a_graph_that_is_not_serving() {
    let (router, _engine) = emulator();

    // A graph nobody created names no data.
    let (status, body) = summary(&router, "g-0000000000", None).await;
    assert_eq!(status, 404, "case an unknown graph: expected 404, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case an unknown graph: got {body}"
    );

    // An identifier the modelled pattern refuses never reaches the store.
    let (status, body) = summary(&router, "not-a-graph-id", None).await;
    assert_eq!(status, 400, "case a malformed identifier: expected 400, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a malformed identifier: got {body}"
    );

    // (case, the status the graph is in when the summary is asked for) — a graph
    // that is still provisioning and one that has been stopped: in neither does the
    // graph's own data endpoint serve.
    let (status, created) = call(
        &router,
        "POST",
        "/graphs",
        &[],
        Some(json!({"graphName": "provisioning-graph", "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(status, 201, "case creating a graph: expected 201, got {status}: {created}");
    let provisioning = created["id"].as_str().expect("an id").to_string();
    let (status, body) = summary(&router, &provisioning, None).await;
    assert_eq!(
        status, 400,
        "case a graph that is still CREATING: expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a graph that is still CREATING: got {body}"
    );
    assert!(
        body["message"].as_str().unwrap_or_default().contains("CREATING"),
        "case a graph that is still CREATING: expected the status in the message, got {body}"
    );

    let stopped = available_graph(&router, "stopped-graph").await;
    let (status, body) = call(
        &router,
        "POST",
        &format!("/graphs/{stopped}/stop"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, 200, "case stopping a graph: expected 200, got {status}: {body}");
    for _ in 0..20 {
        let (_, graph) = call(&router, "GET", &format!("/graphs/{stopped}"), &[], None).await;
        if graph["status"] == json!("STOPPED") {
            break;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (status, body) = summary(&router, &stopped, None).await;
    assert_eq!(
        status, 400,
        "case a graph that is STOPPED: expected 400, got {status}: {body}"
    );
    assert!(
        body["message"].as_str().unwrap_or_default().contains("STOPPED"),
        "case a graph that is STOPPED: expected the status in the message, got {body}"
    );

    // A mode outside the modelled `GraphSummaryMode` value set is refused.
    let available = available_graph(&router, "mode-graph").await;
    for mode in ["basic", "FULL", ""] {
        let (status, body) = summary(&router, &available, Some(mode)).await;
        assert_eq!(
            status, 400,
            "case mode {mode:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case mode {mode:?}: got {body}"
        );
    }
}
