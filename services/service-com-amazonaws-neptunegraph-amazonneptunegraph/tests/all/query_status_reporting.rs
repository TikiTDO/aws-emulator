// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `GetQuery` REPORTS about one in-flight query, driven over
//! the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: `GetQuery` reports the stored query record —
//! its id and text verbatim, the state it is in, and the two durations measured in
//! milliseconds off the clock against the instants that record holds (`waited` from the
//! query's admission to its start, `elapsed` from its start to now) — and changes none
//! of it, so repeating the read neither moves the state nor shortens the query's life.
//!
//! `tests/all/query_cancellation.rs` pins how a query's lifecycle is observed (a cancel,
//! the 404s, the terminations the clock owes); this suite pins the REPORT itself, over
//! what the rule generalises over: both states a query is admitted in, every modelled
//! output member, the forms a query text takes (ascii, non-ascii, embedded quotes and
//! newlines, a long text, whitespace), and the spans of clock a caller lets pass before
//! reading.
//!
//! `ExecuteQuery` evaluates a query to completion within the call it arrived on, so it
//! leaves no query IN FLIGHT for these operations to find: the queries under test are
//! admitted through the state constructors a query that is still waiting or running is
//! held in (`state::Query::running` / `state::Query::waiting`); everything
//! the assertions look at afterwards goes over the wire.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ComAmazonawsNeptunegraphAmazonneptunegraphState as Store, DEFAULT_QUERY_TIMEOUT,
    MAX_QUERY_STRING_CHARS,
};

use super::query_cancellation::{
    admit_query, cancel_query, emulator, get_query, graph_in_service, termination_due,
};

/// Every member `GetQueryOutput` declares, from
/// `smithy_query(query="operation",operation="GetQuery")`: `id` (String), `queryString`
/// (String), `state` (QueryState), `waited` (Integer), `elapsed` (Integer). A report that
/// omits one of these is a report a client cannot read the query's status from.
const OUTPUT_MEMBERS: &[&str] = &["id", "queryString", "state", "waited", "elapsed"];

/// Slack on a duration read back off the clock, so an assertion about a span that has
/// passed is not a race with the emulator's own scheduling.
const DURATION_SLACK_MILLIS: i64 = 5;

/// The milliseconds of `DEFAULT_QUERY_TIMEOUT`, the upper bound on how long a query this
/// store still reports can have been in flight.
fn query_timeout_millis() -> i64 {
    i64::try_from(DEFAULT_QUERY_TIMEOUT.as_millis()).expect("the timeout fits in an i64")
}

/// The instant the stored query's own timeout runs out, as the store holds it.
fn timeout_due(
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
        .map(|query| query.times_out_at)
}

/// (case, admitted RUNNING, the span of clock the caller lets pass before reading). Each
/// span stays well inside `DEFAULT_QUERY_TIMEOUT`, so the query is still in flight when
/// it is read.
const DURATION_CASES: &[(&str, bool, u64)] = &[
    ("running, read at once", true, 0),
    ("running, read after a short span", true, 80),
    ("running, read after a longer span", true, 200),
    ("waiting, read at once", false, 0),
    ("waiting, read after a short span", false, 80),
    ("waiting, read after a longer span", false, 200),
];

