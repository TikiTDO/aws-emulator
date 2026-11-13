// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListQueries`, driven over the wire (`restJson1`) through the
//! same router the emulator serves from.
//!
//! The rule every assertion here is about: a listing of the queries of a graph reports
//! one summary per query still in flight ON the graph the request names — its id, its
//! text, the state it is in and the milliseconds it waited and has been running — in
//! admission order, restricted to one state when the request names one rather than
//! `ALL`, capped at the count of results the request asked for, and changing nothing a
//! later caller sees; a graph this service does not hold is a refusal rather than an
//! empty list, and a result count below the published minimum is refused too.
//!
//! So the tables below span what that rule generalises over: every value the state
//! filter takes (absent, `ALL`, and each state a stored query can be in), result counts
//! from the published bound up past the number of queries in flight and one below that
//! bound, the number of queries a graph holds, and the ways a request can name a graph
//! that holds no queries at all.
//!
//! `tests/all/query_cancellation.rs` pins how one query's lifecycle is observed and
//! `tests/all/query_status_reporting.rs` what `GetQuery` reports about it; this suite
//! pins the LISTING.
//!
//! `ExecuteQuery` evaluates a query to completion within the call it arrived on, so it
//! leaves no query IN FLIGHT for these operations to find: the queries under test are
//! admitted through the state constructors a query that is still waiting or running is
//! held in (`state::Query::running` / `state::Query::waiting`); everything
//! the assertions look at afterwards goes over the wire.

use std::time::Duration;

use serde_json::{json, Value};

use super::query_cancellation::{
    admit_query, admit_query_with_timeout, call, cancel_query, emulator, error_code,
    graph_in_service,
};

/// Room for the emulator's own scheduling on top of a span the clock owes, so a wait for
/// a due transition is not a race.
const SETTLE_MARGIN: Duration = Duration::from_millis(150);

/// Every member `QuerySummary` declares, from
/// `smithy_query(query="shape",shape="QuerySummary")`: `id`, `queryString`, `waited`,
/// `elapsed` and `state`. A summary that omits one of these is a summary a client cannot
/// read a query's status from.
const SUMMARY_MEMBERS: &[&str] = &["id", "queryString", "waited", "elapsed", "state"];

/// `GET /queries` on one graph: the result count the request asks for, and the state
/// filter when the case names one.
async fn list_queries(
    router: &smithy_server_runtime::MultiServiceRouter,
    graph_identifier: &str,
    max_results: i32,
    state: Option<&str>,
) -> (u16, Value) {
    let path = match state {
        Some(state) => format!("/queries?maxResults={max_results}&state={state}"),
        None => format!("/queries?maxResults={max_results}"),
    };
    call(router, "GET", &path, Some(graph_identifier), None).await
}

/// The reported query ids, in the order the answer lists them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("the required member `queries` must be a list, got {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// (queryId, queryString, admitted RUNNING), admitted in this order, so the listing's
/// order is the order this table is written in. The rows differ along the forms a
/// `queryId` takes (the model puts no pattern on it), the shape of the query text, and
/// both states a query is admitted in.
const FLEET: &[(&str, &str, bool)] = &[
    (
        "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607",
        "MATCH (n) RETURN n LIMIT 10",
        true,
    ),
    ("1234567890", "CREATE (:Person {name: 'ada'})", false),
    (
        "query00000000000000000000000000000000000000000000000000000000000001",
        "MATCH (a)-[:KNOWS]->(b)\nWHERE a.name = 'ada'\nRETURN b.name",
        true,
    ),
    ("aq_2026.10-24~01", "MATCH (n:Person) WHERE n.age > $minAge RETURN n", false),
];

