// SPDX-License-Identifier: CC0-1.0

//! The X-Ray operations that answer a graph, a sampling rule, a retrieval token,
//! an insight or a group update — end to end over the wire.
//!
//! Every case drives the same front door the production router does (the harness
//! in `trace_ingest_and_groups`), so the assertions are about HTTP responses
//! rather than internal state.
//!
//! The tests are tables, and each row differs along what the rule it checks
//! generalises over — the shape of a window, the topology of a trace, the values
//! of a rule's members, the number of resources relative to a page, the number
//! of concurrent callers — because the rules are:
//!
//! * an operation that takes a required `[StartTime, EndTime]` window refuses,
//!   with `InvalidRequestException` at HTTP 400, a window that names no interval
//!   of recorded traffic: one running backwards, or one whose bounds are not
//!   times the data could carry;
//! * a service graph is DERIVED from the stored segment documents — a node per
//!   distinct segment name, an edge per stored `parent_id` link between two
//!   services, and every count, time and histogram recomputed from the
//!   documents — and the same derivation answers the per-trace graph;
//! * a reader names a resource that is not stored (a trace id, a group, an
//!   insight) with `InvalidRequestException` at HTTP 400 rather than with a
//!   success that left it out;
//! * a create is one atomic step, so N concurrent calls for one name have
//!   exactly one winner and every loser is refused;
//! * a resource is answered from its stored record: a created sampling rule is
//!   listed with the ARN and timestamps it was stored with, and an updated group
//!   reads back through `GetGroup` with the members the update did not name left
//!   alone;
//! * `StartTraceRetrieval` mints and STORES one retrieval per accepted call, and
//!   refuses a call that arrives with the account's request budget empty with
//!   `ThrottledException` at HTTP 429 — a budget that refills rather than
//!   latching.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::types::Insight;
use service_com_amazonaws_xray_awsxray::state::{
    ComAmazonawsXrayAwsxrayState, MAX_WINDOW_FUTURE_SKEW_SECONDS, TRACE_RETRIEVAL_BURST,
    TRACE_RETRIEVAL_RATE_PER_SECOND,
};

use crate::trace_ingest_and_groups::{array, close, root_document, trace_id, Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as — the
/// crate's one clock, the same one the handlers read.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// The outcome an instrumentation DECLARES on a document beside the status it
/// recorded: a 5xx answer is a fault, a 429 a throttle, any other 4xx an error.
/// The service classifies a call by the flag its document declared and never by
/// the status, so a fixture whose node is to report a fault or a throttle
/// declares it here.
fn declared_for(status: i64) -> Option<&'static str> {
    match status {
        429 => Some("throttle"),
        status if (400..500).contains(&status) => Some("error"),
        status if status >= 500 => Some("fault"),
        _ => None,
    }
}

/// A child segment document under `parent`, with an optional recorded HTTP
/// status and the outcome declared beside it.
fn child_document(
    id: &str,
    parent: &str,
    trace: &str,
    name: &str,
    start: f64,
    end: f64,
    status: Option<i64>,
) -> Value {
    let mut document = json!({
        "trace_id": trace,
        "id": id,
        "parent_id": parent,
        "name": name,
        "start_time": start,
        "end_time": end,
    });
    if let Some(status) = status {
        document["http"] = json!({"response": {"status": status}});
        if let Some(flag) = declared_for(status) {
            document[flag] = json!(true);
        }
    }
    document
}

/// A root segment document with a recorded HTTP status and the outcome declared
/// beside it.
fn root_with_status(
    id: &str,
    trace: &str,
    name: &str,
    start: f64,
    end: f64,
    status: i64,
) -> Value {
    let mut document = root_document(id, trace, name, start, Some(end));
    document["http"] = json!({"response": {"status": status}});
    if let Some(flag) = declared_for(status) {
        document[flag] = json!(true);
    }
    document
}

/// The `Service` named `name` in a graph answer.
fn service_named<'a>(services: &'a [Value], name: &str) -> Option<&'a Value> {
    services
        .iter()
        .find(|service| service.get("Name").and_then(Value::as_str) == Some(name))
}

