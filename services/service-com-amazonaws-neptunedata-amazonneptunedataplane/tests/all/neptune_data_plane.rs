// SPDX-License-Identifier: CC0-1.0

//! The neptunedata engine status and the Gremlin query registry, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! Two rules are under test:
//!
//! * `GetEngineStatus` reports the ONE engine record this service's state was created
//!   with, so every member comes from that record and a value fixed at engine start
//!   (`startTime`) is the same on every read.
//! * the Gremlin query-status family computes every answer by looking the query up in
//!   the registry: the list reports the queries that are accepted and not completed (the
//!   queued ones only when the request asks for them), the status read reports the stored
//!   query string and the statistics derived from the record's own instants, and a cancel
//!   marks the record cancelled without removing it — so the cancel is idempotent and the
//!   later reads observe one evolving record.
//!
//! The Gremlin endpoints that EVALUATE a traversal are deliberately still 501 (this
//! emulator has no in-process TinkerPop engine), which
//! [`execute_gremlin_query_is_refused_and_stores_nothing`]
//! pins: a result for a traversal nothing ran would be a fabrication. Explaining a query
//! needs no evaluation, and `tests/all/gremlin_explain.rs` covers that endpoint.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_neptunedata_amazonneptunedataplane::state::{
    ComAmazonawsNeptunedataAmazonneptunedataplaneState, GremlinQueryRecord, QueryState,
};
use service_com_amazonaws_neptunedata_amazonneptunedataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. `neptune-db` is neptunedata's sigv4
/// signing name, and it is what routes a request to this service.
const SIGNED_FOR_NEPTUNE_DB: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/neptune-db/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `GetEngineStatusOutput$dfeQueryEngine`'s documented default, spelled out here rather
/// than imported from the crate so a wrong default in `src/state.rs` fails this test.
const DFE_QUERY_ENGINE_DEFAULT: &str = "viaQueryHint";

/// The setting `GetEngineStatusOutput$settings` documents it carries, and the default of
/// the `neptune_query_timeout` cluster parameter it reflects, in milliseconds.
const CLUSTER_QUERY_TIMEOUT: (&str, &str) = ("clusterQueryTimeoutInMs", "120000");

/// One emulator: a router carrying only this service, over one shared engine. The engine
/// is handed back so a test can seed the registry the way whatever admits a query would.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "neptune-db.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_NEPTUNE_DB);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn engine_status(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/status", None).await
}

async fn list_queries(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/gremlin/status{query}"), None).await
}

async fn query_status(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/gremlin/status/{id}"), None).await
}

async fn cancel_query(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "DELETE", &format!("/gremlin/status/{id}"), None).await
}

/// One seeded query: the record to put in the registry, plus what the status family must
/// then report for it. The rows differ along everything the rules generalise over — the
/// query string's form and length, how far the query has got, how many subqueries it
/// has, and whether its wait and evaluation spans are closed or still open.
#[derive(Debug, Clone, Copy)]
struct Case {
    id: &'static str,
    query: &'static str,
    state: QueryState,
    subqueries: u64,
    /// Seconds before "now" the engine accepted the query.
    accepted_secs_ago: u64,
    /// Seconds before "now" evaluation began, if it has.
    started_secs_ago: Option<u64>,
    /// Seconds before "now" evaluation ended, if it has.
    finished_secs_ago: Option<u64>,
    /// `waited` the record must report: both instants are stored, so it is exact.
    expected_waited_ms: Option<i32>,
    /// `elapsed` the record must report when the evaluation span is closed.
    expected_elapsed_ms: Option<i32>,
}