#[tokio::test]
async fn the_reported_durations_measure_the_clock_against_the_stored_instants() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-duration-graph").await;

    for (index, (case, running, span_millis)) in DURATION_CASES.iter().enumerate() {
        let query_id = format!("q-duration-{index}");
        admit_query(&engine, &graph, &query_id, "MATCH (n) RETURN n", *running);
        if *span_millis > 0 {
            tokio::time::sleep(Duration::from_millis(*span_millis)).await;
        }

        let (status, reported) = get_query(&router, &graph, &query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetQuery, got {status}: {reported}"
        );
        let waited = reported["waited"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: waited is not an integer in {reported}"));
        let elapsed = reported["elapsed"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: elapsed is not an integer in {reported}"));
        let span = i64::try_from(*span_millis).expect("the case span fits in an i64");
        let floor = span - DURATION_SLACK_MILLIS;

        if *running {
            // A query that executes from its admission never queued, and the span it has
            // been running is the span the caller let pass.
            assert_eq!(
                waited, 0,
                "case {case:?}: a query that never queued waited 0ms, got {waited}"
            );
            assert!(
                elapsed >= floor,
                "case {case:?}: expected elapsed of at least {floor}ms after {span}ms, got {elapsed}"
            );
            assert!(
                elapsed < query_timeout_millis(),
                "case {case:?}: a query still in flight has run for less than {}ms, got {elapsed}",
                query_timeout_millis()
            );
        } else {
            // A query that has not started has run for no time at all, and the span it
            // has been waiting is the span the caller let pass.
            assert_eq!(
                elapsed, 0,
                "case {case:?}: a query that has not started has run 0ms, got {elapsed}"
            );
            assert!(
                waited >= floor,
                "case {case:?}: expected waited of at least {floor}ms after {span}ms, got {waited}"
            );
            assert!(
                waited < query_timeout_millis(),
                "case {case:?}: a query still in flight has waited less than {}ms, got {waited}",
                query_timeout_millis()
            );
        }
    }
}

/// (case, the query text). The rows differ along the forms a `queryString` takes — the
/// model declares it a plain `String`
/// (`smithy_query(query="operation",operation="GetQuery")`) — so the report has to carry
/// each one back unchanged.
fn query_text_cases() -> Vec<(String, String)> {
    vec![
        (
            "ascii read query".to_string(),
            "MATCH (n:Person) RETURN n LIMIT 1".to_string(),
        ),
        (
            "non-ascii literals".to_string(),
            "MATCH (v:Ville {nom: 'Zürich'})-[:VOISIN]->(w) RETURN w.名前".to_string(),
        ),
        (
            "embedded quotes and newlines".to_string(),
            "MATCH (n)\nWHERE n.note = \"a \\\"quoted\\\" word\"\nRETURN n".to_string(),
        ),
        (
            // Well inside the bound the store puts on a stored text
            // (`state::MAX_QUERY_STRING_CHARS`), so what is under test is the report, not
            // that bound.
            "a long text".to_string(),
            format!(
                "MATCH (n) WHERE n.tag = '{}' RETURN n",
                "x".repeat(MAX_QUERY_STRING_CHARS / 16)
            ),
        ),
        ("whitespace only".to_string(), "  \t ".to_string()),
    ]
}

#[tokio::test]
async fn every_modelled_member_is_reported_from_the_record_in_each_state() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-report-graph").await;

    for (index, (text_case, query_string)) in query_text_cases().iter().enumerate() {
        // The three states a reported query can be in: the two it is admitted in, and
        // the one an accepted cancel puts it in.
        for (state_case, slug, running, cancel) in [
            ("admitted running", "running", true, false),
            ("admitted waiting", "waiting", false, false),
            ("cancelled while running", "cancelled-running", true, true),
            ("cancelled while waiting", "cancelled-waiting", false, true),
        ] {
            let case = format!("{text_case} / {state_case}");
            let query_id = format!("q-report-{index}-{slug}");
            admit_query(&engine, &graph, &query_id, query_string, running);
            let mut expected_state = if running { "RUNNING" } else { "WAITING" };
            if cancel {
                let (status, body) = cancel_query(&router, &graph, &query_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from CancelQuery, got {status}: {body}"
                );
                expected_state = "CANCELLING";
            }

            let (status, reported) = get_query(&router, &graph, &query_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetQuery, got {status}: {reported}"
            );
            for member in OUTPUT_MEMBERS {
                assert!(
                    reported.get(*member).is_some_and(|value| !value.is_null()),
                    "case {case:?}: expected member {member:?} in the report, got {reported}"
                );
            }
            assert_eq!(
                reported["id"],
                json!(query_id),
                "case {case:?}: expected id {query_id:?}, got {}",
                reported["id"]
            );
            assert_eq!(
                reported["queryString"],
                json!(query_string),
                "case {case:?}: the text must come back unchanged, got {}",
                reported["queryString"]
            );
            assert_eq!(
                reported["state"],
                json!(expected_state),
                "case {case:?}: expected state {expected_state}, got {}",
                reported["state"]
            );
            assert!(
                reported["waited"].as_i64().is_some_and(|waited| waited >= 0),
                "case {case:?}: expected a non-negative waited, got {}",
                reported["waited"]
            );
            assert!(
                reported["elapsed"]
                    .as_i64()
                    .is_some_and(|elapsed| elapsed >= 0),
                "case {case:?}: expected a non-negative elapsed, got {}",
                reported["elapsed"]
            );
        }
    }
}

