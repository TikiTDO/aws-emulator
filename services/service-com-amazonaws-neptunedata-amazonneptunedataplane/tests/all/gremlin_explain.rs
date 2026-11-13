// SPDX-License-Identifier: CC0-1.0

//! `ExecuteGremlinExplainQuery` end to end over HTTP.
//!
//! The rule under test: an explain answers with the report for the traversal that was
//! SUBMITTED. Every line of that report follows from the query — the query string, the
//! steps the query is made of, and the quad patterns the plan matches, with the steps the
//! planner does not push down left for the step iterators — so a query that is not a
//! traversal is refused instead of explained, the same query always explains the same
//! way, and the query the engine read and planned is recorded in the registry the Gremlin
//! status family reports from.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_neptunedata_amazonneptunedataplane::state::{
    ComAmazonawsNeptunedataAmazonneptunedataplaneState, QueryState, MAX_ARGUMENT_DEPTH,
    MAX_QUERY_LENGTH,
};
use service_com_amazonaws_neptunedata_amazonneptunedataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. `neptune-db` is neptunedata's sigv4
/// signing name, and it is what routes a request to this service.
const SIGNED_FOR_NEPTUNE_DB: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/neptune-db/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The title the explain report is headed with, which is also what tells the report
/// apart from a JSON error body on the wire.
const REPORT_TITLE: &str = "Neptune Gremlin Explain";

/// One emulator: a router carrying only this service, over one shared engine. The engine
/// is handed back so a test can read the query registry the handler writes.
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

/// What the emulator answered: the status and the raw body, which on a success is the
/// report text rather than JSON.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The wire error code (`__type`) without its shape namespace, or the empty string
    /// when the body is not a modelled error.
    fn error_code(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| {
                body["__type"]
                    .as_str()
                    .and_then(|code| code.rsplit('#').next())
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    fn detailed_message(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| body["detailedMessage"].as_str().map(str::to_owned))
            .unwrap_or_default()
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
    Answer {
        status,
        body: response.into_body().to_vec(),
    }
}

/// `ExecuteGremlinExplainQuery` is `POST /gremlin/explain`, with the query in the body
/// member `gremlin`.
/// Source: smithy_query(query="operation", operation="ExecuteGremlinExplainQuery") — the
/// http binding and `gremlinQuery`'s `jsonName`.
async fn explain(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "POST",
        "/gremlin/explain",
        Some(json!({ "gremlin": query })),
    )
    .await
}

