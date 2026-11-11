// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the import-task resource: `StartImportTask`, the
//! `GetImportTask` read it round-trips through, and the `CancelImportTask` that stops
//! it — driven over the wire (`restJson1`) through the same router the emulator
//! serves from.
//!
//! The rules every assertion here is about:
//!
//! * `StartImportTask` mints a task against a stored `AVAILABLE` graph, persists it,
//!   and reports the stored record — a minted `^t-[a-z0-9]{10}$` id, the graph it
//!   loads into, the source, format, parquet type, role and options the request
//!   chose, and the modelled `INITIALIZING` status. The graph reports `IMPORTING`
//!   while the task runs.
//! * An accepted task's run is the load its source holds: with no object store
//!   mounted beside this service the source cannot be read, so the task stops in the
//!   modelled `FAILED` status carrying what the run met, and loads nothing. What a
//!   task that DOES read its source reports is the sibling suite `import_task_run`.
//! * `CancelImportTask` is accepted only while a task's import is still in flight: it
//!   reports `CANCELLING`, the clock settles the task to `CANCELLED`, and the graph
//!   stops importing. A task that has already stopped, or is already cancelling, is
//!   refused `ConflictException` (409).
//! * An identifier naming no stored task is `ResourceNotFoundException` (404), and one
//!   outside the modelled `TaskId` pattern is `ValidationException` (400).
//!
//! So each table below spans what its rule generalises over — the format and parquet
//! type, the presence of import options, the source's length at the store's bound and
//! one character past it, and the status the task is in when the cancellation arrives
//! — rather than re-running one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    MAX_IMPORT_SOURCE_CHARS, PROVISIONING_SPAN,
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

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
fn is_task_id(value: &str) -> bool {
    value.strip_prefix("t-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
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

/// `GET /importtasks/{taskIdentifier}`.
async fn get_import(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/importtasks/{task_id}"), None).await
}

/// `DELETE /importtasks/{taskIdentifier}` — the wire form of `CancelImportTask`.
async fn cancel_import(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/importtasks/{task_id}"), None).await
}

/// The smallest request body `StartImportTask` accepts.
fn minimal_import_body() -> Value {
    json!({"source": "s3://example-bucket/data/", "roleArn": ROLE_ARN})
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

/// Poll one import task until it reports `expected`.
async fn wait_until_task_status(
    router: &MultiServiceRouter,
    task_id: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = get_import(router, task_id).await;
        assert_eq!(
            status, 200,
            "case {task_id} waiting for {expected}: expected 200 from GetImportTask, \
             got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = get_import(router, task_id).await;
    panic!(
        "case {task_id}: never reached {expected}, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
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

/// Start one task on a serving graph and return its id.
async fn task_in_flight(router: &MultiServiceRouter, graph_id: &str) -> String {
    wait_until_available(router, &format!("/graphs/{graph_id}")).await;
    let (status, started) = start_import(router, graph_id, minimal_import_body()).await;
    assert_eq!(
        status, 201,
        "the fixture task on {graph_id}: expected 201, got {status}: {started}"
    );
    started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("the fixture task on {graph_id}: no taskId in {started}"))
        .to_string()
}

#[tokio::test]
async fn start_import_task_persists_the_record_and_reads_it_back() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "import-target").await;

    // One row per form the request takes: each modelled `Format` value
    // (`smithy_query(query="shape",shape="Format")` -> CSV, OPEN_CYPHER, PARQUET,
    // NTRIPLES), the one modelled `ParquetType` (COLUMNAR), the `blankNodeHandling`
    // an NTRIPLES load must carry, `failOnError` either way, and a source at the
    // store's length bound.
    let source_at_bound = format!(
        "s3://example-bucket/{}",
        "d".repeat(MAX_IMPORT_SOURCE_CHARS - "s3://example-bucket/".len())
    );
    let cases: Vec<(&str, Value)> = vec![
        (
            "only the required members",
            json!({"source": "s3://example-bucket/plain/", "roleArn": ROLE_ARN}),
        ),
        (
            "the CSV format, failing on error",
            json!({
                "source": "s3://example-bucket/gremlin/",
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "failOnError": true,
            }),
        ),
        (
            "the openCypher format, skipping errors",
            json!({
                "source": "s3://example-bucket/opencypher/",
                "roleArn": ROLE_ARN,
                "format": "OPEN_CYPHER",
                "failOnError": false,
            }),
        ),
        (
            "the parquet format with its only parquet type",
            json!({
                "source": "s3://example-bucket/parquet/",
                "roleArn": ROLE_ARN,
                "format": "PARQUET",
                "parquetType": "COLUMNAR",
            }),
        ),
        (
            "the N-Triples format with blank-node handling",
            json!({
                "source": "s3://example-bucket/ntriples/",
                "roleArn": ROLE_ARN,
                "format": "NTRIPLES",
                "blankNodeHandling": "convertToIri",
            }),
        ),
        (
            "import options for a Neptune source",
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
        (
            "a source at the store's length bound",
            json!({"source": source_at_bound, "roleArn": ROLE_ARN}),
        ),
    ];

    for (case, body) in cases {
        // Every row starts from a serving graph, so it is about the REQUEST and not
        // about a transition an earlier row left outstanding.
        wait_until_available(&router, &format!("/graphs/{graph_id}")).await;

        let (status, started) = start_import(&router, &graph_id, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {started}"
        );

        let task_id = started["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
            .to_string();
        assert!(
            is_task_id(&task_id),
            "case {case:?}: expected an id matching ^t-[a-z0-9]{{10}}$, got {task_id:?}"
        );
        assert_eq!(
            started["graphId"],
            json!(graph_id),
            "case {case:?}: expected graphId {graph_id:?}, got {}",
            started["graphId"]
        );
        assert_eq!(
            started["status"],
            json!("INITIALIZING"),
            "case {case:?}: the load has not begun when the call returns, expected \
             INITIALIZING, got {}",
            started["status"]
        );
        for member in ["source", "roleArn", "format", "parquetType", "importOptions"] {
            let requested = body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                started[member], requested,
                "case {case:?}: expected {member} {requested}, got {}",
                started[member]
            );
        }

        // The graph loads data for as long as the task runs.
        let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            graph_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {graph_status}: {graph}"
        );
        assert_eq!(
            graph["status"],
            json!("IMPORTING"),
            "case {case:?}: expected the target graph to report IMPORTING, got {}",
            graph["status"]
        );

        // The round trip: an independent read reports the record that was stored, not
        // the response that was rendered.
        let (read_status, read) = get_import(&router, &task_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetImportTask, got {read_status}: {read}"
        );
        for member in [
            "taskId",
            "graphId",
            "source",
            "roleArn",
            "format",
            "parquetType",
            "importOptions",
        ] {
            assert_eq!(
                read[member], started[member],
                "case {case:?}: GetImportTask's {member} must be the stored value {}, \
                 got {}",
                started[member], read[member]
            );
        }
        assert_eq!(
            read["attemptNumber"],
            json!(1),
            "case {case:?}: a task nothing retries has been attempted once, got {}",
            read["attemptNumber"]
        );
        // The run has not written anything when the call returns, so the details of it
        // report none of the load's work done — and never a count of work nothing did.
        let details = &read["importTaskDetails"];
        assert_eq!(
            details["status"],
            json!("INITIALIZING"),
            "case {case:?}: the details report the status the task is in, got {}",
            details["status"]
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
                "case {case:?}: nothing has been loaded yet, so {member} is 0, got {}",
                details[member]
            );
        }

        // There is no object store mounted beside this service, so the source cannot be
        // read: the task stops FAILED with a reason that says what the run met — never
        // SUCCEEDED, and never a count of a load nothing performed.
        let stopped = wait_until_task_status(&router, &task_id, "FAILED").await;
        let reason = stopped["statusReason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: a FAILED task must say why, got {}",
            stopped["statusReason"]
        );
        for member in ["taskId", "graphId", "source", "roleArn"] {
            assert_eq!(
                stopped[member], started[member],
                "case {case:?}: stopping must not change {member}, expected {}, got {}",
                started[member], stopped[member]
            );
        }

        // The graph stops importing on its own once the task has stopped.
        wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
    }
}

#[tokio::test]
async fn cancel_import_task_stops_an_in_flight_task_and_its_graph_import() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "cancel-target").await;
    let task_id = task_in_flight(&router, &graph_id).await;

    let (status, cancelled) = cancel_import(&router, &task_id).await;
    assert_eq!(
        status, 200,
        "case an in-flight task: expected 200, got {status}: {cancelled}"
    );
    assert_eq!(
        cancelled["taskId"],
        json!(task_id),
        "case an in-flight task: expected taskId {task_id:?}, got {}",
        cancelled["taskId"]
    );
    assert_eq!(
        cancelled["graphId"],
        json!(graph_id),
        "case an in-flight task: expected graphId {graph_id:?}, got {}",
        cancelled["graphId"]
    );
    // The cancellation is accepted, not completed: "Status is CANCELLING when the
    // import task is cancelled".
    assert_eq!(
        cancelled["status"],
        json!("CANCELLING"),
        "case an in-flight task: expected CANCELLING, got {}",
        cancelled["status"]
    );
    assert_eq!(
        cancelled["source"],
        json!("s3://example-bucket/data/"),
        "case an in-flight task: the stored source must be reported, got {}",
        cancelled["source"]
    );
    assert_eq!(
        cancelled["roleArn"],
        json!(ROLE_ARN),
        "case an in-flight task: the stored roleArn must be reported, got {}",
        cancelled["roleArn"]
    );

    // The cancellation is PERSISTED: an independent read sees the task winding down,
    // and the graph is still importing until it has stopped.
    let (read_status, read) = get_import(&router, &task_id).await;
    assert_eq!(
        read_status, 200,
        "case the cancelled task: expected 200 from GetImportTask, got {read_status}: \
         {read}"
    );
    assert_eq!(
        read["status"],
        json!("CANCELLING"),
        "case the cancelled task straight after the call: expected CANCELLING, got {}",
        read["status"]
    );

    // The clock settles the task into CANCELLED — not the FAILED outcome the
    // uncancelled task would have reached — and the graph stops importing.
    let stopped = wait_until_task_status(&router, &task_id, "CANCELLED").await;
    let reason = stopped["statusReason"].as_str().unwrap_or_default();
    assert!(
        reason.to_ascii_lowercase().contains("cancel"),
        "case the stopped task: expected a reason naming the cancellation, got {}",
        stopped["statusReason"]
    );
    let graph = wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
    assert_eq!(
        graph["status"],
        json!("AVAILABLE"),
        "case the target graph: expected AVAILABLE once the task stopped, got {}",
        graph["status"]
    );

    // Settling changes the status and its reason and nothing else about the record.
    for member in ["taskId", "graphId", "source", "roleArn"] {
        assert_eq!(
            stopped[member], cancelled[member],
            "case the stopped task: settling must not change {member}, expected {}, \
             got {}",
            cancelled[member], stopped[member]
        );
    }
}

