// SPDX-License-Identifier: CC0-1.0

//! WHICH error shape each refusal of this service carries, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a refusal is answered with the error shape,
//! status and SENTENCE the service AWS emulates answers that decision with, read from the
//! recorded response for that operation and decision — not with one house-wide grammar
//! applied to every precondition. Two decisions of this service are AWS's own sentences
//! rather than this crate's conflict grammar, and both are permanent input/configuration
//! failures (400 `ValidationException`) naming the graph they are about, where a
//! transition the caller only has to wait out stays the retryable 409:
//!
//! * `DeleteGraph` on a graph whose stored `deletionProtection` is set — a SETTING the
//!   caller clears with `UpdateGraph` ("Graphs cannot be deleted if delete-protection is
//!   enabled.", `smithy_query(query="doc",shape="DeleteGraph")`);
//! * `CreateGraphSnapshot` whose source graph is not in a status a snapshot is taken
//!   from.
//!
//! Both operations model `ValidationException` @httpError 400 beside
//! `ConflictException` 409
//! (`smithy_query(query="operation",operation="DeleteGraph,CreateGraphSnapshot")` ->
//! errors), so which one a decision carries is part of the answer: a client that retries
//! a 409 and treats a 400 as permanent must not be handed the retryable shape for a
//! condition no amount of waiting changes.
//!
//! So the tables below span what that rule generalises over rather than re-running one
//! request: the refusal is driven on several graphs (so the id the sentence names
//! varies), with and without the final snapshot a delete can ask for, and across every
//! status this service puts a graph into — including the statuses that are NOT refused,
//! which is what keeps the rows measuring the shape of a refusal rather than a blanket
//! one. `tests/all/precondition_refusal_grammar.rs` pins the 409 grammar every other
//! precondition of this service words.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphStatus, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`).
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An emulator holding only this service, with the engine it was built on, so a test can
/// park a graph in a status the clock owes nothing out of. Each test starts from an empty
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