/// Every `Service$Name` in a graph answer, in the order it came back.
fn service_names(services: &[Value]) -> Vec<String> {
    services
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// One count out of a `Service$SummaryStatistics`, through an optional nested
/// member (`ErrorStatistics`, `FaultStatistics`) when `nested` is given.
fn stat(service: &Value, nested: Option<&str>, member: &str) -> i64 {
    let statistics = service.get("SummaryStatistics").unwrap_or(&Value::Null);
    let holder = match nested {
        Some(nested) => statistics.get(nested).unwrap_or(&Value::Null),
        None => statistics,
    };
    holder.get(member).and_then(Value::as_i64).unwrap_or(-1)
}

/// A minimal, valid `SamplingRule` body for `CreateSamplingRule`.
fn sampling_rule(name: &str) -> Value {
    json!({
        "SamplingRule": {
            "RuleName": name,
            "ResourceARN": "*",
            "Priority": 1,
            "FixedRate": 0.05,
            "ReservoirSize": 1,
            "ServiceName": "*",
            "ServiceType": "*",
            "Host": "*",
            "HTTPMethod": "*",
            "URLPath": "*",
            "Version": 1,
        }
    })
}

// =============================================================================
// 1. A required [StartTime, EndTime] window is refused when it names no interval
// =============================================================================

/// One window, and whether every operation that takes one must accept it.
#[derive(Debug)]
struct WindowCase {
    case: &'static str,
    start: f64,
    end: f64,
    accepted: bool,
}

fn window_cases() -> Vec<WindowCase> {
    let now = now();
    // A millisecond-scale epoch value read as seconds lands thousands of years
    // out — the shape of a malformed time a client sends by passing
    // `time.time() * 1000`.
    let millis_scale = now * 1_000.0;
    vec![
        WindowCase {
            case: "an hour-long window ending now",
            start: now - 3_600.0,
            end: now,
            accepted: true,
        },
        WindowCase {
            case: "a window of a single instant",
            start: now - 60.0,
            end: now - 60.0,
            accepted: true,
        },
        WindowCase {
            case: "a window reaching a minute past now",
            start: now - 60.0,
            end: now + 60.0,
            accepted: true,
        },
        WindowCase {
            // Kept a minute wide, so the row is about where the window ENDS and
            // not about how wide it is: each reader also publishes the widest
            // window it serves, which is a different rule and a different row.
            case: "a window ending just inside the clock-skew allowance",
            start: now + MAX_WINDOW_FUTURE_SKEW_SECONDS - 120.0,
            end: now + MAX_WINDOW_FUTURE_SKEW_SECONDS - 60.0,
            accepted: true,
        },
        WindowCase {
            case: "a window ending just past the clock-skew allowance",
            start: now - 60.0,
            end: now + MAX_WINDOW_FUTURE_SKEW_SECONDS + 60.0,
            accepted: false,
        },
        WindowCase {
            case: "a window starting past the clock-skew allowance",
            start: now + MAX_WINDOW_FUTURE_SKEW_SECONDS + 60.0,
            end: now + MAX_WINDOW_FUTURE_SKEW_SECONDS + 120.0,
            accepted: false,
        },
        WindowCase {
            case: "EndTime one second before StartTime",
            start: now,
            end: now - 1.0,
            accepted: false,
        },
        WindowCase {
            case: "EndTime a day before StartTime",
            start: now,
            end: now - 86_400.0,
            accepted: false,
        },
        WindowCase {
            case: "a millisecond-scale EndTime",
            start: now - 3_600.0,
            end: millis_scale,
            accepted: false,
        },
        WindowCase {
            case: "a millisecond-scale StartTime and EndTime",
            start: millis_scale,
            end: millis_scale + 3_600.0,
            accepted: false,
        },
    ]
}

/// How one operation builds its request body out of a [`WindowCase`].
type WindowBody = fn(&WindowCase) -> Value;

#[tokio::test]
async fn a_window_that_names_no_interval_is_refused_by_every_operation_that_takes_one() {
    // Both operations whose input carries a required [StartTime, EndTime]: one
    // reader of a window, one starter of work over a window.
    let operations: [(&str, WindowBody); 2] = [
        (
            "/ServiceGraph",
            |case: &WindowCase| json!({"StartTime": case.start, "EndTime": case.end}),
        ),
        ("/StartTraceRetrieval", |case: &WindowCase| {
            json!({
                "TraceIds": [trace_id(1)],
                "StartTime": case.start,
                "EndTime": case.end,
            })
        }),
    ];

    for (path, body_of) in operations {
        for case in window_cases() {
            // A fresh emulator per row, so no row spends another row's request
            // budget.
            let xray = Xray::new();
            let label = case.case;
            let answer = xray.call(path, &body_of(&case)).await;
            if case.accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {label:?} on {path}: a well-formed window must be accepted, got {answer:?}"
                );
            } else {
                assert_eq!(
                    (answer.status, answer.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {label:?} on {path}: the window must be refused, got {answer:?}"
                );
            }
        }
    }
}

// =============================================================================
// 2. The service graph is derived from the stored segment documents
// =============================================================================

/// What one node of the graph the fixture below stores must report.
#[derive(Debug)]
struct NodeCase {
    name: &'static str,
    root: bool,
    state: &'static str,
    total: i64,
    ok: i64,
    throttle: i64,
    error_other: i64,
    fault: i64,
    downstream: &'static [&'static str],
}

/// Ingest the graph fixture: three traces whose segments span the axes the
/// derivation generalises over — a chain, a fan-out, a repeated service, a
/// 500-series fault, a 429 throttle and an in-progress segment.
async fn seed_graph(xray: &Xray, base: f64) {
    let chain = trace_id(101);
    let fanned = trace_id(102);
    let pending = trace_id(103);
    let documents = vec![
        root_with_status("a1a1a1a1a1a1a1a1", &chain, "frontend", base, base + 1.0, 200).to_string(),
        child_document(
            "a2a2a2a2a2a2a2a2",
            "a1a1a1a1a1a1a1a1",
            &chain,
            "backend",
            base + 0.1,
            base + 0.6,
            Some(200),
        )
        .to_string(),
        child_document(
            "a3a3a3a3a3a3a3a3",
            "a2a2a2a2a2a2a2a2",
            &chain,
            "db",
            base + 0.2,
            base + 0.3,
            None,
        )
        .to_string(),
        root_with_status(
            "b1b1b1b1b1b1b1b1",
            &fanned,
            "frontend",
            base + 10.0,
            base + 10.5,
            503,
        )
        .to_string(),
        child_document(
            "b2b2b2b2b2b2b2b2",
            "b1b1b1b1b1b1b1b1",
            &fanned,
            "cache",
            base + 10.1,
            base + 10.2,
            Some(429),
        )
        .to_string(),
        root_document("c1c1c1c1c1c1c1c1", &pending, "worker", base + 20.0, None).to_string(),
    ];
    let ingested = xray
        .call(
            "/TraceSegments",
            &json!({"TraceSegmentDocuments": documents}),
        )
        .await;
    assert_eq!(
        ingested.status, 200,
        "seeding the graph fixture must succeed, got {ingested:?}"
    );
    assert!(
        array(&ingested.body, "UnprocessedTraceSegments").is_empty(),
        "every fixture document must be stored, got {ingested:?}"
    );
}

fn node_cases() -> Vec<NodeCase> {
    vec![
        NodeCase {
            name: "frontend",
            root: true,
            state: "active",
            total: 2,
            ok: 1,
            throttle: 0,
            error_other: 0,
            fault: 1,
            downstream: &["backend", "cache"],
        },
        NodeCase {
            name: "backend",
            root: false,
            state: "active",
            total: 1,
            ok: 1,
            throttle: 0,
            error_other: 0,
            fault: 0,
            downstream: &["db"],
        },
        NodeCase {
            name: "db",
            root: false,
            state: "active",
            total: 1,
            ok: 1,
            throttle: 0,
            error_other: 0,
            fault: 0,
            downstream: &[],
        },
        NodeCase {
            name: "cache",
            root: false,
            state: "active",
            total: 1,
            ok: 0,
            throttle: 1,
            error_other: 0,
            fault: 0,
            downstream: &[],
        },
        NodeCase {
            name: "worker",
            root: true,
            // Every segment of this node is still in progress, so nothing is
            // known about how it answered.
            state: "unknown",
            total: 1,
            ok: 1,
            throttle: 0,
            error_other: 0,
            fault: 0,
            downstream: &[],
        },
    ]
}