#[tokio::test]
async fn cancellation_is_accepted_only_while_the_import_is_in_flight() {
    let router = emulator();

    // (case, the status the task is in when the cancellation arrives, expected status,
    // expected error code)
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        ("an in-flight task", "INITIALIZING", 200, ""),
        ("a task already cancelling", "CANCELLING", 409, "ConflictException"),
        ("a task that stopped cancelled", "CANCELLED", 409, "ConflictException"),
        ("a task that stopped failed", "FAILED", 409, "ConflictException"),
    ];
    for (index, (case, when, expected, expected_code)) in cases.into_iter().enumerate() {
        // A graph per row, so a row is about the TASK's status and not about a
        // transition another row left outstanding on a shared graph.
        let graph_id = graph_in_service(&router, &format!("cancel-case-{index}")).await;
        let task_id = task_in_flight(&router, &graph_id).await;

        // Bring the task to the status this row is about.
        match when {
            "INITIALIZING" => {}
            "CANCELLING" => {
                let (status, body) = cancel_import(&router, &task_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the first cancellation must be accepted, got \
                     {status}: {body}"
                );
            }
            "CANCELLED" => {
                let (status, body) = cancel_import(&router, &task_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the first cancellation must be accepted, got \
                     {status}: {body}"
                );
                wait_until_task_status(&router, &task_id, "CANCELLED").await;
            }
            other => {
                assert_eq!(other, "FAILED", "case {case:?}: unknown starting status");
                wait_until_task_status(&router, &task_id, "FAILED").await;
            }
        }

        let (status, body) = cancel_import(&router, &task_id).await;
        assert_eq!(
            status, expected,
            "case {case:?} (task in {when}): expected {expected}, got {status}: {body}"
        );
        if expected_code.is_empty() {
            assert_eq!(
                body["status"],
                json!("CANCELLING"),
                "case {case:?}: expected CANCELLING, got {}",
                body["status"]
            );
        } else {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} (task in {when}): expected {expected_code}, got \
                 {code:?}: {body}"
            );
            // A refused cancellation persists NOTHING: the task still reports the
            // status it was in.
            let (read_status, read) = get_import(&router, &task_id).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected 200 from GetImportTask, got {read_status}: \
                 {read}"
            );
            let settled = if when == "CANCELLING" {
                // The first cancellation's own transition may have come due by now.
                ["CANCELLING", "CANCELLED"]
            } else {
                [when, when]
            };
            let actual = read["status"].as_str().unwrap_or_default();
            assert!(
                settled.contains(&actual),
                "case {case:?}: expected the task to still be one of {settled:?}, got \
                 {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_task_identifier_that_names_no_stored_task_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "identifier-target").await;
    let task_id = task_in_flight(&router, &graph_id).await;

    // (case, the taskIdentifier the request carries, expected status, expected code)
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a well-formed task id that was never minted",
            "t-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        (
            "another well-formed absent task id",
            "t-0000000000",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a graph id where a task id belongs",
            "g-abcdefghij",
            400,
            "ValidationException",
        ),
        (
            "an identifier outside the modelled pattern",
            "nope",
            400,
            "ValidationException",
        ),
        (
            "a task id one character too short",
            "t-abcdefghi",
            400,
            "ValidationException",
        ),
    ];
    for (case, identifier, expected, expected_code) in cases {
        for (operation, (status, body)) in [
            ("GetImportTask", get_import(&router, identifier).await),
            ("CancelImportTask", cancel_import(&router, identifier).await),
        ] {
            assert_eq!(
                status, expected,
                "case {case:?} ({identifier:?}) on {operation}: expected {expected}, \
                 got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({identifier:?}) on {operation}: expected \
                 {expected_code}, got {code:?}: {body}"
            );
        }
    }

    // The stored task is still served by both operations, so the refusals above were
    // about the identifier and nothing else.
    let (read_status, read) = get_import(&router, &task_id).await;
    assert_eq!(
        read_status, 200,
        "case the stored task: expected 200 from GetImportTask, got {read_status}: {read}"
    );
    // A task minted immediately before the call is in flight, so this row is about the
    // identifier resolving rather than about the window the fixture task was in.
    let fresh_task = task_in_flight(&router, &graph_id).await;
    let (cancel_status, cancelled) = cancel_import(&router, &fresh_task).await;
    assert_eq!(
        cancel_status, 200,
        "case a freshly minted task: expected 200 from CancelImportTask, got \
         {cancel_status}: {cancelled}"
    );
}

