// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the query operations a query's own lifecycle is observed
//! through — `CancelQuery` and `GetQuery` — driven over the wire (`restJson1`) through
//! the same router the emulator serves from.
//!
//! The rule every assertion here is about: a query is addressed as a query OF one
//! graph, so both operations resolve `graphIdentifier` first — answering
//! `ResourceNotFoundException` (404) when it names no graph — and then `queryId` within
//! it, answering `ValidationException` (400) "Supplied queryId <id> is invalid" when that
//! names nothing in flight: a query id is not a resource identifier a caller can hold on
//! to, so once the query is over the id resolves to nothing at all.
//! `GetQuery` reports the stored record (its id, its text, its state and the
//! milliseconds it waited and has been running) and changes none of it, while
//! `CancelQuery` accepts the cancel with an empty 200, after which the query reports
//! `CANCELLING` until the clock makes its termination due and is then no longer a query
//! either operation can name — and, the operation being `@idempotent`, a repeated cancel
//! of a query already stopping is the same empty 200 and does not postpone that
//! termination. A query NOBODY cancels is reported for as long as it may run: only its
//! own timeout ends it.
//!
//! So the tables below span what that rule generalises over: the forms a `queryId` takes
//! (the model puts no pattern on it), both states a query can be cancelled from
//! (`WAITING`, `RUNNING`), query texts of different shapes, and every way a request can
//! name a query the store does not hold — rather than re-running one recorded request.
//!
//! `ExecuteQuery` evaluates a query to completion within the call it arrived on, so it
//! leaves no query IN FLIGHT for these operations to find: the queries under test are
//! admitted through the state constructors a query that is still waiting or running is
//! held in (`state::Query::running` / `state::Query::waiting`); everything
//! the assertions look at afterwards goes over the wire.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ComAmazonawsNeptunegraphAmazonneptunegraphState as Store, Query, CANCELLATION_SPAN,
    DEFAULT_QUERY_TIMEOUT, DELETION_SPAN, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
pub(crate) const REGION: &str = "us-east-1";

/// Room for the emulator's own scheduling on top of a span the clock owes, so a wait
/// for a due transition is not a race.
const SETTLE_MARGIN: Duration = Duration::from_millis(150);

/// An emulator holding only this service, with the engine it was built on, so the suite
/// can admit a query that is still in flight.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
///
/// `graph_identifier` is sent as the `graphIdentifier` header, which is where the model
/// binds it for the query operations (`smithy_query(query="operation",
/// operation="CancelQuery,GetQuery")`: `httpHeader: graphIdentifier`).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    graph_identifier: Option<&str>,
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
    if let Some(graph_identifier) = graph_identifier {
        request = request.header("graphIdentifier", graph_identifier);
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

/// `GET /queries/{queryId}` on one graph.
pub(crate) async fn get_query(
    router: &MultiServiceRouter,
    graph_identifier: &str,
    query_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/queries/{query_id}"),
        Some(graph_identifier),
        None,
    )
    .await
}

/// `DELETE /queries/{queryId}` on one graph.
pub(crate) async fn cancel_query(
    router: &MultiServiceRouter,
    graph_identifier: &str,
    query_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/queries/{query_id}"),
        Some(graph_identifier),
        None,
    )
    .await
}

/// The wire error code of a failure body (`__type`), without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create a graph and wait until it is serving, returning its id.
pub(crate) async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        None,
        Some(json!({"graphName": name, "provisionedMemory": 16})),
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
        let (get_status, graph) = call(router, "GET", &format!("/graphs/{id}"), None, None).await;
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

/// Admit a query on `graph_id` as one still in flight, in the state the case asks
/// for — under the timeout a request that names none runs under.
pub(crate) fn admit_query(
    engine: &Arc<engine::InMemoryEngine>,
    graph_id: &str,
    query_id: &str,
    query_string: &str,
    running: bool,
) {
    admit_query_with_timeout(
        engine,
        graph_id,
        query_id,
        query_string,
        running,
        DEFAULT_QUERY_TIMEOUT,
    );
}

/// [`admit_query`] under the timeout `timeout`: what a request naming its own
/// `queryTimeoutMilliseconds` admits, and what lets a test watch a query time out without
/// waiting out the default.
pub(crate) fn admit_query_with_timeout(
    engine: &Arc<engine::InMemoryEngine>,
    graph_id: &str,
    query_id: &str,
    query_string: &str,
    running: bool,
    timeout: Duration,
) {
    let now = Instant::now();
    let query = if running {
        Query::running(query_id, graph_id, query_string, now)
    } else {
        Query::waiting(query_id, graph_id, query_string, now)
    };
    engine
        .state::<Store>()
        .lock()
        .insert_query(query.with_timeout(timeout))
        .expect("the store admits the query under test");
}