/// Assert one node of a graph answer against its case.
fn assert_node(label: &str, services: &[Value], case: &NodeCase) {
    let Some(service) = service_named(services, case.name) else {
        panic!(
            "{label}: the graph must carry the service {:?}, got {:?}",
            case.name,
            service_names(services)
        );
    };
    assert_eq!(
        service.get("Root").and_then(Value::as_bool),
        Some(case.root),
        "{label}: {} must report Root {}, got {service:?}",
        case.name,
        case.root
    );
    assert_eq!(
        service.get("State").and_then(Value::as_str),
        Some(case.state),
        "{label}: {} must report State {:?}, got {service:?}",
        case.name,
        case.state
    );
    for (nested, member, expected) in [
        (None, "TotalCount", case.total),
        (None, "OkCount", case.ok),
        (Some("ErrorStatistics"), "ThrottleCount", case.throttle),
        (Some("ErrorStatistics"), "OtherCount", case.error_other),
        (Some("FaultStatistics"), "TotalCount", case.fault),
    ] {
        let actual = stat(service, nested, member);
        assert_eq!(
            actual, expected,
            "{label}: {}'s {nested:?}/{member} must be {expected}, got {actual}",
            case.name
        );
    }

    // Each edge names its downstream node by ReferenceId, so the downstream
    // service's own ReferenceId is what resolves it.
    let edges = array(service, "Edges");
    let mut reached: Vec<String> = Vec::new();
    for edge in edges {
        let reference = edge.get("ReferenceId").and_then(Value::as_i64);
        let downstream = services.iter().find(|candidate| {
            candidate.get("ReferenceId").and_then(Value::as_i64) == reference
        });
        let Some(downstream) = downstream else {
            panic!(
                "{label}: {}'s edge {edge:?} names ReferenceId {reference:?}, which is no service in {:?}",
                case.name,
                service_names(services)
            );
        };
        reached.push(
            downstream
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    reached.sort();
    let mut expected: Vec<String> = case.downstream.iter().map(|name| (*name).to_string()).collect();
    expected.sort();
    assert_eq!(
        reached, expected,
        "{label}: {} must have edges to {expected:?}, got {reached:?}",
        case.name
    );
}

#[tokio::test]
async fn the_service_graph_is_derived_from_the_stored_segments() {
    let xray = Xray::new();
    let base = now() - 3_600.0;
    seed_graph(&xray, base).await;

    let window = json!({"StartTime": base - 60.0, "EndTime": base + 60.0});
    let graph = xray.call("/ServiceGraph", &window).await;
    assert_eq!(
        graph.status, 200,
        "GetServiceGraph over the fixture's window must succeed, got {graph:?}"
    );
    // The whole graph, across pages: a root service's `client` node pages beside
    // it, so the fixture's nodes no longer fit in one page.
    let mut services: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = window.clone();
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let page = xray.call("/ServiceGraph", &body).await;
        assert_eq!(
            page.status, 200,
            "every page of the fixture's window must succeed, got {page:?}"
        );
        services.extend(array(&page.body, "Services").iter().cloned());
        token = page.get("NextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            services.len() < 100,
            "paging the fixture's window did not terminate, got {services:?}"
        );
    }
    // One node per distinct segment name, plus the `client` node of every root
    // service — "represents the clients that sent requests to a root service".
    let roots = node_cases().iter().filter(|case| case.root).count();
    assert_eq!(
        service_names(&services).len(),
        node_cases().len() + roots,
        "the graph must carry one node per distinct segment name and one client node per root, got {:?}",
        service_names(&services)
    );
    for case in node_cases() {
        assert_node("the fixture's window", &services, &case);
    }
    // Every root service's clients are a node of their own, typed `client`,
    // carrying that service's name and reporting nothing about its own state.
    for case in node_cases().iter().filter(|case| case.root) {
        let client = services.iter().find(|service| {
            service.get("Name").and_then(Value::as_str) == Some(case.name)
                && service.get("Type").and_then(Value::as_str) == Some("client")
        });
        let Some(client) = client else {
            panic!(
                "the fixture's window: the root service {:?} must have a client node, got {:?}",
                case.name,
                services
            );
        };
        assert_eq!(
            client.get("State").and_then(Value::as_str),
            Some("unknown"),
            "the client node of {:?} reported no segment of its own, got {client:?}",
            case.name
        );
    }

    // `ReferenceId` is unique within the map, and `Names` carries the canonical
    // name.
    let references: BTreeSet<i64> = services
        .iter()
        .filter_map(|service| service.get("ReferenceId").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        references.len(),
        services.len(),
        "every node must carry a distinct ReferenceId, got {services:?}"
    );

    // The node's span and response time come from the stored documents: the
    // frontend's two segments span [base, base + 10.5] and took 1.0 + 0.5 s.
    let frontend = service_named(&services, "frontend").expect("the frontend node is in the graph");
    let start = frontend
        .get("StartTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let end = frontend
        .get("EndTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        close(start, base) && close(end, base + 10.5),
        "the frontend node must span the stored segments [{base}, {}], got [{start}, {end}]",
        base + 10.5
    );
    let response_time = frontend
        .get("SummaryStatistics")
        .and_then(|statistics| statistics.get("TotalResponseTime"))
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        close(response_time, 1.5),
        "the frontend node's TotalResponseTime must be the stored durations' sum 1.5, got {response_time}"
    );
    assert!(
        !array(frontend, "DurationHistogram").is_empty(),
        "the frontend node's DurationHistogram must be built from the stored durations, got {frontend:?}"
    );

    // A window that contains no stored segment selects nothing, and reading the
    // graph twice answers the same thing: a read changes nothing.
    let empty = xray
        .call(
            "/ServiceGraph",
            &json!({"StartTime": base - 7_200.0, "EndTime": base - 3_600.0}),
        )
        .await;
    assert_eq!(
        (empty.status, array(&empty.body, "Services").len()),
        (200, 0),
        "a window with no stored segment must select nothing, got {empty:?}"
    );
    let again = xray.call("/ServiceGraph", &window).await;
    assert_eq!(
        again.get("Services"),
        graph.get("Services"),
        "reading the graph again must answer the same services, got {again:?}"
    );
}

#[tokio::test]
async fn the_service_graph_pages_and_resumes_exactly_once() {
    use service_com_amazonaws_xray_awsxray::ops::get_service_graph::SERVICE_GRAPH_PAGE_SIZE;

    // Counts either side of one page, so a row exists where no token is due and
    // one where it is.
    for count in [SERVICE_GRAPH_PAGE_SIZE - 1, SERVICE_GRAPH_PAGE_SIZE + 2] {
        let xray = Xray::new();
        let base = now() - 1_800.0;
        let trace = trace_id(110);
        let mut documents =
            vec![root_document("f0f0f0f0f0f0f0f0", &trace, "svc-000", base, Some(base + 1.0))
                .to_string()];
        for index in 1..count {
            documents.push(
                child_document(
                    &format!("f{index}f{index}f{index}f{index}f{index}f{index}f{index}f{index}"),
                    "f0f0f0f0f0f0f0f0",
                    &trace,
                    &format!("svc-{index:03}"),
                    base + 0.1,
                    base + 0.2,
                    None,
                )
                .to_string(),
            );
        }
        let ingested = xray
            .call(
                "/TraceSegments",
                &json!({"TraceSegmentDocuments": documents}),
            )
            .await;
        assert_eq!(
            ingested.status, 200,
            "count {count}: seeding must succeed, got {ingested:?}"
        );

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"StartTime": base - 60.0, "EndTime": base + 60.0});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let page = xray.call("/ServiceGraph", &body).await;
            assert_eq!(
                page.status, 200,
                "count {count}: a page must succeed, got {page:?}"
            );
            let names = service_names(array(&page.body, "Services"));
            assert!(
                names.len() <= SERVICE_GRAPH_PAGE_SIZE,
                "count {count}: a page carries at most {SERVICE_GRAPH_PAGE_SIZE} services, got {names:?}"
            );
            seen.extend(names);
            token = page
                .get("NextToken")
                .as_str()
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        // Zero-padded to the width of the largest index a row seeds, so the
        // name order the graph answers in IS the index order asserted here
        // however many services a page carries.
        let expected: Vec<String> = std::iter::once("svc-000".to_string())
            // The root service's `client` node carries the root's own name, so
            // it pages beside it.
            .chain((0..count).map(|index| format!("svc-{index:03}")))
            .collect();
        assert_eq!(
            seen, expected,
            "count {count}: paging must yield every service exactly once, got {seen:?}"
        );
    }
}