#[tokio::test]
async fn start_import_task_refuses_a_request_the_rules_forbid() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "refusal-target").await;

    // One row per rule a request can break: the store's bound on `source` (at the
    // bound and one character past it), the documented dependency "blankNodeHandling
    // must be provided when format is ntriples", and the modelled enum value sets.
    let prefix = "s3://example-bucket/";
    let at_bound = format!("{prefix}{}", "d".repeat(MAX_IMPORT_SOURCE_CHARS - prefix.len()));
    let past_bound = format!("{prefix}{}", "d".repeat(MAX_IMPORT_SOURCE_CHARS - prefix.len() + 1));
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a source at the store's bound",
            json!({"source": at_bound, "roleArn": ROLE_ARN}),
            201,
            "",
        ),
        (
            "an empty source",
            json!({"source": "", "roleArn": ROLE_ARN}),
            400,
            "ValidationException",
        ),
        (
            "a source one character past the store's bound",
            json!({"source": past_bound, "roleArn": ROLE_ARN}),
            400,
            "ValidationException",
        ),
        (
            "the N-Triples format without blank-node handling",
            json!({
                "source": "s3://example-bucket/ntriples/",
                "roleArn": ROLE_ARN,
                "format": "NTRIPLES",
            }),
            400,
            "ValidationException",
        ),
        (
            "the N-Triples format with blank-node handling",
            json!({
                "source": "s3://example-bucket/ntriples/",
                "roleArn": ROLE_ARN,
                "format": "NTRIPLES",
                "blankNodeHandling": "convertToIri",
            }),
            201,
            "",
        ),
        (
            "a format outside the modelled enum",
            json!({
                "source": "s3://example-bucket/data/",
                "roleArn": ROLE_ARN,
                "format": "YAML",
            }),
            400,
            "ValidationException",
        ),
        (
            "a parquet type outside the modelled enum",
            json!({
                "source": "s3://example-bucket/data/",
                "roleArn": ROLE_ARN,
                "format": "PARQUET",
                "parquetType": "ROW",
            }),
            400,
            "ValidationException",
        ),
        (
            // The pass-role decision is taken before the shape check, so a value that
            // names no role the caller may pass is the authorization failure it is.
            "a role ARN that names no IAM role",
            json!({"source": "s3://example-bucket/data/", "roleArn": "not-an-arn"}),
            403,
            "AccessDeniedException",
        ),
    ];
    for (case, body, expected, expected_code) in cases {
        wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
        let (status, answered) = start_import(&router, &graph_id, body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {answered}"
        );
        if expected_code.is_empty() {
            continue;
        }
        let code = error_code(&answered);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}: {answered}"
        );
    }
}

