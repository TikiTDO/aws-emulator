// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ExecuteQuery` — the graph's own openCypher data plane — driven
//! over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rules every assertion here is about, in the order the operation decides them:
//!
//! 1. **authorization first.** "The IAM user or role making the request must have a
//!    policy attached that allows one of the following IAM actions in that cluster,
//!    depending on the query: `neptune-graph:ReadDataViaQuery`
//!    `neptune-graph:WriteDataViaQuery` `neptune-graph:DeleteDataViaQuery`"
//!    (`smithy_query(query="doc",shape="ExecuteQuery")`), so a caller whose identity
//!    policies allow none of them is refused `AccessDeniedException` (403) naming the
//!    principal, the three actions and the graph ARN — whatever the query says, and
//!    whether or not the graph exists.
//! 2. **the endpoint next.** The operation is bound to the host prefix
//!    `{graphIdentifier}.` (`smithy_query(query="operation",operation="ExecuteQuery")` ->
//!    endpoint), which is registered only while the graph is serving, so a
//!    `graphIdentifier` that names no serving graph of the Region reaches no data plane
//!    at all and is answered `ResourceNotFoundException` (404).
//! 3. **then the query.** A query text the openCypher parser cannot read is refused
//!    `ValidationException` (400) with the modelled `MALFORMED_QUERY` reason and the
//!    position the parser could not continue at; a query it can read is EVALUATED over
//!    the graph's stored nodes and edges, and the payload is what that evaluation
//!    produced.
//!
//! So the tables below span what those rules generalise over rather than re-running one
//! request: the four kinds of credential a call can be signed with crossed with the
//! policies they carry, every status this service puts a graph into plus an identifier no
//! graph holds, several shapes of malformed text, and queries of different shapes
//! (projection, filter, traversal, aggregate, ordering) over two graphs holding DIFFERENT
//! data — so no answer here can be a constant.
//!
//! The data is seeded through the store's own writers (`crate::state::GraphData`), which
//! is where a bulk load and a restore write it too: the engine behind this operation
//! evaluates read-only openCypher, so a query cannot be used to write the rows another
//! query reads.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphEdge, GraphNode, MAX_QUERY_STRING_CHARS, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`).
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The access key id no store holds, which is the emulator's own published credential:
/// the caller nothing bounds.
const AMBIENT_KEY: &str = "AKIDEXAMPLE";

/// The query every row that is not about the query TEXT sends: a well-formed openCypher
/// read the engine evaluates over whatever the graph holds.
const ANY_QUERY: &str = "MATCH (n) RETURN count(n) AS nodes";

/// This service and STS mounted on the SAME shared engine, so the identity a credential
/// speaks for is resolved from the records STS itself answers from, and the IAM store the
/// policies are read from is the one on that engine.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one `restJson1` request signed (in the routing sense) with `access_key_id`.
///
/// `graph_identifier` is sent as the `graphIdentifier` header, which is where the model
/// binds it for this operation (`smithy_query(query="operation",operation="ExecuteQuery")`
/// -> input).
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
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
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/neptune-graph/\
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

/// One `ExecuteQuery` call, as the emulator's own credential.
async fn execute_query(
    router: &MultiServiceRouter,
    graph_identifier: &str,
    query: &str,
) -> (u16, Value) {
    execute_query_as(router, AMBIENT_KEY, graph_identifier, query).await
}

/// One `ExecuteQuery` call signed with `access_key_id`.
async fn execute_query_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    graph_identifier: &str,
    query: &str,
) -> (u16, Value) {
    call_as(
        router,
        access_key_id,
        "POST",
        "/queries",
        Some(graph_identifier),
        Some(json!({"query": query, "language": "OPEN_CYPHER"})),
    )
    .await
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

/// Create a graph and wait until it is serving, returning its id and its ARN.
///
/// It waits for the CONDITION — the status a read reports — rather than for a span, with
/// a deadline generous beside the one the provisioning transition is owed on.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, created) = call_as(
        router,
        AMBIENT_KEY,
        "POST",
        "/graphs",
        None,
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
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
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no arn in {created}"))
        .to_string();
    for _ in 0..40 {
        let (get_status, graph) =
            call_as(router, AMBIENT_KEY, "GET", &format!("/graphs/{id}"), None, None).await;
        assert_eq!(
            get_status, 200,
            "case {name:?}: expected 200 from GetGraph, got {get_status}: {graph}"
        );
        if graph["status"] == json!("AVAILABLE") {
            return (id, arn);
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {name:?}: the graph never reached AVAILABLE");
}

/// One seeded node: its id, its labels, and the properties set on it.
type SeedNode<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, Value)]);