// =============================================================================
// 3. The trace graph is per trace, and an unindexed trace id is refused
// =============================================================================

#[tokio::test]
async fn the_trace_graph_is_per_trace_and_an_unstored_trace_id_is_refused() {
    use service_com_amazonaws_xray_awsxray::ops::get_trace_graph::TRACE_ID_NOT_FOUND;

    // The handler's constant must BE the recorded wording, not merely agree
    // with itself: the refusals below assert the literal.
    assert_eq!(
        TRACE_ID_NOT_FOUND, "Trace id not found",
        "the refusal wording must be the recorded one, got {TRACE_ID_NOT_FOUND:?}"
    );

    let xray = Xray::new();
    let base = now() - 3_600.0;
    seed_graph(&xray, base).await;
    let chain = trace_id(101);
    let fanned = trace_id(102);
    let unstored = trace_id(199);

    // Rows differ in how many stored traces are asked for: the per-trace graph
    // carries exactly the services of the traces named, never the whole store.
    let rows: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        (
            "one trace",
            vec![chain.clone()],
            // The root service's `client` node carries that service's own name,
            // so `frontend` appears twice: the service, and the clients that
            // sent it the request.
            vec!["backend", "db", "frontend", "frontend"],
        ),
        (
            "two traces",
            vec![chain.clone(), fanned.clone()],
            vec!["backend", "cache", "db", "frontend", "frontend"],
        ),
        (
            "another trace alone",
            vec![fanned.clone()],
            vec!["cache", "frontend", "frontend"],
        ),
        ("no trace at all", vec![], vec![]),
    ];
    for (label, ids, expected) in rows {
        let answer = xray.call("/TraceGraph", &json!({"TraceIds": ids})).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: GetTraceGraph must succeed, got {answer:?}"
        );
        let mut names = service_names(array(&answer.body, "Services"));
        names.sort();
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            names, expected,
            "case {label:?}: the graph must carry exactly the named traces' services, got {names:?}"
        );
    }

    // The frontend served one request in the chain trace and two across both, so
    // the counts are the named traces' own.
    let one = xray
        .call("/TraceGraph", &json!({"TraceIds": [chain.clone()]}))
        .await;
    let both = xray
        .call(
            "/TraceGraph",
            &json!({"TraceIds": [chain.clone(), fanned.clone()]}),
        )
        .await;
    for (label, answer, expected) in [("one trace", &one, 1), ("two traces", &both, 2)] {
        let services = array(&answer.body, "Services").to_vec();
        let frontend = service_named(&services, "frontend").expect("the frontend node is present");
        let actual = stat(frontend, None, "TotalCount");
        assert_eq!(
            actual, expected,
            "case {label:?}: the frontend node must count {expected} requests, got {actual}"
        );
    }

    // An id that is not stored is refused, whether it is alone or beside a
    // stored one — unlike BatchGetTraces, which reports it as unprocessed.
    for (label, ids) in [
        ("an unstored id alone", vec![unstored.clone()]),
        (
            "an unstored id beside a stored one",
            vec![chain.clone(), unstored.clone()],
        ),
    ] {
        let answer = xray
            .call("/TraceGraph", &json!({"TraceIds": ids.clone()}))
            .await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: GetTraceGraph must refuse the request, got {answer:?}"
        );
        // The literal AWS answers for an unindexed trace id, as recorded — not
        // the implementation's own constant, so a drift in the wording fails
        // here.
        assert_eq!(
            answer.get("message").as_str().or(answer.get("Message").as_str()),
            Some("Trace id not found"),
            "case {label:?}: the refusal must say \"Trace id not found\", got {answer:?}"
        );

        let batch = xray.call("/Traces", &json!({"TraceIds": ids})).await;
        assert_eq!(
            batch.status, 200,
            "case {label:?}: BatchGetTraces reports an unknown id instead of refusing, got {batch:?}"
        );
    }
}

// =============================================================================
// 4. A sampling rule is created once, and read back from its stored record
// =============================================================================

/// One sampling rule to create. Rows differ along the members the rule matches
/// on, its priority, its rate and its reservoir.
fn sampling_rule_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("a rule matching everything", sampling_rule("rule-all")),
        (
            "a rule at the lowest priority and highest rate",
            json!({"SamplingRule": {
                "RuleName": "rule-bounds-high",
                "ResourceARN": "*",
                "Priority": 9999,
                "FixedRate": 1.0,
                "ReservoirSize": 0,
                "ServiceName": "*",
                "ServiceType": "*",
                "Host": "*",
                "HTTPMethod": "*",
                "URLPath": "*",
                "Version": 1,
            }}),
        ),
        (
            "a rule on one resource, method and path",
            json!({"SamplingRule": {
                "RuleName": "rule-scoped",
                "ResourceARN": "arn:aws:apigateway:us-east-1::/restapis/abc123/stages/prod",
                "Priority": 100,
                "FixedRate": 0.5,
                "ReservoirSize": 50,
                "ServiceName": "checkout",
                "ServiceType": "AWS::ECS::Container",
                "Host": "api.example.com",
                "HTTPMethod": "POST",
                "URLPath": "/orders/*",
                "Version": 1,
            }}),
        ),
        (
            "a rule with attributes",
            json!({"SamplingRule": {
                "RuleName": "rule-attributes",
                "ResourceARN": "*",
                "Priority": 2000,
                "FixedRate": 0.1,
                "ReservoirSize": 5,
                "ServiceName": "*",
                "ServiceType": "*",
                "Host": "*",
                "HTTPMethod": "GET",
                "URLPath": "*",
                "Version": 1,
                "Attributes": {"tier": "gold"},
            }}),
        ),
    ]
}

