// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the precondition EVERY operation on a graph's own data plane
//! applies, driven over the wire (`restJson1`) through the same router the emulator
//! serves from.
//!
//! The rule every assertion here is about: an operation the model reaches through the
//! graph's OWN endpoint — the host prefix `{graphIdentifier}.`
//! (`smithy_query(query="operation",operation="GetGraphSummary,ListQueries,GetQuery,CancelQuery")`
//! -> endpoint) — answers only while the graph it names is `AVAILABLE`, and in every
//! other status answers the modelled `ValidationException` (400) naming the graph and
//! the status that was read, having changed nothing. A graph that is not serving has no
//! data plane behind that endpoint, so an empty success would tell the caller the graph
//! is serving and idle — a different fact from "this graph is not serving, ask again
//! later".
//!
//! So the table below spans what that rule generalises over: every status this service
//! puts a graph into, crossed with every operation bound to that endpoint, rather than
//! re-running one recorded request. `tests/all/graph_summary.rs` pins what a summary of
//! a SERVING graph reports and `tests/all/query_listing.rs` what a listing of one
//! reports; this suite pins the gate in front of both.
//!
//! The operation that RUNS a query (`ExecuteQuery`) is on the same endpoint and applies
//! the same precondition, but AWS answers it with the modelled
//! `ResourceNotFoundException` (404) rather than this refusal — the request is addressed
//! to the per-graph endpoint as its HOST, and an endpoint that is not registered is a
//! resource that is missing — so it is driven by `tests/all/execute_query_data_plane.rs`
//! instead. The queries under test here are admitted through the state constructors a
//! query that is still in flight is held in; everything the assertions look at goes over
//! the wire.

use serde_json::{json, Value};

use super::graph_state_gate_messages::{graph_in_status, wait_for_status};
use super::query_cancellation::{
    admit_query, call, cancel_query, emulator, error_code, get_query, graph_in_service,
    termination_due,
};

/// The query admitted on every graph under test, so the two operations that address one
/// query by id would have a query to find if the gate let them look.
const QUERY_ID: &str = "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607";

/// The text of that query.
const QUERY_TEXT: &str = "MATCH (n) RETURN n LIMIT 10";

/// Every status this service puts a graph into, and whether a graph in it is SERVING.
///
/// Source for the status set: the modelled `GraphStatus` enum
/// (`smithy_query(query="shape",shape="GraphStatus")`) minus `FAILED`, which no path in
/// this service produces. Only `AVAILABLE` serves: in every other status the graph is
/// provisioning, reconfiguring, importing, snapshotting, starting, winding down,
/// stopped or on its way out, and nothing is behind its endpoint to answer.
const STATUSES: &[(&str, bool)] = &[
    ("CREATING", false),
    ("AVAILABLE", true),
    ("UPDATING", false),
    ("SNAPSHOTTING", false),
    ("IMPORTING", false),
    ("STOPPING", false),
    ("STOPPED", false),
    ("STARTING", false),
    ("RESETTING", false),
    ("DELETING", false),
];

/// The operations bound to the per-graph data-plane host prefix, as (operation, the
/// clause its refusal words, method, path), ordered so the one that WRITES comes last:
/// an accepted cancel changes the query the readers before it report.
///
/// Source: the operations' HTTP bindings and `endpoint` traits
/// (`smithy_query(query="operation",operation="GetGraphSummary,ListQueries,GetQuery,CancelQuery")`).
/// `ExecuteQuery` is bound to the same prefix and answers a graph that is not serving
/// with the modelled 404 (see the module docs), so it is not driven here.
const DATA_PLANE_OPERATIONS: &[(&str, &str, &str, &str)] = &[
    ("GetGraphSummary", "summarise", "GET", "/summary?mode=BASIC"),
    (
        "ListQueries",
        "list queries on",
        "GET",
        "/queries?maxResults=10",
    ),
    ("GetQuery", "report a query on", "GET", "/queries/{queryId}"),
    (
        "CancelQuery",
        "cancel a query on",
        "DELETE",
        "/queries/{queryId}",
    ),
];

