// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the snapshot resource: `CreateGraphSnapshot` and the
//! `GetGraphSnapshot` read it round-trips through, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * `CreateGraphSnapshot` mints a snapshot OF a stored graph, persists it, and
//!   reports the stored record — a minted `^gs-[a-z0-9]{10}$` id, the ARN of a
//!   `graph-snapshot` resource in the request's region and account, the name the
//!   request chose, the source graph's id and encryption key, and a `CREATING`
//!   status the clock settles to `AVAILABLE`.
//! * A snapshot name is unique within the account, so a second snapshot by a stored
//!   name is refused `ConflictException` (409) whichever graph it is taken from.
//! * A graph whose transition is still in flight cannot be snapshotted
//!   (`ValidationException`, 400, naming the graph), and
//!   taking a snapshot takes the source graph through the modelled `SNAPSHOTTING`
//!   status (a graph AT REST, serving or stopped, is snapshottable — covered by
//!   `snapshot_of_a_graph_at_rest`).
//! * A name outside the modelled `SnapshotName` constraints, or an identifier naming
//!   no stored resource, is refused by its modelled error rather than served.
//!
//! So each table below spans what its rule generalises over — the name's form and
//! length at the published bound and one past it, the tag entry count, whether the
//! source graph is encrypted, and which graph a conflicting name was taken from —
//! rather than re-running one recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ARN_NAMESPACE, PROVISIONING_SPAN, SERVICE_OWNED_KMS_KEY,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs the service mints
/// are predictable.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The longest `snapshotName` the model allows: `SnapshotName` is `@length(1..63)`
/// (`smithy_query(query="operation",operation="CreateGraphSnapshot")`).
const MAX_SNAPSHOT_NAME_CHARS: usize = 63;

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

/// Percent-encode one path segment.
///
/// `resourceArn` is bound to a NON-greedy `{resourceArn}` label
/// (`/tags/{resourceArn}`), so the `/` inside an ARN has to arrive encoded or it would
/// split the URI into another path segment. Only the RFC 3986 unreserved set is left
/// as-is.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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