/// The sentence either query operation refuses an id that names no query in flight with.
pub(crate) fn invalid_query_id_message(query_id: &str) -> String {
    format!("Supplied queryId {query_id} is invalid")
}

/// Assert that `answer` is the refusal of a `queryId` that names no query in flight.
pub(crate) fn assert_invalid_query_id(case: &str, query_id: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for a queryId naming nothing in flight, \
         got {status}: {answer}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: unexpected error code in {answer}"
    );
    assert_eq!(
        answer["message"],
        json!(invalid_query_id_message(query_id)),
        "case {case:?}: expected {:?}, got {}",
        invalid_query_id_message(query_id),
        answer["message"]
    );
}

/// The instant the stored query's termination is due, as the store holds it.
pub(crate) fn termination_due(
    engine: &Arc<engine::InMemoryEngine>,
    graph_id: &str,
    query_id: &str,
) -> Option<Instant> {
    engine
        .state::<Store>()
        .lock()
        .queries
        .get(graph_id)
        .and_then(|queries| queries.get(query_id))
        .and_then(|query| query.terminates_at)
}

/// (case, queryId, queryString, admitted RUNNING). The rows differ along what the rule
/// generalises over: the form of the id (the model declares `queryId` as a plain
/// `String` with no pattern — `smithy_query(query="operation",operation="GetQuery")`),
/// the shape of the query text, and both states a query can be cancelled from.
const CASES: &[(&str, &str, &str, bool)] = &[
    (
        "uuid id, running read query",
        "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607",
        "MATCH (n) RETURN n LIMIT 10",
        true,
    ),
    (
        "numeric id, waiting write query",
        "1234567890",
        "CREATE (:Person {name: 'ada'})",
        false,
    ),
    (
        "long opaque id, multi-line query",
        "query00000000000000000000000000000000000000000000000000000000000001",
        "MATCH (a)-[:KNOWS]->(b)\nWHERE a.name = 'ada'\nRETURN b.name",
        true,
    ),
    (
        "punctuated id, parameterised query",
        "aq_2026.10-24~01",
        "MATCH (n:Person) WHERE n.age > $minAge RETURN n",
        false,
    ),
    ("single-character id, empty-ish query", "q", " ", true),
];

#[tokio::test]
async fn get_reports_an_admitted_query_and_a_cancel_stops_it_once_the_clock_says_so() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-cancel-graph").await;

    for (case, query_id, query_string, running) in CASES {
        admit_query(&engine, &graph, query_id, query_string, *running);

        // The reader reports the stored record, under the id the request named.
        let (status, reported) = get_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetQuery, got {status}: {reported}"
        );
        assert_eq!(
            reported["id"],
            json!(query_id),
            "case {case:?}: expected id {query_id:?}, got {}",
            reported["id"]
        );
        assert_eq!(
            reported["queryString"],
            json!(query_string),
            "case {case:?}: expected queryString {query_string:?}, got {}",
            reported["queryString"]
        );
        let expected_state = if *running { "RUNNING" } else { "WAITING" };
        assert_eq!(
            reported["state"],
            json!(expected_state),
            "case {case:?}: expected state {expected_state}, got {}",
            reported["state"]
        );
        // A query that has not started has run for no time at all; one that is running
        // started as soon as it was admitted, so it waited for no time at all.
        let waited = reported["waited"].as_i64();
        let elapsed = reported["elapsed"].as_i64();
        assert!(
            waited.is_some_and(|waited| waited >= 0),
            "case {case:?}: expected a non-negative waited, got {:?}",
            reported["waited"]
        );
        assert!(
            elapsed.is_some_and(|elapsed| elapsed >= 0),
            "case {case:?}: expected a non-negative elapsed, got {:?}",
            reported["elapsed"]
        );
        if *running {
            assert_eq!(
                waited,
                Some(0),
                "case {case:?}: a query that never queued waited 0ms, got {waited:?}"
            );
        } else {
            assert_eq!(
                elapsed,
                Some(0),
                "case {case:?}: a query that has not started has run 0ms, got {elapsed:?}"
            );
        }

        // Reading does not change what the next caller sees: the state is the same, and
        // neither duration runs backwards.
        let (status, again) = get_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from a second GetQuery, got {status}: {again}"
        );
        assert_eq!(
            again["state"], reported["state"],
            "case {case:?}: a read must not change the state, {} -> {}",
            reported["state"], again["state"]
        );
        assert!(
            again["elapsed"].as_i64() >= elapsed,
            "case {case:?}: elapsed must not run backwards, {:?} -> {:?}",
            reported["elapsed"],
            again["elapsed"]
        );

        // The cancel is accepted with the modelled empty 200 body.
        let (status, cancelled) = cancel_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CancelQuery, got {status}: {cancelled}"
        );
        assert_eq!(
            cancelled,
            Value::Null,
            "case {case:?}: CancelQuery models no output members, got {cancelled}"
        );

        // It is accepted, not finished: a caller reading straight after still sees the
        // cancellation in flight, from either state the query was cancelled in.
        let (status, stopping) = get_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetQuery while stopping, got {status}: {stopping}"
        );
        assert_eq!(
            stopping["state"],
            json!("CANCELLING"),
            "case {case:?}: expected CANCELLING after the cancel, got {}",
            stopping["state"]
        );
        assert_eq!(
            stopping["queryString"],
            json!(query_string),
            "case {case:?}: a cancel must not rewrite the record, got {stopping}"
        );

        // Only the clock finishes it, after which the query is gone for both operations
        // — and an id that names nothing in flight is an invalid argument, not a missing
        // resource.
        tokio::time::sleep(CANCELLATION_SPAN + SETTLE_MARGIN).await;
        for (operation, (status, body)) in [
            ("GetQuery", get_query(&router, &graph, query_id).await),
            ("CancelQuery", cancel_query(&router, &graph, query_id).await),
        ] {
            assert_invalid_query_id(&format!("{case} / {operation}"), query_id, status, &body);
        }
    }
}