/// The seeded rows. Ids are the UUID-shaped strings Neptune reports a query under.
const CASES: [Case; 5] = [
    // Queued, never started: still waiting, so `elapsed` is 0 and `waited` keeps growing.
    Case {
        id: "0f0c1a1e-0000-4000-8000-000000000001",
        query: "g.V().limit(1)",
        state: QueryState::Waiting,
        subqueries: 0,
        accepted_secs_ago: 5,
        started_secs_ago: None,
        finished_secs_ago: None,
        expected_waited_ms: None,
        expected_elapsed_ms: Some(0),
    },
    // Running, with a closed wait span: `waited` is exactly the gap between the two
    // stored instants.
    Case {
        id: "0f0c1a1e-0000-4000-8000-000000000002",
        query: "g.V().hasLabel('person').out('knows').values('name').dedup()",
        state: QueryState::Running,
        subqueries: 3,
        accepted_secs_ago: 5,
        started_secs_ago: Some(4),
        finished_secs_ago: None,
        expected_waited_ms: Some(1_000),
        expected_elapsed_ms: None,
    },
    // Running, started the instant it was accepted: a zero wait is a real value.
    Case {
        id: "0f0c1a1e-0000-4000-8000-000000000003",
        query: "g.addV('person').property('name','Ärger: a \"quoted\" ünïcode name')",
        state: QueryState::Running,
        subqueries: 1,
        accepted_secs_ago: 2,
        started_secs_ago: Some(2),
        finished_secs_ago: None,
        expected_waited_ms: Some(0),
        expected_elapsed_ms: None,
    },
    // Completed: both spans are closed, so both statistics are exact and frozen.
    Case {
        id: "0f0c1a1e-0000-4000-8000-000000000004",
        query: "g.V().count()",
        state: QueryState::Done,
        subqueries: 7,
        accepted_secs_ago: 9,
        started_secs_ago: Some(8),
        finished_secs_ago: Some(7),
        expected_waited_ms: Some(1_000),
        expected_elapsed_ms: Some(1_000),
    },
    // A long, multi-step traversal, still queued: the stored string round-trips whole.
    Case {
        id: "0f0c1a1e-0000-4000-8000-000000000005",
        query: "g.V().hasLabel('airport').has('code','SEA').repeat(out('route').simplePath())\
                .times(2).path().by('code').limit(10)",
        state: QueryState::Waiting,
        subqueries: 12,
        accepted_secs_ago: 3,
        started_secs_ago: None,
        finished_secs_ago: None,
        expected_waited_ms: None,
        expected_elapsed_ms: Some(0),
    },
];

/// The ids of the rows in `state`.
fn ids_in_state(state: QueryState) -> Vec<&'static str> {
    CASES
        .iter()
        .filter(|case| case.state == state)
        .map(|case| case.id)
        .collect()
}

/// Put [`CASES`] in the registry, the way whatever admits a query would.
fn seed(engine: &engine::InMemoryEngine) {
    let now = SystemTime::now();
    let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    let mut store = state.lock();
    for case in &CASES {
        let mut record = GremlinQueryRecord::accepted(
            case.id,
            case.query,
            now - Duration::from_secs(case.accepted_secs_ago),
        );
        record.state = case.state;
        record.subqueries = case.subqueries;
        record.started_at = case
            .started_secs_ago
            .map(|secs| now - Duration::from_secs(secs));
        record.finished_at = case
            .finished_secs_ago
            .map(|secs| now - Duration::from_secs(secs));
        store.gremlin_queries.insert(record.query_id.clone(), record);
    }
}

