// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REGIONAL scoping of this service's control plane, driven
//! over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: neptune-graph is a regional control plane,
//! so a graph (and a graph snapshot) exists in exactly one Region — the Region the
//! request that created it was signed for — and every later lookup, listing,
//! mutation, tagging call and name-uniqueness check answers only within that Region;
//! from any other Region the resource does not exist (404
//! `ResourceNotFoundException`) and its name is free.
//!
//! The tables below span what that rule generalises over: which Region creates a
//! resource, which Region reads it, how many resources a Region holds, which
//! operation does the reaching (read, list, update, run-state, delete, tag, query,
//! import, export), and whether the resource is a graph, a snapshot, a query or a
//! task. One Region on its own is a single row of that space, which is why the
//! earlier suites — all driving one signing Region — could not see this.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ComAmazonawsNeptunegraphAmazonneptunegraphState as Store, Query, ARN_NAMESPACE,
    PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Regions these tests sign for. Four rows so a Region can be compared against
/// more than one other, differing in form (two hyphen groups, three, a digit-suffixed
/// geography) and including one that holds no resource at all.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2", "ca-central-1"];

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`, which
/// `StartExportTask` requires.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneExportRole";

/// An emulator holding only this service, with the engine it was built on, so each
/// test starts from an empty store. ONE store serves every Region, which is exactly
/// what makes the Region a predicate rather than a label.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// [`emulator_with_engine`] for a test that drives everything over the wire.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// Dispatch one `restJson1` request signed for `region` and return its status and
/// parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/neptune-graph/\
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

/// Dispatch one query-operation request signed for `region`. `GetQuery` and
/// `CancelQuery` bind `graphIdentifier` to a HEADER rather than to the path
/// (`smithy_query(query="operation",operation="CancelQuery,GetQuery")`).
async fn query_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    graph_identifier: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("graphIdentifier", graph_identifier)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::new())
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

/// Percent-encode one path segment, so the `/` inside an ARN does not split the
/// non-greedy `{resourceArn}` label of `/tags/{resourceArn}`. Only the RFC 3986
/// unreserved set is left as-is.
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

/// `POST /graphs` signed for `region`.
async fn create_graph(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, Value) {
    let body = json!({ "graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY });
    call(router, region, "POST", "/graphs", Some(body)).await
}

/// `GET /graphs/{id}` signed for `region`.
async fn get_graph(router: &MultiServiceRouter, region: &str, id: &str) -> (u16, Value) {
    call(router, region, "GET", &format!("/graphs/{id}"), None).await
}

/// `GET /graphs` signed for `region`, returning the listed graph ids in order.
async fn list_graph_ids(router: &MultiServiceRouter, region: &str, path: &str) -> Vec<String> {
    let (status, listed) = call(router, region, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case listing {path:?} in {region}: expected 200, got {status}: {listed}"
    );
    listed["graphs"]
        .as_array()
        .unwrap_or_else(|| panic!("case listing {path:?} in {region}: no graphs in {listed}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case listing {path:?} in {region}: no id in {summary}"))
                .to_string()
        })
        .collect()
}

/// Create a graph in `region` and return its id and ARN, failing the test unless the
/// call answered 201.
async fn create_graph_ok(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (String, String) {
    let (status, created) = create_graph(router, region, name).await;
    assert_eq!(
        status, 201,
        "case {name:?} in {region}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?} in {region}: no id in {created}"))
        .to_string();
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?} in {region}: no arn in {created}"))
        .to_string();
    (id, arn)
}

/// Wait out the provisioning transition the clock owes a graph, then leave it
/// `AVAILABLE`. No handler ever sleeps; only this test does, because it asks about a
/// state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, region: &str, id: &str) {
    for _ in 0..20 {
        let (status, body) = get_graph(router, region, id).await;
        assert_eq!(
            status, 200,
            "case {id} in {region}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {id} in {region}: the graph never became AVAILABLE");
}

/// A graph created in `region` and waited into service, with its id and ARN.
async fn available_graph(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (String, String) {
    let (id, arn) = create_graph_ok(router, region, name).await;
    wait_until_available(router, region, &id).await;
    (id, arn)
}

/// Wait until the graph `id` is no longer readable in `region`: an accepted deletion
/// keeps the record (and its name) until the deletion the clock owes falls due. No
/// handler sleeps for it; only this test does.
async fn wait_until_gone(router: &MultiServiceRouter, region: &str, id: &str) {
    for _ in 0..40 {
        let (status, _) = get_graph(router, region, id).await;
        if status == 404 {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {id} in {region}: the deleted graph was never reclaimed");
}

/// A graph exists in exactly one Region: every operation that resolves one by id
/// answers it in the Region that holds it and 404 `ResourceNotFoundException`
/// everywhere else — whatever the operation, and whichever other Region asks.
#[tokio::test]
async fn a_graph_resolves_only_in_the_region_that_holds_it() {
    let router = emulator();

    // One graph per Region, all sharing one name (see the name-uniqueness test), so
    // the only thing distinguishing the rows is the Region that created them.
    let mut placed: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let (id, _) = available_graph(&router, region, "shared").await;
        placed.push((region, id));
    }

    // (case, HTTP method, the path template the id goes into) — every implemented
    // operation that resolves a graph by `graphIdentifier`. `DELETE` comes last so the
    // earlier rows still have a graph to reach.
    let reaching: &[(&str, &str, &str)] = &[
        ("GetGraph", "GET", "/graphs/{id}"),
        ("StopGraph", "POST", "/graphs/{id}/stop"),
        ("StartGraph", "POST", "/graphs/{id}/start"),
        ("DeleteGraph", "DELETE", "/graphs/{id}?skipSnapshot=true"),
    ];

    for (case, method, template) in reaching {
        for (owner, id) in &placed {
            let path = template.replace("{id}", id);
            for caller in REGIONS.iter().filter(|caller| *caller != owner) {
                let (status, body) = call(&router, caller, method, &path, None).await;
                assert_eq!(
                    status, 404,
                    "case {case} on a {owner} graph from {caller}: expected 404, \
                     got {status}: {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case} on a {owner} graph from {caller}: expected \
                     ResourceNotFoundException, got {code:?}: {body}"
                );
            }
        }
    }

    // `UpdateGraph` carries a body, so it gets its own row rather than a `None` one.
    for (owner, id) in &placed {
        for caller in REGIONS.iter().filter(|caller| *caller != owner) {
            let (status, body) = call(
                &router,
                caller,
                "PATCH",
                &format!("/graphs/{id}"),
                Some(json!({ "deletionProtection": true })),
            )
            .await;
            assert_eq!(
                status, 404,
                "case UpdateGraph on a {owner} graph from {caller}: expected 404, \
                 got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case UpdateGraph on a {owner} graph from {caller}: expected \
                 ResourceNotFoundException, got {code:?}: {body}"
            );
        }
    }

    // Every graph survived the calls from the other Regions, and still answers in its
    // own Region with the ARN and endpoint of that Region.
    for (owner, id) in &placed {
        let (status, body) = get_graph(&router, owner, id).await;
        assert_eq!(
            status, 200,
            "case {id} in its own Region {owner}: expected 200, got {status}: {body}"
        );
        let arn = body["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            arn.contains(&format!(":{owner}:")),
            "case {id} in {owner}: expected an ARN naming the Region, got {arn:?}"
        );
        let endpoint = body["endpoint"].as_str().unwrap_or_default().to_string();
        assert!(
            endpoint.contains(&format!(".{owner}.")),
            "case {id} in {owner}: expected an endpoint naming the Region, got {endpoint:?}"
        );
    }
}

/// `ListGraphs` reports the graphs of the Region it was signed for and no others — at
/// every population size, and through pagination.
#[tokio::test]
async fn list_graphs_reports_only_the_signing_regions_graphs() {
    let router = emulator();

    // (Region, how many graphs it holds) — a Region with none, one, and several, so
    // the listing is asserted over a population that differs along count as well as
    // along Region.
    let population: Vec<(&str, usize)> = vec![
        ("us-east-1", 1),
        ("eu-west-1", 3),
        ("ap-southeast-2", 2),
        ("ca-central-1", 0),
    ];

    let mut expected: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (region, count) in &population {
        let mut ids = Vec::new();
        for index in 0..*count {
            let (id, _) = create_graph_ok(&router, region, &format!("graph-{index}")).await;
            ids.push(id);
        }
        // `graphs` is keyed by the minted id, and the listing is in id order.
        ids.sort();
        expected.insert(region, ids);
    }

    for (region, count) in &population {
        let listed = list_graph_ids(&router, region, "/graphs").await;
        let want = &expected[region];
        assert_eq!(
            &listed, want,
            "case ListGraphs in {region} ({count} graphs there): expected {want:?}, \
             got {listed:?}"
        );
    }

    // Pagination stays inside the Region too: one page per graph, following the token,
    // yields exactly that Region's graphs and then stops.
    for (region, _) in &population {
        let mut seen: Vec<String> = Vec::new();
        let mut path = "/graphs?maxResults=1".to_string();
        loop {
            let (status, body) = call(&router, region, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case paging {region} at {path:?}: expected 200, got {status}: {body}"
            );
            let page = body["graphs"].as_array().cloned().unwrap_or_default();
            for summary in &page {
                seen.push(summary["id"].as_str().unwrap_or_default().to_string());
            }
            match body["nextToken"].as_str() {
                Some(token) => path = format!("/graphs?maxResults=1&nextToken={token}"),
                None => break,
            }
            assert!(
                seen.len() <= REGIONS.len() * 4,
                "case paging {region}: the token sequence did not terminate, saw {seen:?}"
            );
        }
        let want = &expected[region];
        assert_eq!(
            &seen, want,
            "case paging {region} one graph at a time: expected {want:?}, got {seen:?}"
        );
    }
}

/// Graph and snapshot names are unique per account PER REGION: the same name is
/// accepted once in every Region, refused a second time within one Region, and freed
/// for that Region alone when its holder is deleted.
#[tokio::test]
async fn resource_names_are_unique_per_region() {
    let router = emulator();

    let mut created: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let (id, _) = create_graph_ok(&router, region, "shared").await;
        created.push((region, id));
    }

    // A second graph by that name in the SAME Region is the conflict.
    for (region, _) in &created {
        let (status, body) = create_graph(&router, region, "shared").await;
        assert_eq!(
            status, 409,
            "case a second 'shared' graph in {region}: expected 409, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ConflictException",
            "case a second 'shared' graph in {region}: expected ConflictException, \
             got {code:?}: {body}"
        );
    }

    // Deleting one Region's holder frees the name in THAT Region only.
    let (freed_region, freed_id) = created[1].clone();
    wait_until_available(&router, freed_region, &freed_id).await;
    let (status, body) = call(
        &router,
        freed_region,
        "DELETE",
        &format!("/graphs/{freed_id}?skipSnapshot=true"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case deleting {freed_id} in {freed_region}: expected 200, got {status}: {body}"
    );
    // A deleted graph holds its name until the deletion the clock owes falls due, so
    // the test waits for the record to go before the name is expected to be free. No
    // handler sleeps for it; only this test does, because it asks about a state the
    // clock reaches on its own.
    wait_until_gone(&router, freed_region, &freed_id).await;
    let (status, body) = create_graph(&router, freed_region, "shared").await;
    assert_eq!(
        status, 201,
        "case recreating 'shared' in {freed_region} after the delete: expected 201, \
         got {status}: {body}"
    );
    for (region, _) in created.iter().filter(|(region, _)| *region != freed_region) {
        let (status, body) = create_graph(&router, region, "shared").await;
        assert_eq!(
            status, 409,
            "case 'shared' still held in {region}: expected 409, got {status}: {body}"
        );
    }

    // Snapshot names are per-Region in the same way: the same `snapshotName` is
    // accepted once in each Region.
    for region in REGIONS {
        let (graph_id, _) = available_graph(&router, region, "snapshot-source").await;
        let (status, body) = call(
            &router,
            region,
            "POST",
            "/snapshots",
            Some(json!({ "graphIdentifier": graph_id, "snapshotName": "shared-snapshot" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case 'shared-snapshot' in {region}: expected 201, got {status}: {body}"
        );
        let (status, body) = call(
            &router,
            region,
            "POST",
            "/snapshots",
            Some(json!({ "graphIdentifier": graph_id, "snapshotName": "shared-snapshot" })),
        )
        .await;
        assert_eq!(
            status, 409,
            "case a second 'shared-snapshot' in {region}: expected 409, got {status}: {body}"
        );
    }
}

/// A snapshot lives in the Region of the graph it was taken from: it is readable only
/// there, a graph of another Region cannot be snapshotted, and the tagging operations
/// reach neither resource across a Region boundary.
#[tokio::test]
async fn snapshots_and_tagging_are_scoped_to_the_region_too() {
    let router = emulator();

    // (Region, the graph it holds, that graph's ARN, the snapshot taken of it, that
    // snapshot's ARN).
    let mut placed: Vec<(&str, String, String, String, String)> = Vec::new();
    for region in REGIONS {
        let (graph_id, graph_arn) = available_graph(&router, region, "source").await;
        let (status, created) = call(
            &router,
            region,
            "POST",
            "/snapshots",
            Some(json!({ "graphIdentifier": graph_id, "snapshotName": "snap" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case a snapshot in {region}: expected 201, got {status}: {created}"
        );
        let snapshot_id = created["id"].as_str().unwrap_or_default().to_string();
        let snapshot_arn = created["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            snapshot_arn.contains(&format!(":{region}:")),
            "case a snapshot in {region}: expected an ARN naming the Region, \
             got {snapshot_arn:?}"
        );
        placed.push((region, graph_id, graph_arn, snapshot_id, snapshot_arn));
    }

    for (owner, graph_id, graph_arn, snapshot_id, snapshot_arn) in &placed {
        // The snapshot reads back in its own Region ...
        let (status, body) = call(
            &router,
            owner,
            "GET",
            &format!("/snapshots/{snapshot_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {snapshot_id} in its own Region {owner}: expected 200, got {status}: {body}"
        );

        for caller in REGIONS.iter().filter(|caller| *caller != owner) {
            // ... and nowhere else.
            let (status, body) = call(
                &router,
                caller,
                "GET",
                &format!("/snapshots/{snapshot_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case GetGraphSnapshot on a {owner} snapshot from {caller}: expected 404, \
                 got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case GetGraphSnapshot on a {owner} snapshot from {caller}: expected \
                 ResourceNotFoundException, got {code:?}: {body}"
            );

            // A graph of another Region is not snapshottable from here.
            let (status, body) = call(
                &router,
                caller,
                "POST",
                "/snapshots",
                Some(json!({
                    "graphIdentifier": graph_id,
                    "snapshotName": format!("cross-{caller}"),
                })),
            )
            .await;
            assert_eq!(
                status, 404,
                "case CreateGraphSnapshot of a {owner} graph from {caller}: expected 404, \
                 got {status}: {body}"
            );

            // Tagging resolves its subject by ARN, and the ARN of another Region's
            // resource names no resource through this Region's endpoint.
            for (case, arn) in [("graph", graph_arn), ("snapshot", snapshot_arn)] {
                let (status, body) = call(
                    &router,
                    caller,
                    "GET",
                    &format!("/tags/{}", encode(arn)),
                    None,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case ListTagsForResource on a {owner} {case} from {caller}: \
                     expected 404, got {status}: {body}"
                );
                let (status, body) = call(
                    &router,
                    caller,
                    "POST",
                    &format!("/tags/{}", encode(arn)),
                    Some(json!({ "tags": { "env": "prod" } })),
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case TagResource on a {owner} {case} from {caller}: expected 404, \
                     got {status}: {body}"
                );
                let (status, body) = call(
                    &router,
                    caller,
                    "DELETE",
                    &format!("/tags/{}?tagKeys=env", encode(arn)),
                    None,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case UntagResource on a {owner} {case} from {caller}: expected 404, \
                     got {status}: {body}"
                );
            }
        }

        // The cross-Region tagging attempts wrote nothing: the resource's own Region
        // still reports the tag set it had (empty).
        for (case, arn) in [("graph", graph_arn), ("snapshot", snapshot_arn)] {
            let (status, body) =
                call(&router, owner, "GET", &format!("/tags/{}", encode(arn)), None).await;
            assert_eq!(
                status, 200,
                "case ListTagsForResource on the {owner} {case}: expected 200, \
                 got {status}: {body}"
            );
            let tags = body["tags"].as_object().cloned().unwrap_or_default();
            assert!(
                tags.is_empty(),
                "case the {owner} {case}: expected no tags after the cross-Region \
                 writes, got {tags:?}"
            );
        }
    }
}

/// A query is addressed as a query OF a graph, and the graph is resolved in the Region
/// of the request, so a query in flight on one Region's graph is unreachable from
/// another — `GetQuery` and `CancelQuery` both answer 404 there, and the query keeps
/// running.
#[tokio::test]
async fn a_query_is_reachable_only_from_the_region_of_its_graph() {
    let (router, engine) = emulator_with_engine();

    // A query `ExecuteQuery` runs is over within the call it arrived on, so the queries
    // under test are admitted through the store constructor a query still in flight is
    // held in — this suite is about WHICH REGION reaches them.
    let query_id = "6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607";
    let mut placed: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let (graph_id, _) = available_graph(&router, region, "query-host").await;
        placed.push((region, graph_id));
    }

    // A query is over once its own run span is out, so each row admits its query
    // immediately before the calls that reach for it rather than relying on one
    // admission to outlive the whole table.
    let admit = |graph_id: &str| {
        engine
            .state::<Store>()
            .lock()
            .insert_query(Query::running(
                query_id,
                graph_id,
                "MATCH (n) RETURN n",
                Instant::now(),
            ))
            .expect("the store admits the query under test");
    };

    // (case, HTTP method) — both implemented query operations, on the one path the
    // model binds them to.
    for (case, method) in [("GetQuery", "GET"), ("CancelQuery", "DELETE")] {
        for (owner, graph_id) in &placed {
            admit(graph_id);
            for caller in REGIONS.iter().filter(|caller| *caller != owner) {
                let (status, body) = query_call(
                    &router,
                    caller,
                    method,
                    &format!("/queries/{query_id}"),
                    graph_id,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case} on a {owner} graph's query from {caller}: expected 404, \
                     got {status}: {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case} on a {owner} graph's query from {caller}: expected \
                     ResourceNotFoundException, got {code:?}: {body}"
                );
            }
            // The cross-Region calls reached nothing: the query is still in flight in
            // its own Region.
            let (status, body) = query_call(
                &router,
                owner,
                "GET",
                &format!("/queries/{query_id}"),
                graph_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the {owner} query must still be in flight in its own \
                 Region, got {status}: {body}"
            );
        }
    }
}

/// An export task lives in the Region it was started in: `GetExportTask` answers it
/// only there, a graph of another Region cannot be exported, and the task's tags are
/// not reachable across a Region boundary.
#[tokio::test]
async fn export_tasks_are_scoped_to_the_region_too() {
    let router = emulator();

    // (Region, the graph it holds, the export task started on it, that task's ARN).
    let mut placed: Vec<(&str, String, String, String)> = Vec::new();
    for region in REGIONS {
        let (graph_id, _) = available_graph(&router, region, "export-source").await;
        let (status, created) = call(
            &router,
            region,
            "POST",
            "/exporttasks",
            Some(json!({
                "graphIdentifier": graph_id,
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": "s3://export-bucket",
                "kmsKeyIdentifier": KMS_KEY_ARN,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case an export task in {region}: expected 201, got {status}: {created}"
        );
        let task_id = created["taskId"].as_str().unwrap_or_default().to_string();
        let task_arn =
            ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("export-task/{task_id}"));
        placed.push((region, graph_id, task_id, task_arn));
    }

    for (owner, graph_id, task_id, task_arn) in &placed {
        let (status, body) = call(
            &router,
            owner,
            "GET",
            &format!("/exporttasks/{task_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {task_id} in its own Region {owner}: expected 200, got {status}: {body}"
        );

        for caller in REGIONS.iter().filter(|caller| *caller != owner) {
            let (status, body) = call(
                &router,
                caller,
                "GET",
                &format!("/exporttasks/{task_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case GetExportTask on a {owner} task from {caller}: expected 404, \
                 got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case GetExportTask on a {owner} task from {caller}: expected \
                 ResourceNotFoundException, got {code:?}: {body}"
            );

            // A graph of another Region is not exportable from here.
            let (status, body) = call(
                &router,
                caller,
                "POST",
                "/exporttasks",
                Some(json!({
                    "graphIdentifier": graph_id,
                    "roleArn": ROLE_ARN,
                    "format": "CSV",
                    "destination": "s3://export-bucket",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                })),
            )
            .await;
            assert_eq!(
                status, 404,
                "case StartExportTask on a {owner} graph from {caller}: expected 404, \
                 got {status}: {body}"
            );

            // And neither are the task's tags.
            let (status, body) = call(
                &router,
                caller,
                "GET",
                &format!("/tags/{}", encode(task_arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case ListTagsForResource on a {owner} task from {caller}: expected 404, \
                 got {status}: {body}"
            );
        }

        // The ARN this suite built is the one the service minted, so the owner's own
        // Region does resolve it — otherwise the 404s above would prove nothing.
        let (status, body) = call(
            &router,
            owner,
            "GET",
            &format!("/tags/{}", encode(task_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case ListTagsForResource on the {owner} task: expected 200, got {status}: {body}"
        );
    }
}

/// An import task lives in the Region of the graph it loads into: `GetImportTask` and
/// `CancelImportTask` answer it only there, and a graph of another Region cannot be
/// imported into.
#[tokio::test]
async fn import_tasks_are_scoped_to_the_region_too() {
    let router = emulator();

    // (Region, the graph it holds, the import task started on it).
    let mut placed: Vec<(&str, String, String)> = Vec::new();
    for region in REGIONS {
        let (graph_id, _) = available_graph(&router, region, "import-target").await;
        let (status, created) = call(
            &router,
            region,
            "POST",
            &format!("/graphs/{graph_id}/importtasks"),
            Some(json!({ "source": "s3://example-bucket/data/", "roleArn": ROLE_ARN })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case an import task in {region}: expected 201, got {status}: {created}"
        );
        let task_id = created["taskId"].as_str().unwrap_or_default().to_string();
        placed.push((region, graph_id, task_id));
    }

    // (case, HTTP method) — both operations that resolve an import task by id.
    // `DELETE` (CancelImportTask) comes last so the `GET` rows still find a task.
    for (case, method) in [("GetImportTask", "GET"), ("CancelImportTask", "DELETE")] {
        for (owner, graph_id, task_id) in &placed {
            for caller in REGIONS.iter().filter(|caller| *caller != owner) {
                let (status, body) = call(
                    &router,
                    caller,
                    method,
                    &format!("/importtasks/{task_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case} on a {owner} task from {caller}: expected 404, \
                     got {status}: {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case} on a {owner} task from {caller}: expected \
                     ResourceNotFoundException, got {code:?}: {body}"
                );

                // A graph of another Region cannot be imported into from here.
                let (status, body) = call(
                    &router,
                    caller,
                    "POST",
                    &format!("/graphs/{graph_id}/importtasks"),
                    Some(json!({ "source": "s3://example-bucket/data/", "roleArn": ROLE_ARN })),
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case StartImportTask into a {owner} graph from {caller}: expected 404, \
                     got {status}: {body}"
                );
            }
            // The cross-Region calls reached nothing: the task still answers in its own
            // Region.
            let (status, body) = call(
                &router,
                owner,
                "GET",
                &format!("/importtasks/{task_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the {owner} task must still answer in its own Region, \
                 got {status}: {body}"
            );
        }
    }
}