#[tokio::test]
async fn a_repeated_cancel_is_accepted_and_does_not_postpone_the_termination() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "idempotent-cancel-graph").await;

    for (case, query_id, query_string, running) in CASES {
        admit_query(&engine, &graph, query_id, query_string, *running);

        let (status, _) = cancel_query(&router, &graph, query_id).await;
        assert_eq!(status, 200, "case {case:?}: the first cancel must be accepted, got {status}");
        let due = termination_due(&engine, &graph, query_id);
        assert!(
            due.is_some(),
            "case {case:?}: an accepted cancel owes a termination, got {due:?}"
        );

        // Repeating an `@idempotent` request has the same effect as making it once: the
        // query is still stopping, and still stops at the instant the first cancel set.
        for repeat in 0..3 {
            let (status, body) = cancel_query(&router, &graph, query_id).await;
            assert_eq!(
                status, 200,
                "case {case:?} (repeat {repeat}): expected 200, got {status}: {body}"
            );
            assert_eq!(
                termination_due(&engine, &graph, query_id),
                due,
                "case {case:?} (repeat {repeat}): a repeated cancel must not postpone \
                 the termination"
            );
            let (get_status, stopping) = get_query(&router, &graph, query_id).await;
            assert_eq!(
                get_status, 200,
                "case {case:?} (repeat {repeat}): expected 200 from GetQuery, \
                 got {get_status}: {stopping}"
            );
            assert_eq!(
                stopping["state"],
                json!("CANCELLING"),
                "case {case:?} (repeat {repeat}): expected CANCELLING, got {}",
                stopping["state"]
            );
        }
    }
}