/// The `queryId`s a list answer carries, in the order it carries them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["queries"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|query| query["queryId"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn engine_status_reports_the_one_stored_engine_record() {
    let Emulator { router, .. } = emulator();

    let first = engine_status(&router).await;
    assert_eq!(
        first.status, 200,
        "GET /status: expected 200, got {} with body {:?}",
        first.status, first.body
    );

    // The members the model declares, each answered from the stored record.
    assert_eq!(
        first.body["status"].as_str(),
        Some("healthy"),
        "status: expected the healthy instance the record holds, got {:?}",
        first.body["status"]
    );
    assert_eq!(
        first.body["role"].as_str(),
        Some("writer"),
        "role: this emulator serves the primary instance, got {:?}",
        first.body["role"]
    );
    assert_eq!(
        first.body["dfeQueryEngine"].as_str(),
        Some(DFE_QUERY_ENGINE_DEFAULT),
        "dfeQueryEngine: expected the documented default, got {:?}",
        first.body["dfeQueryEngine"]
    );

    let engine_version = first.body["dbEngineVersion"].as_str().unwrap_or_default();
    assert!(
        !engine_version.is_empty() && !engine_version.starts_with("Patch-"),
        "dbEngineVersion: expected an unpatched engine version, got {engine_version:?}"
    );

    // Every query language the engine serves reports a version.
    // Source: smithy_query(query="shape", shape="GetEngineStatusOutput") — `gremlin`,
    // `sparql` and `opencypher` each target QueryLanguageVersion{version}.
    for language in ["gremlin", "sparql", "opencypher"] {
        let version = first.body[language]["version"].as_str();
        assert!(
            version.is_some_and(|value| !value.is_empty()),
            "case {language}: expected a non-empty version, got {:?}",
            first.body[language]
        );
    }

    // `labMode` and `settings` are string-valued maps; `settings` carries the cluster
    // query timeout the member's own documentation names.
    assert!(
        first.body["labMode"].is_object(),
        "labMode: expected a map of lab-mode settings, got {:?}",
        first.body["labMode"]
    );
    let (timeout_key, timeout_value) = CLUSTER_QUERY_TIMEOUT;
    assert_eq!(
        first.body["settings"][timeout_key].as_str(),
        Some(timeout_value),
        "settings.{timeout_key}: expected the cluster query timeout, got {:?}",
        first.body["settings"]
    );

    // `features` is a DOCUMENT-valued map: each feature reports its own status object.
    let features = first.body["features"]
        .as_object()
        .unwrap_or_else(|| panic!("features: expected a map, got {:?}", first.body["features"]));
    assert!(
        !features.is_empty(),
        "features: expected the engine's feature statuses, got an empty map"
    );
    for (name, feature) in features {
        assert!(
            feature["status"].is_string(),
            "case {name}: expected a feature status document, got {feature:?}"
        );
    }

    // "If there are transactions being rolled back, this field is set to the number of
    // such transactions. If there are none, the field doesn't appear at all" — and the
    // earliest-start member goes the same way. Nothing is rolling back in a freshly
    // started engine, so neither member is on the wire.
    for member in ["rollingBackTrxCount", "rollingBackTrxEarliestStartTime"] {
        assert!(
            first.body[member].is_null(),
            "case {member}: must be absent while nothing is rolling back, got {:?}",
            first.body[member]
        );
    }

    // `startTime` is fixed when the engine record is created, so a later read answers
    // the same instant rather than a fresh clock reading.
    let start_time = first.body["startTime"].as_str().unwrap_or_default().to_string();
    assert!(
        start_time.contains('T') && start_time.ends_with('Z'),
        "startTime: expected a UTC timestamp, got {start_time:?}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = engine_status(&router).await;
    assert_eq!(
        second.body["startTime"].as_str(),
        Some(start_time.as_str()),
        "startTime must be stable across reads: got {:?} then {:?}",
        start_time,
        second.body["startTime"]
    );
    assert_eq!(
        second.body, first.body,
        "a second read of an unchanged engine record must answer identically"
    );
}

