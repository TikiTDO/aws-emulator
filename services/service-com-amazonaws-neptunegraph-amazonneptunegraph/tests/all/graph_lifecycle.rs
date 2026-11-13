// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the graph-resource lifecycle: `CreateGraph`, `GetGraph`,
//! `ListGraphs`, `UpdateGraph` and `DeleteGraph`, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! Every assertion here is about a RULE, exercised over a set of inputs rather than
//! one recorded request: the whole `provisionedMemory`/`replicaCount` range at both
//! published bounds and one past each, every clause of the `graphName` pattern, and
//! every page size over a fixed population of graphs.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    DELETION_SPAN, MAX_GRAPHS, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The region the test's SigV4 credential scope names, so the ARNs and endpoints the
/// service builds are predictable.
const REGION: &str = "us-east-1";

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
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

/// `POST /graphs` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/graphs", Some(body)).await
}

/// `GET /graphs/{id}`.
async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
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

/// Create a graph and return its id, failing the test if the call did not answer 201.
async fn create_ok(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = create(router, body.clone()).await;
    assert_eq!(status, 201, "case {body}: expected 201, got {status}: {created}");
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string()
}

/// Wait out the provisioning transition the clock owes, then assert the graph reports
/// `AVAILABLE`. No handler ever sleeps; only this test does, because it is asking
/// about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, id: &str) {
    for _ in 0..20 {
        let (status, body) = get(router, id).await;
        assert_eq!(status, 200, "case {id}: expected 200 from GetGraph, got {status}: {body}");
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = get(router, id).await;
    panic!("case {id}: graph never became AVAILABLE, last status {}", body["status"]);
}

/// A graph created and then waited into service, ready to be updated or deleted.
async fn create_available(router: &MultiServiceRouter, body: Value) -> String {
    let id = create_ok(router, body).await;
    wait_until_available(router, &id).await;
    id
}

/// Read the graph until `GetGraph` no longer finds it, which is the condition an
/// accepted deletion reaches on its own. It waits for THAT rather than for a span, with
/// a deadline generous beside the one the deletion is owed on.
async fn wait_until_gone(router: &MultiServiceRouter, id: &str) {
    for _ in 0..40 {
        let (status, _) = get(router, id).await;
        if status == 404 {
            return;
        }
        tokio::time::sleep(DELETION_SPAN / 4).await;
    }
    let (status, body) = get(router, id).await;
    panic!("case {id}: the graph never went away, last read {status}: {body}");
}