#[tokio::test]
async fn both_operations_refuse_a_graph_or_a_query_the_store_does_not_hold() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "known-query-graph").await;
    let other = graph_in_service(&router, "other-query-graph").await;
    admit_query(
        &engine,
        &graph,
        "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607",
        "MATCH (n) RETURN n",
        true,
    );

    // (case, graphIdentifier, queryId): every way a request can name a query that is
    // not in flight on the graph it named, with the graph the request named HELD by the
    // store — so what is refused is the query id, as the invalid-argument the value is.
    // Ids no query holds on a graph that exists, and the known query addressed on the
    // OTHER graph — a query belongs to one graph.
    let unknown_queries: Vec<(String, String, String)> = vec![
        (
            "known graph, unknown query id".to_string(),
            graph.clone(),
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "known graph, empty-looking query id".to_string(),
            graph.clone(),
            "unknown".to_string(),
        ),
        (
            "known graph, punctuated query id".to_string(),
            graph.clone(),
            "aq_2026.10-24~01".to_string(),
        ),
        (
            "known query addressed on another graph".to_string(),
            other.clone(),
            "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607".to_string(),
        ),
    ];

    for (case, graph_identifier, query_id) in &unknown_queries {
        for (operation, (status, body)) in [
            ("GetQuery", get_query(&router, graph_identifier, query_id).await),
            (
                "CancelQuery",
                cancel_query(&router, graph_identifier, query_id).await,
            ),
        ] {
            assert_invalid_query_id(&format!("{case} / {operation}"), query_id, status, &body);
        }
    }

    // The GRAPH is resolved first and is a resource: a well-formed graph id the store
    // does not hold is the 404 that names it, whatever query id the request carried.
    let absent_graphs: Vec<(&str, &str, &str)> = vec![
        (
            "graph id no graph holds",
            "g-0000000000",
            "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607",
        ),
        ("another graph id no graph holds", "g-zzzzzzzzzz", "1234567890"),
    ];
    for (case, graph_identifier, query_id) in &absent_graphs {
        for (operation, (status, body)) in [
            ("GetQuery", get_query(&router, graph_identifier, query_id).await),
            (
                "CancelQuery",
                cancel_query(&router, graph_identifier, query_id).await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 from {operation}, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code from {operation} in {body}"
            );
        }
    }

    // None of those refusals touched the query that does exist.
    let (status, still_running) = get_query(
        &router,
        &graph,
        "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607",
    )
    .await;
    assert_eq!(
        status, 200,
        "case refused requests: the stored query must survive, got {status}: {still_running}"
    );
    assert_eq!(
        still_running["state"],
        json!("RUNNING"),
        "case refused requests: expected RUNNING, got {}",
        still_running["state"]
    );

    // A `graphIdentifier` the modelled `^g-[a-z0-9]{10}$` refuses is a 400, not a 404.
    for bad in ["not-a-graph-id", "g-TOOSHORT", "gs-0123456789", "g-123"] {
        for (operation, (status, body)) in [
            ("GetQuery", get_query(&router, bad, "1234567890").await),
            ("CancelQuery", cancel_query(&router, bad, "1234567890").await),
        ] {
            assert_eq!(
                status, 400,
                "case {bad:?}: expected 400 from {operation}, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {bad:?}: unexpected error code from {operation} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_cancel_touches_only_the_query_it_names() {
    let (router, engine) = emulator();
    let first = graph_in_service(&router, "multi-query-graph").await;
    let second = graph_in_service(&router, "second-multi-query-graph").await;

    // The same ids are admitted on both graphs, so a cancel that leaked across graphs
    // or across ids would show up as a changed state on one of the other four rows.
    let fleet: Vec<(&str, String, &str, bool)> = vec![
        ("cancel target", first.clone(), "MATCH (n) RETURN n", true),
        ("sibling on the same graph", first.clone(), "CREATE (:A)", false),
        ("same id on another graph", second.clone(), "MATCH (n) RETURN n", true),
        ("sibling on another graph", second.clone(), "CREATE (:B)", false),
    ];
    let ids = ["q-one", "q-two", "q-one", "q-two"];
    for (index, (case, graph, text, running)) in fleet.iter().enumerate() {
        admit_query(&engine, graph, ids[index], text, *running);
        let (status, _) = get_query(&router, graph, ids[index]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the query under test must be readable, got {status}"
        );
    }

    let (status, _) = cancel_query(&router, &first, "q-one").await;
    assert_eq!(status, 200, "the cancel under test must be accepted, got {status}");

    for (index, (case, graph, _, running)) in fleet.iter().enumerate().skip(1) {
        let (status, body) = get_query(&router, graph, ids[index]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetQuery, got {status}: {body}"
        );
        let expected = if *running { "RUNNING" } else { "WAITING" };
        assert_eq!(
            body["state"],
            json!(expected),
            "case {case:?}: expected {expected} after cancelling another query, got {}",
            body["state"]
        );
    }

    // Cancelling a query is not a change to the graph it ran on.
    let (status, graph) = call(&router, "GET", &format!("/graphs/{first}"), None, None).await;
    assert_eq!(
        status, 200,
        "case graph after a cancel: expected 200 from GetGraph, got {status}: {graph}"
    );
    assert_eq!(
        graph["status"],
        json!("AVAILABLE"),
        "case graph after a cancel: the graph must still be serving, got {}",
        graph["status"]
    );
}

/// A query nobody cancels is reported for as long as it may RUN: only its own timeout
/// ends it, so nothing takes a query out of the store while a caller is still listing it,
/// cancelling it and polling the id.
#[tokio::test]
async fn only_its_own_timeout_ends_a_query_nobody_cancels() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "self-terminating-query-graph").await;

    // The timeout is the request's own when it names one
    // (`queryTimeoutMilliseconds`), so the rows differ in the timeout the admitting
    // request asked for: short enough to watch run out, and the default a request that
    // names none runs under.
    let short = Duration::from_millis(250);
    for (case, query_id, query_string, running) in CASES {
        admit_query_with_timeout(&engine, &graph, query_id, query_string, *running, short);
        let (status, body) = get_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the query under test must be readable, got {status}: {body}"
        );
    }
    // A query admitted under the default timeout, which is the one under test below.
    admit_query(&engine, &graph, "q-default-timeout", "MATCH (n) RETURN n", true);

    // Nothing is cancelled here: the queries with the short timeout are over because
    // that timeout is, which is also what keeps the store from holding a record per
    // request forever.
    tokio::time::sleep(short + SETTLE_MARGIN).await;
    for (case, query_id, _, _) in CASES {
        let (status, body) = get_query(&router, &graph, query_id).await;
        assert_invalid_query_id(case, query_id, status, &body);
    }

    // The query whose timeout has NOT run out is still in flight: a caller that submitted
    // it and never cancelled it can still find, read and cancel it.
    let (status, still_running) = get_query(&router, &graph, "q-default-timeout").await;
    assert_eq!(
        status, 200,
        "case the default timeout: a query nobody cancelled must still be reported, \
         got {status}: {still_running}"
    );
    assert_eq!(
        still_running["state"],
        json!("RUNNING"),
        "case the default timeout: expected RUNNING, got {}",
        still_running["state"]
    );
    assert_eq!(
        engine.state::<Store>().lock().query_count(),
        1,
        "case the short timeouts are out: every timed-out record must be reclaimed, \
         and only those"
    );
}

