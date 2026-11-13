// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a 409 that a stored record's state held the call back with,
//! driven over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a `ConflictException` raised because a stored
//! record did not meet a precondition states WHAT THE CALL COULD NOT DO and THE
//! CONDITION that held it back — `Unable to <action> because <the record> <condition>.`
//! — and nothing else: every one of these calls addressed its record by an identifier
//! the caller sent, so no refusal fronts one. The condition's subject is the record
//! itself ("it") where the action already named it, and the graph where the call was
//! asking for something the graph had to be serving for.
//!
//! Source for the template: the recorded AWS response for `StartGraph` on an `AVAILABLE`
//! graph, "Unable to start the graph because it is in the AVAILABLE state.", and the
//! emptiness half of the two-part precondition `StartImportTask` documents ("The graph
//! needs to be empty and in the AVAILABLE state",
//! `smithy_query(query="doc",shape="StartImportTask")`), "Unable to start the import task
//! because the graph is not empty."
//!
//! So the tables below span what that rule generalises over rather than re-running one
//! recorded request: every status this service puts a graph into that holds a call back,
//! crossed with every operation whose precondition is the graph's status and whose
//! refusal is that `ConflictException`; both halves of the one documented sentence
//! `StartImportTask` states, which must read as one sentence; and the three other record
//! kinds whose own status refuses a call (import task, export task, private graph
//! endpoint), each in every status that refuses it.
//!
//! Two refusals of this service are deliberately NOT this grammar, because AWS words
//! them itself and the recorded response is what each carries: a delete held back by the
//! graph's stored `deletionProtection`, and a snapshot of a graph whose data is in
//! motion, are both `ValidationException` (400) sentences naming the graph
//! (`tests/all/refusal_shape_per_decision.rs`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphNode, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::graph_state_gate_messages::{graph_in_status, wait_for_status};

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The `source` every import request here names; nothing reads it.
const SOURCE: &str = "s3://example-bucket/data/";

/// The `destination` every export request here names; nothing reads it.
const DESTINATION: &str = "s3://export-bucket/out/";

/// An emulator holding only this service, with the engine it was built on, so a test can
/// seed a graph's data the way the data plane writes it. Each test starts from an empty
/// store.
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

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The whole message a precondition refusal must carry, written out here rather than
/// reached from the service's own helper so the test states the expectation
/// independently of the code under test.
fn expected_message(action: &str, subject: &str, condition: &str) -> String {
    format!("Unable to {action} because {subject} {condition}.")
}

/// The condition a refusal words the state of a record with.
fn status_condition(status: &str) -> String {
    format!("is in the {status} state")
}

/// Assert one refusal: the modelled status and code, the whole message the rule words,
/// and that nothing the caller addressed the record with is fronted.
fn assert_refusal(
    case: &str,
    code: u16,
    body: &Value,
    expected: &str,
    identifiers: &[&str],
) {
    assert_eq!(code, 409, "case {case:?}: expected 409, got {code}: {body}");
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case:?}: unexpected error code in {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
    for identifier in identifiers {
        assert!(
            !actual.contains(identifier),
            "case {case:?}: the refusal must not front the identifier {identifier:?}, got \
             {actual:?}"
        );
    }
}

/// The wire request of one operation whose precondition is the graph's status, as
/// (method, path, body), with `nonce` making every name-scoped member unique.
///
/// Source: the operations' HTTP bindings and required members
/// (`smithy_query(query="operation",operation="StartImportTask,CreateGraphSnapshot,StartExportTask,CreatePrivateGraphEndpoint")`).
fn graph_precondition_request(
    operation: &str,
    graph_id: &str,
    nonce: &str,
) -> (&'static str, String, Option<Value>) {
    match operation {
        "StartImportTask" => (
            "POST",
            format!("/graphs/{graph_id}/importtasks"),
            Some(json!({"source": SOURCE, "roleArn": ROLE_ARN})),
        ),
        "CreateGraphSnapshot" => (
            "POST",
            "/snapshots".to_string(),
            Some(json!({
                "graphIdentifier": graph_id,
                "snapshotName": format!("snap-{nonce}"),
            })),
        ),
        "StartExportTask" => (
            "POST",
            "/exporttasks".to_string(),
            Some(json!({
                "graphIdentifier": graph_id,
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": DESTINATION,
                "kmsKeyIdentifier": KMS_KEY_ARN,
            })),
        ),
        "CreatePrivateGraphEndpoint" => (
            "POST",
            format!("/graphs/{graph_id}/endpoints/"),
            Some(json!({
                "vpcId": "vpc-gate01",
                "subnetIds": ["subnet-0a1b2c3d4e5f67890"],
            })),
        ),
        other => panic!("no graph-status precondition is stated by {other:?}"),
    }
}