#[tokio::test]
async fn create_graph_persists_every_supplied_member_and_defaults_the_rest() {
    let router = emulator();

    // (case, request body, the members the response and a later GetGraph must report).
    // The rows differ along what the rule generalises over: each published bound of
    // `provisionedMemory` and `replicaCount`, both booleans, the optional
    // `vectorSearchConfiguration`/`kmsKeyIdentifier`, tag counts, and name shapes.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "only the required members: the optional ones take their documented defaults",
            json!({"graphName": "defaults-graph", "provisionedMemory": 16}),
            json!({
                "name": "defaults-graph",
                "provisionedMemory": 16,
                "replicaCount": 1,
                "publicConnectivity": false,
                "deletionProtection": false,
            }),
        ),
        (
            "the top of the provisionedMemory range with zero replicas",
            json!({
                "graphName": "max-memory-graph",
                "provisionedMemory": 24576,
                "replicaCount": 0,
                "publicConnectivity": true,
                "deletionProtection": true,
            }),
            json!({
                "name": "max-memory-graph",
                "provisionedMemory": 24576,
                "replicaCount": 0,
                "publicConnectivity": true,
                "deletionProtection": true,
            }),
        ),
        (
            "every optional member supplied, at the top of the replica range",
            json!({
                "graphName": "a",
                "provisionedMemory": 128,
                "replicaCount": 2,
                "publicConnectivity": false,
                "deletionProtection": false,
                "kmsKeyIdentifier": KMS_KEY_ARN,
                "vectorSearchConfiguration": {"dimension": 1536},
                "tags": {"team": "graph", "env": "test"},
            }),
            json!({
                "name": "a",
                "provisionedMemory": 128,
                "replicaCount": 2,
                "publicConnectivity": false,
                "deletionProtection": false,
                "kmsKeyIdentifier": KMS_KEY_ARN,
                "vectorSearchConfiguration": {"dimension": 1536},
            }),
        ),
        (
            "a hyphenated name carrying digits, and a one-dimension vector index",
            json!({
                "graphName": "g9-analytics-7",
                "provisionedMemory": 17,
                "vectorSearchConfiguration": {"dimension": 1},
                "tags": {},
            }),
            json!({
                "name": "g9-analytics-7",
                "provisionedMemory": 17,
                "replicaCount": 1,
                "publicConnectivity": false,
                "deletionProtection": false,
                "vectorSearchConfiguration": {"dimension": 1},
            }),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, request, expected) in cases {
        let (status, created) = create(&router, request.clone()).await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status}: {created}");

        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
            .to_string();
        let suffix = id.strip_prefix("g-").unwrap_or_default();
        assert!(
            suffix.len() == 10
                && suffix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case:?}: id must match ^g-[a-z0-9]{{10}}$, got {id:?}"
        );
        assert!(minted.insert(id.clone()), "case {case:?}: id {id:?} was minted twice");

        assert_eq!(
            created["arn"],
            json!(format!(
                "arn:aws:neptune-graph:{REGION}:{}:graph/{id}",
                engine::EMULATOR_ACCOUNT_ID
            )),
            "case {case:?}: unexpected arn {}",
            created["arn"]
        );
        assert_eq!(
            created["endpoint"],
            json!(format!("{id}.{REGION}.neptune-graph.amazonaws.com")),
            "case {case:?}: unexpected endpoint {}",
            created["endpoint"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: a freshly created graph is not in service yet, got {}",
            created["status"]
        );
        assert!(
            created["createTime"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: expected a createTime, got {}",
            created["createTime"]
        );
        assert!(
            created.get("sourceSnapshotId").is_none_or(Value::is_null),
            "case {case:?}: a graph not restored from a snapshot has no sourceSnapshotId, got {}",
            created["sourceSnapshotId"]
        );

        let expected_members = expected.as_object().expect("the expectation is an object");
        for (member, want) in expected_members {
            assert_eq!(
                &created[member], want,
                "case {case:?}: CreateGraph member {member:?}: expected {want}, got {}",
                created[member]
            );
        }

        // ROUND TRIP: the record a later read returns is the one that was stored.
        let (get_status, fetched) = get(&router, &id).await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {get_status}: {fetched}"
        );
        for (member, want) in expected_members {
            assert_eq!(
                &fetched[member], want,
                "case {case:?}: GetGraph member {member:?}: expected {want}, got {}",
                fetched[member]
            );
        }
        for member in ["id", "arn", "endpoint", "createTime"] {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: GetGraph member {member:?} differs from CreateGraph: {} vs {}",
                fetched[member], created[member]
            );
        }
    }
}

#[tokio::test]
async fn create_graph_refuses_a_name_another_graph_already_holds() {
    let router = emulator();
    let first = create_ok(
        &router,
        json!({"graphName": "taken-name", "provisionedMemory": 16}),
    )
    .await;

    let (status, body) = create(
        &router,
        json!({"graphName": "taken-name", "provisionedMemory": 4096}),
    )
    .await;
    assert_eq!(status, 409, "case duplicate name: expected 409, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case duplicate name: unexpected error code in {body}"
    );

    // The conflict changed nothing: the first graph is still the only one.
    let (list_status, listed) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(list_status, 200, "expected 200 from ListGraphs, got {list_status}: {listed}");
    let ids: Vec<&str> = listed["graphs"]
        .as_array()
        .expect("graphs is a list")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec![first.as_str()], "case duplicate name: store changed, got {listed}");
}

