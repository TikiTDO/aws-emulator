// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteGraph`, driven over the wire (`restJson1`) through the
//! same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A delete of a graph at rest is ACCEPTED, not performed: the call answers 200 with
//!   the graph reporting the modelled `DELETING` status, and the graph stays a resource
//!   the service holds — readable, listed, and holding the name it reserves — until the
//!   deletion the clock owes falls due, after which it is gone and its name is free.
//! * `skipSnapshot` decides whether a FINAL snapshot of the graph is taken first:
//!   `false` mints one (a `^gs-[a-z0-9]{10}$` id, the source graph's id and encryption
//!   key, `CREATING` settling to `AVAILABLE`) that outlives the graph, `true` mints
//!   nothing. Neither value changes the answer or the status the delete reports.
//! * The request carries no name for that snapshot, so the service names it after the
//!   graph, falling back to a name built from the snapshot's own id when that would
//!   break the modelled `SnapshotName` constraints or collide with a stored snapshot.
//!
//! So each table below spans what its rule generalises over: both values of
//! `skipSnapshot`, both statuses a graph rests in, and the graph-name lengths and
//! collisions the final snapshot's name has to be derived around — rather than
//! re-running one recorded request.
//!
//! `ListGraphSnapshots` is still a scaffold stub with its own issue, so the id of the
//! snapshot a delete minted is read out of the store (which is also what proves the
//! delete minted exactly one); every assertion about that snapshot then goes over the
//! wire through `GetGraphSnapshot`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ComAmazonawsNeptunegraphAmazonneptunegraphState as Store, DELETION_SPAN,
    FINAL_SNAPSHOT_SUFFIX, MAX_RESOURCE_NAME_CHARS, PROVISIONING_SPAN, SERVICE_OWNED_KMS_KEY,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// Room for the emulator's own scheduling on top of a span the clock owes, so a wait
/// for a due transition is not a race.
const SETTLE_MARGIN: Duration = Duration::from_millis(150);

/// An emulator holding only this service, with the engine it was built on, so a test
/// can read the store for the id of a resource no implemented list operation reports
/// yet.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
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

/// `GET /graphs/{id}`.
async fn get_graph(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
}

/// `GET /snapshots/{id}`.
async fn get_snapshot(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/snapshots/{id}"), None).await
}

/// `DELETE /graphs/{id}?skipSnapshot=<skip_snapshot>`.
async fn delete_graph(
    router: &MultiServiceRouter,
    id: &str,
    skip_snapshot: bool,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/graphs/{id}?skipSnapshot={skip_snapshot}"),
        None,
    )
    .await
}

/// The ids `ListGraphs` reports, in the order it pages them.
async fn listed_graph_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, listed) = call(router, "GET", "/graphs", None).await;
    assert_eq!(status, 200, "ListGraphs: expected 200, got {status}: {listed}");
    listed["graphs"]
        .as_array()
        .expect("graphs is a list")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
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

/// Poll one resource until it reports `expected`, then return its record. No handler
/// ever sleeps; only this test waits, because it is asking about a status the clock
/// reaches on its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// Create a graph (optionally encrypted) and wait it into service, returning its id.
async fn graph_in_service(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    kms_key: Option<&str>,
) -> String {
    let mut body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    if let Some(key) = kms_key {
        body["kmsKeyIdentifier"] = json!(key);
    }
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph of {body}, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
    id
}

/// The snapshot ids the store holds.
fn stored_snapshot_ids(engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    engine
        .state::<Store>()
        .lock()
        .snapshots
        .keys()
        .cloned()
        .collect()
}

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// One row of the delete table: the case name, the graph's name, the status the graph
/// rests in when it is deleted, the `skipSnapshot` the delete carries, and the KMS key
/// the graph was created with (`None` for an unencrypted graph).
type DeleteCase = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    Option<&'static str>,
);

