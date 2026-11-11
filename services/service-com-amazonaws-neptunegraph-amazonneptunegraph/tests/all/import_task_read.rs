// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `GetImportTask` read, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rule every assertion here is about: `GetImportTask` reports the stored task
//! record addressed by the task's OWN minted id, so a task is served for as long as the
//! store holds that record — whatever became of the graph the load named — and
//! repeating the read never changes what a later caller sees. An identifier naming no
//! stored task record is `ResourceNotFoundException` (404).
//!
//! The round trip of a freshly started task and the refusals the identifier itself
//! earns live in `import_task_lifecycle`; what this suite adds is the INDEPENDENCE of
//! the read from the graph's lifetime, which is what "addressed by its own id" means:
//! the graph a task loaded into is deleted under it, and the task still answers the
//! record it held, down to the `graphId` of the graph that is gone.
//!
//! So the table spans what that rule generalises over: the terminal status the task
//! stopped in before its graph went away (the load stopping on its own, and a
//! cancellation completing), whether the graph's delete took a final snapshot, and the
//! member set the task record carries (the required members alone, a format, a parquet
//! type, import options) — rather than re-running one recorded request.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    DELETION_SPAN, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

/// Room for the emulator's own scheduling on top of a span the clock owes, so a wait
/// for a due transition is not a race.
const SETTLE_MARGIN: Duration = Duration::from_millis(150);

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

/// `GET /importtasks/{taskIdentifier}`.
async fn get_import(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/importtasks/{task_id}"), None).await
}

/// One read of a task with the one member that is a LIVE measurement removed, so two
/// reads of a settled record can be compared for equality.
///
/// `ImportTaskDetails$timeElapsedSeconds` is "Seconds elapsed since the import task
/// started" (`smithy_query(query="shape",shape="ImportTaskDetails")`), so it keeps
/// rising with the clock on a record nothing has changed — which is what makes it the
/// one member two reads of the same stored record may differ in.
fn settled_record(body: &Value) -> Value {
    let mut stripped = body.clone();
    if let Some(details) = stripped
        .get_mut("importTaskDetails")
        .and_then(Value::as_object_mut)
    {
        details.remove("timeElapsedSeconds");
    }
    stripped
}

/// `DELETE /importtasks/{taskIdentifier}` — the wire form of `CancelImportTask`.
async fn cancel_import(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/importtasks/{task_id}"), None).await
}

/// `POST /graphs/{graphIdentifier}/importtasks` with `body`.
async fn start_import(router: &MultiServiceRouter, graph_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        Some(body),
    )
    .await
}

/// `GET /graphs/{graphIdentifier}`.
async fn get_graph(router: &MultiServiceRouter, graph_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{graph_id}"), None).await
}