/// `POST /graphs` plus the wait for the graph to come up serving.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({"graphName": name, "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_for_status(router, case, &id, "AVAILABLE").await;
    id
}

/// Poll one record until it reports `expected`. No handler ever sleeps; only this test
/// waits, because it is asking about a status the clock reaches on its own.
async fn wait_for_record(router: &MultiServiceRouter, case: &str, path: &str, expected: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// Drive one call that must be ACCEPTED and assert the status it reports, so the setup of
/// a transient status is self-checking. Returns the record it answered with.
async fn accept(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected_code: u16,
    expected_status: &str,
) -> Value {
    let (code, answered) = call(router, method, path, body).await;
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected {expected_code} from {method} {path}, got {code}: {answered}"
    );
    assert_eq!(
        answered["status"],
        json!(expected_status),
        "case {case:?}: expected {expected_status} after {method} {path}, got {}",
        answered["status"]
    );
    answered
}

#[tokio::test]
async fn an_operation_held_back_by_the_graphs_status_words_its_action_and_that_status() {
    let (router, _engine) = emulator();

    // (the operation, the action its refusal words) — every operation of this service
    // whose precondition is the status of a graph it was handed the identifier of AND
    // whose refusal is this grammar's `ConflictException`, as opposed to the five that
    // ask for a transition OF the graph (covered by `graph_state_gate_messages`).
    // `CreateGraphSnapshot` is not among them: AWS words that refusal itself, as a 400
    // naming the graph (`tests/all/refusal_shape_per_decision.rs`).
    let actions: &[(&str, &str)] = &[
        ("StartImportTask", "start the import task"),
        ("StartExportTask", "start the export task"),
        (
            "CreatePrivateGraphEndpoint",
            "create the private graph endpoint",
        ),
    ];
    // The three that need the graph's SERVING data plane, so every status but `AVAILABLE`
    // holds them back.
    let needs_serving: &[&str] = &[
        "StartImportTask",
        "StartExportTask",
        "CreatePrivateGraphEndpoint",
    ];
    // All three, which is what any status with a transition still in flight holds back.
    let every_operation: &[&str] = &[
        "StartImportTask",
        "StartExportTask",
        "CreatePrivateGraphEndpoint",
    ];

    // (the status the graph is in, the operations that status holds back). Every status
    // this service puts a graph into except `AVAILABLE`, which is the one that lets all
    // four calls through. Source for the status set: the modelled `GraphStatus` enum
    // (`smithy_query(query="shape",shape="GraphStatus")`) minus `FAILED`, which no path
    // in this service produces.
    let rows: &[(&str, &[&str])] = &[
        ("CREATING", every_operation),
        ("UPDATING", every_operation),
        ("SNAPSHOTTING", every_operation),
        ("IMPORTING", every_operation),
        ("STOPPING", every_operation),
        // A stopped graph RESTS, and the three calls here need the serving data plane, so
        // it holds all three back for that reason rather than for a transition in
        // flight.
        ("STOPPED", needs_serving),
        ("STARTING", every_operation),
        ("RESETTING", every_operation),
        ("DELETING", every_operation),
    ];

    for (graph_status, refused) in rows {
        let nonce = graph_status.to_lowercase();
        let setup = format!("a graph in {graph_status}");
        let graph_id = graph_in_status(
            &router,
            &setup,
            graph_status,
            &format!("precondition-{nonce}"),
        )
        .await;

        // Fired back to back with no wait: an in-flight transition is in flight only
        // until the clock makes it due, and a refusal changes nothing, so one graph
        // serves every operation this status holds back.
        for operation in *refused {
            let action = actions
                .iter()
                .find(|(named, _)| named == operation)
                .map(|(_, action)| *action)
                .unwrap_or_else(|| panic!("no action is worded for {operation:?}"));
            let case = format!("{operation} against a graph in {graph_status}");
            let (method, path, body) = graph_precondition_request(operation, &graph_id, &nonce);
            let (code, answered) = call(&router, method, &path, body).await;
            assert_refusal(
                &case,
                code,
                &answered,
                &expected_message(action, "the graph", &status_condition(graph_status)),
                &[&graph_id],
            );
        }
    }
}