#[tokio::test]
async fn an_import_needs_a_stored_graph_that_is_available() {
    let router = emulator();

    // A graph that is still provisioning cannot accept an import.
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

    // (case, the graphIdentifier the request names, expected status, expected code)
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a graph that is still provisioning",
            &graph_id,
            409,
            "ConflictException",
        ),
        (
            "a well-formed graph id that was never created",
            "g-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identifier outside the modelled pattern",
            "not-a-graph-id",
            400,
            "ValidationException",
        ),
        (
            "a task id where a graph id belongs",
            "t-abcdefghij",
            400,
            "ValidationException",
        ),
    ];
    for (case, identifier, expected, expected_code) in cases {
        let (status, body) = start_import(&router, identifier, minimal_import_body()).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({identifier:?}): expected {expected}, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?} ({identifier:?}): expected {expected_code}, got {code:?}: \
             {body}"
        );
    }

    // Once the graph is serving, the same call is accepted — so the refusal persisted
    // nothing and was about the graph's status alone.
    wait_until_available(&router, &format!("/graphs/{graph_id}")).await;
    let (status, started) = start_import(&router, &graph_id, minimal_import_body()).await;
    assert_eq!(
        status, 201,
        "case the same call once the graph is serving: expected 201, got {status}: \
         {started}"
    );
    let task_id = started["taskId"].as_str().expect("a taskId").to_string();

    // A graph already loading data cannot take a second import, because an import
    // needs an AVAILABLE graph.
    let (second_status, second) = start_import(&router, &graph_id, minimal_import_body()).await;
    assert_eq!(
        second_status, 409,
        "case a second import on an importing graph: expected 409, got \
         {second_status}: {second}"
    );
    let code = error_code(&second);
    assert_eq!(
        code, "ConflictException",
        "case a second import on an importing graph: expected ConflictException, got \
         {code:?}: {second}"
    );

    // And the first task is the only one the store holds.
    let (read_status, read) = get_import(&router, &task_id).await;
    assert_eq!(
        read_status, 200,
        "case the accepted task: expected 200 from GetImportTask, got {read_status}: \
         {read}"
    );
    assert_eq!(
        read["graphId"],
        json!(graph_id),
        "case the accepted task: expected graphId {graph_id:?}, got {}",
        read["graphId"]
    );
}