/// The whole message a data-plane gate must carry, written out here rather than reached
/// from the service's own helper so the test states the expectation independently of the
/// code under test.
///
/// It names the graph as well as the status: the request addressed the graph as the HOST
/// it was sent to, and the sibling that already enforced this rule
/// (`GetGraphSummary`) words it this way.
fn expected_message(graph_id: &str, graph_status: &str, action: &str) -> String {
    format!("graph {graph_id} is in status {graph_status} and has no serving data plane to {action}")
}

/// Drive one data-plane operation against one graph.
async fn data_plane_call(
    router: &smithy_server_runtime::MultiServiceRouter,
    method: &str,
    path: &str,
    graph_id: &str,
) -> (u16, Value) {
    let path = path.replace("{queryId}", QUERY_ID);
    call(router, method, &path, Some(graph_id), None).await
}

#[tokio::test]
async fn every_data_plane_operation_is_refused_unless_the_graph_it_names_is_serving() {
    let (router, engine) = emulator();

    for (graph_status, serving) in STATUSES {
        let setup = format!("a graph in {graph_status}");
        let name = format!("plane-{}", graph_status.to_lowercase());
        let graph = graph_in_status(&router, &setup, graph_status, &name).await;
        // The query is admitted whatever the graph's status, so a refusal can only come
        // from the gate and never from an empty query store.
        admit_query(&engine, &graph, QUERY_ID, QUERY_TEXT, true);

        // Fired back to back with no wait: an in-flight transition lasts only until the
        // clock makes it due, and a refusal changes nothing, so one graph serves every
        // operation bound to its endpoint.
        for (operation, action, method, path) in DATA_PLANE_OPERATIONS {
            let case = format!("{operation} on a graph in {graph_status}");
            let (code, body) = data_plane_call(&router, method, path, &graph).await;
            if *serving {
                assert_eq!(
                    code, 200,
                    "case {case:?}: a serving graph answers its own data plane, \
                     got {code}: {body}"
                );
                continue;
            }
            assert_eq!(
                code, 400,
                "case {case:?}: expected 400, got {code}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: unexpected error code in {body}"
            );
            let actual = body["message"].as_str().unwrap_or_default();
            let wanted = expected_message(&graph, graph_status, action);
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected message {wanted:?}, got {actual:?}"
            );
            assert_eq!(
                body["reason"],
                json!("BAD_REQUEST"),
                "case {case:?}: expected the modelled reason BAD_REQUEST, got {body}"
            );
        }

        // None of the refusals touched the query they would not look at: only the
        // accepted cancel on the serving graph owes a termination.
        let owed = termination_due(&engine, &graph, QUERY_ID);
        assert_eq!(
            owed.is_some(),
            *serving,
            "case a graph in {graph_status}: a refused cancel must not stop the query, \
             got {owed:?}"
        );
    }
}

