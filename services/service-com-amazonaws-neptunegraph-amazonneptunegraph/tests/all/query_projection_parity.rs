// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE projection a stored query record is reported through,
//! driven over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: `GetQueryOutput` and `QuerySummary` declare
//! exactly the same five members (`id`, `queryString`, `waited`, `elapsed`, `state` —
//! `smithy_query(query="shape",shape="GetQueryOutput,QuerySummary")`), so the two
//! operations that report a query — `GetQuery`, which reports one, and `ListQueries`,
//! which reports one summary per query in flight — render the stored record through the
//! single projection `crate::state` holds for it: the three stored facts come back
//! identical from both, and the two durations are the same spans measured off the clock
//! at the instant each call reads, so whichever call is made second reports durations at
//! least as long as the one made first.
//!
//! `tests/all/query_status_reporting.rs` pins what `GetQuery` reports and
//! `tests/all/query_listing.rs` what the listing reports; this suite pins that the two
//! cannot DRIFT, which is what sharing one projection buys. The table spans what the
//! rule generalises over: both states a query is admitted in and the state a cancel puts
//! it in, the forms a query text takes, the span of clock a caller lets pass before
//! reading, and both orders the two calls can be made in.
//!
//! `ExecuteQuery` evaluates a query to completion within the call it arrived on, so it
//! leaves no query IN FLIGHT for these operations to find: the queries under test are
//! admitted through the state constructors a query that is still waiting or running is
//! held in (`state::Query::running` / `state::Query::waiting`); everything
//! the assertions look at afterwards goes over the wire.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::MAX_QUERY_STRING_CHARS;

use super::query_cancellation::{
    admit_query, call, cancel_query, emulator, get_query, graph_in_service,
};

/// Every member both shapes declare, from
/// `smithy_query(query="shape",shape="GetQueryOutput,QuerySummary")`: `id`,
/// `queryString`, `waited`, `elapsed` and `state`. A report that omits one of these from
/// either operation is a report the two have drifted apart.
const PROJECTED_MEMBERS: &[&str] = &["id", "queryString", "waited", "elapsed", "state"];

/// The stored facts of a query record: the members that are read straight off the record
/// rather than measured off the clock, so both operations must answer them identically.
const STORED_MEMBERS: &[&str] = &["id", "queryString", "state"];

/// The members measured off the clock at the instant a call reads, so the later call
/// reports a span at least as long as the earlier one.
const CLOCK_MEMBERS: &[&str] = &["waited", "elapsed"];

/// `GET /queries` on one graph, asking for more results than any case admits so the cap
/// never hides the query under test (`tests/all/query_listing.rs` pins the cap itself).
async fn list_queries(
    router: &smithy_server_runtime::MultiServiceRouter,
    graph_identifier: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        "/queries?maxResults=100",
        Some(graph_identifier),
        None,
    )
    .await
}

/// The summary the listing reports for one query id, or `None` when it reports none.
fn summary_of(body: &Value, query_id: &str) -> Option<Value> {
    body["queries"]
        .as_array()
        .unwrap_or_else(|| panic!("the required member `queries` must be a list, got {body}"))
        .iter()
        .find(|summary| summary["id"] == json!(query_id))
        .cloned()
}

/// (case, query text). The rows differ along the forms a `queryString` takes — the model
/// declares it a plain `String` — because the projection carries whatever was stored.
fn query_text_cases() -> Vec<(&'static str, String)> {
    vec![
        ("ascii read query", "MATCH (n:Person) RETURN n LIMIT 1".to_string()),
        (
            "non-ascii literals",
            "MATCH (v:Ville {nom: 'Zürich'})-[:VOISIN]->(w) RETURN w.名前".to_string(),
        ),
        (
            "embedded quotes and newlines",
            "MATCH (n)\nWHERE n.note = \"a \\\"quoted\\\" word\"\nRETURN n".to_string(),
        ),
        (
            // Well inside the bound the store puts on a stored text
            // (`state::MAX_QUERY_STRING_CHARS`), so what is under test is the projection,
            // not that bound.
            "a long text",
            format!(
                "MATCH (n) WHERE n.tag = '{}' RETURN n",
                "x".repeat(MAX_QUERY_STRING_CHARS / 32)
            ),
        ),
        ("whitespace only", "  \t ".to_string()),
    ]
}