#[tokio::test]
async fn reading_a_query_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let graph = graph_in_service(&router, "query-read-only-graph").await;

    for (case, query_id, running, cancelled) in [
        ("running, never cancelled", "q-read-running", true, false),
        ("waiting, never cancelled", "q-read-waiting", false, false),
        ("stopping after a cancel", "q-read-cancelling", true, true),
    ] {
        let text = "MATCH (n)-[:KNOWS]->(m) RETURN m";
        admit_query(&engine, &graph, query_id, text, running);
        let mut expected_state = if running { "RUNNING" } else { "WAITING" };
        if cancelled {
            let (status, _) = cancel_query(&router, &graph, query_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the cancel under test must be accepted, got {status}"
            );
            expected_state = "CANCELLING";
        }

        // What the record owes the clock before anybody reads it: when its own timeout
        // runs out, and when its accepted cancellation (if any) falls due.
        let times_out_at = timeout_due(&engine, &graph, query_id);
        let terminates_at = termination_due(&engine, &graph, query_id);
        assert!(
            times_out_at.is_some(),
            "case {case:?}: an admitted query runs under a timeout, got {times_out_at:?}"
        );
        assert_eq!(
            terminates_at.is_some(),
            cancelled,
            "case {case:?}: only a cancelled query is owed a termination, got {terminates_at:?}"
        );

        let mut last_elapsed = 0_i64;
        for read in 0..6 {
            let (status, reported) = get_query(&router, &graph, query_id).await;
            assert_eq!(
                status, 200,
                "case {case:?} (read {read}): expected 200 from GetQuery, got {status}: {reported}"
            );
            assert_eq!(
                reported["state"],
                json!(expected_state),
                "case {case:?} (read {read}): a read must not move the state, got {}",
                reported["state"]
            );
            assert_eq!(
                reported["queryString"],
                json!(text),
                "case {case:?} (read {read}): a read must not rewrite the record, got {}",
                reported["queryString"]
            );
            let elapsed = reported["elapsed"].as_i64().unwrap_or_else(|| {
                panic!("case {case:?} (read {read}): elapsed is not an integer in {reported}")
            });
            assert!(
                elapsed >= last_elapsed,
                "case {case:?} (read {read}): elapsed must not run backwards, \
                 {last_elapsed} -> {elapsed}"
            );
            last_elapsed = elapsed;

            // Reading neither shortens the query's life nor starts a termination the
            // caller never asked for.
            assert_eq!(
                timeout_due(&engine, &graph, query_id),
                times_out_at,
                "case {case:?} (read {read}): a read must not move when the query times out"
            );
            assert_eq!(
                termination_due(&engine, &graph, query_id),
                terminates_at,
                "case {case:?} (read {read}): a read must not change the termination owed"
            );
        }

        // Six reads later the query is still a query the next caller can name.
        let (status, reported) = get_query(&router, &graph, query_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the query must survive being read, got {status}: {reported}"
        );
    }
}