/// Create a graph and wait until it reports `AVAILABLE`, returning its id.
///
/// It waits for the CONDITION a read reports rather than for a span, with a deadline
/// generous beside the one the provisioning transition is owed on.
async fn graph_in_service(router: &MultiServiceRouter, name: &str, protected: bool) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({
            "graphName": name,
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
            "deletionProtection": protected,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no id in {created}"))
        .to_string();
    for _ in 0..40 {
        let (get_status, graph) = call(router, "GET", &format!("/graphs/{id}"), None).await;
        assert_eq!(
            get_status, 200,
            "case {name:?}: expected 200 from GetGraph, got {get_status}: {graph}"
        );
        if graph["status"] == json!("AVAILABLE") {
            return id;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {name:?}: the graph never reached AVAILABLE");
}

/// Park `graph_id` in `status` with nothing owed out of it, so a row's answer cannot
/// depend on how long the call took to arrive.
fn park(engine: &Arc<engine::InMemoryEngine>, case: &str, graph_id: &str, status: GraphStatus) {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let mut store = state.lock();
    let graph = store
        .graphs
        .get_mut(graph_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no graph {graph_id}"));
    graph.status = status;
    graph.pending = None;
    graph.deletes_at = None;
}

/// The sentence a delete held back by deletion protection carries, written out here
/// rather than reached from the service's own builder so the test states the expectation
/// independently of the code under test.
fn protected_message(graph_id: &str) -> String {
    format!("Graph [{graph_id}] cannot be deleted because deletion protection is enabled.")
}

/// The sentence a snapshot of a graph that is not in a status it can be taken from
/// carries.
fn unsnapshottable_message(graph_id: &str) -> String {
    format!(
        "Graph [{graph_id}] is not available for snapshot. You can only create a snapshot \
         for an AVAILABLE Graph."
    )
}

/// Assert one refused call: the modelled status, the error shape and the whole sentence.
fn assert_refused(case: &str, code: u16, body: &Value, expected: &str) {
    assert_eq!(code, 400, "case {case:?}: expected 400, got {code}: {body}");
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: unexpected error code in {body}"
    );
    let actual = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn deletion_protection_refuses_the_delete_as_a_configuration_problem() {
    let (router, engine) = emulator();

    // (case, the graph's name, whether the delete asks for a final snapshot). The rows
    // differ along the graph the refusal is about — so the id the sentence names varies —
    // and along whether the refused delete was also asking for a snapshot, which it must
    // not have minted.
    let rows: &[(&str, &str, bool)] = &[
        ("a delete skipping the final snapshot", "protected-one", true),
        ("a delete asking for a final snapshot", "protected-two", false),
        ("another protected graph", "protected-three", true),
    ];

    for (case, name, skip_snapshot) in rows {
        let graph_id = graph_in_service(&router, name, true).await;
        let (code, body) = call(
            &router,
            "DELETE",
            &format!("/graphs/{graph_id}?skipSnapshot={skip_snapshot}"),
            None,
        )
        .await;
        assert_refused(case, code, &body, &protected_message(&graph_id));

        // The graph is left intact: still readable, still serving, still protected.
        let (read_code, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            read_code, 200,
            "case {case:?}: expected the graph to survive the refusal, got {read_code}: \
             {graph}"
        );
        assert_eq!(
            (&graph["status"], &graph["deletionProtection"]),
            (&json!("AVAILABLE"), &json!(true)),
            "case {case:?}: expected an AVAILABLE, still protected graph, got {graph}"
        );
        // And no snapshot of it was taken, whatever the refused delete asked for.
        let (listed_code, listed) = call(&router, "GET", "/snapshots", None).await;
        assert_eq!(
            listed_code, 200,
            "case {case:?}: expected 200 from ListGraphSnapshots, got {listed_code}: \
             {listed}"
        );
        let snapshots = listed["graphSnapshots"].as_array().cloned().unwrap_or_default();
        assert!(
            snapshots.is_empty(),
            "case {case:?}: a refused delete must mint no snapshot, got {listed}"
        );

        // Clearing the setting is what lets the identical request through, so the refusal
        // was about the stored setting and nothing else.
        let (cleared, patched) = call(
            &router,
            "PATCH",
            &format!("/graphs/{graph_id}"),
            Some(json!({"deletionProtection": false})),
        )
        .await;
        assert_eq!(
            cleared, 200,
            "case {case:?}: expected 200 from UpdateGraph, got {cleared}: {patched}"
        );
        park(&engine, case, &graph_id, GraphStatus::Available);
        let (accepted, deleting) = call(
            &router,
            "DELETE",
            &format!("/graphs/{graph_id}?skipSnapshot=true"),
            None,
        )
        .await;
        assert_eq!(
            accepted, 200,
            "case {case:?}: expected 200 once the protection was cleared, got {accepted}: \
             {deleting}"
        );
        assert_eq!(
            deleting["status"],
            json!("DELETING"),
            "case {case:?}: expected the accepted delete to report DELETING, got {deleting}"
        );
    }
}

#[tokio::test]
async fn a_snapshot_of_a_graph_that_cannot_be_captured_is_refused_as_a_bad_request() {
    let (router, engine) = emulator();

    // Every status this service puts a graph into, and whether a snapshot of a graph in
    // it is SERVED: a graph at rest holds its data settled, while a transition still in
    // flight has that data in motion. Source for the status set: the modelled
    // `GraphStatus` enum (`smithy_query(query="shape",shape="GraphStatus")`) minus
    // `FAILED`, which no path in this service produces.
    let statuses: &[(&str, GraphStatus, bool)] = &[
        ("CREATING", GraphStatus::Creating, false),
        ("AVAILABLE", GraphStatus::Available, true),
        ("UPDATING", GraphStatus::Updating, false),
        ("SNAPSHOTTING", GraphStatus::Snapshotting, false),
        ("IMPORTING", GraphStatus::Importing, false),
        ("STOPPING", GraphStatus::Stopping, false),
        ("STOPPED", GraphStatus::Stopped, true),
        ("STARTING", GraphStatus::Starting, false),
        ("RESETTING", GraphStatus::Resetting, false),
        ("DELETING", GraphStatus::Deleting, false),
    ];

    let graph_id = graph_in_service(&router, "snapshot-source", false).await;
    for (wire, status, served) in statuses {
        let case = format!("a snapshot of a graph in {wire}");
        park(&engine, &case, &graph_id, *status);
        let snapshot_name = format!("snap-{}", wire.to_lowercase());
        let (code, body) = call(
            &router,
            "POST",
            "/snapshots",
            Some(json!({"graphIdentifier": graph_id, "snapshotName": snapshot_name})),
        )
        .await;
        if *served {
            assert_eq!(
                code, 201,
                "case {case:?}: a graph at rest can be captured, got {code}: {body}"
            );
            continue;
        }
        assert_refused(&case, code, &body, &unsnapshottable_message(&graph_id));

        // Nothing of the snapshot it asked for was minted: the name it named is still
        // free, and the listing holds no snapshot under it.
        let (listed_code, listed) = call(&router, "GET", "/snapshots", None).await;
        assert_eq!(
            listed_code, 200,
            "case {case:?}: expected 200 from ListGraphSnapshots, got {listed_code}: \
             {listed}"
        );
        let held: Vec<String> = listed["graphSnapshots"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|snapshot| snapshot["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !held.contains(&snapshot_name),
            "case {case:?}: a refused snapshot must mint nothing, got {held:?}"
        );
    }
}
