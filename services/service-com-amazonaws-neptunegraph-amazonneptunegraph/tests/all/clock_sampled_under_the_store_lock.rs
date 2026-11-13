// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHEN a handler of this service reads the clock while serving a
//! request, driven over the wire (`restJson1`) through the same router the emulator
//! serves from.
//!
//! The rule every assertion here is about: every lifecycle decision this service takes
//! compares an instant the handler supplies against a due instant the record holds, and
//! that instant is read with the store ALREADY HELD
//! (`state::locked_store_now`) — so a transition the clock made due while the handler was
//! on its way to the store has been settled, and a record whose reclamation fell due has
//! been reaped, before the handler reads or writes anything, however long it waited to
//! get there. A handler that sampled the clock before it took the store would answer —
//! and then write — the world as it stood when it STARTED waiting.
//!
//! What a client sees: a graph whose stop came due while the request waited is startable
//! for THAT request too, a graph whose deletion came due is gone for it (its name free
//! again, every reference to it resolving to nothing), and a query whose run span ran out
//! is no longer a query any operation reports.
//!
//! Every test below holds the store lock on a thread of its own for longer than the span
//! the clock owes the transition under test, so the request under test really does begin
//! before that transition falls due and reach the store after it has: the window is the
//! handler's own waiting, not the wall clock's, so nothing here is raced. The tables span
//! what the rule generalises over — each kind of transition this service owes (a stop, a
//! start, a provisioning, a reconfiguration, a snapshot, a reset, a deletion, a query's
//! timeout), and the reporting, writing and listing operations that read the store
//! afterwards — rather than re-running one recorded request.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ComAmazonawsNeptunegraphAmazonneptunegraphState as Store, DELETION_SPAN, PROVISIONING_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_cancellation::{
    admit_query_with_timeout, assert_invalid_query_id, call, emulator, error_code,
    graph_in_service, REGION,
};
use crate::resource_tagging::encode;

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), so no row
/// depends on the size of the graph it drives.
const SMALLEST_MEMORY: i64 = 16;

/// How much longer than the span under test the store lock is held, so the transition is
/// due well before the lock is released however loaded the machine is.
const MARGIN: Duration = Duration::from_millis(250);

/// How long to wait between polls while a status the clock owes settles.
const POLL: Duration = Duration::from_millis(50);

/// One request: a method, a path and an optional body.
///
/// `{graph}` in the path and in the body is replaced with the id of the graph under test,
/// and `{arn}` in the path with its ARN, percent-encoded because `resourceArn` is bound
/// to a non-greedy label.
struct Step {
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    /// Whether the graph under test is named in the `graphIdentifier` HEADER, which is
    /// where the model binds it for the query and summary operations.
    graph_header: bool,
}

/// A request that carries the graph in its path (or does not name one at all).
fn step(method: &'static str, path: &'static str, body: Option<Value>) -> Step {
    Step {
        method,
        path,
        body,
        graph_header: false,
    }
}

/// A request that carries the graph in the `graphIdentifier` header.
fn header_step(method: &'static str, path: &'static str, body: Option<Value>) -> Step {
    Step {
        method,
        path,
        body,
        graph_header: true,
    }
}

/// Hold this service's store lock for `hold` on a thread of its own, and answer the
/// handle that reports the instant the guard was dropped.
///
/// This makes a handler's own waiting explicit: a request issued once this returns really
/// does block on the store until the span has elapsed. It returns only once the lock is
/// held, so the request under test cannot slip in ahead of it.
fn hold_store_lock(
    engine: &Arc<engine::InMemoryEngine>,
    hold: Duration,
) -> thread::JoinHandle<Instant> {
    let state = engine.state::<Store>();
    let (acquired, held) = mpsc::channel();
    let holder = thread::spawn(move || {
        let guard = state.lock();
        acquired
            .send(())
            .expect("the test waits for the store lock to be held");
        thread::sleep(hold);
        let released_at = Instant::now();
        drop(guard);
        released_at
    });
    held.recv().expect("the holder thread takes the store lock");
    holder
}

/// The instant the holder thread released the store lock.
fn released_at(holder: thread::JoinHandle<Instant>) -> Instant {
    holder.join().expect("the holder thread releases the lock")
}