#[tokio::test]
async fn create_graph_refuses_values_outside_the_modelled_constraints() {
    let router = emulator();

    // Each published bound of a range is accepted and one step past it is refused; the
    // name rows walk every clause of the modelled `graphName` pattern
    // `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "provisionedMemory at its minimum (16)",
            json!({"graphName": "bound-min-memory", "provisionedMemory": 16}),
        ),
        (
            "provisionedMemory at its maximum (24576)",
            json!({"graphName": "bound-max-memory", "provisionedMemory": 24576}),
        ),
        (
            "replicaCount at its minimum (0)",
            json!({"graphName": "bound-min-replicas", "provisionedMemory": 16, "replicaCount": 0}),
        ),
        (
            "replicaCount at its maximum (2)",
            json!({"graphName": "bound-max-replicas", "provisionedMemory": 16, "replicaCount": 2}),
        ),
    ];
    for (case, request) in accepted {
        let (status, body) = create(&router, request).await;
        assert_eq!(status, 201, "case {case:?}: expected 201, got {status}: {body}");
    }

    let refused: Vec<(&str, Value)> = vec![
        (
            "provisionedMemory one below the minimum",
            json!({"graphName": "under-memory", "provisionedMemory": 15}),
        ),
        (
            "provisionedMemory one above the maximum",
            json!({"graphName": "over-memory", "provisionedMemory": 24577}),
        ),
        (
            "replicaCount one below the minimum",
            json!({"graphName": "under-replicas", "provisionedMemory": 16, "replicaCount": -1}),
        ),
        (
            "replicaCount one above the maximum",
            json!({"graphName": "over-replicas", "provisionedMemory": 16, "replicaCount": 3}),
        ),
        (
            "graphName reserved for graph ids (the pattern's negative lookahead)",
            json!({"graphName": "g-reserved", "provisionedMemory": 16}),
        ),
        (
            "graphName starting with a digit",
            json!({"graphName": "1graph", "provisionedMemory": 16}),
        ),
        (
            "graphName with an uppercase letter",
            json!({"graphName": "MyGraph", "provisionedMemory": 16}),
        ),
        (
            "graphName with two consecutive hyphens",
            json!({"graphName": "my--graph", "provisionedMemory": 16}),
        ),
        (
            "graphName ending with a hyphen",
            json!({"graphName": "my-graph-", "provisionedMemory": 16}),
        ),
        (
            "graphName shorter than the minimum length",
            json!({"graphName": "", "provisionedMemory": 16}),
        ),
        (
            "graphName longer than the maximum length (64 characters)",
            json!({"graphName": "a".repeat(64), "provisionedMemory": 16}),
        ),
        (
            "kmsKeyIdentifier that is not a KMS key ARN",
            json!({
                "graphName": "bad-kms",
                "provisionedMemory": 16,
                "kmsKeyIdentifier": "not-an-arn",
            }),
        ),
        (
            "more tags than one record may hold",
            json!({
                "graphName": "too-many-tags",
                "provisionedMemory": 16,
                "tags": (0..51)
                    .map(|index| (format!("key{index}"), json!("v")))
                    .collect::<serde_json::Map<String, Value>>(),
            }),
        ),
        (
            "a tag value longer than one record may hold",
            json!({
                "graphName": "long-tag-value",
                "provisionedMemory": 16,
                "tags": {"key": "v".repeat(257)},
            }),
        ),
    ];
    for (case, request) in refused {
        let (status, body) = create(&router, request).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A refused request stored nothing: only the four accepted graphs exist.
    let (_, listed) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(
        listed["graphs"].as_array().map(Vec::len),
        Some(4),
        "a refused CreateGraph must store nothing, got {listed}"
    );
}

#[tokio::test]
async fn get_graph_refuses_an_id_no_graph_holds() {
    let router = emulator();
    create_ok(&router, json!({"graphName": "present", "provisionedMemory": 16})).await;

    // Well-formed ids (the modelled `^g-[a-z0-9]{10}$`) that name no stored graph.
    for id in ["g-0000000000", "g-zzzzzzzzzz", "g-a1b2c3d4e5"] {
        let (status, body) = get(&router, id).await;
        assert_eq!(status, 404, "case {id:?}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: unexpected error code in {body}"
        );
    }

    // An identifier the modelled pattern refuses is a 400, not a 404.
    for id in ["not-a-graph-id", "g-TOOSHORT", "gs-0123456789"] {
        let (status, body) = get(&router, id).await;
        assert_eq!(status, 400, "case {id:?}: expected 400, got {status}: {body}");
    }
}