#[tokio::test]
async fn a_sampling_rule_is_created_once_and_listed_from_its_stored_record() {
    let xray = Xray::new();
    let before = now();

    for (label, body) in sampling_rule_cases() {
        let created = xray.call("/CreateSamplingRule", &body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateSamplingRule must succeed, got {created:?}"
        );
        let record = created.get("SamplingRuleRecord");
        let name = body["SamplingRule"]["RuleName"].as_str().unwrap_or_default();
        let stored = record.get("SamplingRule").unwrap_or(&Value::Null);
        assert_eq!(
            stored.get("RuleName").and_then(Value::as_str),
            Some(name),
            "case {label:?}: the record must carry the rule's name, got {record:?}"
        );
        let arn = stored
            .get("RuleARN")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            arn.starts_with("arn:aws:xray:") && arn.ends_with(&format!(":sampling-rule/{name}")),
            "case {label:?}: the rule's ARN must be minted for its name, got {arn:?}"
        );
        for member in ["Priority", "FixedRate", "ReservoirSize", "URLPath", "Host"] {
            assert_eq!(
                stored.get(member),
                body["SamplingRule"].get(member),
                "case {label:?}: the record must carry the rule's {member}, got {record:?}"
            );
        }
        for member in ["CreatedAt", "ModifiedAt"] {
            let stamped = record.get(member).and_then(Value::as_f64).unwrap_or(0.0);
            assert!(
                stamped >= before - 1.0,
                "case {label:?}: the record's {member} must be stamped when it was created, got {stamped}"
            );
        }

        // The second create of one name finds the first already stored.
        let again = xray.call("/CreateSamplingRule", &body).await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: creating the same rule name twice must be refused, got {again:?}"
        );
    }

    // Every created rule is in the listing, in rule-name order, followed by the
    // account's service-managed `Default` rule, with the ARN and the timestamps
    // each was stored with.
    let listed = xray.call("/GetSamplingRules", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "GetSamplingRules must succeed, got {listed:?}"
    );
    let names: Vec<String> = array(&listed.body, "SamplingRuleRecords")
        .iter()
        .filter_map(|record| {
            record
                .get("SamplingRule")
                .and_then(|rule| rule.get("RuleName"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .collect();
    let mut expected: Vec<String> = sampling_rule_cases()
        .iter()
        .map(|(_, body)| {
            body["SamplingRule"]["RuleName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    expected.sort();
    expected.push("Default".to_string());
    assert_eq!(
        names, expected,
        "every created rule must be listed in rule-name order, then the Default rule, got {names:?}"
    );
}

#[tokio::test]
async fn the_sampling_rule_listing_pages_and_resumes_exactly_once() {
    use service_com_amazonaws_xray_awsxray::ops::get_sampling_rules::SAMPLING_RULES_PAGE_SIZE;

    for count in [SAMPLING_RULES_PAGE_SIZE - 1, SAMPLING_RULES_PAGE_SIZE + 2] {
        let xray = Xray::new();
        for index in 0..count {
            let created = xray
                .call("/CreateSamplingRule", &sampling_rule(&format!("rule-{index:02}")))
                .await;
            assert_eq!(
                created.status, 200,
                "count {count}: seeding rule-{index:02} must succeed, got {created:?}"
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let page = xray.call("/GetSamplingRules", &body).await;
            assert_eq!(
                page.status, 200,
                "count {count}: a page must succeed, got {page:?}"
            );
            let records = array(&page.body, "SamplingRuleRecords");
            assert!(
                records.len() <= SAMPLING_RULES_PAGE_SIZE,
                "count {count}: a page carries at most {SAMPLING_RULES_PAGE_SIZE} records, got {page:?}"
            );
            seen.extend(records.iter().filter_map(|record| {
                record
                    .get("SamplingRule")
                    .and_then(|rule| rule.get("RuleName"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }));
            token = page.get("NextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        // The created rules, then the account's service-managed `Default` rule,
        // which the listing always carries.
        let mut expected: Vec<String> = (0..count).map(|index| format!("rule-{index:02}")).collect();
        expected.push("Default".to_string());
        assert_eq!(
            seen, expected,
            "count {count}: paging must yield every rule exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_sampling_rule_the_service_cannot_describe_is_refused() {
    let xray = Xray::new();

    // Rows span the ways a submitted rule cannot describe a stored one: the
    // selector (both, neither), the rule-format version, and a ResourceARN that
    // is neither an ARN nor the any-resource wildcard.
    let rows: Vec<(&str, Value)> = vec![
        (
            "both RuleName and RuleARN",
            json!({"SamplingRule": {
                "RuleName": "rule-both",
                "RuleARN": "arn:aws:xray:us-east-1:000000000000:sampling-rule/rule-both",
                "ResourceARN": "*",
                "Priority": 1, "FixedRate": 0.05, "ReservoirSize": 1,
                "ServiceName": "*", "ServiceType": "*", "Host": "*",
                "HTTPMethod": "*", "URLPath": "*", "Version": 1,
            }}),
        ),
        (
            "neither RuleName nor RuleARN",
            json!({"SamplingRule": {
                "ResourceARN": "*",
                "Priority": 1, "FixedRate": 0.05, "ReservoirSize": 1,
                "ServiceName": "*", "ServiceType": "*", "Host": "*",
                "HTTPMethod": "*", "URLPath": "*", "Version": 1,
            }}),
        ),
        (
            "a RuleARN alone, which names a rule that does not exist yet",
            json!({"SamplingRule": {
                "RuleARN": "arn:aws:xray:us-east-1:000000000000:sampling-rule/rule-arn-only",
                "ResourceARN": "*",
                "Priority": 1, "FixedRate": 0.05, "ReservoirSize": 1,
                "ServiceName": "*", "ServiceType": "*", "Host": "*",
                "HTTPMethod": "*", "URLPath": "*", "Version": 1,
            }}),
        ),
        (
            "a rule-format version the service does not publish",
            json!({"SamplingRule": {
                "RuleName": "rule-v2",
                "ResourceARN": "*",
                "Priority": 1, "FixedRate": 0.05, "ReservoirSize": 1,
                "ServiceName": "*", "ServiceType": "*", "Host": "*",
                "HTTPMethod": "*", "URLPath": "*", "Version": 2,
            }}),
        ),
        (
            "a ResourceARN that is not an ARN",
            json!({"SamplingRule": {
                "RuleName": "rule-bad-resource",
                "ResourceARN": "not-an-arn",
                "Priority": 1, "FixedRate": 0.05, "ReservoirSize": 1,
                "ServiceName": "*", "ServiceType": "*", "Host": "*",
                "HTTPMethod": "*", "URLPath": "*", "Version": 1,
            }}),
        ),
    ];

    for (label, body) in rows {
        let answer = xray.call("/CreateSamplingRule", &body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: the rule must be refused, got {answer:?}"
        );
    }

    // Nothing above was stored: the listing holds only the account's
    // service-managed `Default` rule, which nobody created.
    let listed = xray.call("/GetSamplingRules", &json!({})).await;
    assert_eq!(
        (listed.status, array(&listed.body, "SamplingRuleRecords").len()),
        (200, 1),
        "a refused rule must not be stored, got {listed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_of_one_rule_name_have_exactly_one_winner() {
    let xray = Xray::new();
    let body = sampling_rule("rule-race");
    // Five callers, one name: the create is one step on the store, so exactly
    // one of them can find the name free.
    let answers: Vec<Answer> = {
        let (a, b, c, d, e) = tokio::join!(
            xray.call("/CreateSamplingRule", &body),
            xray.call("/CreateSamplingRule", &body),
            xray.call("/CreateSamplingRule", &body),
            xray.call("/CreateSamplingRule", &body),
            xray.call("/CreateSamplingRule", &body),
        );
        vec![a, b, c, d, e]
    };

    let winners = answers.iter().filter(|answer| answer.status == 200).count();
    assert_eq!(
        winners, 1,
        "exactly one concurrent create may win, got {winners} winners in {answers:?}"
    );
    for answer in answers.iter().filter(|answer| answer.status != 200) {
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "a losing concurrent create must be refused as invalid, got {answer:?}"
        );
    }

    // And the winner's rule is the only one stored, beside the account's
    // service-managed `Default` rule.
    let listed = xray.call("/GetSamplingRules", &json!({})).await;
    assert_eq!(
        (listed.status, array(&listed.body, "SamplingRuleRecords").len()),
        (200, 2),
        "one winner must leave exactly one stored rule, got {listed:?}"
    );
}

// =============================================================================
// 5. StartTraceRetrieval mints a stored token, and throttles past the budget
// =============================================================================

#[tokio::test]
async fn start_trace_retrieval_mints_a_stored_token_and_throttles_past_the_budget() {
    let (xray, engine) = Xray::with_engine();
    let base = now() - 600.0;

    // Rows differ in how many trace ids one retrieval is started for.
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for index in 0..TRACE_RETRIEVAL_BURST {
        let ids: Vec<String> = (0..=index).map(|at| trace_id(200 + at)).collect();
        let label = format!("{} trace id(s)", ids.len());
        let answer = xray
            .call(
                "/StartTraceRetrieval",
                &json!({"TraceIds": ids.clone(), "StartTime": base, "EndTime": base + 300.0}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a call inside the budget must succeed, got {answer:?}"
        );
        let token = answer
            .get("RetrievalToken")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !token.is_empty(),
            "case {label:?}: the answer must carry a RetrievalToken, got {answer:?}"
        );
        assert!(
            tokens.insert(token.clone()),
            "case {label:?}: every retrieval must get its own token, saw {token:?} twice"
        );

        // The token names a STORED retrieval, holding what was asked for.
        let state = engine.state::<ComAmazonawsXrayAwsxrayState>();
        let store = state.lock();
        let stored = store.retrievals.get(&token);
        let Some(stored) = stored else {
            panic!(
                "case {label:?}: the token {token:?} must name a stored retrieval, stored: {:?}",
                store.retrievals.keys().collect::<Vec<_>>()
            );
        };
        assert_eq!(
            stored.trace_ids, ids,
            "case {label:?}: the stored retrieval must hold the requested ids, got {stored:?}"
        );
        assert!(
            close(stored.start_time, base) && close(stored.end_time, base + 300.0),
            "case {label:?}: the stored retrieval must hold the requested window, got {stored:?}"
        );
    }

    // The budget is empty now, so the next call is throttled rather than served.
    let throttled = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": [trace_id(210)], "StartTime": base, "EndTime": base + 300.0}),
        )
        .await;
    assert_eq!(
        (throttled.status, throttled.error_type.as_deref()),
        (429, Some("ThrottledException")),
        "a call past the budget must be throttled, got {throttled:?}"
    );

    // The budget REFILLS: after long enough to earn one request back, a call is
    // served again.
    let refill = std::time::Duration::from_secs_f64(2.0 / TRACE_RETRIEVAL_RATE_PER_SECOND);
    tokio::time::sleep(refill).await;
    let served = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": [trace_id(211)], "StartTime": base, "EndTime": base + 300.0}),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "the budget must refill rather than latch, got {served:?}"
    );
}

#[tokio::test]
async fn start_trace_retrieval_honours_the_modelled_trace_id_bound() {
    // The modelled @length bound on TraceIds is 0..100: the maximum is accepted
    // and one past it is refused before the handler runs.
    for (label, count, accepted) in [
        ("an empty list", 0_usize, true),
        ("the modelled maximum of ids", 100, true),
        ("one id past the modelled maximum", 101, false),
    ] {
        let xray = Xray::new();
        let base = now() - 600.0;
        let ids: Vec<String> = (0..count)
            .map(|at| trace_id(300 + u32::try_from(at).unwrap_or(0)))
            .collect();
        let answer = xray
            .call(
                "/StartTraceRetrieval",
                &json!({"TraceIds": ids, "StartTime": base, "EndTime": base + 60.0}),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {label:?}: a list inside the modelled bound must be accepted, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {label:?}: a list past the modelled bound must be refused, got {answer:?}"
            );
        }
    }
}

// =============================================================================
// 6. GetInsight answers from the stored insight, and refuses an id naming none
// =============================================================================

#[tokio::test]
async fn get_insight_answers_from_the_stored_insight_and_refuses_an_unknown_id() {
    use service_com_amazonaws_xray_awsxray::ops::get_insight::INSIGHT_NOT_FOUND;

    assert_eq!(
        INSIGHT_NOT_FOUND, "Insight not found",
        "the refusal wording must stay the fixed string, got {INSIGHT_NOT_FOUND:?}"
    );

    let (xray, engine) = Xray::with_engine();

    // Rows differ in the insight's state, group and summary. The ids are UUIDs
    // the modelled @pattern on InsightId accepts.
    let stored: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an active insight",
            "11111111-2222-4333-8444-555555555555",
            "ACTIVE",
            "Fault rate above the expected range",
        ),
        (
            "a closed insight",
            "66666666-7777-4888-9999-aaaaaaaaaaaa",
            "CLOSED",
            "Fault rate back inside the expected range",
        ),
    ];
    {
        let state = engine.state::<ComAmazonawsXrayAwsxrayState>();
        let mut store = state.lock();
        for (_, id, insight_state, summary) in &stored {
            store.insights.insert(
                (*id).to_string(),
                Insight {
                    insight_id: Some((*id).to_string()),
                    group_name: Some("insights-group".to_string()),
                    state: Some((*insight_state).to_string()),
                    summary: Some((*summary).to_string()),
                    ..Insight::default()
                },
            );
        }
    }

    for (label, id, insight_state, summary) in &stored {
        let answer = xray.call("/Insight", &json!({"InsightId": id})).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: GetInsight must answer from the stored insight, got {answer:?}"
        );
        let insight = answer.get("Insight");
        assert_eq!(
            (
                insight.get("InsightId").and_then(Value::as_str),
                insight.get("State").and_then(Value::as_str),
                insight.get("Summary").and_then(Value::as_str),
            ),
            (Some(*id), Some(*insight_state), Some(*summary)),
            "case {label:?}: the answer must carry the stored insight, got {answer:?}"
        );
    }

    // Rows differ in the unknown id's shape, within what the pattern accepts.
    for (label, id) in [
        ("an id that names no insight", "99999999-8888-4777-b666-555544443333"),
        ("another id that names no insight", "0a0a0a0a-1b1b-4c1c-8d1d-2e2e2e2e2e2e"),
        (
            "the stored id with one digit changed",
            "11111111-2222-4333-8444-555555555556",
        ),
    ] {
        let answer = xray.call("/Insight", &json!({"InsightId": id})).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: an unknown InsightId must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.get("message").as_str().or(answer.get("Message").as_str()),
            Some("Insight not found"),
            "case {label:?}: the refusal must say \"Insight not found\", got {answer:?}"
        );
    }

    // An id the modelled @pattern rejects never reaches the lookup.
    let malformed = xray
        .call("/Insight", &json!({"InsightId": "not-a-uuid"}))
        .await;
    assert_eq!(
        malformed.status, 400,
        "an InsightId outside the modelled pattern must be refused, got {malformed:?}"
    );
}

// =============================================================================
// 7. UpdateGroup mutates the stored record, and refuses a request that does not
//    name exactly one group or that states a forbidden configuration
// =============================================================================

/// One accepted `UpdateGroup` call, and what the group must read back as
/// afterwards.
#[derive(Debug)]
struct UpdateCase {
    case: &'static str,
    body: Value,
    group: &'static str,
    filter: Option<&'static str>,
    insights: Option<bool>,
}

#[tokio::test]
async fn update_group_updates_the_stored_record_and_refuses_a_conflicting_selector() {
    let xray = Xray::new();

    // Two groups, so a row exists where the two selectors name different ones.
    let mut arns: Vec<String> = Vec::new();
    for name in ["alpha", "beta"] {
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": name, "FilterExpression": "fault = true"}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "seeding group {name} must succeed, got {created:?}"
        );
        arns.push(
            created
                .get("Group")
                .get("GroupARN")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    let alpha_arn = arns[0].clone();
    let beta_arn = arns[1].clone();

    // Rows differ in the selector used and the member updated.
    let updates: Vec<UpdateCase> = vec![
        UpdateCase {
            case: "a new filter expression, by GroupName",
            body: json!({"GroupName": "alpha", "FilterExpression": "error = true"}),
            group: "alpha",
            filter: Some("error = true"),
            insights: None,
        },
        UpdateCase {
            case: "an insights configuration, by GroupARN",
            body: json!({
                "GroupARN": alpha_arn,
                "InsightsConfiguration": {"InsightsEnabled": true, "NotificationsEnabled": true},
            }),
            group: "alpha",
            // The omitted FilterExpression keeps the value the row above stored.
            filter: Some("error = true"),
            insights: Some(true),
        },
        UpdateCase {
            case: "a new filter expression and an insights configuration at once",
            body: json!({
                "GroupName": "alpha",
                "FilterExpression": "responsetime > 2",
                "InsightsConfiguration": {"InsightsEnabled": true, "NotificationsEnabled": false},
            }),
            group: "alpha",
            filter: Some("responsetime > 2"),
            insights: Some(true),
        },
    ];

    for update in updates {
        let UpdateCase {
            case: label,
            body,
            group,
            filter,
            insights,
        } = update;
        let updated = xray.call("/UpdateGroup", &body).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateGroup must succeed, got {updated:?}"
        );
        assert_eq!(
            updated.get("Group").get("GroupARN").and_then(Value::as_str),
            Some(alpha_arn.as_str()),
            "case {label:?}: an update must not move the group's ARN, got {updated:?}"
        );

        // The update is read back from the STORED record.
        let read = xray.call("/GetGroup", &json!({"GroupName": group})).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetGroup must succeed, got {read:?}"
        );
        assert_eq!(
            read.get("Group").get("FilterExpression").and_then(Value::as_str),
            filter,
            "case {label:?}: the stored FilterExpression must be {filter:?}, got {read:?}"
        );
        assert_eq!(
            read.get("Group")
                .get("InsightsConfiguration")
                .and_then(|configuration| configuration.get("InsightsEnabled"))
                .and_then(Value::as_bool),
            insights,
            "case {label:?}: the stored InsightsConfiguration must report {insights:?}, got {read:?}"
        );
    }

    // Rows span the ways an update names no single stored group, plus the one
    // configuration the documentation forbids.
    let refusals: Vec<(&str, Value)> = vec![
        ("no selector at all", json!({"FilterExpression": "fault = true"})),
        (
            "a GroupName naming no stored group",
            json!({"GroupName": "absent", "FilterExpression": "fault = true"}),
        ),
        (
            "a GroupARN naming no stored group",
            json!({
                "GroupARN": "arn:aws:xray:us-east-1:000000000000:group/absent/0123456789ab",
                "FilterExpression": "fault = true",
            }),
        ),
        (
            "a GroupName and a GroupARN naming different groups",
            json!({
                "GroupName": "alpha",
                "GroupARN": beta_arn,
                "FilterExpression": "fault = true",
            }),
        ),
        (
            "a GroupName and the matching GroupARN of one group",
            json!({
                "GroupName": "alpha",
                "GroupARN": alpha_arn,
                "FilterExpression": "fault = true",
            }),
        ),
        (
            "notifications enabled on a group with insights disabled",
            json!({
                "GroupName": "beta",
                "InsightsConfiguration": {"InsightsEnabled": false, "NotificationsEnabled": true},
            }),
        ),
        (
            "notifications enabled with InsightsEnabled omitted",
            json!({
                "GroupName": "beta",
                "InsightsConfiguration": {"NotificationsEnabled": true},
            }),
        ),
    ];

    for (label, body) in refusals {
        let answer = xray.call("/UpdateGroup", &body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: the update must be refused, got {answer:?}"
        );
    }

    // The same configuration rule holds on the way IN, so a group cannot be
    // created in a state an update refuses.
    for (label, body) in [
        (
            "creating a group with notifications and insights disabled",
            json!({
                "GroupName": "forbidden",
                "InsightsConfiguration": {"InsightsEnabled": false, "NotificationsEnabled": true},
            }),
        ),
        (
            "creating a group with notifications and InsightsEnabled omitted",
            json!({
                "GroupName": "forbidden",
                "InsightsConfiguration": {"NotificationsEnabled": true},
            }),
        ),
    ] {
        let answer = xray.call("/CreateGroup", &body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: the create must be refused, got {answer:?}"
        );
        let gone = xray
            .call("/GetGroup", &json!({"GroupName": "forbidden"}))
            .await;
        assert_eq!(
            gone.status, 400,
            "case {label:?}: a refused create must store nothing, got {gone:?}"
        );
    }

    // A refused update changed nothing: beta still carries what it was created
    // with, and alpha still carries the last accepted update.
    let beta = xray.call("/GetGroup", &json!({"GroupName": "beta"})).await;
    assert_eq!(
        beta.get("Group").get("FilterExpression").and_then(Value::as_str),
        Some("fault = true"),
        "a refused update must leave the group it named alone, got {beta:?}"
    );
    let alpha = xray.call("/GetGroup", &json!({"GroupName": "alpha"})).await;
    assert_eq!(
        alpha.get("Group").get("FilterExpression").and_then(Value::as_str),
        Some("responsetime > 2"),
        "a refused update must leave the other group alone, got {alpha:?}"
    );
}

// =============================================================================
// 8. A graph of a group: the group has to exist, and its filter expression is
//    what the graph is derived through
// =============================================================================

#[tokio::test]
async fn a_service_graph_of_a_group_resolves_the_group_and_applies_its_filter() {
    let xray = Xray::new();
    let base = now() - 3_600.0;
    seed_graph(&xray, base).await;

    // The reserved account-level `Default` group is the group with NO filter
    // expression: X-Ray refuses a `CreateGroup` that supplies none (see
    // `group_control_plane.rs`), so the group every region already holds is what
    // exercises the unfiltered path, by either selector.
    let listed = xray.call("/Groups", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "GetGroups must succeed, got {listed:?}"
    );
    let unfiltered_arn = array(&listed.body, "Groups")
        .iter()
        .find(|group| group.get("GroupName").and_then(Value::as_str) == Some("Default"))
        .and_then(|group| group.get("GroupARN").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("the reserved Default group must be listed, got {listed:?}"))
        .to_string();
    let filtered = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": "filtered", "FilterExpression": "fault = true"}),
        )
        .await;
    assert_eq!(
        filtered.status, 200,
        "seeding the filtered group must succeed, got {filtered:?}"
    );

    let window = |body: Value| -> Value {
        let mut body = body;
        body["StartTime"] = json!(base - 60.0);
        body["EndTime"] = json!(base + 60.0);
        body
    };

    // A group with no filter expression selects everything in the window, by
    // either selector.
    //
    // "Everything" is every service the window holds PLUS the `client` node each
    // ROOT service is reached through, which the model documents as a node of
    // the map in its own right — so the count is derived from the fixture rather
    // than from how much of the map one page happens to carry.
    let roots = node_cases().iter().filter(|case| case.root).count();
    let whole_map = node_cases().len() + roots;
    for (label, body) in [
        ("by GroupName", json!({"GroupName": "Default"})),
        ("by GroupARN", json!({"GroupARN": unfiltered_arn})),
    ] {
        let answer = xray.call("/ServiceGraph", &window(body)).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a group without a filter must be served, got {answer:?}"
        );
        assert_eq!(
            array(&answer.body, "Services").len(),
            whole_map,
            "case {label:?}: the graph must carry every service in the window, got {answer:?}"
        );
    }

    // A group that names nothing is refused rather than answered with the
    // unfiltered graph; a group that carries a filter expression is ANSWERED
    // with the graph of the traces that expression selects.
    for (label, body) in [
        ("a group that does not exist", json!({"GroupName": "absent"})),
        (
            "a group ARN that does not exist",
            json!({"GroupARN": "arn:aws:xray:us-east-1:000000000000:group/absent/0123456789ab"}),
        ),
    ] {
        let answer = xray.call("/ServiceGraph", &window(body)).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: the request must be refused, got {answer:?}"
        );
    }

    // The group's `FilterExpression` is "fault = true", and exactly one seeded
    // trace recorded a fault (the 503 root and the service it called), so the
    // graph is derived from that trace alone: narrower than the unfiltered one,
    // and naming only the services that trace touched.
    let answer = xray
        .call("/ServiceGraph", &window(json!({"GroupName": "filtered"})))
        .await;
    assert_eq!(
        answer.status, 200,
        "a group carrying a FilterExpression must be answered, got {answer:?}"
    );
    let named: Vec<String> = array(&answer.body, "Services")
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    for present in ["frontend", "cache"] {
        assert!(
            named.iter().any(|name| name == present),
            "the filtered graph must carry {present:?}, the service the faulted trace touched, got {answer:?}"
        );
    }
    for absent in ["db", "worker"] {
        assert!(
            !named.iter().any(|name| name == absent),
            "the filtered graph must not carry {absent:?}, which no faulted trace touched, got {answer:?}"
        );
    }
    assert_eq!(
        answer.get("ContainsOldGroupVersions"),
        &json!(false),
        "the graph is derived at read time, so it mixes no older group version, got {answer:?}"
    );
}