/// Assert the window the row needs really happened: the store stayed locked for at least
/// the span the clock owed the transition that was in flight.
fn assert_window(case: &str, waited_from: Instant, released: Instant, span: Duration) {
    let held = released.saturating_duration_since(waited_from);
    assert!(
        held >= span,
        "case {case:?}: the store must stay locked past the {span:?} the clock owed, but \
         it was released {held:?} after the request under test was issued"
    );
}

/// Dispatch one step against the graph under test.
async fn dispatch(
    router: &MultiServiceRouter,
    step: &Step,
    graph: &str,
    arn: &str,
) -> (u16, Value) {
    let path = step
        .path
        .replace("{graph}", graph)
        .replace("{arn}", &encode(arn));
    let body = step.body.as_ref().map(|body| {
        let substituted = body.to_string().replace("{graph}", graph);
        serde_json::from_str(&substituted).expect("a substituted request body is JSON")
    });
    let header = if step.graph_header { Some(graph) } else { None };
    call(router, step.method, &path, header, body).await
}

/// `GET /graphs/{graphIdentifier}`.
async fn get_graph(router: &MultiServiceRouter, graph: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{graph}"), None, None).await
}

/// `POST /graphs` without waiting for the provisioning the clock owes, answering the
/// created graph's id.
async fn create_graph(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        None,
        Some(json!({"graphName": name, "provisionedMemory": SMALLEST_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string()
}

/// Poll `GetGraph` until the graph reports `expected`, then answer its record.
///
/// No handler ever sleeps; only this test waits, because it is asking about a status the
/// clock reaches on its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    graph: &str,
    expected: &str,
) -> Value {
    for _ in 0..80 {
        let (status, body) = get_graph(router, graph).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(POLL).await;
    }
    let (_, body) = get_graph(router, graph).await;
    panic!(
        "case {case:?}: the graph never reported {expected}, last status {}",
        body["status"]
    );
}

/// Where the graph under test stands when the row's transition is started.
enum StartFrom {
    /// The graph's own provisioning is the transition in flight: the request under test
    /// is issued against a graph that is still `CREATING`.
    Provisioning,
    /// The graph is waited into service, then this call starts the transition.
    Serving(Step),
    /// The graph is waited into service, stopped and waited into `STOPPED`, then this
    /// call starts the transition.
    Stopped(Step),
}

/// One row of the settle table: the transition the clock makes due while the request
/// under test waits for the store, and what that request must answer once it gets there.
struct Settles {
    case: &'static str,
    /// The graph name the row creates (the modelled `graphName` pattern).
    name: &'static str,
    start: StartFrom,
    /// The request issued while the store is held past the span the transition owes.
    probe: Step,
    expect_http: u16,
    /// The `status` member the answer must carry — the transient status of the
    /// transition the probe itself started.
    expect_status: &'static str,
    /// The status the graph itself reports once the probe has been served, read back
    /// through `GetGraph`.
    expect_graph_status: &'static str,
}

/// Every row: each kind of transition this service owes a graph, against an operation
/// that is accepted ONLY from the settled status on the far side of it.
fn settle_rows() -> Vec<Settles> {
    vec![
        Settles {
            case: "a stop that came due makes the graph startable",
            name: "stop-settles-graph",
            start: StartFrom::Serving(step("POST", "/graphs/{graph}/stop", None)),
            probe: step("POST", "/graphs/{graph}/start", None),
            expect_http: 200,
            expect_status: "STARTING",
            expect_graph_status: "STARTING",
        },
        Settles {
            case: "a start that came due makes the graph stoppable",
            name: "start-settles-graph",
            start: StartFrom::Stopped(step("POST", "/graphs/{graph}/start", None)),
            probe: step("POST", "/graphs/{graph}/stop", None),
            expect_http: 200,
            expect_status: "STOPPING",
            expect_graph_status: "STOPPING",
        },
        Settles {
            case: "a provisioning that came due makes the new graph stoppable",
            name: "provisioning-settles-graph",
            start: StartFrom::Provisioning,
            probe: step("POST", "/graphs/{graph}/stop", None),
            expect_http: 200,
            expect_status: "STOPPING",
            expect_graph_status: "STOPPING",
        },
        Settles {
            case: "a reconfiguration that came due makes the graph deletable",
            name: "update-settles-graph",
            start: StartFrom::Serving(step(
                "PATCH",
                "/graphs/{graph}",
                Some(json!({"provisionedMemory": 32})),
            )),
            probe: step("DELETE", "/graphs/{graph}?skipSnapshot=true", None),
            expect_http: 200,
            expect_status: "DELETING",
            expect_graph_status: "DELETING",
        },
        Settles {
            case: "a snapshot that came due makes the graph reconfigurable",
            name: "snapshot-settles-graph",
            start: StartFrom::Serving(step(
                "POST",
                "/snapshots",
                Some(json!({
                    "graphIdentifier": "{graph}",
                    "snapshotName": "settling-snapshot",
                })),
            )),
            probe: step(
                "PATCH",
                "/graphs/{graph}",
                Some(json!({"provisionedMemory": 64})),
            ),
            expect_http: 200,
            expect_status: "UPDATING",
            expect_graph_status: "UPDATING",
        },
        Settles {
            case: "a reset that came due makes the graph snapshottable",
            name: "reset-settles-graph",
            start: StartFrom::Serving(step(
                "PUT",
                "/graphs/{graph}",
                Some(json!({"skipSnapshot": true})),
            )),
            // The answer here is the SNAPSHOT's record, so its `status` member is the
            // snapshot's own transient status, not the graph's.
            probe: step(
                "POST",
                "/snapshots",
                Some(json!({
                    "graphIdentifier": "{graph}",
                    "snapshotName": "after-the-reset",
                })),
            ),
            expect_http: 201,
            expect_status: "CREATING",
            expect_graph_status: "SNAPSHOTTING",
        },
    ]
}

/// What a row of the deletion table expects once the reclamation the clock already owed
/// has been carried out.
enum Expected {
    /// The modelled `ResourceNotFoundException` (404): the graph is gone for this
    /// request too, so every reference to it resolves to nothing.
    NotFound,
    /// HTTP 201: the name the reclaimed graph held is free again.
    Created,
    /// HTTP 200 with the reclaimed graph absent from the named list.
    AbsentFrom(&'static str),
}

/// One row of the deletion table: an operation issued while the store is held past the
/// deletion deadline of the graph it references, and what it must answer.
struct Reaped {
    case: &'static str,
    probe: Step,
    expected: Expected,
}

/// Every row: the reporting, writing, tagging and listing operations that read a graph,
/// against the graph whose deletion falls due while the request waits for the store.
fn reaped_rows(freed_name: &'static str) -> Vec<Reaped> {
    vec![
        Reaped {
            case: "GetGraph resolves the reclaimed graph to nothing",
            probe: step("GET", "/graphs/{graph}", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "StartGraph finds no graph to start",
            probe: step("POST", "/graphs/{graph}/start", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "StopGraph finds no graph to stop",
            probe: step("POST", "/graphs/{graph}/stop", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "UpdateGraph has no record to reconfigure",
            probe: step(
                "PATCH",
                "/graphs/{graph}",
                Some(json!({"provisionedMemory": 32})),
            ),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "ResetGraph has no data to clear",
            probe: step(
                "PUT",
                "/graphs/{graph}",
                Some(json!({"skipSnapshot": true})),
            ),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "DeleteGraph has nothing left to delete",
            probe: step("DELETE", "/graphs/{graph}?skipSnapshot=true", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "GetGraphSummary has no graph to summarise",
            probe: header_step("GET", "/summary", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "CreateGraphSnapshot has no graph to snapshot",
            probe: step(
                "POST",
                "/snapshots",
                Some(json!({
                    "graphIdentifier": "{graph}",
                    "snapshotName": "snapshot-of-nothing",
                })),
            ),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "ListTagsForResource finds no resource behind the ARN",
            probe: step("GET", "/tags/{arn}", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "TagResource finds no resource to tag",
            probe: step("POST", "/tags/{arn}", Some(json!({"tags": {"env": "test"}}))),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "UntagResource finds no resource to untag",
            probe: step("DELETE", "/tags/{arn}?tagKeys=env", None),
            expected: Expected::NotFound,
        },
        Reaped {
            case: "CreateGraph takes the name the finished deletion freed",
            probe: step(
                "POST",
                "/graphs",
                Some(json!({
                    "graphName": freed_name,
                    "provisionedMemory": SMALLEST_MEMORY,
                })),
            ),
            expected: Expected::Created,
        },
        Reaped {
            case: "ListGraphs reports the reclaimed graph no more",
            probe: step("GET", "/graphs", None),
            expected: Expected::AbsentFrom("graphs"),
        },
    ]
}

#[tokio::test]
async fn an_operation_sees_the_transition_the_clock_made_due_while_it_waited_for_the_store() {
    for row in settle_rows() {
        let Settles {
            case,
            name,
            start,
            probe,
            expect_http,
            expect_status,
            expect_graph_status,
        } = row;
        // A fresh emulator per row, so one row's graph cannot answer another's call.
        let (router, engine) = emulator();
        let graph = create_graph(&router, case, name).await;
        let arn = get_graph(&router, &graph).await.1["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created graph has an ARN"))
            .to_string();

        match &start {
            StartFrom::Provisioning => {}
            StartFrom::Serving(starter) => {
                wait_for_status(&router, case, &graph, "AVAILABLE").await;
                let (status, started) = dispatch(&router, starter, &graph, &arn).await;
                assert!(
                    (200..300).contains(&status),
                    "case {case:?}: the call that starts the transition must be \
                     accepted, got {status}: {started}"
                );
            }
            StartFrom::Stopped(starter) => {
                wait_for_status(&router, case, &graph, "AVAILABLE").await;
                let (status, stopping) =
                    call(&router, "POST", &format!("/graphs/{graph}/stop"), None, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from the setup StopGraph, got {status}: \
                     {stopping}"
                );
                wait_for_status(&router, case, &graph, "STOPPED").await;
                let (status, started) = dispatch(&router, starter, &graph, &arn).await;
                assert!(
                    (200..300).contains(&status),
                    "case {case:?}: the call that starts the transition must be \
                     accepted, got {status}: {started}"
                );
            }
        }

        // The window: the request under test begins while the transition is still in
        // flight and reaches the store only after the clock has made it due.
        let issued_at = Instant::now();
        let holder = hold_store_lock(&engine, PROVISIONING_SPAN + MARGIN);
        let (status, answered) = dispatch(&router, &probe, &graph, &arn).await;
        let released = released_at(holder);
        assert_window(case, issued_at, released, PROVISIONING_SPAN);

        assert_eq!(
            status, expect_http,
            "case {case:?}: the transition was due before the request read the store, so \
             the request is accepted from the settled status, got {status}: {answered}"
        );
        assert_eq!(
            answered["status"],
            json!(expect_status),
            "case {case:?}: expected status {expect_status}, got {}",
            answered["status"]
        );

        // Read the graph back: what the answer reported is what the store holds, so the
        // transition the probe started is the one a later reader sees too.
        let (status, fetched) = get_graph(&router, &graph).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the GetGraph read-back, got {status}: \
             {fetched}"
        );
        assert_eq!(
            fetched["status"],
            json!(expect_graph_status),
            "case {case:?}: expected the graph to report {expect_graph_status}, got {}",
            fetched["status"]
        );
    }
}

#[tokio::test]
async fn every_operation_reads_a_store_whose_owed_deletion_was_carried_out_first() {
    const FREED_NAME: &str = "reclaimed-graph";

    for row in reaped_rows(FREED_NAME) {
        let Reaped {
            case,
            probe,
            expected,
        } = row;
        // A fresh emulator per row, so the graph each row reclaims is its own.
        let (router, engine) = emulator();
        let graph = graph_in_service(&router, FREED_NAME).await;
        let arn = get_graph(&router, &graph).await.1["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created graph has an ARN"))
            .to_string();

        let (status, deleting) = call(
            &router,
            "DELETE",
            &format!("/graphs/{graph}?skipSnapshot=true"),
            None,
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the setup DeleteGraph, got {status}: \
             {deleting}"
        );

        let issued_at = Instant::now();
        let holder = hold_store_lock(&engine, DELETION_SPAN + MARGIN);
        let (status, answered) = dispatch(&router, &probe, &graph, &arn).await;
        let released = released_at(holder);
        assert_window(case, issued_at, released, DELETION_SPAN);

        match expected {
            Expected::NotFound => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected 404 for the reclaimed graph, got {status}: \
                     {answered}"
                );
                assert_eq!(
                    error_code(&answered),
                    "ResourceNotFoundException",
                    "case {case:?}: unexpected error code in {answered}"
                );
            }
            Expected::Created => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: the deletion was due before the create read the \
                     store, so the name is free, got {status}: {answered}"
                );
                assert_ne!(
                    answered["id"].as_str(),
                    Some(graph.as_str()),
                    "case {case:?}: the create must mint a new graph, not answer the \
                     reclaimed one, got {answered}"
                );
                assert_eq!(
                    answered["name"],
                    json!(FREED_NAME),
                    "case {case:?}: the new graph must hold the freed name, got {answered}"
                );
            }
            Expected::AbsentFrom(member) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from the listing, got {status}: \
                     {answered}"
                );
                let listed: Vec<&str> = answered[member]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: no {member} in {answered}"))
                    .iter()
                    .filter_map(|summary| summary["id"].as_str())
                    .collect();
                assert!(
                    !listed.contains(&graph.as_str()),
                    "case {case:?}: the reclaimed graph must not be listed, got {answered}"
                );
            }
        }

        // And the graph is gone for every later reader, not only for the request that
        // waited: a second read answers the same 404.
        let (status, fetched) = get_graph(&router, &graph).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the reclaimed graph must stay gone, got {status}: {fetched}"
        );
    }
}