#[tokio::test]
async fn list_graphs_pages_over_every_graph_without_repeats() {
    let router = emulator();

    const POPULATION: usize = 7;
    let mut all_ids: BTreeSet<String> = BTreeSet::new();
    for index in 0..POPULATION {
        all_ids.insert(
            create_ok(
                &router,
                json!({
                    "graphName": format!("page-graph-{index}"),
                    "provisionedMemory": 16 + i64::try_from(index).unwrap_or(0),
                }),
            )
            .await,
        );
    }

    // The `@paginated` contract, over every interesting page size: 1, a size that
    // does not divide the population, exactly the population, and more than it.
    for page_size in [1_usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=POPULATION {
            let path = match &token {
                Some(value) => format!("/graphs?maxResults={page_size}&nextToken={value}"),
                None => format!("/graphs?maxResults={page_size}"),
            };
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}, page {page}: expected 200, got {status}: {body}"
            );
            let graphs = body["graphs"].as_array().unwrap_or_else(|| {
                panic!("case page size {page_size}, page {page}: graphs is required, got {body}")
            });
            assert!(
                graphs.len() <= page_size,
                "case page size {page_size}, page {page}: expected at most {page_size} graphs, \
                 got {}",
                graphs.len()
            );
            for summary in graphs {
                let id = summary["id"].as_str().unwrap_or_default().to_string();
                assert!(
                    all_ids.contains(&id),
                    "case page size {page_size}, page {page}: unknown id {id:?}"
                );
                assert!(
                    !seen.contains(&id),
                    "case page size {page_size}, page {page}: id {id:?} was returned twice"
                );
                // The summary projection carries the stored record's own values.
                assert_eq!(
                    summary["arn"],
                    json!(format!(
                        "arn:aws:neptune-graph:{REGION}:{}:graph/{id}",
                        engine::EMULATOR_ACCOUNT_ID
                    )),
                    "case page size {page_size}: unexpected arn for {id:?}: {}",
                    summary["arn"]
                );
                seen.push(id);
            }
            token = body["nextToken"].as_str().map(ToString::to_string);
            match &token {
                // A token is issued only while graphs remain, so the page it came
                // from was full.
                Some(_) => assert_eq!(
                    graphs.len(),
                    page_size,
                    "case page size {page_size}, page {page}: a token was issued after a \
                     short page"
                ),
                None => break,
            }
        }
        assert!(
            token.is_none(),
            "case page size {page_size}: paging never finished, token {token:?}"
        );
        assert_eq!(
            seen.len(),
            POPULATION,
            "case page size {page_size}: expected {POPULATION} graphs in total, got {}",
            seen.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all_ids,
            "case page size {page_size}: the pages did not cover every stored graph"
        );
    }

    // A token this service never issued names no page.
    for token in ["zzz", "deadbeefdeadbeef", "not-a-token", &hex::encode("not-a-graph-id")] {
        let (status, body) =
            call(&router, "GET", &format!("/graphs?nextToken={token}"), None).await;
        assert_eq!(status, 400, "case token {token:?}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case token {token:?}: unexpected error code in {body}"
        );
    }

    // A token OF THIS LISTING that does name a position in the graph-id space resumes
    // from that position, even though no graph was ever minted with that id: a page
    // resumes after a POSITION, so a graph deleted between two pages cannot hide the
    // graphs that follow it. (A token is the minting listing and the position it
    // reached, hex-encoded; which listing a token belongs to is
    // `tests/all/page_token_listing_scope.rs`'s rule.)
    let unissued = hex::encode("ListGraphs:g-aaaaaaaaaa");
    let (status, body) =
        call(&router, "GET", &format!("/graphs?nextToken={unissued}"), None).await;
    assert_eq!(status, 200, "case unissued position: expected 200, got {status}: {body}");
    for summary in body["graphs"].as_array().expect("graphs is a list") {
        let id = summary["id"].as_str().unwrap_or_default();
        assert!(
            id > "g-aaaaaaaaaa",
            "case unissued position: {id:?} does not sort after the token's position"
        );
    }

    // maxResults outside its modelled 1..100 range is refused at the boundary.
    for max_results in ["0", "101"] {
        let (status, body) =
            call(&router, "GET", &format!("/graphs?maxResults={max_results}"), None).await;
        assert_eq!(
            status, 400,
            "case maxResults {max_results}: expected 400, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn update_graph_changes_only_the_members_the_request_supplies() {
    let router = emulator();

    // (case, patch body, the members that must hold after it). Each row starts from
    // the state the previous row left, so the unsupplied members are checked against
    // a value that actually changed earlier.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an empty patch changes nothing",
            json!({}),
            json!({"provisionedMemory": 16, "publicConnectivity": false, "deletionProtection": false}),
        ),
        (
            "provisionedMemory alone",
            json!({"provisionedMemory": 32}),
            json!({"provisionedMemory": 32, "publicConnectivity": false, "deletionProtection": false}),
        ),
        (
            "publicConnectivity alone",
            json!({"publicConnectivity": true}),
            json!({"provisionedMemory": 32, "publicConnectivity": true, "deletionProtection": false}),
        ),
        (
            "deletionProtection alone",
            json!({"deletionProtection": true}),
            json!({"provisionedMemory": 32, "publicConnectivity": true, "deletionProtection": true}),
        ),
        (
            "every updatable member at once, at the top of the memory range",
            json!({
                "provisionedMemory": 24576,
                "publicConnectivity": false,
                "deletionProtection": false,
            }),
            json!({"provisionedMemory": 24576, "publicConnectivity": false, "deletionProtection": false}),
        ),
    ];

    let id = create_available(
        &router,
        json!({"graphName": "patched-graph", "provisionedMemory": 16}),
    )
    .await;

    for (case, patch, expected) in cases {
        wait_until_available(&router, &id).await;
        let (status, updated) =
            call(&router, "PATCH", &format!("/graphs/{id}"), Some(patch.clone())).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {updated}");
        assert_eq!(
            updated["status"],
            json!("UPDATING"),
            "case {case:?}: a reconfigured graph is not back in service yet, got {}",
            updated["status"]
        );
        // The untouched members are still reported in full.
        assert_eq!(
            updated["name"],
            json!("patched-graph"),
            "case {case:?}: UpdateGraph must not rename the graph, got {}",
            updated["name"]
        );
        assert_eq!(
            updated["id"],
            json!(id.clone()),
            "case {case:?}: unexpected id {}",
            updated["id"]
        );

        for (member, want) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &updated[member], want,
                "case {case:?}: UpdateGraph member {member:?}: expected {want}, got {}",
                updated[member]
            );
            // ROUND TRIP: the change is persisted, not just echoed.
            let (_, fetched) = get(&router, &id).await;
            assert_eq!(
                &fetched[member], want,
                "case {case:?}: GetGraph member {member:?}: expected {want}, got {}",
                fetched[member]
            );
        }
    }

    // An unknown id is a 404, and a value outside the modelled range a 400.
    let (status, body) = call(
        &router,
        "PATCH",
        "/graphs/g-0000000000",
        Some(json!({"provisionedMemory": 32})),
    )
    .await;
    assert_eq!(status, 404, "case unknown id: expected 404, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case unknown id: unexpected error code in {body}"
    );

    for provisioned_memory in [15, 24577] {
        let (status, body) = call(
            &router,
            "PATCH",
            &format!("/graphs/{id}"),
            Some(json!({"provisionedMemory": provisioned_memory})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case provisionedMemory {provisioned_memory}: expected 400, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn delete_graph_removes_the_graph_unless_deletion_protection_holds_it() {
    let router = emulator();

    let id = create_available(
        &router,
        json!({
            "graphName": "deletable-graph",
            "provisionedMemory": 16,
            "deletionProtection": true,
        }),
    )
    .await;
    let keeper = create_available(
        &router,
        json!({"graphName": "surviving-graph", "provisionedMemory": 16}),
    )
    .await;

    // "Graphs cannot be deleted if delete-protection is enabled." The flag is a setting
    // of the addressed graph the caller clears, so the refusal is the 400 a permanent
    // input/configuration problem gets and it names the graph that stays.
    let (status, body) =
        call(&router, "DELETE", &format!("/graphs/{id}?skipSnapshot=true"), None).await;
    assert_eq!(status, 400, "case protected graph: expected 400, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case protected graph: unexpected error code in {body}"
    );
    assert_eq!(
        body["message"],
        json!(format!(
            "Graph [{id}] cannot be deleted because deletion protection is enabled."
        )),
        "case protected graph: unexpected message in {body}"
    );
    let (get_status, _) = get(&router, &id).await;
    assert_eq!(get_status, 200, "case protected graph: it must still exist, got {get_status}");

    // Clear the protection, then the delete is accepted and the record it reports is
    // the graph on its way out.
    call(
        &router,
        "PATCH",
        &format!("/graphs/{id}"),
        Some(json!({"deletionProtection": false})),
    )
    .await;
    wait_until_available(&router, &id).await;
    let (status, deleted) =
        call(&router, "DELETE", &format!("/graphs/{id}?skipSnapshot=true"), None).await;
    assert_eq!(status, 200, "case delete: expected 200, got {status}: {deleted}");
    assert_eq!(deleted["id"], json!(id.clone()), "case delete: unexpected id {}", deleted["id"]);
    assert_eq!(
        deleted["name"],
        json!("deletable-graph"),
        "case delete: unexpected name {}",
        deleted["name"]
    );
    assert_eq!(
        deleted["status"],
        json!("DELETING"),
        "case delete: expected a status reflecting the deletion, got {}",
        deleted["status"]
    );

    // It is on its way out, not gone: while the deletion the clock owes is in flight
    // GetGraph answers 200 DELETING, ListGraphs still shows it, its name is still
    // reserved and a second delete conflicts with the one already running.
    let (get_status, get_body) = get(&router, &id).await;
    assert_eq!(
        get_status, 200,
        "case deleting graph: expected 200, got {get_status}: {get_body}"
    );
    assert_eq!(
        get_body["status"],
        json!("DELETING"),
        "case deleting graph: expected DELETING, got {}",
        get_body["status"]
    );
    let (_, listed) = call(&router, "GET", "/graphs", None).await;
    let ids: Vec<&str> = listed["graphs"]
        .as_array()
        .expect("graphs is a list")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default())
        .collect();
    assert!(
        ids.contains(&id.as_str()) && ids.contains(&keeper.as_str()),
        "case deleting graph: ListGraphs must still show it, got {listed}"
    );
    let (reuse_status, reuse_body) = create(
        &router,
        json!({"graphName": "deletable-graph", "provisionedMemory": 16}),
    )
    .await;
    assert_eq!(
        reuse_status, 409,
        "case name of a deleting graph: expected 409, got {reuse_status}: {reuse_body}"
    );
    let (again_status, again_body) =
        call(&router, "DELETE", &format!("/graphs/{id}?skipSnapshot=true"), None).await;
    assert_eq!(
        again_status, 409,
        "case second delete while deleting: expected 409, got {again_status}: {again_body}"
    );
    assert_eq!(
        error_code(&again_body),
        "ConflictException",
        "case second delete while deleting: unexpected error code in {again_body}"
    );

    // Once the deletion falls due it is gone: GetGraph 404s, ListGraphs omits it, and a
    // delete of it 404s.
    wait_until_gone(&router, &id).await;
    let (get_status, get_body) = get(&router, &id).await;
    assert_eq!(get_status, 404, "case deleted graph: expected 404, got {get_status}: {get_body}");
    let (_, listed) = call(&router, "GET", "/graphs", None).await;
    let ids: Vec<&str> = listed["graphs"]
        .as_array()
        .expect("graphs is a list")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![keeper.as_str()],
        "case deleted graph: ListGraphs must show only the survivor, got {listed}"
    );
    let (again_status, again_body) =
        call(&router, "DELETE", &format!("/graphs/{id}?skipSnapshot=true"), None).await;
    assert_eq!(
        again_status, 404,
        "case second delete: expected 404, got {again_status}: {again_body}"
    );
    assert_eq!(
        error_code(&again_body),
        "ResourceNotFoundException",
        "case second delete: unexpected error code in {again_body}"
    );

    // The name the deleted graph held is free again.
    let (status, body) = create(
        &router,
        json!({"graphName": "deletable-graph", "provisionedMemory": 16}),
    )
    .await;
    assert_eq!(status, 201, "case recreate under the freed name: expected 201, got {status}: {body}");

    // A delete that asks for a FINAL snapshot is served the same way: 200 with the
    // graph reporting DELETING. What it mints and how that snapshot is named is covered
    // by `graph_deletion`.
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/graphs/{keeper}?skipSnapshot=false"),
        None,
    )
    .await;
    assert_eq!(status, 200, "case skipSnapshot=false: expected 200, got {status}: {body}");
    assert_eq!(
        body["status"],
        json!("DELETING"),
        "case skipSnapshot=false: expected DELETING, got {}",
        body["status"]
    );
}