#[tokio::test]
async fn the_listing_reports_every_query_in_flight_with_every_summary_member() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-listing-graph").await;
    for (query_id, query_string, running) in FLEET {
        admit_query(&engine, &graph, query_id, query_string, *running);
    }

    let (status, body) = list_queries(&router, &graph, 100, None).await;
    assert_eq!(
        status, 200,
        "case every query in flight: expected 200 from ListQueries, got {status}: {body}"
    );
    let ids = reported_ids(&body);
    let admitted: Vec<String> = FLEET
        .iter()
        .map(|(query_id, _, _)| (*query_id).to_string())
        .collect();
    assert_eq!(
        ids, admitted,
        "case every query in flight: expected the admitted queries in admission order, got {ids:?}"
    );

    let summaries = body["queries"].as_array().expect("a list of summaries");
    for ((query_id, query_string, running), summary) in FLEET.iter().zip(summaries) {
        for member in SUMMARY_MEMBERS {
            assert!(
                !summary[member].is_null(),
                "case {query_id:?}: the summary must carry {member}, got {summary}"
            );
        }
        assert_eq!(
            summary["queryString"],
            json!(query_string),
            "case {query_id:?}: expected queryString {query_string:?}, got {}",
            summary["queryString"]
        );
        let expected_state = if *running { "RUNNING" } else { "WAITING" };
        assert_eq!(
            summary["state"],
            json!(expected_state),
            "case {query_id:?}: expected state {expected_state}, got {}",
            summary["state"]
        );
        let waited = summary["waited"].as_i64();
        let elapsed = summary["elapsed"].as_i64();
        assert!(
            waited.is_some_and(|waited| waited >= 0),
            "case {query_id:?}: expected a non-negative waited, got {:?}",
            summary["waited"]
        );
        assert!(
            elapsed.is_some_and(|elapsed| elapsed >= 0),
            "case {query_id:?}: expected a non-negative elapsed, got {:?}",
            summary["elapsed"]
        );
        if *running {
            assert_eq!(
                waited,
                Some(0),
                "case {query_id:?}: a query that never queued waited 0ms, got {waited:?}"
            );
        } else {
            assert_eq!(
                elapsed,
                Some(0),
                "case {query_id:?}: a query that has not started has run 0ms, got {elapsed:?}"
            );
        }
    }

    // Listing is a read: a second listing reports the same queries in the same states.
    let (status, again) = list_queries(&router, &graph, 100, None).await;
    assert_eq!(
        status, 200,
        "case a repeated listing: expected 200, got {status}: {again}"
    );
    assert_eq!(
        reported_ids(&again),
        ids,
        "case a repeated listing: a read must not change what the next caller sees"
    );
    for (index, summary) in again["queries"]
        .as_array()
        .expect("a list of summaries")
        .iter()
        .enumerate()
    {
        assert_eq!(
            summary["state"], summaries[index]["state"],
            "case a repeated listing: the state of {} must not move, {} -> {}",
            summary["id"], summaries[index]["state"], summary["state"]
        );
    }
}