#[tokio::test]
async fn the_two_halves_of_the_import_precondition_read_as_one_sentence() {
    let (router, engine) = emulator();

    // The STATUS half: a graph whose provisioning is still in flight.
    let creating = graph_in_status(
        &router,
        "the status half",
        "CREATING",
        "precondition-status-half",
    )
    .await;
    let (status_code, status_body) = call(
        &router,
        "POST",
        &format!("/graphs/{creating}/importtasks"),
        Some(json!({"source": SOURCE, "roleArn": ROLE_ARN})),
    )
    .await;
    assert_refusal(
        "the status half of the import precondition",
        status_code,
        &status_body,
        &expected_message(
            "start the import task",
            "the graph",
            &status_condition("CREATING"),
        ),
        &[&creating],
    );

    // The EMPTINESS half: a serving graph that holds data. The data is written through
    // the store's own writer, which is where the data plane writes, because the rule is
    // about the data the graph holds however it got there.
    let filled = graph_in_service(&router, "the emptiness half", "precondition-empty-half").await;
    {
        let state = service_state(&*engine).expect("the neptune-graph store is mounted");
        let mut store = state.lock();
        let graph = store
            .graphs
            .get_mut(&filled)
            .expect("the graph was created over the wire");
        graph
            .data
            .insert_node(
                "n-1",
                GraphNode {
                    labels: ["Person".to_string()].into_iter().collect(),
                    properties: [("name".to_string(), json!("a value"))]
                        .into_iter()
                        .collect(),
                },
                std::time::SystemTime::now(),
            )
            .expect("an empty graph takes one node");
    }
    let (empty_code, empty_body) = call(
        &router,
        "POST",
        &format!("/graphs/{filled}/importtasks"),
        Some(json!({"source": SOURCE, "roleArn": ROLE_ARN})),
    )
    .await;
    assert_refusal(
        "the emptiness half of the import precondition",
        empty_code,
        &empty_body,
        &expected_message("start the import task", "the graph", "is not empty"),
        &[&filled],
    );

    // One documented sentence, so one grammar: both halves open with what the call could
    // not do and go on to the condition on the graph that held it back, and differ only
    // in that condition.
    let halves = [message(&status_body), message(&empty_body)];
    let opening = "Unable to start the import task because the graph ";
    for half in &halves {
        assert!(
            half.starts_with(opening) && half.ends_with('.'),
            "case both halves of one documented sentence: expected a refusal opening \
             {opening:?} and ending in a full stop, got {half:?}"
        );
    }
    assert_ne!(
        halves[0], halves[1],
        "case both halves of one documented sentence: the two preconditions are different \
         conditions, so they cannot word one message, got {halves:?}"
    );
}

