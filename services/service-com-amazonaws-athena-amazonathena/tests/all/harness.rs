// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite of this test binary can share: one service mounted on
//! its own engine, reached through the same `MultiServiceRouter` the emulator boots, and
//! the few calls a query-execution test makes over it.
//!
//! The suites of `tests/all.rs` are modules of ONE binary, so a helper belongs here
//! rather than beside each test that needs it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_athena_amazonathena::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `x-amz-target` prefix of this service: the local name of the Smithy service
/// shape (`com.amazonaws.athena#AmazonAthena`), which is how the awsJson protocols
/// build that header.
const TARGET_PREFIX: &str = "AmazonAthena";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `athena` (this service's `sigv4Name`). Neither the key id
/// nor the signature is a credential: [`router`] turns request authentication off, so
/// nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/athena/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=not-verified";

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
pub fn router() -> MultiServiceRouter {
    engine_and_router().1
}

/// The same mounting, handing back the ENGINE as well, for an assertion whose reader is
/// still a scaffold and that therefore has to be made against the store itself.
pub fn engine_and_router() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pinned off so a suite does not depend on how the emulator process was started:
    // these tests assert Athena's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Invoke one operation and return `(status, body)`.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "athena.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Start one query and return the `QueryExecutionId` it minted.
pub async fn start(router: &MultiServiceRouter, body: &Value) -> String {
    let (status, answer) = call(router, "StartQueryExecution", body).await;
    assert_eq!(
        status, 200,
        "StartQueryExecution({body}) must be accepted, got {status} with {answer:?}"
    );
    answer["QueryExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQueryExecution must answer with an id, got {answer:?}"))
        .to_string()
}

/// The stored `QueryExecution` of `id`.
pub async fn execution(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "GetQueryExecution", &json!({ "QueryExecutionId": id })).await;
    assert_eq!(
        status, 200,
        "GetQueryExecution({id}) must answer 200, got {status} with {body:?}"
    );
    body["QueryExecution"].clone()
}

/// The ids `work_group`'s query history holds.
pub async fn history(router: &MultiServiceRouter, work_group: &str) -> Vec<String> {
    let (status, listed) = call(
        router,
        "ListQueryExecutions",
        &json!({ "WorkGroup": work_group, "MaxResults": 50 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListQueryExecutions({work_group}) must answer 200, got {status} with {listed:?}"
    );
    listed["QueryExecutionIds"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// Create the workgroup `name`, with no configuration of its own.
pub async fn create_work_group(router: &MultiServiceRouter, name: &str) {
    let (status, created) = call(router, "CreateWorkGroup", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "the workgroup {name} must be created, got {status} with {created:?}"
    );
}

/// A `ClientRequestToken` of the modelled length (32 to 128 characters), built from
/// `seed` so each caller carries its own.
pub fn request_token(seed: &str) -> String {
    format!("{seed:-<32}")
}

/// The wire error code (`__type`) of an error body, with the shape-id prefix the awsJson
/// protocols may carry stripped.
pub fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or("<no __type>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The refusal wording of an error body, under either casing of the `Message` member.
pub fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("<no Message>")
        .to_string()
}