#[tokio::test]
async fn the_query_operations_read_a_store_whose_owed_terminations_were_reaped_first() {
    // The id forms the model allows (`queryId` is a plain `String` with no pattern), one
    // per probe, so each row drives a query of its own on a store of its own. The last
    // row is a LISTING, which answers an empty page rather than a refusal.
    let rows: Vec<(&str, &str, Step, bool)> = vec![
        (
            "GetQuery reports no query whose timeout ran out",
            "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607",
            header_step("GET", "/queries/6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607", None),
            false,
        ),
        (
            "CancelQuery accepts no second cancel of a terminated query",
            "1234567890",
            header_step("DELETE", "/queries/1234567890", None),
            false,
        ),
        (
            "ListQueries walks no query whose timeout ran out",
            "q",
            header_step("GET", "/queries?maxResults=100", None),
            true,
        ),
    ];

    // The timeout the admitting request asks for
    // (`ExecuteQuery$queryTimeoutMilliseconds`), short enough to fall due while the
    // request under test waits for the store.
    let timeout = Duration::from_millis(250);
    for (case, query_id, probe, listing) in rows {
        let (router, engine) = emulator();
        let graph = graph_in_service(&router, "query-clock-graph").await;
        let arn = get_graph(&router, &graph).await.1["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created graph has an ARN"))
            .to_string();
        // Admitted as a query still in flight, so its timeout
        // starts now and falls due while the request under test waits for the store.
        admit_query_with_timeout(&engine, &graph, query_id, "MATCH (n) RETURN n", true, timeout);

        let issued_at = Instant::now();
        let holder = hold_store_lock(&engine, timeout + MARGIN);
        let (status, answered) = dispatch(&router, &probe, &graph, &arn).await;
        let released = released_at(holder);
        assert_window(case, issued_at, released, timeout);

        if listing {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from ListQueries, got {status}: {answered}"
            );
            assert_eq!(
                answered["queries"].as_array().map_or(usize::MAX, Vec::len),
                0,
                "case {case:?}: the terminated query must not be listed, got {answered}"
            );
        } else {
            // The timeout was out before the request read the store, so the id names no
            // query at all.
            assert_invalid_query_id(case, query_id, status, &answered);
        }

        // The graph the query ran on is untouched by the reaping: it is still in service
        // in the Region the request was signed for.
        let (status, fetched) = get_graph(&router, &graph).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetGraph, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["status"],
            json!("AVAILABLE"),
            "case {case:?}: the graph must still be serving, got {fetched}"
        );
        assert!(
            arn.contains(REGION),
            "case {case:?}: the graph's ARN must name the signed Region, got {arn}"
        );
    }
}