/// The lines of one section of the report: the lines after the section's name and its
/// underline, up to the next blank line.
fn section(report: &str, name: &str) -> Vec<String> {
    let mut lines = report.lines();
    assert!(
        lines.by_ref().any(|line| line == name),
        "the report has no {name:?} section:\n{report}"
    );
    let underline = lines.next().unwrap_or_default();
    assert!(
        !underline.is_empty() && underline.chars().all(|character| character == '='),
        "the {name:?} section is not underlined, found {underline:?}:\n{report}"
    );
    lines
        .take_while(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// One query to explain, and what the report for it must say. The rows differ along
/// everything the rule generalises over: how the start is found, which steps the plan
/// pushes down and which it leaves behind, how arguments are written, and which steps and
/// modulators the query is made of.
#[derive(Debug, Clone, Copy)]
struct Case {
    case: &'static str,
    query: &'static str,
    /// The whole "Original Traversal" line: the steps the query compiles to.
    traversal: &'static str,
    /// Lines the plan must carry, each compared with a whole line of the section.
    plan: &'static [&'static str],
    /// Text no line of the plan may carry.
    absent: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        case: "a bare vertex scan is found by label",
        query: "g.V()",
        traversal: "[GraphStep(vertex,[])]",
        plan: &[
            "+- NeptuneGraphQueryStep(Vertex) {",
            "PatternNode[(?1, <~label>, ?2, <~>) . project ?1,?2 .]",
            "}, annotations={path=[Vertex(?1):GraphNode], maxVarId=3}",
            "+- NeptuneTraverserConverterStep",
        ],
        absent: &[],
    },
    Case {
        case: "a property filter is what finds the start",
        query: "g.V().has('code','ANC')",
        traversal: "[GraphStep(vertex,[]), HasStep([code.eq(ANC)])]",
        plan: &[
            "PatternNode[(?1, <code>, \"ANC\", <~>) . project ?1 .]",
            "}, annotations={path=[Vertex(?1):GraphNode], maxVarId=2}",
        ],
        // The filter binds the start, so no label scan is planned for it.
        absent: &["<~label>"],
    },
    Case {
        case: "a label filter and an edge walk, with the range left to the step iterators",
        query: "g.V().hasLabel(\"airport\").out('route').limit(5)",
        traversal: "[GraphStep(vertex,[]), HasStep([~label.eq(airport)]), \
                    VertexStep(OUT,[route],vertex), RangeGlobalStep(0,5)]",
        plan: &[
            "PatternNode[(?1, <~label>, \"airport\", <~>) . project ?1 .]",
            "PatternNode[(?1, <route>, ?2, ?3) . project ?1,?2 .]",
            "}, annotations={path=[Vertex(?1):GraphNode, Vertex(?2):GraphNode], maxVarId=4}",
            "+- RangeGlobalStep(0,5)",
        ],
        absent: &[],
    },
    Case {
        case: "an id-bound start needs no lookup, and a property binds a value",
        query: "g.V('3').values('code')",
        traversal: "[GraphStep(vertex,[3]), PropertiesStep([code],value)]",
        plan: &[
            "+- NeptuneGraphQueryStep(Value) {",
            "PatternNode[(\"3\", <~label>, ?1, <~>) . project ?1 .]",
            "PatternNode[(\"3\", <code>, ?2, <~>) . project ?2 .]",
            "}, annotations={path=[Vertex(\"3\"):GraphNode, Value(?2):ValueNode], maxVarId=3}",
        ],
        absent: &[],
    },
    Case {
        case: "an edge walk binds both endpoints, so its endpoint step matches nothing more",
        query: "g.V().outE('route').inV()",
        traversal: "[GraphStep(vertex,[]), VertexStep(OUT,[route],edge), EdgeVertexStep(IN)]",
        plan: &[
            "+- NeptuneGraphQueryStep(Vertex) {",
            "PatternNode[(?1, <~label>, ?2, <~>) . project ?1,?2 .]",
            "PatternNode[(?1, <route>, ?3, ?4) . project ?1,?3,?4 .]",
            "}, annotations={path=[Vertex(?1):GraphNode, Vertex(?3):GraphNode, \
             Edge(?4):GraphNode], maxVarId=5}",
        ],
        absent: &[],
    },
    Case {
        case: "a predicate is not a value, so the filter stays with the step iterators",
        query: "g.V().has('age',gt(30))",
        traversal: "[GraphStep(vertex,[]), HasStep([age.gt(30)])]",
        plan: &[
            "PatternNode[(?1, <~label>, ?2, <~>) . project ?1,?2 .]",
            "+- HasStep([age.gt(30)])",
        ],
        absent: &[],
    },
    Case {
        case: "several labels are alternatives, which one pattern does not express",
        query: "g.V().hasLabel('airport','country')",
        traversal: "[GraphStep(vertex,[]), HasStep([~label.within(airport,country)])]",
        plan: &["+- HasStep([~label.within(airport,country)])"],
        absent: &[],
    },
    Case {
        case: "whitespace and the other quotation do not change the reading",
        query: "  g . V ( ) . has ( \"code\" , 'ANC' )  ",
        traversal: "[GraphStep(vertex,[]), HasStep([code.eq(ANC)])]",
        plan: &["PatternNode[(?1, <code>, \"ANC\", <~>) . project ?1 .]"],
        absent: &["<~label>"],
    },
    Case {
        case: "a traversal action asks for the answer, so it is not a step",
        query: "g.V().count().next()",
        traversal: "[GraphStep(vertex,[]), CountGlobalStep]",
        plan: &["+- CountGlobalStep"],
        absent: &["NextStep"],
    },
    Case {
        case: "a modulator is printed on the step it configures",
        query: "g.V().order().by('code')",
        traversal: "[GraphStep(vertex,[]), OrderGlobalStep.by(code)]",
        plan: &["+- OrderGlobalStep.by(code)"],
        absent: &["ByStep"],
    },
    Case {
        case: "a step label is printed on its step, which is then not pushed down",
        query: "g.V().as('a').out()",
        traversal: "[GraphStep(vertex,[])@[a], VertexStep(OUT,vertex)]",
        plan: &[
            "+- GraphStep(vertex,[])@[a]",
            "+- VertexStep(OUT,vertex)",
        ],
        absent: &["NeptuneGraphQueryStep", "PatternNode"],
    },
    Case {
        case: "a step this engine does not name explicitly keeps the step-class convention",
        query: "g.V().coalesce(__.out('a'),__.in('b'))",
        traversal: "[GraphStep(vertex,[]), CoalesceStep(__.out(a),__.in(b))]",
        plan: &["+- CoalesceStep(__.out(a),__.in(b))"],
        absent: &[],
    },
];