#[tokio::test]
async fn the_state_filter_reports_the_queries_in_that_state_and_all_reports_every_one() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-filter-graph").await;
    for (query_id, query_string, running) in FLEET {
        admit_query(&engine, &graph, query_id, query_string, *running);
    }
    // The third row is moved into the remaining state over the wire, so every state a
    // stored query can be in is represented in one listing.
    let cancelling = FLEET[2].0;
    let (status, _) = cancel_query(&router, &graph, cancelling).await;
    assert_eq!(
        status, 200,
        "case admit a CANCELLING query: the cancel must be accepted, got {status}"
    );

    let running: Vec<&str> = vec![FLEET[0].0];
    let waiting: Vec<&str> = vec![FLEET[1].0, FLEET[3].0];
    let every: Vec<&str> = vec![FLEET[0].0, FLEET[1].0, FLEET[2].0, FLEET[3].0];
    // (case, the `state` the request names, the ids the listing must report). The values
    // are the modelled `QueryStateInput` set, from
    // `smithy_query(query="shape",shape="QueryStateInput")`: ALL | RUNNING | WAITING |
    // CANCELLING — plus the absent member, which filters nothing.
    let cases: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("no filter", None, every.clone()),
        ("ALL", Some("ALL"), every.clone()),
        ("RUNNING", Some("RUNNING"), running),
        ("WAITING", Some("WAITING"), waiting),
        ("CANCELLING", Some("CANCELLING"), vec![cancelling]),
    ];

    for (case, filter, expected) in &cases {
        let (status, body) = list_queries(&router, &graph, 100, *filter).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListQueries, got {status}: {body}"
        );
        let ids = reported_ids(&body);
        assert_eq!(
            ids,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
        for summary in body["queries"].as_array().expect("a list of summaries") {
            if let Some(state) = filter.filter(|filter| *filter != "ALL") {
                assert_eq!(
                    summary["state"],
                    json!(state),
                    "case {case:?}: every reported query must be in the filtered state, \
                     got {summary}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_result_count_caps_the_listing_and_keeps_the_oldest_queries() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-count-graph").await;
    for (query_id, query_string, running) in FLEET {
        admit_query(&engine, &graph, query_id, query_string, *running);
    }
    let admitted: Vec<String> = FLEET
        .iter()
        .map(|(query_id, _, _)| (*query_id).to_string())
        .collect();

    // The published minimum, every count up to the number in flight, and counts past it.
    for max_results in [1, 2, 3, 4, 5, 100] {
        let (status, body) = list_queries(&router, &graph, max_results, None).await;
        assert_eq!(
            status, 200,
            "case maxResults {max_results}: expected 200, got {status}: {body}"
        );
        let ids = reported_ids(&body);
        let kept = usize::try_from(max_results)
            .expect("a positive count fits in a usize")
            .min(admitted.len());
        assert_eq!(
            ids,
            admitted[..kept],
            "case maxResults {max_results}: expected the first {kept} admitted queries, \
             got {ids:?}"
        );
    }

    // A count below the published minimum is refused, not silently treated as a cap of
    // zero — and the refusal names the member that broke it.
    for max_results in [0, -1, -100] {
        let (status, body) = list_queries(&router, &graph, max_results, None).await;
        assert_eq!(
            status, 400,
            "case maxResults {max_results}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case maxResults {max_results}: unexpected error code in {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("maxResults"),
            "case maxResults {max_results}: the refusal must name the member, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_is_of_one_graph_and_a_graph_this_service_does_not_hold_is_refused() {
    let (router, engine) = emulator();
    let first = graph_in_service(&router, "listing-first-graph").await;
    let second = graph_in_service(&router, "listing-second-graph").await;
    // The same ids on both graphs: a listing that leaked across graphs would report four
    // queries instead of two.
    admit_query(&engine, &first, "q-one", "MATCH (n) RETURN n", true);
    admit_query(&engine, &first, "q-two", "CREATE (:A)", false);
    admit_query(&engine, &second, "q-one", "MATCH (n) RETURN n", true);

    for (case, graph, expected) in [
        ("the first graph", &first, vec!["q-one", "q-two"]),
        ("the second graph", &second, vec!["q-one"]),
    ] {
        let (status, body) = list_queries(&router, graph, 100, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let mut ids = reported_ids(&body);
        ids.sort();
        assert_eq!(
            ids,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected {expected:?}, got {ids:?}"
        );
    }

    // A well-formed `graphIdentifier` this service holds no graph for is a failed lookup
    // rather than an empty list. The operation declares no `ResourceNotFoundException`
    // (`smithy_query(query="operation",operation="ListQueries")` -> errors), so the one
    // client error it models is what the refusal carries.
    for unknown in ["g-0000000000", "g-zzzzzzzzzz", "g-abcdefghij"] {
        let (status, body) = list_queries(&router, unknown, 100, None).await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected 400 from ListQueries, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {unknown:?}: unexpected error code in {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(unknown),
            "case {unknown:?}: the refusal must name the identifier, got {message:?}"
        );
    }

    // A `graphIdentifier` the modelled `^g-[a-z0-9]{10}$` refuses is refused for the
    // constraint it broke, before any lookup.
    for malformed in ["not-a-graph-id", "g-TOOSHORT", "gs-0123456789", "g-123"] {
        let (status, body) = list_queries(&router, malformed, 100, None).await;
        assert_eq!(
            status, 400,
            "case {malformed:?}: expected 400 from ListQueries, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {malformed:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_graph_whose_queries_are_over_lists_none_of_them() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "emptied-listing-graph").await;

    // A graph that has never run a query holds none: the required member is reported as
    // the empty list it is, not omitted.
    let (status, body) = list_queries(&router, &graph, 100, None).await;
    assert_eq!(
        status, 200,
        "case a graph with no queries: expected 200, got {status}: {body}"
    );
    assert_eq!(
        reported_ids(&body),
        Vec::<String>::new(),
        "case a graph with no queries: expected an empty list, got {body}"
    );

    // Each query is admitted under a timeout of its own, the way a request naming
    // `queryTimeoutMilliseconds` admits one, so the listing can be watched past it.
    let timeout = Duration::from_millis(250);
    for (query_id, query_string, running) in FLEET {
        admit_query_with_timeout(&engine, &graph, query_id, query_string, *running, timeout);
    }
    let (_, listed) = list_queries(&router, &graph, 100, None).await;
    assert_eq!(
        reported_ids(&listed).len(),
        FLEET.len(),
        "case queries in flight: every admitted query must be listed, got {listed}"
    );

    // Nothing is cancelled here: the queries are over because their own timeout is, and
    // a listing never reports a query that has stopped.
    tokio::time::sleep(timeout + SETTLE_MARGIN).await;
    let (status, emptied) = list_queries(&router, &graph, 100, None).await;
    assert_eq!(
        status, 200,
        "case the timeout is out: expected 200, got {status}: {emptied}"
    );
    assert_eq!(
        reported_ids(&emptied),
        Vec::<String>::new(),
        "case the timeout is out: expected an empty list, got {emptied}"
    );

    // A query nobody cancelled and whose timeout has NOT run out stays listed, so the
    // listing is what a caller manages in-flight work through.
    admit_query(&engine, &graph, "q-still-running", "MATCH (n) RETURN n", true);
    tokio::time::sleep(timeout + SETTLE_MARGIN).await;
    let (status, still_listed) = list_queries(&router, &graph, 100, None).await;
    assert_eq!(
        status, 200,
        "case a query under the default timeout: expected 200, got {status}: {still_listed}"
    );
    assert_eq!(
        reported_ids(&still_listed),
        vec!["q-still-running".to_string()],
        "case a query under the default timeout: it must still be listed, got {still_listed}"
    );
}