/// `DELETE /graphs/{graphIdentifier}?skipSnapshot=<skip_snapshot>`.
async fn delete_graph(
    router: &MultiServiceRouter,
    graph_id: &str,
    skip_snapshot: bool,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/graphs/{graph_id}?skipSnapshot={skip_snapshot}"),
        None,
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

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
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

/// One row of the table: the case name, the graph's name, whether the task is stopped
/// by a cancellation (rather than by the load stopping on its own), the `skipSnapshot`
/// the graph's delete carries, and the `StartImportTask` body.
type ReadCase = (&'static str, &'static str, bool, bool, Value);

#[tokio::test]
async fn an_import_task_is_served_by_its_own_id_once_its_graph_is_gone() {
    let router = emulator();

    // The rows differ along what the rule generalises over: the terminal status the
    // task stopped in, whether the delete of its graph took a final snapshot, and the
    // member set the stored record carries.
    let cases: Vec<ReadCase> = vec![
        (
            "a stopped task, its graph deleted without a snapshot",
            "read-after-delete-plain",
            false,
            true,
            json!({"source": "s3://example-bucket/plain/", "roleArn": ROLE_ARN}),
        ),
        (
            "a stopped task, its graph deleted with a final snapshot",
            "read-after-delete-csv",
            false,
            false,
            json!({
                "source": "s3://example-bucket/gremlin/",
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "failOnError": true,
            }),
        ),
        (
            "a cancelled task, its graph deleted without a snapshot",
            "read-after-delete-parquet",
            true,
            true,
            json!({
                "source": "s3://example-bucket/parquet/",
                "roleArn": ROLE_ARN,
                "format": "PARQUET",
                "parquetType": "COLUMNAR",
            }),
        ),
        (
            "a cancelled task, its graph deleted with a final snapshot",
            "read-after-delete-options",
            true,
            false,
            json!({
                "source": "s3://example-bucket/neptune/",
                "roleArn": ROLE_ARN,
                "importOptions": {
                    "neptune": {
                        "s3ExportPath": "s3://example-bucket/export/",
                        "s3ExportKmsKeyId": "alias/aws/s3",
                        "preserveDefaultVertexLabels": true,
                        "preserveEdgeIds": false,
                    },
                },
            }),
        ),
    ];

    // Every task this test has already read, so each row can prove that deleting ONE
    // graph took nothing else with it: each earlier task still answers its own record.
    let mut read_earlier: Vec<(&str, String, Value)> = Vec::new();

    for (case, graph_name, cancelled, skip_snapshot, body) in cases {
        let graph_id = graph_in_service(&router, case, graph_name).await;

        let (status, started) = start_import(&router, &graph_id, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from StartImportTask of {body}, got {status}: {started}"
        );
        let task_id = started["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
            .to_string();

        // Drive the task into the terminal status this row is about: a cancellation
        // settles it to CANCELLED, and a task nothing loads stops FAILED on its own.
        let terminal = if cancelled {
            let (cancel_status, cancel_body) = cancel_import(&router, &task_id).await;
            assert_eq!(
                cancel_status, 200,
                "case {case:?}: expected 200 from CancelImportTask, got {cancel_status}: \
                 {cancel_body}"
            );
            "CANCELLED"
        } else {
            "FAILED"
        };
        wait_for_status(&router, case, &format!("/importtasks/{task_id}"), terminal).await;
        // The graph loads data only while the task runs, so it is back at rest once the
        // task has stopped — which is what makes the delete below acceptable.
        wait_for_status(&router, case, &format!("/graphs/{graph_id}"), "AVAILABLE").await;

        // The record the task holds once it has stopped: the clock owes it nothing
        // more, so this is what every later read must answer.
        let (before_status, before) = get_import(&router, &task_id).await;
        assert_eq!(
            before_status, 200,
            "case {case:?}: expected 200 from GetImportTask, got {before_status}: {before}"
        );
        assert_eq!(
            before["taskId"],
            json!(task_id.clone()),
            "case {case:?}: expected the task's own id, got {}",
            before["taskId"]
        );
        assert_eq!(
            before["graphId"],
            json!(graph_id.clone()),
            "case {case:?}: expected graphId {graph_id:?}, got {}",
            before["graphId"]
        );

        // Deleting the graph the load named is accepted, and takes nothing of the task
        // with it — not while the deletion is in flight...
        let (delete_status, deleted) = delete_graph(&router, &graph_id, skip_snapshot).await;
        assert_eq!(
            delete_status, 200,
            "case {case:?}: expected 200 from DeleteGraph, got {delete_status}: {deleted}"
        );
        let (in_flight_status, in_flight) = get_import(&router, &task_id).await;
        assert_eq!(
            in_flight_status, 200,
            "case {case:?}: a task must still be served while its graph is deleting, got \
             {in_flight_status}: {in_flight}"
        );
        assert_eq!(
            settled_record(&in_flight),
            settled_record(&before),
            "case {case:?}: an accepted delete of the graph must not change the task \
             record, expected {before}, got {in_flight}"
        );

        // ...and not once the deletion the clock owes has fallen due and the graph is
        // gone. The task is addressed by its own id, so it outlives the graph, down to
        // the `graphId` of the graph that is gone.
        tokio::time::sleep(DELETION_SPAN + SETTLE_MARGIN).await;
        let (gone_status, gone) = get_graph(&router, &graph_id).await;
        assert_eq!(
            gone_status, 404,
            "case {case:?}: expected 404 from GetGraph once the deletion is due, got \
             {gone_status}: {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code from GetGraph in {gone}"
        );

        let (after_status, after) = get_import(&router, &task_id).await;
        assert_eq!(
            after_status, 200,
            "case {case:?}: a task must outlive the graph it loaded into, got \
             {after_status}: {after}"
        );
        assert_eq!(
            settled_record(&after),
            settled_record(&before),
            "case {case:?}: the record must survive the graph it names, expected \
             {before}, got {after}"
        );
        assert_eq!(
            after["status"],
            json!(terminal),
            "case {case:?}: expected the status the task stopped in ({terminal}), got {}",
            after["status"]
        );

        // Repeating the read never changes what a later caller sees: the one write a
        // read may make was the settling the clock already owed, and that is done.
        let (again_status, again) = get_import(&router, &task_id).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected 200 from a repeated GetImportTask, got \
             {again_status}: {again}"
        );
        assert_eq!(
            settled_record(&again),
            settled_record(&before),
            "case {case:?}: repeating the read must answer the same record, expected \
             {before}, got {again}"
        );

        // Every task of an earlier row is still served by its own id too, so this row's
        // delete reclaimed exactly one graph and no task at all.
        read_earlier.push((case, task_id, before));
        for (earlier_case, earlier_id, earlier_record) in &read_earlier {
            let (status, record) = get_import(&router, earlier_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the task of case {earlier_case:?} must still be served, \
                 got {status}: {record}"
            );
            assert_eq!(
                settled_record(&record),
                settled_record(earlier_record),
                "case {case:?}: the task of case {earlier_case:?} must still answer its \
                 own record, expected {earlier_record}, got {record}"
            );
        }
    }

    // And the 200s above are about a STORED record, not about the read answering
    // whatever it is handed: a well-formed id this store never minted is still a 404,
    // with every graph of this test now gone.
    let (missing_status, missing) = get_import(&router, "t-zzzzzzzzzz").await;
    assert_eq!(
        missing_status, 404,
        "case an id that was never minted: expected 404, got {missing_status}: {missing}"
    );
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "case an id that was never minted: unexpected error code in {missing}"
    );
}