#[tokio::test]
async fn a_graph_is_creating_until_the_provisioning_the_clock_owes_falls_due() {
    let router = emulator();
    let id = create_ok(&router, json!({"graphName": "transient-graph", "provisionedMemory": 16}))
        .await;

    // A caller acting straight after CreateGraph still sees the transient status...
    let (status, body) = get(&router, &id).await;
    assert_eq!(status, 200, "case immediate read: expected 200, got {status}: {body}");
    assert_eq!(
        body["status"],
        json!("CREATING"),
        "case immediate read: expected CREATING, got {}",
        body["status"]
    );
    let (_, listed) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(
        listed["graphs"][0]["status"],
        json!("CREATING"),
        "case immediate list: expected CREATING, got {}",
        listed["graphs"][0]["status"]
    );

    // ...and the state errors that go with it.
    for (case, method, path, body) in [
        (
            "update while creating",
            "PATCH",
            format!("/graphs/{id}"),
            Some(json!({"provisionedMemory": 32})),
        ),
        (
            "delete while creating",
            "DELETE",
            format!("/graphs/{id}?skipSnapshot=true"),
            None,
        ),
    ] {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status}: {response}");
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: unexpected error code in {response}"
        );
    }

    // The transition is owed by the clock alone: nothing but time moves it.
    tokio::time::sleep(PROVISIONING_SPAN + Duration::from_millis(50)).await;
    let (status, body) = get(&router, &id).await;
    assert_eq!(status, 200, "case settled read: expected 200, got {status}: {body}");
    assert_eq!(
        body["status"],
        json!("AVAILABLE"),
        "case settled read: expected AVAILABLE, got {}",
        body["status"]
    );
}