/// One seeded edge: its id, its label, and the nodes it joins.
type SeedEdge<'a> = (&'a str, &'a str, &'a str, &'a str);

/// Write `nodes` and `edges` into the graph's stored data through the store's own
/// writers — the writers a bulk load and a restore use.
fn seed(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    graph_id: &str,
    nodes: &[SeedNode<'_>],
    edges: &[SeedEdge<'_>],
) {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let mut store = state.lock();
    let graph = store
        .graphs
        .get_mut(graph_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no graph {graph_id}"));
    let written_at = std::time::SystemTime::now();
    for (id, labels, properties) in nodes {
        graph
            .data
            .insert_node(
                id,
                GraphNode {
                    labels: labels.iter().map(|label| (*label).to_string()).collect::<BTreeSet<String>>(),
                    properties: properties
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), value.clone()))
                        .collect::<BTreeMap<String, Value>>(),
                },
                written_at,
            )
            .unwrap_or_else(|refused| panic!("case {case:?}: the node {id} was refused: {refused}"));
    }
    for (id, label, from, to) in edges {
        graph
            .data
            .insert_edge(
                id,
                GraphEdge {
                    label: (*label).to_string(),
                    from_node: (*from).to_string(),
                    to_node: (*to).to_string(),
                    properties: BTreeMap::new(),
                },
                written_at,
            )
            .unwrap_or_else(|refused| panic!("case {case:?}: the edge {id} was refused: {refused}"));
    }
}