#[tokio::test]
async fn an_accepted_delete_reports_deleting_and_takes_a_final_snapshot_when_asked() {
    let (router, engine) = emulator();

    // The rows differ along what the rule generalises over: both values of the required
    // `skipSnapshot`, both statuses a graph rests in (so the delete is accepted from
    // either), and whether the graph is encrypted (which the final snapshot inherits).
    let cases: &[DeleteCase] = &[
        ("serving graph, snapshot skipped", "serving-skip", "AVAILABLE", true, None),
        ("serving graph, final snapshot", "serving-snap", "AVAILABLE", false, None),
        ("stopped graph, snapshot skipped", "stopped-skip", "STOPPED", true, None),
        ("stopped graph, final snapshot", "stopped-snap", "STOPPED", false, None),
        (
            "encrypted graph, final snapshot",
            "encrypted-snap",
            "AVAILABLE",
            false,
            Some(KMS_KEY_ARN),
        ),
    ];

    for (case, graph_name, resting, skip_snapshot, kms_key) in cases {
        let id = graph_in_service(&router, case, graph_name, *kms_key).await;
        if *resting == "STOPPED" {
            let (status, stopped) =
                call(&router, "POST", &format!("/graphs/{id}/stop"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from StopGraph, got {status}: {stopped}"
            );
            wait_for_status(&router, case, &format!("/graphs/{id}"), "STOPPED").await;
        }
        let before: Vec<String> = stored_snapshot_ids(&engine);

        // The delete is accepted whichever value `skipSnapshot` carries, and reports the
        // graph on its way out.
        let (status, deleted) = delete_graph(&router, &id, *skip_snapshot).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {deleted}");
        assert_eq!(
            deleted["status"],
            json!("DELETING"),
            "case {case:?}: expected DELETING, got {}",
            deleted["status"]
        );
        assert_eq!(
            deleted["id"],
            json!(id.clone()),
            "case {case:?}: expected the deleted graph's id, got {}",
            deleted["id"]
        );
        assert_eq!(
            deleted["name"],
            json!(*graph_name),
            "case {case:?}: expected the deleted graph's name, got {}",
            deleted["name"]
        );

        // A final snapshot is minted exactly when the request asked for one.
        let after: Vec<String> = stored_snapshot_ids(&engine);
        let minted: Vec<String> = after
            .iter()
            .filter(|snapshot| !before.contains(snapshot))
            .cloned()
            .collect();
        let expected_mints = usize::from(!*skip_snapshot);
        assert_eq!(
            minted.len(),
            expected_mints,
            "case {case:?}: expected {expected_mints} snapshot(s) minted, got {minted:?}"
        );

        // While the deletion is in flight the graph is still a resource this service
        // holds: it reads back as DELETING, it is still listed, its name is still
        // reserved, and a second delete conflicts with the one already running.
        let (get_status, in_flight) = get_graph(&router, &id).await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: a deleting graph must still be readable, got {get_status}: {in_flight}"
        );
        assert_eq!(
            in_flight["status"],
            json!("DELETING"),
            "case {case:?}: expected DELETING from GetGraph, got {}",
            in_flight["status"]
        );
        assert!(
            listed_graph_ids(&router).await.contains(&id),
            "case {case:?}: a deleting graph must still be listed"
        );
        let (reuse_status, reuse) = call(
            &router,
            "POST",
            "/graphs",
            Some(json!({"graphName": *graph_name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
        )
        .await;
        assert_eq!(
            reuse_status, 409,
            "case {case:?}: the name must stay reserved while the graph is DELETING, \
             got {reuse_status}: {reuse}"
        );
        assert_eq!(
            error_code(&reuse),
            "ConflictException",
            "case {case:?}: unexpected error code for the reserved name in {reuse}"
        );
        let (again_status, again) = delete_graph(&router, &id, true).await;
        assert_eq!(
            again_status, 409,
            "case {case:?}: a second delete of a deleting graph must conflict, \
             got {again_status}: {again}"
        );

        // Once the deletion the clock owes falls due the graph is gone, its name is free
        // again, and a delete of it is a 404.
        tokio::time::sleep(DELETION_SPAN + SETTLE_MARGIN).await;
        let (gone_status, gone) = get_graph(&router, &id).await;
        assert_eq!(
            gone_status, 404,
            "case {case:?}: expected 404 once the deletion is due, got {gone_status}: {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {gone}"
        );
        assert!(
            !listed_graph_ids(&router).await.contains(&id),
            "case {case:?}: a deleted graph must not be listed"
        );
        let (gone_delete_status, gone_delete) = delete_graph(&router, &id, true).await;
        assert_eq!(
            gone_delete_status, 404,
            "case {case:?}: expected 404 from a delete of a graph that is gone, \
             got {gone_delete_status}: {gone_delete}"
        );
        let (freed_status, freed) = call(
            &router,
            "POST",
            "/graphs",
            Some(json!({"graphName": *graph_name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
        )
        .await;
        assert_eq!(
            freed_status, 201,
            "case {case:?}: the name must be free once the deletion is done, \
             got {freed_status}: {freed}"
        );

        // The final snapshot outlives the graph it was taken of, and reads back through
        // `GetGraphSnapshot` as a snapshot of that graph, carrying the graph's own key.
        if let Some(snapshot_id) = minted.first() {
            assert!(
                is_snapshot_id(snapshot_id),
                "case {case:?}: expected a minted id matching ^gs-[a-z0-9]{{10}}$, \
                 got {snapshot_id:?}"
            );
            let (snapshot_status, snapshot) = get_snapshot(&router, snapshot_id).await;
            assert_eq!(
                snapshot_status, 200,
                "case {case:?}: the final snapshot must outlive the graph, \
                 got {snapshot_status}: {snapshot}"
            );
            assert_eq!(
                snapshot["sourceGraphId"],
                json!(id.clone()),
                "case {case:?}: expected the deleted graph as the source, got {}",
                snapshot["sourceGraphId"]
            );
            assert_eq!(
                snapshot["name"],
                json!(format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}")),
                "case {case:?}: expected the final snapshot named after the graph, got {}",
                snapshot["name"]
            );
            // The final snapshot carries the key the graph's data was encrypted with,
            // which is the service-owned key's sentinel when the graph was created
            // without a customer managed key.
            assert_eq!(
                snapshot["kmsKeyIdentifier"],
                kms_key.map_or_else(|| json!(SERVICE_OWNED_KMS_KEY), |key| json!(key)),
                "case {case:?}: expected the source graph's key on the snapshot, got {}",
                snapshot["kmsKeyIdentifier"]
            );
            // It was minted CREATING and the clock settles it to AVAILABLE, like any
            // other snapshot of this service.
            let settled =
                wait_for_status(&router, case, &format!("/snapshots/{snapshot_id}"), "AVAILABLE")
                    .await;
            assert_eq!(
                settled["id"],
                json!(snapshot_id.clone()),
                "case {case:?}: expected the snapshot's own id in {settled}"
            );
        }
    }
}

#[tokio::test]
async fn a_final_snapshot_is_named_after_the_graph_or_after_its_own_minted_id() {
    let (router, engine) = emulator();

    // A graph name of the greatest length the model allows (`GraphName` is
    // `@length(1..63)`), so the name derived from it would not fit `SnapshotName`'s
    // identical bound.
    let longest_graph_name = "a".repeat(MAX_RESOURCE_NAME_CHARS);

    // (case, the graph's name, a snapshotName to park on the account first, whether the
    // final snapshot may be named after the graph). The rows differ along what the rule
    // generalises over: the length of the name it is derived from, and whether that
    // derived name is free.
    let cases: &[(&str, &str, Option<&str>, bool)] = &[
        ("a short, free name", "short-name-graph", None, true),
        (
            "a graph name that leaves no room for the suffix",
            longest_graph_name.as_str(),
            None,
            false,
        ),
        (
            "a derived name a stored snapshot already holds",
            "taken-name-graph",
            Some("taken-name-graph-final-snapshot"),
            false,
        ),
    ];

    // The graph the parked snapshots are taken of, so a parked name belongs to a real
    // snapshot of the account rather than being written into the store.
    let donor = graph_in_service(&router, "donor", "snapshot-name-donor", None).await;

    for (case, graph_name, parked, after_the_graph) in cases {
        if let Some(parked_name) = parked {
            let (status, created) = call(
                &router,
                "POST",
                "/snapshots",
                Some(json!({"graphIdentifier": donor, "snapshotName": parked_name})),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateGraphSnapshot of {parked_name}, \
                 got {status}: {created}"
            );
            wait_for_status(&router, case, &format!("/graphs/{donor}"), "AVAILABLE").await;
        }

        let id = graph_in_service(&router, case, graph_name, None).await;
        let before = stored_snapshot_ids(&engine);
        let (status, deleted) = delete_graph(&router, &id, false).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {deleted}");
        let after = stored_snapshot_ids(&engine);
        let minted: Vec<String> = after
            .iter()
            .filter(|snapshot| !before.contains(snapshot))
            .cloned()
            .collect();
        assert_eq!(
            minted.len(),
            1,
            "case {case:?}: expected exactly one minted snapshot, got {minted:?}"
        );
        let snapshot_id = &minted[0];

        let (snapshot_status, snapshot) = get_snapshot(&router, snapshot_id).await;
        assert_eq!(
            snapshot_status, 200,
            "case {case:?}: expected 200 from GetGraphSnapshot, got {snapshot_status}: {snapshot}"
        );
        let name = snapshot["name"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            name == format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}"),
            *after_the_graph,
            "case {case:?}: expected named-after-the-graph={after_the_graph}, got {name:?}"
        );
        assert!(
            !name.is_empty() && name.len() <= MAX_RESOURCE_NAME_CHARS,
            "case {case:?}: expected a name within 1..{MAX_RESOURCE_NAME_CHARS} characters, \
             got {name:?}"
        );
        assert_ne!(
            Some(name.as_str()),
            *parked,
            "case {case:?}: the minted name must not be one the account already holds, \
             got {name:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_delete_takes_no_snapshot_and_leaves_the_graph_alone() {
    let (router, engine) = emulator();

    let protected = graph_in_service(&router, "protected", "protected-graph", None).await;
    let (patched_status, patched) = call(
        &router,
        "PATCH",
        &format!("/graphs/{protected}"),
        Some(json!({"deletionProtection": true})),
    )
    .await;
    assert_eq!(
        patched_status, 200,
        "case protected: expected 200 from UpdateGraph, got {patched_status}: {patched}"
    );
    wait_for_status(&router, "protected", &format!("/graphs/{protected}"), "AVAILABLE").await;
    let creating = {
        let (status, created) = call(
            &router,
            "POST",
            "/graphs",
            Some(json!({
                "graphName": "still-creating-graph",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
            })),
        )
        .await;
        assert_eq!(status, 201, "case still creating: expected 201, got {status}: {created}");
        created["id"].as_str().unwrap_or_default().to_string()
    };

    // (case, the graph the delete names, the status it must be refused with, the error
    // code it must be refused as). The rows differ along why the delete is refused.
    let cases: &[(&str, &str, u16, &str)] = &[
        (
            "deletion protection holds the graph",
            protected.as_str(),
            400,
            "ValidationException",
        ),
        (
            "a transition is still in flight",
            creating.as_str(),
            409,
            "ConflictException",
        ),
        (
            "an identifier no graph holds",
            "g-0000000000",
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, identifier, expected_status, expected_code) in cases {
        let before = stored_snapshot_ids(&engine);
        // A refused delete that asked for a final snapshot must not leave one behind:
        // nothing was deleted, so nothing was snapshotted.
        let (status, body) = delete_graph(&router, identifier, false).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            stored_snapshot_ids(&engine),
            before,
            "case {case:?}: a refused delete must mint no snapshot"
        );
    }

    // And the graphs the refusals named are untouched: still readable, still listed.
    for (case, id, expected_status) in [
        ("deletion protection holds the graph", protected.as_str(), "AVAILABLE"),
        ("a transition is still in flight", creating.as_str(), "AVAILABLE"),
    ] {
        let settled = wait_for_status(&router, case, &format!("/graphs/{id}"), expected_status).await;
        assert_eq!(
            settled["id"],
            json!(id),
            "case {case:?}: expected the graph to survive the refused delete, got {settled}"
        );
    }
}