/// (case, admitted RUNNING, cancelled, the span of clock the caller lets pass before
/// reading, the listing is read FIRST). Each span is a small fraction of
/// `state::QUERY_RUN_SPAN`, so the query is still well inside its own run span — rather
/// than near the end of it — when both calls read it.
const READ_CASES: &[(&str, bool, bool, u64, bool)] = &[
    ("running, read at once, get first", true, false, 0, false),
    ("running, read after a span, list first", true, false, 40, true),
    ("waiting, read at once, list first", false, false, 0, true),
    ("waiting, read after a span, get first", false, false, 80, false),
    ("cancelled while running, get first", true, true, 0, false),
    ("cancelled while waiting, list first", false, true, 40, true),
];

#[tokio::test]
async fn one_query_is_reported_identically_by_the_read_and_by_the_listing() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-projection-graph").await;

    for (text_index, (text_case, query_string)) in query_text_cases().iter().enumerate() {
        for (read_index, (read_case, running, cancelled, span_millis, list_first)) in
            READ_CASES.iter().enumerate()
        {
            let case = format!("{text_case} / {read_case}");
            let query_id = format!("q-parity-{text_index}-{read_index}");
            admit_query(&engine, &graph, &query_id, query_string, *running);
            let mut expected_state = if *running { "RUNNING" } else { "WAITING" };
            if *cancelled {
                let (status, body) = cancel_query(&router, &graph, &query_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from CancelQuery, got {status}: {body}"
                );
                expected_state = "CANCELLING";
            }
            if *span_millis > 0 {
                tokio::time::sleep(Duration::from_millis(*span_millis)).await;
            }

            // The two calls, made in the order the case asks for, so neither operation is
            // only ever the later reader.
            let (first_name, first, second_name, second) = if *list_first {
                let (status, listed) = list_queries(&router, &graph).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from ListQueries, got {status}: {listed}"
                );
                let summary = summary_of(&listed, &query_id).unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report the query, got {listed}")
                });
                let (status, reported) = get_query(&router, &graph, &query_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetQuery, got {status}: {reported}"
                );
                ("ListQueries", summary, "GetQuery", reported)
            } else {
                let (status, reported) = get_query(&router, &graph, &query_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetQuery, got {status}: {reported}"
                );
                let (status, listed) = list_queries(&router, &graph).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from ListQueries, got {status}: {listed}"
                );
                let summary = summary_of(&listed, &query_id).unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report the query, got {listed}")
                });
                ("GetQuery", reported, "ListQueries", summary)
            };

            // Every projected member comes back from BOTH, so neither can quietly stop
            // reporting one of the five facts.
            for member in PROJECTED_MEMBERS {
                for (operation, body) in [(first_name, &first), (second_name, &second)] {
                    assert!(
                        body.get(*member).is_some_and(|value| !value.is_null()),
                        "case {case:?}: expected member {member:?} from {operation}, got {body}"
                    );
                }
            }

            // The stored facts are the same facts, whichever operation reports them.
            for member in STORED_MEMBERS {
                assert_eq!(
                    first[*member], second[*member],
                    "case {case:?}: {first_name} and {second_name} disagree on {member:?}: \
                     {} vs {}",
                    first[*member], second[*member]
                );
            }
            assert_eq!(
                first["id"],
                json!(query_id),
                "case {case:?}: expected id {query_id:?}, got {}",
                first["id"]
            );
            assert_eq!(
                first["queryString"],
                json!(query_string),
                "case {case:?}: the stored text must come back unchanged, got {}",
                first["queryString"]
            );
            assert_eq!(
                first["state"],
                json!(expected_state),
                "case {case:?}: expected state {expected_state}, got {}",
                first["state"]
            );

            // Both durations are the same spans, read off the clock at the instant each
            // call serves: non-negative, and never shorter for the later reader.
            for member in CLOCK_MEMBERS {
                let earlier = first[*member].as_i64().unwrap_or_else(|| {
                    panic!("case {case:?}: {member} is not an integer in {first}")
                });
                let later = second[*member].as_i64().unwrap_or_else(|| {
                    panic!("case {case:?}: {member} is not an integer in {second}")
                });
                assert!(
                    earlier >= 0,
                    "case {case:?}: expected a non-negative {member} from {first_name}, \
                     got {earlier}"
                );
                assert!(
                    later >= earlier,
                    "case {case:?}: {member} must not run backwards from {first_name} to \
                     {second_name}, {earlier} -> {later}"
                );
            }

            // A query that executes from its admission never queued; one that has not
            // started has run for no time at all — and both operations say so.
            let settled_member = if *running { "waited" } else { "elapsed" };
            for (operation, body) in [(first_name, &first), (second_name, &second)] {
                assert_eq!(
                    body[settled_member],
                    json!(0),
                    "case {case:?}: expected {settled_member} of 0ms from {operation}, got {}",
                    body[settled_member]
                );
            }
        }
    }
}