/// Record, in the IAM store of the shared engine, that the account holds `access_key_id`
/// for an IAM user named `user_name` carrying `policy` (when it carries one) as an inline
/// policy — the records `iam:CreateUser`, `iam:CreateAccessKey` and `iam:PutUserPolicy`
/// write — and answer that user's ARN.
fn hold_user(
    engine: &Arc<engine::InMemoryEngine>,
    user_name: &str,
    access_key_id: &str,
    policy: Option<&str>,
) -> String {
    let arn = format!(
        "arn:aws:iam::{}:user/{user_name}",
        engine::emulator_account_id()
    );
    let state = iam::service_state(&**engine).expect("the engine carries the IAM store");
    let mut account = state.lock();
    account.users.insert(
        iam::name_key(user_name),
        User {
            path: "/".to_string(),
            user_name: user_name.to_string(),
            user_id: sts::state::unique_id("AIDA", &arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    account.access_keys.insert(
        access_key_id.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(user_name),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
        },
    );
    if let Some(document) = policy {
        account.user_inline_policies.insert(
            iam::name_key(user_name),
            [(
                iam::name_key("query"),
                iam::InlinePolicy {
                    policy_name: "query".to_string(),
                    policy_document: document.to_string(),
                },
            )]
            .into_iter()
            .collect(),
        );
    }
    arn
}

/// The rows a result payload carries: `{"results": [ … ]}` is the openCypher result form
/// this data plane answers in.
fn rows(case: &str, payload: &Value) -> Vec<Value> {
    payload["results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no results array in {payload}"))
        .clone()
}

#[tokio::test]
async fn a_query_is_evaluated_over_the_graphs_stored_data() {
    let (router, engine) = emulator();

    // TWO graphs holding DIFFERENT data, so the same query cannot answer both: the rows
    // below are read from whichever graph the request named.
    let (cities, _) = graph_in_service(&router, "query-cities").await;
    seed(
        &engine,
        "the cities graph",
        &cities,
        &[
            ("c1", &["city"], &[("name", json!("Berlin")), ("population", json!(3_600_000))]),
            ("c2", &["city"], &[("name", json!("Paris")), ("population", json!(2_100_000))]),
            ("c3", &["city"], &[("name", json!("Lisbon")), ("population", json!(545_000))]),
            ("a1", &["airport"], &[("name", json!("BER"))]),
        ],
        &[
            ("r1", "route", "c1", "c2"),
            ("r2", "route", "c2", "c3"),
            ("s1", "serves", "a1", "c1"),
        ],
    );
    let (one_city, _) = graph_in_service(&router, "query-one-city").await;
    seed(
        &engine,
        "the one-city graph",
        &one_city,
        &[("c1", &["city"], &[("name", json!("Reykjavik"))])],
        &[],
    );

    // (case, the query, the graph it runs on, the rows it must answer)
    let rows_of: &[(&str, &str, &str, Value)] = &[
        (
            "a count over every node the graph holds",
            ANY_QUERY,
            &cities,
            json!([{"nodes": 4}]),
        ),
        (
            "a projection that reads nothing of the graph",
            "RETURN 1 AS one",
            &cities,
            json!([{"one": 1}]),
        ),
        (
            "the same projection on the graph holding other data",
            "RETURN 1 AS one",
            &one_city,
            json!([{"one": 1}]),
        ),
        (
            "a property projection, ordered",
            "MATCH (c:city) RETURN c.name AS name ORDER BY name",
            &cities,
            json!([{"name": "Berlin"}, {"name": "Lisbon"}, {"name": "Paris"}]),
        ),
        (
            "the same query on the graph holding other data",
            "MATCH (c:city) RETURN c.name AS name ORDER BY name",
            &one_city,
            json!([{"name": "Reykjavik"}]),
        ),
        (
            "a label the graph holds one node of",
            "MATCH (a:airport) RETURN a.name AS name",
            &cities,
            json!([{"name": "BER"}]),
        ),
        (
            "a filter over a stored property",
            "MATCH (c:city) WHERE c.population > 1000000 RETURN c.name AS name ORDER BY name",
            &cities,
            json!([{"name": "Berlin"}, {"name": "Paris"}]),
        ),
        (
            "a traversal of a stored edge",
            "MATCH (a:city)-[:route]->(b:city) RETURN a.name AS departs, b.name AS arrives \
             ORDER BY departs",
            &cities,
            json!([
                {"departs": "Berlin", "arrives": "Paris"},
                {"departs": "Paris", "arrives": "Lisbon"},
            ]),
        ),
        (
            "an incoming traversal",
            "MATCH (c:city)<-[:serves]-(a:airport) RETURN c.name AS served",
            &cities,
            json!([{"served": "Berlin"}]),
        ),
        (
            "an aggregate over the stored nodes",
            "MATCH (c:city) RETURN count(c) AS total",
            &cities,
            json!([{"total": 3}]),
        ),
        (
            "an aggregate over the other graph's nodes",
            "MATCH (c:city) RETURN count(c) AS total",
            &one_city,
            json!([{"total": 1}]),
        ),
        (
            "a bound limit",
            "MATCH (c:city) RETURN c.name AS name ORDER BY name LIMIT 1",
            &cities,
            json!([{"name": "Berlin"}]),
        ),
        (
            "a pattern no stored edge satisfies",
            "MATCH (a:airport)-[:route]->(c:city) RETURN a.name AS name",
            &cities,
            json!([]),
        ),
    ];

    for (case, query, graph_id, expected) in rows_of {
        let (status, payload) = execute_query(&router, graph_id, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ExecuteQuery, got {status}: {payload}"
        );
        let answered = Value::Array(rows(case, &payload));
        assert_eq!(
            answered, *expected,
            "case {case:?}: expected the rows {expected}, got {answered}"
        );
    }

    // A node returned whole is reported as this service reports one: its id, its labels
    // and its properties, read from the store.
    let (status, payload) = execute_query(
        &router,
        &one_city,
        "MATCH (c:city) RETURN c AS node",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a bound node\": expected 200, got {status}: {payload}"
    );
    assert_eq!(
        Value::Array(rows("a bound node", &payload)),
        json!([{"node": {
            "~id": "c1",
            "~entityType": "node",
            "~labels": ["city"],
            "~properties": {"name": "Reykjavik"},
        }}]),
        "case \"a bound node\": unexpected node projection in {payload}"
    );
}

#[tokio::test]
async fn a_query_the_parser_cannot_read_is_refused_as_a_validation_exception() {
    let (router, _engine) = emulator();
    let (graph_id, _) = graph_in_service(&router, "query-malformed").await;

    // Texts no openCypher grammar accepts, differing in WHERE they stop being readable:
    // an unclosed pattern, a clause with nothing to project, a keyword that is not one,
    // text that is not a query at all, and a second line that breaks.
    let malformed: &[(&str, &str)] = &[
        ("an unclosed node pattern", "MATCH (n RETURN n"),
        ("a RETURN with nothing to project", "MATCH (n) RETURN"),
        ("a keyword that is not one", "MATHC (n) RETURN n"),
        ("text that is not a query", "!!!"),
        ("a break on the second line", "MATCH (n)\nRETRUN n"),
        ("a dangling relationship", "MATCH (n)-[:r]-> RETURN n"),
    ];

    for (case, query) in malformed {
        let (status, body) = execute_query(&router, &graph_id, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["reason"],
            json!("MALFORMED_QUERY"),
            "case {case:?}: expected the MALFORMED_QUERY reason, got {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with("Invalid input") && message.contains("(line "),
            "case {case:?}: expected the refusal to state the position it was read to, \
             got {message:?}"
        );
    }

    // The graph is untouched by a refused query: it still serves, and a well-formed query
    // on it is still answered.
    let (status, payload) = execute_query(&router, &graph_id, ANY_QUERY).await;
    assert_eq!(
        status, 200,
        "case \"a well-formed query after the refusals\": expected 200, got {status}: \
         {payload}"
    );

    // A text past the length this service reads is refused for that, before it is parsed:
    // a different fact about the request, so a different modelled reason.
    let oversized = format!(
        "MATCH (n) RETURN n // {}",
        "x".repeat(MAX_QUERY_STRING_CHARS)
    );
    let (status, body) = execute_query(&router, &graph_id, &oversized).await;
    assert_eq!(
        status, 400,
        "case \"a text past the length the service reads\": expected 400, got {status}: \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a text past the length the service reads\": unexpected error code in {body}"
    );
    assert_eq!(
        body["reason"],
        json!("QUERY_TOO_LARGE"),
        "case \"a text past the length the service reads\": expected the QUERY_TOO_LARGE \
         reason, got {body}"
    );
}

#[tokio::test]
async fn a_query_reaches_only_a_graph_whose_data_plane_is_serving() {
    let (router, engine) = emulator();

    // Every status this service puts a graph into, and whether a graph in it serves its
    // own endpoint. Source for the set: the modelled `GraphStatus` enum
    // (`smithy_query(query="shape",shape="GraphStatus")`) minus `FAILED`, which no path
    // in this service produces.
    let statuses: &[(&str, bool)] = &[
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

    let (graph_id, _) = graph_in_service(&router, "query-statuses").await;
    let not_found = format!(
        "No graph found for customer [{}] with graph id [{graph_id}]",
        engine::emulator_account_id()
    );
    for (graph_status, serving) in statuses {
        // The status is parked with nothing owed out of it, so the answer cannot depend
        // on how long the call took to arrive.
        {
            let state = service_state(&*engine).expect("the neptune-graph store is mounted");
            let mut store = state.lock();
            let graph = store
                .graphs
                .get_mut(&graph_id)
                .expect("the graph was created over the wire");
            graph.status = parked_status(graph_status);
            graph.pending = None;
            graph.deletes_at = None;
        }
        let case = format!("a query on a graph in {graph_status}");
        let (status, body) = execute_query(&router, &graph_id, ANY_QUERY).await;
        if *serving {
            assert_eq!(
                status, 200,
                "case {case:?}: a serving graph answers its own data plane, got {status}: \
                 {body}"
            );
            continue;
        }
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"],
            json!(not_found),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // And an identifier no graph holds at all is the same answer: there is no endpoint at
    // that address either.
    let absent = "g-0000000000";
    let (status, body) = execute_query(&router, absent, ANY_QUERY).await;
    assert_eq!(
        status, 404,
        "case \"an identifier no graph holds\": expected 404, got {status}: {body}"
    );
    assert_eq!(
        body["message"],
        json!(format!(
            "No graph found for customer [{}] with graph id [{absent}]",
            engine::emulator_account_id()
        )),
        "case \"an identifier no graph holds\": unexpected message in {body}"
    );
}

/// The stored status one wire `GraphStatus` value names.
fn parked_status(
    wire: &str,
) -> service_com_amazonaws_neptunegraph_amazonneptunegraph::state::GraphStatus {
    use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::GraphStatus;
    match wire {
        "CREATING" => GraphStatus::Creating,
        "AVAILABLE" => GraphStatus::Available,
        "UPDATING" => GraphStatus::Updating,
        "SNAPSHOTTING" => GraphStatus::Snapshotting,
        "IMPORTING" => GraphStatus::Importing,
        "STOPPING" => GraphStatus::Stopping,
        "STOPPED" => GraphStatus::Stopped,
        "STARTING" => GraphStatus::Starting,
        "RESETTING" => GraphStatus::Resetting,
        "DELETING" => GraphStatus::Deleting,
        other => panic!("no stored status is named {other:?}"),
    }
}

#[tokio::test]
async fn a_caller_whose_policies_allow_no_query_action_is_denied() {
    let (router, engine) = emulator();
    let (graph_id, graph_arn) = graph_in_service(&router, "query-authorization").await;

    /// A document allowing one action on one resource.
    fn allow(action: &str, resource: &str) -> String {
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"{action}","Resource":"{resource}"}}]}}"#
        )
    }

    let other_graph_arn = graph_arn.replace(&graph_id, "g-9999999999");
    // (case, the user the key is held for, its access key id, the policy it carries,
    // whether the query is served). The rows differ along WHICH grant the caller holds:
    // one of the three query actions suffices, a grant for another graph or another
    // action does not, and an explicit deny beats a wildcard grant.
    let rows: &[(&str, &str, &str, Option<String>, bool)] = &[
        ("no policy at all", "analyst-none", "AKIAQUERYNONE", None, false),
        (
            "read on this graph",
            "analyst-read",
            "AKIAQUERYREAD",
            Some(allow("neptune-graph:ReadDataViaQuery", &graph_arn)),
            true,
        ),
        (
            "write on this graph",
            "analyst-write",
            "AKIAQUERYWRITE",
            Some(allow("neptune-graph:WriteDataViaQuery", &graph_arn)),
            true,
        ),
        (
            "delete on this graph",
            "analyst-delete",
            "AKIAQUERYDELETE",
            Some(allow("neptune-graph:DeleteDataViaQuery", &graph_arn)),
            true,
        ),
        (
            "every action of the service on every resource",
            "analyst-all",
            "AKIAQUERYALL",
            Some(allow("neptune-graph:*", "*")),
            true,
        ),
        (
            "read on another graph",
            "analyst-other",
            "AKIAQUERYOTHER",
            Some(allow("neptune-graph:ReadDataViaQuery", &other_graph_arn)),
            false,
        ),
        (
            "a control-plane action on this graph",
            "analyst-control",
            "AKIAQUERYCONTROL",
            Some(allow("neptune-graph:GetGraph", &graph_arn)),
            false,
        ),
        (
            "another service's actions",
            "analyst-elsewhere",
            "AKIAQUERYELSE",
            Some(allow("s3:GetObject", "*")),
            false,
        ),
        (
            "a wildcard grant with the query actions denied",
            "analyst-denied",
            "AKIAQUERYDENIED",
            Some(
                r#"{"Version":"2012-10-17","Statement":[
                {"Effect":"Allow","Action":"neptune-graph:*","Resource":"*"},
                {"Effect":"Deny","Action":["neptune-graph:ReadDataViaQuery",
                 "neptune-graph:WriteDataViaQuery","neptune-graph:DeleteDataViaQuery"],
                 "Resource":"*"}]}"#
                    .to_string(),
            ),
            false,
        ),
    ];

    for (case, user_name, access_key_id, policy, served) in rows {
        let principal = hold_user(&engine, user_name, access_key_id, policy.as_deref());
        let (status, body) =
            execute_query_as(&router, access_key_id, &graph_id, ANY_QUERY).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the query to be served, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(status, 403, "case {case:?}: expected 403, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!(
                "User: {principal} is not authorized to perform: \
                 neptune-graph:DeleteDataViaQuery, neptune-graph:ReadDataViaQuery, \
                 neptune-graph:WriteDataViaQuery on resource: {graph_arn}"
            )),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // The emulator's own published credential holds no policy of the account's and is the
    // one caller this decision cannot refuse.
    let (status, payload) = execute_query(&router, &graph_id, ANY_QUERY).await;
    assert_eq!(
        status, 200,
        "case \"the emulator's own credential\": expected 200, got {status}: {payload}"
    );
}