/// Whether `value` matches the modelled `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
fn is_snapshot_id(value: &str) -> bool {
    value.strip_prefix("gs-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// One row of the create-and-read-back table: the case name, the `snapshotName` the
/// request carries, the tags it carries, and the KMS key the SOURCE GRAPH was created
/// with (`None` for an unencrypted graph).
type SnapshotCase = (
    &'static str,
    String,
    BTreeMap<String, String>,
    Option<&'static str>,
);

/// `GET /graphs/{id}`.
async fn get_graph(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
}

/// `GET /snapshots/{id}`.
async fn get_snapshot(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/snapshots/{id}"), None).await
}

/// `POST /snapshots` with `body`.
async fn create_snapshot(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/snapshots", Some(body)).await
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph (optionally encrypted) and wait it into service, returning its id.
async fn graph_in_service(
    router: &MultiServiceRouter,
    name: &str,
    kms_key: Option<&str>,
) -> String {
    let mut body = json!({
        "graphName": name,
        "provisionedMemory": MIN_PROVISIONED_MEMORY,
    });
    if let Some(key) = kms_key {
        body["kmsKeyIdentifier"] = json!(key);
    }
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    id
}

#[tokio::test]
async fn create_graph_snapshot_persists_the_record_and_reads_it_back() {
    let router = emulator();

    // (case, snapshotName, tags, the source graph's KMS key)
    let long_name = format!("s{}", "x".repeat(MAX_SNAPSHOT_NAME_CHARS - 1));
    let cases: Vec<SnapshotCase> = vec![
        (
            "no tags, an unencrypted graph",
            "my-snapshot-1".to_string(),
            BTreeMap::new(),
            None,
        ),
        (
            "a single-character name, one tag",
            "a".to_string(),
            [("env".to_string(), "dev".to_string())]
                .into_iter()
                .collect(),
            None,
        ),
        (
            "digits in the name, several tags, an encrypted graph",
            "snapshot9".to_string(),
            [
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "graph".to_string()),
                ("cost-centre".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
            Some(KMS_KEY_ARN),
        ),
        (
            "a name at the published length bound",
            long_name,
            BTreeMap::new(),
            Some(KMS_KEY_ARN),
        ),
    ];

    for (index, (case, snapshot_name, tags, kms_key)) in cases.into_iter().enumerate() {
        let graph_id = graph_in_service(&router, &format!("source-graph-{index}"), kms_key).await;
        let (status, created) = create_snapshot(
            &router,
            json!({
                "graphIdentifier": graph_id,
                "snapshotName": snapshot_name,
                "tags": tags,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );

        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
            .to_string();
        assert!(
            is_snapshot_id(&id),
            "case {case:?}: expected an id matching ^gs-[a-z0-9]{{10}}$, got {id:?}"
        );
        let expected_arn =
            engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("graph-snapshot/{id}"));
        assert_eq!(
            created["arn"],
            json!(expected_arn),
            "case {case:?}: expected arn {expected_arn:?}, got {}",
            created["arn"]
        );
        assert_eq!(
            created["name"],
            json!(snapshot_name),
            "case {case:?}: expected name {snapshot_name:?}, got {}",
            created["name"]
        );
        assert_eq!(
            created["sourceGraphId"],
            json!(graph_id),
            "case {case:?}: expected sourceGraphId {graph_id:?}, got {}",
            created["sourceGraphId"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: a snapshot is not restorable when the call returns, \
             expected CREATING, got {}",
            created["status"]
        );
        assert!(
            created["snapshotCreateTime"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: expected a snapshotCreateTime, got {}",
            created["snapshotCreateTime"]
        );
        // A snapshot carries the key its source graph's data is encrypted with, and
        // every graph is encrypted: a graph created without a customer managed key is
        // encrypted with the service-owned key its sentinel names.
        let expected_key = kms_key.map_or_else(|| json!(SERVICE_OWNED_KMS_KEY), |key| json!(key));
        assert_eq!(
            created["kmsKeyIdentifier"],
            expected_key,
            "case {case:?}: a snapshot carries its source graph's key, expected \
             {expected_key}, got {}",
            created["kmsKeyIdentifier"]
        );

        // The round trip: an independent read reports the record that was stored, not
        // the response that was rendered.
        let (read_status, read) = get_snapshot(&router, &id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetGraphSnapshot, got {read_status}: {read}"
        );
        for member in ["id", "arn", "name", "sourceGraphId", "snapshotCreateTime", "kmsKeyIdentifier"] {
            assert_eq!(
                read[member], created[member],
                "case {case:?}: GetGraphSnapshot's {member} must be the stored value \
                 {}, got {}",
                created[member], read[member]
            );
        }

        // The tags supplied at create time are stored ON the snapshot, so the tagging
        // surface reads them back through the snapshot's own ARN.
        let (tags_status, tags_body) =
            call(&router, "GET", &format!("/tags/{}", encode(&expected_arn)), None).await;
        assert_eq!(
            tags_status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {tags_status}: \
             {tags_body}"
        );
        assert_eq!(
            tags_body["tags"],
            json!(tags),
            "case {case:?}: expected the supplied tags {}, got {}",
            json!(tags),
            tags_body["tags"]
        );

        // The clock owes the move to AVAILABLE; nothing else about the record changes.
        let settled = wait_until_available(&router, &format!("/snapshots/{id}")).await;
        for member in ["id", "arn", "name", "sourceGraphId", "snapshotCreateTime"] {
            assert_eq!(
                settled[member], created[member],
                "case {case:?}: settling must not change {member}, expected {}, got {}",
                created[member], settled[member]
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_name_already_in_the_account_conflicts() {
    let router = emulator();
    let first_graph = graph_in_service(&router, "conflict-source-a", None).await;
    let second_graph = graph_in_service(&router, "conflict-source-b", None).await;

    let (status, created) = create_snapshot(
        &router,
        json!({"graphIdentifier": first_graph, "snapshotName": "taken-name"}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the first snapshot: expected 201, got {status}: {created}"
    );

    // (case, the graph the next snapshot is taken from, its name, the expected status)
    let cases: Vec<(&str, &str, &str, u16)> = vec![
        ("the same name on the same graph", &first_graph, "taken-name", 409),
        ("the same name on another graph", &second_graph, "taken-name", 409),
        ("a free name on the same graph", &first_graph, "free-name", 201),
        ("a free name on another graph", &second_graph, "another-free-name", 201),
    ];
    for (case, graph_id, snapshot_name, expected) in cases {
        // Each row starts from a serving graph, so the row is about the NAME and not
        // about a transition an earlier row left outstanding.
        wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
        let (status, body) = create_snapshot(
            &router,
            json!({"graphIdentifier": graph_id, "snapshotName": snapshot_name}),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if expected == 409 {
            let code = error_code(&body);
            assert_eq!(
                code, "ConflictException",
                "case {case:?}: expected ConflictException, got {code:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_name_outside_the_modelled_constraints_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "validation-source", None).await;

    // One row per clause of the modelled `SnapshotName` constraints
    // (`@length(1..63)`, `@pattern("^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$")`), plus the
    // length bound itself and one character past it.
    let at_bound = format!("s{}", "x".repeat(MAX_SNAPSHOT_NAME_CHARS - 1));
    let past_bound = format!("s{}", "x".repeat(MAX_SNAPSHOT_NAME_CHARS));
    let cases: Vec<(&str, String, bool)> = vec![
        ("at the length bound", at_bound, true),
        ("one character past the length bound", past_bound, false),
        ("empty", String::new(), false),
        ("the reserved snapshot-id prefix", "gs-abcdefghij".to_string(), false),
        ("the graph-id prefix is not reserved here", "g-abcdefghij".to_string(), true),
        ("a leading digit", "1snapshot".to_string(), false),
        ("a leading hyphen", "-snapshot".to_string(), false),
        ("an uppercase letter", "My-Snapshot".to_string(), false),
        ("an underscore", "my_snapshot".to_string(), false),
        ("two consecutive hyphens", "my--snapshot".to_string(), false),
        ("a trailing hyphen", "my-snapshot-".to_string(), false),
    ];
    for (case, snapshot_name, accepted) in cases {
        let (status, body) = create_snapshot(
            &router,
            json!({"graphIdentifier": graph_id, "snapshotName": snapshot_name}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?} ({snapshot_name:?}): expected 201, got {status}: {body}"
            );
            // Keep the graph serving for the next row.
            wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?} ({snapshot_name:?}): expected 400, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?} ({snapshot_name:?}): expected ValidationException, got \
             {code:?}: {body}"
        );
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_stored_resource_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "identifier-source", None).await;
    let (status, created) = create_snapshot(
        &router,
        json!({"graphIdentifier": graph_id, "snapshotName": "identifier-snapshot"}),
    )
    .await;
    assert_eq!(status, 201, "the fixture snapshot: got {status}: {created}");
    let stored_snapshot = created["id"].as_str().expect("an id").to_string();

    // (case, the graphIdentifier CreateGraphSnapshot is given, status, error code)
    let create_cases: Vec<(&str, &str, u16, &str)> = vec![
        ("a stored graph", &graph_id, 201, ""),
        (
            "a well-formed graph id that was never created",
            "g-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        (
            "another well-formed absent graph id",
            "g-0000000000",
            404,
            "ResourceNotFoundException",
        ),
        ("an identifier outside the modelled pattern", "not-a-graph-id", 400, "ValidationException"),
        ("a snapshot id where a graph id belongs", "gs-abcdefghij", 400, "ValidationException"),
    ];
    for (index, (case, graph_identifier, expected, expected_code)) in
        create_cases.into_iter().enumerate()
    {
        wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
        let (status, body) = create_snapshot(
            &router,
            json!({
                "graphIdentifier": graph_identifier,
                "snapshotName": format!("identifier-case-{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} ({graph_identifier:?}): expected {expected}, got {status}: {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({graph_identifier:?}): expected {expected_code}, got \
                 {code:?}: {body}"
            );
        }
    }

    // (case, the snapshotIdentifier GetGraphSnapshot is given, status, error code)
    let read_cases: Vec<(&str, &str, u16, &str)> = vec![
        ("the stored snapshot", &stored_snapshot, 200, ""),
        (
            "a well-formed snapshot id that was never created",
            "gs-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        ("a graph id where a snapshot id belongs", "g-abcdefghij", 400, "ValidationException"),
        ("an identifier outside the modelled pattern", "nope", 400, "ValidationException"),
    ];
    for (case, snapshot_identifier, expected, expected_code) in read_cases {
        let (status, body) = get_snapshot(&router, snapshot_identifier).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({snapshot_identifier:?}): expected {expected}, got {status}: \
             {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({snapshot_identifier:?}): expected {expected_code}, got \
                 {code:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn taking_a_snapshot_takes_the_source_graph_through_snapshotting() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "snapshotting-source", None).await;

    let (status, created) = create_snapshot(
        &router,
        json!({"graphIdentifier": graph_id, "snapshotName": "transient-snapshot"}),
    )
    .await;
    assert_eq!(status, 201, "the snapshot call: got {status}: {created}");
    let snapshot_id = created["id"].as_str().expect("an id").to_string();

    // The transition is owed at a future instant, so a caller acting straight after
    // the call still observes both transient statuses.
    let (graph_status, graph) = get_graph(&router, &graph_id).await;
    assert_eq!(graph_status, 200, "the source graph read: got {graph_status}: {graph}");
    assert_eq!(
        graph["status"],
        json!("SNAPSHOTTING"),
        "case the source graph straight after the call: expected SNAPSHOTTING, got {}",
        graph["status"]
    );
    let (read_status, snapshot) = get_snapshot(&router, &snapshot_id).await;
    assert_eq!(read_status, 200, "the snapshot read: got {read_status}: {snapshot}");
    assert_eq!(
        snapshot["status"],
        json!("CREATING"),
        "case the snapshot straight after the call: expected CREATING, got {}",
        snapshot["status"]
    );

    // Both settle on their own, without any further call having to ask for it.
    wait_until_available(&router, &format!("/snapshots/{snapshot_id}")).await;
    wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
}

#[tokio::test]
async fn a_graph_whose_transition_is_in_flight_cannot_be_snapshotted() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/graphs",
        Some(json!({
            "graphName": "still-provisioning",
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
        })),
    )
    .await;
    assert_eq!(status, 201, "the graph call: got {status}: {created}");
    let graph_id = created["id"].as_str().expect("an id").to_string();
    assert_eq!(
        created["status"],
        json!("CREATING"),
        "case a freshly created graph: expected CREATING, got {}",
        created["status"]
    );

    // The refusal is taken while the provisioning transition is still outstanding.
    let (refused_status, refused) = create_snapshot(
        &router,
        json!({"graphIdentifier": graph_id, "snapshotName": "too-early"}),
    )
    .await;
    assert_eq!(
        refused_status, 400,
        "case a graph still provisioning: expected 400, got {refused_status}: {refused}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "ValidationException",
        "case a graph still provisioning: expected ValidationException, got {code:?}: \
         {refused}"
    );
    // Once the graph is serving, the SAME call succeeds and the name it asked for is
    // still free — so the refusal persisted nothing, not even the name index entry,
    // and it was about the graph's status and nothing else.
    wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
    let (status, accepted) = create_snapshot(
        &router,
        json!({"graphIdentifier": graph_id, "snapshotName": "too-early"}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the same call once the graph is serving: expected 201, got {status}: \
         {accepted}"
    );
    let id = accepted["id"].as_str().expect("an id").to_string();
    let (read_status, read) = get_snapshot(&router, &id).await;
    assert_eq!(
        read_status, 200,
        "case the accepted snapshot: expected 200 from GetGraphSnapshot, got \
         {read_status}: {read}"
    );
    assert_eq!(
        read["name"],
        json!("too-early"),
        "case the accepted snapshot: expected the requested name, got {}",
        read["name"]
    );
}