/// The rollback members are reported only while the engine record holds a rollback —
/// the other side of the rule [`engine_status_reports_the_one_stored_engine_record`]
/// pins. Both rows differ in exactly the stored state the rule keys on.
#[tokio::test]
async fn engine_status_reports_a_rollback_only_while_one_is_in_progress() {
    let Emulator { router, engine } = emulator();

    // Rows: the rollback count on the record, whether the record also holds an earliest
    // rollback instant, and what must then be on the wire — the count, and the instant
    // only when the record holds one. The count is what says the engine is rolling
    // something back: a record with a count and no instant reports the count alone, and a
    // record with an instant but no count reports neither member.
    let rows: [(i32, bool); 5] = [
        (0, false),
        (1, true),
        (7, true),
        (0, true),
        (3, false),
    ];
    for (count, store_instant) in rows {
        let earliest = SystemTime::now() - Duration::from_secs(30);
        {
            let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
            let mut store = state.lock();
            store.engine_status.rolling_back_trx_count = count;
            store.engine_status.rolling_back_trx_earliest_start_time =
                store_instant.then_some(earliest);
        }

        let answer = engine_status(&router).await;
        assert_eq!(
            answer.status, 200,
            "case {count}/{store_instant}: expected 200, got {} with body {:?}",
            answer.status, answer.body
        );
        let rolling_back = count > 0;
        if rolling_back {
            assert_eq!(
                answer.body["rollingBackTrxCount"].as_i64(),
                Some(i64::from(count)),
                "case {count}/{store_instant}: expected the stored rollback count, got {:?}",
                answer.body["rollingBackTrxCount"]
            );
        } else {
            assert!(
                answer.body["rollingBackTrxCount"].is_null(),
                "case {count}/{store_instant}: the count must be absent with no rollback, got {:?}",
                answer.body["rollingBackTrxCount"]
            );
        }
        if rolling_back && store_instant {
            let started = answer.body["rollingBackTrxEarliestStartTime"]
                .as_str()
                .unwrap_or_default();
            assert!(
                started.contains('T') && started.ends_with('Z'),
                "case {count}/{store_instant}: expected the earliest rollback start time, got {started:?}"
            );
        } else {
            assert!(
                answer.body["rollingBackTrxEarliestStartTime"].is_null(),
                "case {count}/{store_instant}: the earliest start must be absent unless a rollback records one, got {:?}",
                answer.body["rollingBackTrxEarliestStartTime"]
            );
        }
    }
}

#[tokio::test]
async fn list_reports_the_accepted_queries_and_includes_waiting_only_on_request() {
    let Emulator { router, engine } = emulator();

    let empty = list_queries(&router, "").await;
    assert_eq!(
        empty.status, 200,
        "GET /gremlin/status on an empty registry: expected 200, got {} with body {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        (
            empty.body["acceptedQueryCount"].as_i64(),
            empty.body["runningQueryCount"].as_i64(),
            listed_ids(&empty).len()
        ),
        (Some(0), Some(0), 0),
        "an empty registry must report no queries, got {:?}",
        empty.body
    );

    seed(&engine);
    let running = ids_in_state(QueryState::Running);
    let waiting = ids_in_state(QueryState::Waiting);
    let accepted = i64::try_from(running.len() + waiting.len()).expect("small count");

    // Each row is one way the request can ask about waiting queries, and what the
    // answer must then hold.
    let rows: [(&str, bool); 3] = [
        ("", false),
        ("?includeWaiting=false", false),
        ("?includeWaiting=true", true),
    ];
    for (query, include_waiting) in rows {
        let answer = list_queries(&router, query).await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}: expected 200, got {} with body {:?}",
            answer.status, answer.body
        );

        let mut expected: Vec<&str> = running.clone();
        if include_waiting {
            expected.extend(waiting.iter().copied());
        }
        expected.sort_unstable();
        let mut listed = listed_ids(&answer);
        listed.sort();
        assert_eq!(
            listed,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {query:?}: expected exactly the current queries, got {:?}",
            answer.body["queries"]
        );

        // The counts are of the registry, not of the list: the queued queries count as
        // accepted whether or not this request asked to see them.
        assert_eq!(
            answer.body["acceptedQueryCount"].as_i64(),
            Some(accepted),
            "case {query:?}: acceptedQueryCount must count every accepted, uncompleted query, got {:?}",
            answer.body["acceptedQueryCount"]
        );
        assert_eq!(
            answer.body["runningQueryCount"].as_i64(),
            Some(i64::try_from(running.len()).expect("small count")),
            "case {query:?}: runningQueryCount must count the running queries, got {:?}",
            answer.body["runningQueryCount"]
        );

        // Which count the list matches is exactly what `includeWaiting` chose.
        let expected_len = if include_waiting {
            answer.body["acceptedQueryCount"].as_i64()
        } else {
            answer.body["runningQueryCount"].as_i64()
        };
        assert_eq!(
            Some(i64::try_from(listed.len()).expect("small count")),
            expected_len,
            "case {query:?}: the list length and the count it reflects must agree, got {:?}",
            answer.body
        );
    }

    // A completed query is no longer one of "the current queries", however it is asked
    // for.
    let done = ids_in_state(QueryState::Done);
    let with_waiting = list_queries(&router, "?includeWaiting=true").await;
    for id in done {
        assert!(
            !listed_ids(&with_waiting).contains(&id.to_string()),
            "case {id}: a completed query must not be listed, got {:?}",
            with_waiting.body["queries"]
        );
    }
}