/// (case, queryId, query text, admitted RUNNING), admitted in this order. Several
/// queries in flight at once, so the listing renders more than one record through the
/// projection while each is also read on its own.
const FLEET: &[(&str, &str, &str, bool)] = &[
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
        "punctuated id, parameterised query",
        "aq_2026.10-24~01",
        "MATCH (n:Person) WHERE n.age > $minAge RETURN n",
        true,
    ),
    ("single-character id, empty-ish query", "q", " ", false),
];

#[tokio::test]
async fn every_listed_query_matches_what_a_single_read_of_it_reports() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-projection-fleet-graph").await;
    for (_, query_id, query_string, running) in FLEET {
        admit_query(&engine, &graph, query_id, query_string, *running);
    }

    let (status, listed) = list_queries(&router, &graph).await;
    assert_eq!(
        status, 200,
        "case the whole fleet: expected 200 from ListQueries, got {status}: {listed}"
    );

    for (case, query_id, query_string, running) in FLEET {
        let summary = summary_of(&listed, query_id).unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report the query, got {listed}")
        });
        let (status, reported) = get_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetQuery, got {status}: {reported}"
        );

        for member in PROJECTED_MEMBERS {
            assert!(
                summary.get(*member).is_some_and(|value| !value.is_null()),
                "case {case:?}: expected member {member:?} in the summary, got {summary}"
            );
            assert!(
                reported.get(*member).is_some_and(|value| !value.is_null()),
                "case {case:?}: expected member {member:?} in the report, got {reported}"
            );
        }
        for member in STORED_MEMBERS {
            assert_eq!(
                summary[*member], reported[*member],
                "case {case:?}: the listing and the read disagree on {member:?}: {} vs {}",
                summary[*member], reported[*member]
            );
        }
        assert_eq!(
            summary["queryString"],
            json!(query_string),
            "case {case:?}: the stored text must come back unchanged, got {}",
            summary["queryString"]
        );
        let expected_state = if *running { "RUNNING" } else { "WAITING" };
        assert_eq!(
            summary["state"],
            json!(expected_state),
            "case {case:?}: expected state {expected_state}, got {}",
            summary["state"]
        );
        // The read is the later call, so neither duration it reports is shorter than the
        // listing's.
        for member in CLOCK_MEMBERS {
            let listed_span = summary[*member]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: {member} is not an integer in {summary}"));
            let read_span = reported[*member].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: {member} is not an integer in {reported}")
            });
            assert!(
                listed_span >= 0 && read_span >= listed_span,
                "case {case:?}: expected a non-negative {member} that does not run backwards \
                 from the listing to the read, {listed_span} -> {read_span}"
            );
        }
    }
}