#[tokio::test]
async fn a_query_naming_anything_outside_the_engines_read_only_subset_is_refused() {
    let (router, engine) = emulator();
    let (graph_id, _) = graph_in_service(&router, "query-surface").await;
    seed(
        &engine,
        "the surface graph",
        &graph_id,
        &[("n1", &["Person"], &[("name", json!("Ada"))])],
        &[],
    );

    // What the engine accepts is a READ-ONLY Cypher subset, so everything a caller might
    // reach the host or the store with is outside it and is refused rather than run. The
    // rows span the shapes that would be an escape if one existed, and the modelled
    // reason each is refused under says WHICH way it is outside the subset: a CLAUSE the
    // grammar does not accept (a bulk load from a file, a procedure call, and the four
    // write clauses the engine's own documentation states it does not support) is the
    // unsupported operation it is, while a FUNCTION the evaluator does not implement
    // (named here after a file read, an HTTP fetch of the instance-metadata address and a
    // dynamic import) is the illegal argument it is.
    let outside: &[(&str, &str, &str)] = &[
        (
            "a clause that loads a file",
            "LOAD CSV FROM 'file:///etc/passwd' AS line RETURN line",
            "UNSUPPORTED_OPERATION",
        ),
        (
            "a procedure call",
            "CALL dbms.listConfig() YIELD name RETURN name",
            "UNSUPPORTED_OPERATION",
        ),
        (
            "a function that would read a file",
            "MATCH (n) RETURN readFile('/etc/passwd') AS leaked",
            "ILLEGAL_ARGUMENT",
        ),
        (
            "a function that would fetch a URL",
            "MATCH (n) RETURN httpGet('http://169.254.169.254/latest/meta-data/') AS leaked",
            "ILLEGAL_ARGUMENT",
        ),
        (
            "a dynamic import",
            "MATCH (n) RETURN apocLoadJson('file:///etc/passwd') AS leaked",
            "ILLEGAL_ARGUMENT",
        ),
        (
            "a create",
            "CREATE (n:Person {name: 'Eve'}) RETURN n",
            "UNSUPPORTED_OPERATION",
        ),
        (
            "a merge",
            "MERGE (n:Person {name: 'Eve'}) RETURN n",
            "UNSUPPORTED_OPERATION",
        ),
        (
            "a write to a property",
            "MATCH (n) SET n.name = 'Eve' RETURN n",
            "UNSUPPORTED_OPERATION",
        ),
        (
            "a delete",
            "MATCH (n) DELETE n RETURN n",
            "UNSUPPORTED_OPERATION",
        ),
    ];

    for (case, query, reason) in outside {
        let (status, body) = execute_query(&router, &graph_id, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code in {body}"
        );
        // A well-formed query naming a clause this engine does not run is refused as the
        // UNSUPPORTED operation it is, not as a text that does not parse: a caller must
        // be able to tell "this endpoint does not do that" from "fix your query".
        assert_eq!(
            body["reason"],
            json!(reason),
            "case {case:?}: expected the {reason} reason, got {body}"
        );
    }

    // And the graph is untouched by every one of them: the write clauses stored nothing,
    // so the one node it was seeded with is the one node it still holds.
    let (status, payload) = execute_query(
        &router,
        &graph_id,
        "MATCH (n) RETURN count(n) AS nodes, collect(n.name) AS names",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the graph after the refusals\": expected 200, got {status}: {payload}"
    );
    assert_eq!(
        Value::Array(rows("the graph after the refusals", &payload)),
        json!([{"nodes": 1, "names": ["Ada"]}]),
        "case \"the graph after the refusals\": a refused query must change nothing, got \
         {payload}"
    );
}

#[tokio::test]
async fn authorization_is_decided_before_the_graph_is_resolved() {
    let (router, engine) = emulator();
    let absent = "g-0000000000";
    let principal = hold_user(&engine, "analyst-early", "AKIAQUERYEARLY", None);

    // No graph exists at all, so the only decision this call can meet is the one taken
    // from the request alone: the caller learns it may not query, not what the account
    // holds.
    let (status, body) = execute_query_as(&router, "AKIAQUERYEARLY", absent, ANY_QUERY).await;
    assert_eq!(
        status, 403,
        "case \"an unauthorized caller naming no graph\": expected 403, got {status}: {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(&format!("User: {principal} is not authorized to perform:"))
            && message.ends_with(&format!(":graph/{absent}")),
        "case \"an unauthorized caller naming no graph\": expected the denial to name the \
         principal and the graph the request addressed, got {message:?}"
    );
    // A malformed query is not read either: the decision is taken before the text.
    let (syntax_status, syntax_body) =
        execute_query_as(&router, "AKIAQUERYEARLY", absent, "MATCH (n RETURN n").await;
    assert_eq!(
        syntax_status, 403,
        "case \"an unauthorized caller sending a malformed query\": expected 403, got \
         {syntax_status}: {syntax_body}"
    );
}