#[tokio::test]
async fn a_precondition_aws_words_itself_is_not_this_grammar() {
    let (router, _engine) = emulator();

    // A graph at rest whose stored `deletionProtection` is set: the delete is held back
    // by a setting rather than by a status — "Graphs cannot be deleted if
    // delete-protection is enabled." (`smithy_query(query="doc",shape="DeleteGraph")`) —
    // and AWS words that refusal itself, as a 400 naming the graph that stays, so it is
    // NOT the conflict grammar the rest of this file is about.
    let protected = graph_in_service(&router, "a protected graph", "precondition-protected").await;
    let (patched_code, patched) = call(
        &router,
        "PATCH",
        &format!("/graphs/{protected}"),
        Some(json!({"deletionProtection": true})),
    )
    .await;
    assert_eq!(
        patched_code, 200,
        "case a protected graph: expected 200 from UpdateGraph, got {patched_code}: {patched}"
    );
    wait_for_status(&router, "a protected graph", &protected, "AVAILABLE").await;

    let (code, answered) = call(
        &router,
        "DELETE",
        &format!("/graphs/{protected}?skipSnapshot=true"),
        None,
    )
    .await;
    assert_eq!(
        code, 400,
        "case deleting a graph that deletion protection holds: expected 400, got {code}: \
         {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ValidationException",
        "case deleting a graph that deletion protection holds: unexpected error code in \
         {answered}"
    );
    let actual = message(&answered);
    assert_eq!(
        actual,
        format!("Graph [{protected}] cannot be deleted because deletion protection is enabled."),
        "case deleting a graph that deletion protection holds: unexpected message {actual:?}"
    );
    assert!(
        !actual.starts_with("Unable to "),
        "case deleting a graph that deletion protection holds: the refusal AWS words \
         itself must not be reworded into this file's conflict grammar, got {actual:?}"
    );

    // Turning the setting off is what lets the same request through, so the refusal was
    // about the stored setting and nothing else.
    let (cleared_code, cleared) = call(
        &router,
        "PATCH",
        &format!("/graphs/{protected}"),
        Some(json!({"deletionProtection": false})),
    )
    .await;
    assert_eq!(
        cleared_code, 200,
        "case a protected graph: expected 200 from UpdateGraph, got {cleared_code}: {cleared}"
    );
    wait_for_status(&router, "a protected graph", &protected, "AVAILABLE").await;
    accept(
        &router,
        "a graph whose protection was turned off",
        "DELETE",
        &format!("/graphs/{protected}?skipSnapshot=true"),
        None,
        200,
        "DELETING",
    )
    .await;
}

/// Fire one refused `DELETE` and assert the whole message the rule words for a record
/// its own status held the call back on. Called STRAIGHT AFTER the setup that put the
/// record in that status, because an in-flight status is in flight only until the clock
/// makes it due.
async fn refuse_delete(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    action: &str,
    record_status: &str,
    identifiers: &[&str],
) {
    let (code, answered) = call(router, "DELETE", path, None).await;
    assert_refusal(
        case,
        code,
        &answered,
        &expected_message(action, "it", &status_condition(record_status)),
        identifiers,
    );
}

/// The id of a record a call answered with.
fn task_id_of(case: &str, body: &Value) -> String {
    body["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {body}"))
        .to_string()
}

