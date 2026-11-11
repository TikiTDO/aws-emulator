// SPDX-License-Identifier: CC0-1.0

//! `StartImportTask` mints one INDEPENDENT task record per call, and reading one back
//! is a report rather than a step.
//!
//! The sibling suite `import_task_lifecycle` covers one task at a time: the record it
//! reports, the refusals, and the clock's settling. This suite pins the two invariants
//! that only show up with SEVERAL tasks and SEVERAL reads in the store at once, and
//! that the issue this file closes (`StartImportTask` answering 501 and persisting
//! nothing) made unobservable:
//!
//! * Every accepted call mints a fresh `^t-[a-z0-9]{10}$` identifier no other task in
//!   the store holds, and the record filed under it keeps THAT request's `source`,
//!   `format`, `parquetType`, `roleArn` and `importOptions` and THAT call's target
//!   graph — so no task's values leak into another's. Accepting an import also touches
//!   only its own target: every other stored graph keeps the status it had.
//! * `GetImportTask` is a reporting read (`@readonly` in the model): repeating it never
//!   changes an immutable member of the record, and never moves the status other than
//!   along the one transition the clock already owes
//!   (`INITIALIZING` -> `FAILED`), so a later caller sees what an earlier one saw.
//!
//! The table below therefore spans the request forms the rule generalises over — each
//! modelled `Format` value, the one modelled `ParquetType`, the `blankNodeHandling` an
//! N-Triples load must carry, and a request with and without `importOptions` — one row
//! per target graph, all started before anything is read back.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

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

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
fn is_task_id(value: &str) -> bool {
    value.strip_prefix("t-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Poll one graph until it reports `AVAILABLE`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, graph_id: &str) -> Value {
    let path = format!("/graphs/{graph_id}");
    for _ in 0..40 {
        let (status, body) = call(router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {graph_id}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", &path, None).await;
    panic!(
        "case {graph_id}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no id in {created}"))
        .to_string();
    wait_until_available(router, &id).await;
    id
}

/// The request rows this suite spans: one per request form the minting rule
/// generalises over. The `Format` and `ParquetType` value sets are the modelled ones
/// (`smithy_query(query="shape",shape="Format,ParquetType")` -> CSV | NTRIPLES |
/// OPEN_CYPHER | PARQUET, and COLUMNAR), so the table covers every format including
/// the ones no recorded request cited.
fn request_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            json!({"source": "s3://bucket-a/plain/", "roleArn": role_arn("PlainLoad")}),
        ),
        (
            "the Gremlin CSV format",
            json!({
                "source": "s3://bucket-b/gremlin/",
                "roleArn": role_arn("CsvLoad"),
                "format": "CSV",
            }),
        ),
        (
            "the openCypher format",
            json!({
                "source": "s3://bucket-c/opencypher/",
                "roleArn": role_arn("OpenCypherLoad"),
                "format": "OPEN_CYPHER",
            }),
        ),
        (
            "the parquet format with its only parquet type",
            json!({
                "source": "s3://bucket-d/parquet/",
                "roleArn": role_arn("ParquetLoad"),
                "format": "PARQUET",
                "parquetType": "COLUMNAR",
            }),
        ),
        (
            "the N-Triples format with blank-node handling",
            json!({
                "source": "s3://bucket-e/ntriples/",
                "roleArn": role_arn("NtriplesLoad"),
                "format": "NTRIPLES",
                "blankNodeHandling": "convertToIri",
            }),
        ),
        (
            "a Neptune source with import options",
            json!({
                "source": "s3://bucket-f/neptune/",
                "roleArn": role_arn("NeptuneLoad"),
                "importOptions": {
                    "neptune": {
                        "s3ExportPath": "s3://bucket-f/export/",
                        "s3ExportKmsKeyId": "alias/aws/s3",
                        "preserveDefaultVertexLabels": false,
                        "preserveEdgeIds": true,
                    },
                },
            }),
        ),
    ]
}

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`, named per row
/// so a record that reported another row's role would be visible.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// The members `StartImportTask` reports straight from the request it accepted; a
/// row that is absent from the request must be absent from the record too.
const ECHOED_MEMBERS: [&str; 5] = ["source", "roleArn", "format", "parquetType", "importOptions"];

#[tokio::test]
async fn every_accepted_import_mints_its_own_task_record() {
    let router = emulator();
    let rows = request_rows();

    // A graph per row, all brought into service BEFORE anything is started, so every
    // import below is accepted into a store that already holds the other targets.
    let mut graph_ids = Vec::new();
    for (index, (case, _)) in rows.iter().enumerate() {
        let id = graph_in_service(&router, &format!("mint-target-{index}")).await;
        assert!(
            !graph_ids.contains(&id),
            "case {case:?}: CreateGraph re-used the graph id {id:?}"
        );
        graph_ids.push(id);
    }

    // Start every import before reading anything back, so the store holds one task per
    // row at once and a record that borrowed another row's values would be visible.
    let mut started: Vec<(&str, Value, String, Value)> = Vec::new();
    let mut ids = BTreeSet::new();
    for ((case, body), graph_id) in rows.iter().zip(graph_ids.iter()) {
        let (status, response) = call(
            &router,
            "POST",
            &format!("/graphs/{graph_id}/importtasks"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from StartImportTask, got {status}: {response}"
        );
        let task_id = response["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {response}"))
            .to_string();
        assert!(
            is_task_id(&task_id),
            "case {case:?}: expected an id matching ^t-[a-z0-9]{{10}}$, got {task_id:?}"
        );
        assert!(
            ids.insert(task_id.clone()),
            "case {case:?}: StartImportTask minted the id {task_id:?} a second time"
        );
        started.push((case, body.clone(), task_id, response));
    }

    // Every record is read back by its OWN id and reports its OWN request, against the
    // graph it was started on.
    for ((case, body, task_id, response), graph_id) in started.iter().zip(graph_ids.iter()) {
        let (read_status, read) = call(&router, "GET", &format!("/importtasks/{task_id}"), None).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetImportTask, got {read_status}: {read}"
        );
        assert_eq!(
            read["taskId"],
            json!(task_id),
            "case {case:?}: expected taskId {task_id:?}, got {}",
            read["taskId"]
        );
        assert_eq!(
            read["graphId"],
            json!(graph_id),
            "case {case:?}: expected graphId {graph_id:?}, got {}",
            read["graphId"]
        );
        for member in ECHOED_MEMBERS {
            let requested = body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                read[member], requested,
                "case {case:?}: expected the stored {member} {requested}, got {}",
                read[member]
            );
            assert_eq!(
                response[member], requested,
                "case {case:?}: StartImportTask must report the stored {member} \
                 {requested}, got {}",
                response[member]
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_import_touches_only_its_own_target_graph() {
    let router = emulator();

    // Three graphs in service; exactly one takes an import.
    let mut graph_ids = Vec::new();
    for index in 0..3 {
        graph_ids.push(graph_in_service(&router, &format!("bystander-{index}")).await);
    }
    let target = graph_ids[1].clone();

    let (status, started) = call(
        &router,
        "POST",
        &format!("/graphs/{target}/importtasks"),
        Some(json!({"source": "s3://bucket-g/data/", "roleArn": role_arn("TargetLoad")})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the target graph: expected 201 from StartImportTask, got {status}: {started}"
    );

    for graph_id in &graph_ids {
        let (read_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            read_status, 200,
            "case {graph_id}: expected 200 from GetGraph, got {read_status}: {graph}"
        );
        let expected = if *graph_id == target {
            "IMPORTING"
        } else {
            "AVAILABLE"
        };
        assert_eq!(
            graph["status"],
            json!(expected),
            "case {graph_id} (target is {target}): expected {expected}, got {}",
            graph["status"]
        );
    }
}

#[tokio::test]
async fn repeating_the_read_never_changes_the_task_a_later_caller_sees() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "reporting-read").await;
    let body = json!({
        "source": "s3://bucket-h/repeat/",
        "roleArn": role_arn("RepeatLoad"),
        "format": "CSV",
    });
    let (status, started) = call(
        &router,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture task: expected 201 from StartImportTask, got {status}: {started}"
    );
    let task_id = started["taskId"].as_str().expect("a taskId").to_string();

    // The one transition the clock owes an accepted task, in order: nothing a read
    // does may move the status backwards along it, or off it.
    let lifecycle = ["INITIALIZING", "FAILED"];
    let mut reached = 0usize;
    // Enough reads to cross the owed transition several times over, so a read that
    // consumed or advanced something would show up as a status out of order.
    for round in 0..24 {
        let (read_status, read) = call(&router, "GET", &format!("/importtasks/{task_id}"), None).await;
        assert_eq!(
            read_status, 200,
            "case read {round}: expected 200 from GetImportTask, got {read_status}: {read}"
        );
        let actual = read["status"].as_str().unwrap_or_default().to_string();
        let position = lifecycle
            .iter()
            .position(|status| *status == actual)
            .unwrap_or_else(|| {
                panic!(
                    "case read {round}: expected one of {lifecycle:?} for a task nothing \
                     loads, got {actual:?}"
                )
            });
        assert!(
            position >= reached,
            "case read {round}: a read must not move the status back from \
             {:?} to {actual:?}",
            lifecycle[reached]
        );
        reached = position;

        // The members a read may never touch.
        assert_eq!(
            read["taskId"],
            json!(task_id),
            "case read {round}: expected taskId {task_id:?}, got {}",
            read["taskId"]
        );
        assert_eq!(
            read["graphId"],
            json!(graph_id),
            "case read {round}: expected graphId {graph_id:?}, got {}",
            read["graphId"]
        );
        for member in ECHOED_MEMBERS {
            let requested = body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                read[member], requested,
                "case read {round}: a read must not change {member}, expected \
                 {requested}, got {}",
                read[member]
            );
        }
        assert_eq!(
            read["attemptNumber"],
            json!(1),
            "case read {round}: reading a task must not count as an attempt, got {}",
            read["attemptNumber"]
        );
        // A read reports the run; it never advances it. The details carry the status
        // the task is in, and no count of work this load did not do.
        let details = &read["importTaskDetails"];
        assert_eq!(
            details["status"], read["status"],
            "case read {round}: the details report the status the task is in, expected {}, \
             got {}",
            read["status"], details["status"]
        );
        for member in [
            "progressPercentage",
            "statementCount",
            "dictionaryEntryCount",
            "errorCount",
        ] {
            assert_eq!(
                details[member],
                json!(0),
                "case read {round}: this task loaded nothing, so {member} is 0, got {}",
                details[member]
            );
        }
        tokio::time::sleep(PROVISIONING_SPAN / 8).await;
    }

    assert_eq!(
        reached,
        lifecycle.len() - 1,
        "the fixture task: the clock owed it {:?}, but {} reads over {:?} never saw it",
        lifecycle[lifecycle.len() - 1],
        24,
        PROVISIONING_SPAN * 3
    );
}