#[tokio::test]
async fn deleting_a_graph_takes_its_in_flight_queries_with_it() {
    let (router, engine) = emulator();
    let deleted = graph_in_service(&router, "deleted-query-graph").await;
    let kept = graph_in_service(&router, "kept-query-graph").await;

    for (case, graph, query_id) in [
        ("on the deleted graph", &deleted, "q-gone"),
        ("on the kept graph", &kept, "q-kept"),
    ] {
        admit_query(&engine, graph, query_id, "MATCH (n) RETURN n", true);
        let (status, _) = get_query(&router, graph, query_id).await;
        assert_eq!(status, 200, "case {case:?}: the query must be readable, got {status}");
    }

    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/graphs/{deleted}?skipSnapshot=true"),
        None,
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete the graph: expected 200 from DeleteGraph, got {status}: {body}"
    );

    // The delete is accepted, not performed: while it is in flight the graph is still a
    // resource this service holds, but it is no longer SERVING, so its own data plane
    // answers nothing — the query is refused for the status the graph is in rather than
    // reported (see `tests/all/graph_data_plane_gate.rs`). The other graph is serving, so
    // its query is untouched either way.
    let (status, refused) = get_query(&router, &deleted, "q-gone").await;
    assert_eq!(
        status, 400,
        "case on the deleting graph: a graph on its way out serves no data plane, \
         got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case on the deleting graph: unexpected error code in {refused}"
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("DELETING"),
        "case on the deleting graph: the refusal must name the status read, got {refused}"
    );
    let (status, query) = get_query(&router, &kept, "q-kept").await;
    assert_eq!(
        status, 200,
        "case on the kept graph: the query must still be readable, got {status}: {query}"
    );
    assert_eq!(
        query["state"],
        json!("RUNNING"),
        "case on the kept graph: expected RUNNING, got {}",
        query["state"]
    );

    // Once the deletion the clock owes falls due the graph is gone, and its queries are
    // gone with it — reported as the graph being absent, which is the first thing either
    // operation resolves.
    tokio::time::sleep(DELETION_SPAN + SETTLE_MARGIN).await;
    let (status, body) = get_query(&router, &deleted, "q-gone").await;
    assert_eq!(
        status, 404,
        "case query of a deleted graph: expected 404, got {status}: {body}"
    );
    assert!(
        !engine
            .state::<Store>()
            .lock()
            .queries
            .contains_key(&deleted),
        "case query of a deleted graph: the record must be reclaimed, not orphaned"
    );

    // And only that graph's queries: the kept graph is still there to be asked.
    let (status, kept_graph) = call(&router, "GET", &format!("/graphs/{kept}"), None, None).await;
    assert_eq!(
        status, 200,
        "case the kept graph: expected 200 from GetGraph, got {status}: {kept_graph}"
    );
}