#[tokio::test]
async fn the_report_is_computed_from_the_submitted_traversal() {
    let Emulator { router, .. } = emulator();
    for case in CASES {
        let answer = explain(&router, case.query).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected 200, got {} with body {:?}",
            case.case,
            answer.status,
            answer.text()
        );
        assert!(
            answer.text().lines().any(|line| line == REPORT_TITLE),
            "case {:?}: the answer must be the explain report, got {:?}",
            case.case,
            answer.text()
        );
        let report = answer.text();

        assert_eq!(
            section(&report, "Query String"),
            vec![case.query.trim().to_string()],
            "case {:?}: the report must echo the query as submitted, got:\n{report}",
            case.case
        );
        assert_eq!(
            section(&report, "Original Traversal"),
            vec![case.traversal.to_string()],
            "case {:?}: expected the steps {:?}, got:\n{report}",
            case.case,
            case.traversal
        );

        let plan = section(&report, "Optimized Traversal");
        for expected in case.plan {
            assert!(
                plan.iter().any(|line| line.trim() == *expected),
                "case {:?}: the plan must carry the line {expected:?}, got {plan:?}",
                case.case
            );
        }
        for forbidden in case.absent {
            assert!(
                !plan.iter().any(|line| line.contains(forbidden)),
                "case {:?}: the plan must not carry {forbidden:?}, got {plan:?}",
                case.case
            );
        }
    }
}

/// The report is derived from the query alone, so explaining the same query twice answers
/// the same report — there is no per-run value in it.
#[tokio::test]
async fn the_same_query_explains_the_same_way_twice() {
    let Emulator { router, .. } = emulator();
    for case in CASES {
        let first = explain(&router, case.query).await;
        let second = explain(&router, case.query).await;
        assert_eq!(
            first.text(),
            second.text(),
            "case {:?}: two explains of one query must answer the same report",
            case.case
        );
    }
}

/// The query the engine read and planned is recorded, and the status family reports it
/// from that record: the explain is answered within the call, so the query is finished
/// and is no longer one of the current queries the list reports.
#[tokio::test]
async fn an_explained_query_is_recorded_and_read_back_through_the_status_api() {
    let Emulator { router, engine } = emulator();
    let query = "g.V().hasLabel('airport').has('code','ANC')";

    let answer = explain(&router, query).await;
    assert_eq!(
        answer.status,
        200,
        "the explain must be served, got {} with body {:?}",
        answer.status,
        answer.text()
    );

    let store = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    let recorded = store
        .lock()
        .gremlin_queries
        .values()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        recorded.len(),
        1,
        "the explained query must be the one record in the registry, got {recorded:?}"
    );
    let record = &recorded[0];
    assert_eq!(
        record.query_string, query,
        "the record must hold the query as submitted, got {:?}",
        record.query_string
    );
    assert_eq!(
        record.state,
        QueryState::Done,
        "an explain is answered within the call, so its query is finished, got {:?}",
        record.state
    );

    let status = call(
        &router,
        "GET",
        &format!("/gremlin/status/{}", record.query_id),
        None,
    )
    .await;
    assert_eq!(
        status.status,
        200,
        "the recorded query id must be readable, got {} with body {:?}",
        status.status,
        status.text()
    );
    let body: Value = serde_json::from_slice(&status.body)
        .unwrap_or_else(|error| panic!("the status answer must be JSON ({error}): {:?}", status.text()));
    assert_eq!(
        body["queryString"].as_str(),
        Some(query),
        "the status read must report the stored query string, got {body:?}"
    );
    assert_eq!(
        body["queryEvalStats"]["cancelled"].as_bool(),
        Some(false),
        "an explained query was not cancelled, got {body:?}"
    );

    let listed = call(&router, "GET", "/gremlin/status", None).await;
    let listed_body: Value = serde_json::from_slice(&listed.body)
        .unwrap_or_else(|error| panic!("the list answer must be JSON ({error}): {:?}", listed.text()));
    assert_eq!(
        listed_body["acceptedQueryCount"].as_i64(),
        Some(0),
        "a finished query is not one the engine still owes an answer, got {listed_body:?}"
    );
    assert!(
        listed_body["queries"]
            .as_array()
            .is_none_or(|queries| queries.is_empty()),
        "a finished query is not one of the current queries, got {listed_body:?}"
    );
}

/// One query that is not a traversal this engine can plan, and the refusal it must get.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    case: &'static str,
    query: &'static str,
    /// The wire error code, without its shape namespace.
    code: &'static str,
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "an argument list that is never closed",
        query: "g.V(",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a query that starts from no traversal source",
        query: "V().count()",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a query that starts from another name",
        query: "h.V().count()",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "two dots between steps",
        query: "g.V()..count()",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a trailing dot",
        query: "g.V().",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a string that is never closed",
        query: "g.V().has('code)",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a character the Gremlin string form has no token for",
        query: "g.V().filter{it.get()}",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a missing comma between arguments",
        query: "g.V().has('code' 'ANC')",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a modulator with no step before it",
        query: "g.by('code')",
        code: "MalformedQueryException",
    },
    Refusal {
        case: "a query that is only whitespace",
        query: "   ",
        code: "MissingParameterException",
    },
    Refusal {
        case: "an empty query",
        query: "",
        code: "MissingParameterException",
    },
];