#[tokio::test]
async fn create_graph_stops_at_the_bound_on_how_far_the_store_may_grow() {
    let router = emulator();

    // The store is shared and grows by one graph per CreateGraph, so it has a
    // ceiling; the call that would pass it is refused with the modelled
    // ServiceQuotaExceededException (402) rather than growing memory further.
    for index in 0..MAX_GRAPHS {
        let (status, body) = create(
            &router,
            json!({"graphName": format!("bulk-graph-{index}"), "provisionedMemory": 16}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case graph {index} of {MAX_GRAPHS}: expected 201, got {status}: {body}"
        );
    }

    for case in ["the first call past the bound", "and every call after it"] {
        let (status, body) = create(
            &router,
            json!({"graphName": "one-graph-too-many", "provisionedMemory": 16}),
        )
        .await;
        assert_eq!(status, 402, "case {case:?}: expected 402, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["resourceType"],
            json!("Graph"),
            "case {case:?}: the refusal must name the resource type, got {body}"
        );
    }

    // Freeing a graph makes room for exactly one more.
    let (_, listed) = call(&router, "GET", "/graphs?maxResults=1", None).await;
    let victim = listed["graphs"][0]["id"]
        .as_str()
        .expect("a stored graph to delete")
        .to_string();
    wait_until_available(&router, &victim).await;
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/graphs/{victim}?skipSnapshot=true"),
        None,
    )
    .await;
    assert_eq!(status, 200, "case free a slot: expected 200, got {status}: {body}");
    // The slot is freed by the DELETION, which the clock owes — not by the call that
    // accepted it.
    tokio::time::sleep(DELETION_SPAN + Duration::from_millis(150)).await;
    let (status, body) = create(
        &router,
        json!({"graphName": "one-graph-too-many", "provisionedMemory": 16}),
    )
    .await;
    assert_eq!(status, 201, "case reuse the freed slot: expected 201, got {status}: {body}");
}