#[tokio::test]
async fn a_record_refused_by_its_own_status_words_the_action_and_that_status() {
    let (router, _engine) = emulator();

    // An import task that stopped on its own: its cancellation has no work left to stop.
    let import_graph = graph_in_service(&router, "a stopped import", "precondition-import").await;
    let settled = accept(
        &router,
        "a stopped import",
        "POST",
        &format!("/graphs/{import_graph}/importtasks"),
        Some(json!({"source": SOURCE, "roleArn": ROLE_ARN})),
        201,
        "INITIALIZING",
    )
    .await;
    let settled_task = task_id_of("a stopped import", &settled);
    wait_for_record(
        &router,
        "a stopped import",
        &format!("/importtasks/{settled_task}"),
        "FAILED",
    )
    .await;
    refuse_delete(
        &router,
        "cancelling an import task that already stopped",
        &format!("/importtasks/{settled_task}"),
        "cancel the import task",
        "FAILED",
        &[&settled_task, &import_graph],
    )
    .await;

    // An import task that is already winding down: a second cancellation is a conflict.
    let cancelling_graph =
        graph_in_service(&router, "a cancelling import", "precondition-import-two").await;
    let cancelling = accept(
        &router,
        "a cancelling import",
        "POST",
        &format!("/graphs/{cancelling_graph}/importtasks"),
        Some(json!({"source": SOURCE, "roleArn": ROLE_ARN})),
        201,
        "INITIALIZING",
    )
    .await;
    let cancelling_task = task_id_of("a cancelling import", &cancelling);
    accept(
        &router,
        "a cancelling import",
        "DELETE",
        &format!("/importtasks/{cancelling_task}"),
        None,
        200,
        "CANCELLING",
    )
    .await;
    refuse_delete(
        &router,
        "cancelling an import task that is already winding down",
        &format!("/importtasks/{cancelling_task}"),
        "cancel the import task",
        "CANCELLING",
        &[&cancelling_task, &cancelling_graph],
    )
    .await;

    // The same two shapes on the export side, where the task is a record of its own
    // namespace.
    let export_graph = graph_in_service(&router, "the export tasks", "precondition-export").await;
    let export_body = json!({
        "graphIdentifier": export_graph,
        "roleArn": ROLE_ARN,
        "format": "CSV",
        "destination": DESTINATION,
        "kmsKeyIdentifier": KMS_KEY_ARN,
    });
    let settled_export = accept(
        &router,
        "a stopped export",
        "POST",
        "/exporttasks",
        Some(export_body.clone()),
        201,
        "INITIALIZING",
    )
    .await;
    let settled_export_task = task_id_of("a stopped export", &settled_export);
    wait_for_record(
        &router,
        "a stopped export",
        &format!("/exporttasks/{settled_export_task}"),
        "SUCCEEDED",
    )
    .await;
    refuse_delete(
        &router,
        "cancelling an export task that already stopped",
        &format!("/exporttasks/{settled_export_task}"),
        "cancel the export task",
        "SUCCEEDED",
        &[&settled_export_task, &export_graph],
    )
    .await;

    let cancelling_export = accept(
        &router,
        "a cancelling export",
        "POST",
        "/exporttasks",
        Some(export_body),
        201,
        "INITIALIZING",
    )
    .await;
    let cancelling_export_task = task_id_of("a cancelling export", &cancelling_export);
    accept(
        &router,
        "a cancelling export",
        "DELETE",
        &format!("/exporttasks/{cancelling_export_task}"),
        None,
        200,
        "CANCELLING",
    )
    .await;
    refuse_delete(
        &router,
        "cancelling an export task that is already winding down",
        &format!("/exporttasks/{cancelling_export_task}"),
        "cancel the export task",
        "CANCELLING",
        &[&cancelling_export_task, &export_graph],
    )
    .await;

    // Both shapes on a private graph endpoint, which the caller addresses by TWO
    // identifiers it sent — the graph and the VPC — so the refusal fronts neither.
    let endpoint_graph = graph_in_service(&router, "the endpoints", "precondition-endpoint").await;
    let creating_vpc = "vpc-creating01";
    accept(
        &router,
        "an endpoint still being attached",
        "POST",
        &format!("/graphs/{endpoint_graph}/endpoints/"),
        Some(json!({"vpcId": creating_vpc, "subnetIds": ["subnet-0a1b2c3d4e5f67890"]})),
        201,
        "CREATING",
    )
    .await;
    refuse_delete(
        &router,
        "deleting an endpoint that is still being attached",
        &format!("/graphs/{endpoint_graph}/endpoints/{creating_vpc}"),
        "delete the private graph endpoint",
        "CREATING",
        &[creating_vpc, &endpoint_graph],
    )
    .await;

    let deleting_vpc = "vpc-deleting01";
    accept(
        &router,
        "an endpoint on its way out",
        "POST",
        &format!("/graphs/{endpoint_graph}/endpoints/"),
        Some(json!({"vpcId": deleting_vpc, "subnetIds": ["subnet-0a1b2c3d4e5f67890"]})),
        201,
        "CREATING",
    )
    .await;
    wait_for_record(
        &router,
        "an endpoint on its way out",
        &format!("/graphs/{endpoint_graph}/endpoints/{deleting_vpc}"),
        "AVAILABLE",
    )
    .await;
    accept(
        &router,
        "an endpoint on its way out",
        "DELETE",
        &format!("/graphs/{endpoint_graph}/endpoints/{deleting_vpc}"),
        None,
        200,
        "DELETING",
    )
    .await;
    refuse_delete(
        &router,
        "deleting an endpoint that is already on its way out",
        &format!("/graphs/{endpoint_graph}/endpoints/{deleting_vpc}"),
        "delete the private graph endpoint",
        "DELETING",
        &[deleting_vpc, &endpoint_graph],
    )
    .await;
}