#[tokio::test]
async fn a_query_that_is_not_a_traversal_is_refused_and_recorded_nowhere() {
    let Emulator { router, engine } = emulator();
    for refusal in REFUSALS {
        let answer = explain(&router, refusal.query).await;
        assert_eq!(
            answer.status,
            400,
            "case {:?}: expected 400, got {} with body {:?}",
            refusal.case,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            refusal.code,
            "case {:?}: expected {}, got body {:?}",
            refusal.case,
            refusal.code,
            answer.text()
        );
        assert!(
            !answer.detailed_message().is_empty(),
            "case {:?}: the refusal must say what was wrong, got body {:?}",
            refusal.case,
            answer.text()
        );

        let store = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
        let recorded = store.lock().gremlin_queries.len();
        assert_eq!(
            recorded, 0,
            "case {:?}: a refused query must leave the registry empty, it holds {recorded}",
            refusal.case
        );
    }
}

/// A traversal of `steps` walks, which is a long query that is not a nested one.
fn chained(steps: usize) -> String {
    let mut query = "g.V()".to_string();
    for _ in 0..steps {
        query.push_str(".out('route')");
    }
    query
}

/// The length bound is a limit on the query, not on what it says: a query inside the
/// bound is explained however long it is, and one past it is refused as too large rather
/// than read.
#[tokio::test]
async fn length_is_bounded_at_the_published_size() {
    let Emulator { router, .. } = emulator();

    // The longest chain of walks that still fits inside the bound.
    let inside = chained((MAX_QUERY_LENGTH - "g.V()".len()) / ".out('route')".len());
    assert!(
        inside.len() <= MAX_QUERY_LENGTH,
        "the case must be inside the bound, it is {} characters",
        inside.len()
    );
    let answer = explain(&router, &inside).await;
    assert_eq!(
        answer.status,
        200,
        "case a query of {} characters (inside the bound): expected 200, got {}",
        inside.len(),
        answer.status
    );

    for length in [MAX_QUERY_LENGTH + 1, MAX_QUERY_LENGTH * 2] {
        // A query past the bound, made of a valid traversal padded with a comment-free
        // run of whitespace, so it is the LENGTH that is refused and nothing else.
        let query = format!("g.V(){}", " ".repeat(length - "g.V()".len()));
        let answer = explain(&router, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case a query of {} characters (past the bound): expected 400, got {} with body {:?}",
            query.len(),
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "QueryTooLargeException",
            "case a query of {} characters (past the bound): expected QueryTooLargeException, got {:?}",
            query.len(),
            answer.text()
        );
    }
}

/// A traversal nested `wrappers` deep: `wrappers` nested `where` steps around an
/// innermost traversal, so the argument nesting the parse walks is `wrappers + 1`.
fn nested(wrappers: usize) -> String {
    let mut inner = "__.out()".to_string();
    for _ in 0..wrappers {
        inner = format!("__.where({inner})");
    }
    format!("g.V().where({inner})")
}

/// The nesting bound is a limit on the query, not on what it says: a query at the bound
/// is explained, and one past it is refused as exceeding a system limit rather than
/// walked to the bottom of the stack.
#[tokio::test]
async fn nesting_is_bounded_at_the_published_depth() {
    let Emulator { router, engine } = emulator();

    for wrappers in [0, 1, MAX_ARGUMENT_DEPTH - 2, MAX_ARGUMENT_DEPTH - 1] {
        let query = nested(wrappers);
        let answer = explain(&router, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case {wrappers} wrappers (at or inside the bound): expected 200, got {} with body {:?}",
            answer.status,
            answer.text()
        );
    }

    for wrappers in [MAX_ARGUMENT_DEPTH, MAX_ARGUMENT_DEPTH + 8] {
        let query = nested(wrappers);
        let answer = explain(&router, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {wrappers} wrappers (past the bound): expected 400, got {} with body {:?}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.error_code(),
            "QueryLimitException",
            "case {wrappers} wrappers (past the bound): expected QueryLimitException, got {:?}",
            answer.text()
        );
    }

    let store = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    let recorded = store.lock().gremlin_queries.len();
    assert_eq!(
        recorded, 4,
        "only the queries inside the bound are recorded, the registry holds {recorded}"
    );
}