#[tokio::test]
async fn query_status_reports_the_stored_record_for_every_seeded_query() {
    let Emulator { router, engine } = emulator();
    seed(&engine);

    for case in &CASES {
        let answer = query_status(&router, case.id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["queryId"].as_str(),
            Some(case.id),
            "case {case:?}: queryId must be the id looked up, got {:?}",
            answer.body["queryId"]
        );
        assert_eq!(
            answer.body["queryString"].as_str(),
            Some(case.query),
            "case {case:?}: queryString must be the stored query, got {:?}",
            answer.body["queryString"]
        );

        let stats = &answer.body["queryEvalStats"];
        assert_eq!(
            stats["cancelled"].as_bool(),
            Some(false),
            "case {case:?}: an uncancelled query must report cancelled false, got {stats:?}"
        );
        assert_eq!(
            stats["subqueries"].as_u64(),
            Some(case.subqueries),
            "case {case:?}: subqueries must be the stored count, got {stats:?}"
        );

        match case.expected_waited_ms {
            // Both ends of the wait are stored, so the value is exact.
            Some(waited) => assert_eq!(
                stats["waited"].as_i64(),
                Some(i64::from(waited)),
                "case {case:?}: waited must be the gap between the stored instants, got {stats:?}"
            ),
            // Still queued: the wait is open, so it is at least as long as the seed.
            None => {
                let seeded = i64::try_from(case.accepted_secs_ago * 1_000).expect("small count");
                assert!(
                    stats["waited"].as_i64().is_some_and(|waited| waited >= seeded),
                    "case {case:?}: a queued query must report at least {seeded} ms waited, got {stats:?}"
                );
            }
        }
        match case.expected_elapsed_ms {
            Some(elapsed) => assert_eq!(
                stats["elapsed"].as_i64(),
                Some(i64::from(elapsed)),
                "case {case:?}: elapsed must be the stored evaluation span, got {stats:?}"
            ),
            None => {
                let started = case.started_secs_ago.expect("an open span has a start");
                let seeded = i64::try_from(started * 1_000).expect("small count");
                assert!(
                    stats["elapsed"].as_i64().is_some_and(|elapsed| elapsed >= seeded),
                    "case {case:?}: a running query must report at least {seeded} ms elapsed, got {stats:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unknown_query_id_is_refused_by_both_the_status_read_and_the_cancel() {
    let Emulator { router, engine } = emulator();
    seed(&engine);

    // Ids that name no record: a well-formed id that was never seeded, a prefix of a
    // seeded one, and a value that is not UUID-shaped at all.
    let unknown = [
        "0f0c1a1e-0000-4000-8000-00000000beef",
        "0f0c1a1e-0000-4000-8000-00000000000",
        "not-a-query-id",
    ];
    let mut request_ids: Vec<String> = Vec::new();
    for id in unknown {
        for (method, answer) in [
            ("GET", query_status(&router, id).await),
            ("DELETE", cancel_query(&router, id).await),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {method} {id}: an unknown queryId must be refused, got {} with body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code().rsplit('#').next(),
                Some("InvalidParameterException"),
                "case {method} {id}: expected the modelled InvalidParameterException, got {:?}",
                answer.body
            );
            let detail = answer.body["detailedMessage"].as_str().unwrap_or_default();
            assert!(
                detail.contains(id),
                "case {method} {id}: the message must name the value that was wrong, got {detail:?}"
            );
            assert_eq!(
                answer.body["code"].as_str(),
                Some("400"),
                "case {method} {id}: code carries the HTTP status, got {:?}",
                answer.body["code"]
            );
            // Each error names its own request: the id is non-empty and no two responses
            // share one, which is the whole requirement on the member.
            let request_id = answer.body["requestId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !request_id.is_empty(),
                "case {method} {id}: the error must name its request, got {:?}",
                answer.body["requestId"]
            );
            assert!(
                !request_ids.contains(&request_id),
                "case {method} {id}: requestId {request_id:?} was already used by an earlier response"
            );
            request_ids.push(request_id);
        }
    }

    // Refusing a lookup changed nothing: the seeded registry is still exactly itself.
    let listed = list_queries(&router, "?includeWaiting=true").await;
    let mut expected: Vec<&str> = ids_in_state(QueryState::Running);
    expected.extend(ids_in_state(QueryState::Waiting));
    expected.sort_unstable();
    let mut got = listed_ids(&listed);
    got.sort();
    assert_eq!(
        got,
        expected
            .iter()
            .map(|id| (*id).to_string())
            .collect::<Vec<_>>(),
        "a refused lookup must not change the registry, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn cancel_marks_the_record_and_a_repeated_cancel_answers_the_same() {
    let Emulator { router, engine } = emulator();
    seed(&engine);

    for case in &CASES {
        let first = cancel_query(&router, case.id).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200, got {} with body {:?}",
            first.status, first.body
        );
        assert!(
            first.body["status"]
                .as_str()
                .is_some_and(|status| !status.is_empty()),
            "case {case:?}: the cancellation must report its status, got {:?}",
            first.body
        );

        // `@idempotent`: the second cancel of one id answers exactly like the first.
        let second = cancel_query(&router, case.id).await;
        assert_eq!(
            (second.status, &second.body),
            (first.status, &first.body),
            "case {case:?}: a repeated cancel must not answer differently"
        );

        let after = query_status(&router, case.id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the record must survive its cancel, got {} with body {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.body["queryString"].as_str(),
            Some(case.query),
            "case {case:?}: cancelling must not change the stored query, got {:?}",
            after.body["queryString"]
        );

        // A query that had already completed cannot be cancelled; every other one is.
        let expected_cancelled = case.state != QueryState::Done;
        assert_eq!(
            after.body["queryEvalStats"]["cancelled"].as_bool(),
            Some(expected_cancelled),
            "case {case:?}: expected cancelled {expected_cancelled}, got {:?}",
            after.body["queryEvalStats"]
        );

        // A finished query's spans are closed, so neither statistic moves any more —
        // including the WAIT of a query that was cancelled before it ever started.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let again = query_status(&router, case.id).await;
        for statistic in ["waited", "elapsed"] {
            assert_eq!(
                again.body["queryEvalStats"][statistic].as_i64(),
                after.body["queryEvalStats"][statistic].as_i64(),
                "case {case:?}/{statistic}: must be frozen once the query has finished, got {:?} then {:?}",
                after.body["queryEvalStats"],
                again.body["queryEvalStats"]
            );
        }
    }

    // Nothing is accepted or running any more, so nothing is listed.
    let listed = list_queries(&router, "?includeWaiting=true").await;
    assert_eq!(
        (
            listed.body["acceptedQueryCount"].as_i64(),
            listed.body["runningQueryCount"].as_i64(),
            listed_ids(&listed).len()
        ),
        (Some(0), Some(0), 0),
        "after cancelling every query the list must be empty, got {:?}",
        listed.body
    );
}

/// The Gremlin EXECUTION endpoint is still a stub, and must stay one until this emulator
/// can really evaluate a traversal: a `requestId`, a `result` or a query record for a
/// traversal nothing ran would be fabricated. A 501 says "nobody has written it yet",
/// which is the truth, and it must not leave anything behind in the registry.
#[tokio::test]
async fn execute_gremlin_query_is_refused_and_stores_nothing() {
    let Emulator { router, .. } = emulator();

    let answer = call(
        &router,
        "POST",
        "/gremlin",
        Some(serde_json::json!({"gremlin": "g.V().limit(1)"})),
    )
    .await;
    assert_eq!(
        answer.status, 501,
        "POST /gremlin: expected the scaffold's 501 while no engine can run a traversal, got {} with body {:?}",
        answer.status, answer.body
    );

    let listed = list_queries(&router, "?includeWaiting=true").await;
    assert_eq!(
        (
            listed.body["acceptedQueryCount"].as_i64(),
            listed_ids(&listed).len()
        ),
        (Some(0), 0),
        "a refused execution must not record a query, got {:?}",
        listed.body
    );
}