#[tokio::test]
async fn the_status_a_data_plane_refusal_names_is_the_one_the_graph_currently_reports() {
    let (router, engine) = emulator();

    // ONE request — ListQueries — against ONE graph, as the stored record moves: the
    // answer can only differ by the status the store holds.
    let (code, created) = call(
        &router,
        "POST",
        "/graphs",
        None,
        Some(json!({"graphName": "plane-tracking", "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(
        code, 201,
        "case creating the graph under test: expected 201, got {code}: {created}"
    );
    let graph = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case creating the graph under test: no id in {created}"))
        .to_string();

    // Provisioning: the plane is not up yet.
    let (code, body) = data_plane_call(&router, "GET", "/queries?maxResults=10", &graph).await;
    assert_eq!(
        code, 400,
        "case a provisioning graph: expected 400, got {code}: {body}"
    );
    let creating = body["message"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        creating,
        expected_message(&graph, "CREATING", "list queries on"),
        "case a provisioning graph: got {creating:?}"
    );

    // Serving: the same request is answered, with the queries in flight.
    wait_for_status(&router, "the graph under test", &graph, "AVAILABLE").await;
    admit_query(&engine, &graph, QUERY_ID, QUERY_TEXT, true);
    let (code, listed) = data_plane_call(&router, "GET", "/queries?maxResults=10", &graph).await;
    assert_eq!(
        code, 200,
        "case a serving graph: expected 200, got {code}: {listed}"
    );
    assert_eq!(
        listed["queries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case a serving graph: expected the admitted query to be listed, got {listed}"
    );

    // Winding down: the same request again, while the stop the clock owes is in flight.
    let (code, stopping) = call(&router, "POST", &format!("/graphs/{graph}/stop"), None, None).await;
    assert_eq!(
        code, 200,
        "case stopping the graph under test: expected 200, got {code}: {stopping}"
    );
    let (code, body) = data_plane_call(&router, "GET", "/queries?maxResults=10", &graph).await;
    assert_eq!(
        code, 400,
        "case a stopping graph: expected 400, got {code}: {body}"
    );
    let stopping_message = body["message"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        stopping_message,
        expected_message(&graph, "STOPPING", "list queries on"),
        "case a stopping graph: got {stopping_message:?}"
    );
    assert_ne!(
        stopping_message, creating,
        "case one request across three statuses: the status in the refusal must come \
         from the stored record, got {stopping_message:?} twice"
    );
}

#[tokio::test]
async fn the_gate_opens_again_once_the_graph_is_serving_and_a_refusal_changed_nothing() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "plane-reopened").await;
    admit_query(&engine, &graph, QUERY_ID, QUERY_TEXT, true);

    // A reconfiguration takes the plane down for the span the clock owes it.
    let (code, updating) = call(
        &router,
        "PATCH",
        &format!("/graphs/{graph}"),
        None,
        Some(json!({"publicConnectivity": true})),
    )
    .await;
    assert_eq!(
        code, 200,
        "case reconfiguring the graph: expected 200, got {code}: {updating}"
    );
    assert_eq!(
        updating["status"],
        json!("UPDATING"),
        "case reconfiguring the graph: expected UPDATING, got {}",
        updating["status"]
    );

    for (operation, _, method, path) in DATA_PLANE_OPERATIONS {
        let (code, body) = data_plane_call(&router, method, path, &graph).await;
        assert_eq!(
            code, 400,
            "case {operation} while the graph reconfigures: expected 400, got {code}: {body}"
        );
    }

    // Once the transition the clock owes falls due the graph serves again, so the same
    // four requests are answered — and the query the refused cancel never stopped is
    // still a query the plane reports.
    wait_for_status(&router, "the reconfigured graph", &graph, "AVAILABLE").await;
    admit_query(&engine, &graph, QUERY_ID, QUERY_TEXT, true);
    for (operation, _, method, path) in DATA_PLANE_OPERATIONS {
        let (code, body) = data_plane_call(&router, method, path, &graph).await;
        assert_eq!(
            code, 200,
            "case {operation} once the graph serves again: expected 200, got {code}: {body}"
        );
    }

    // The last of those four was the cancel, which is accepted now that the plane is
    // up: the query reports the cancellation in flight rather than having been stopped
    // by a refused request.
    let (code, reported) = get_query(&router, &graph, QUERY_ID).await;
    assert_eq!(
        code, 200,
        "case the query after an accepted cancel: expected 200, got {code}: {reported}"
    );
    assert_eq!(
        reported["state"],
        json!("CANCELLING"),
        "case the query after an accepted cancel: expected CANCELLING, got {}",
        reported["state"]
    );
    let (code, repeated) = cancel_query(&router, &graph, QUERY_ID).await;
    assert_eq!(
        code, 200,
        "case a repeated cancel on a serving graph: expected 200, got {code}: {repeated}"
    );
}
