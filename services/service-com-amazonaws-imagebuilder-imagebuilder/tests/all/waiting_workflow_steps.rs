// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the waiting-step listing and for how a step execution id this
//! API does not admit is refused.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door and the handlers behind it — so what is asserted
//! is what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * `ListWaitingWorkflowSteps` "lists the workflow steps in your Amazon Web Services account
//!   that have paused at a `WaitForAction` step, and are waiting for you to respond". A step
//!   pauses inside a workflow RUN, and nothing in this service starts one, so an account
//!   whose builds ran no workflow has no paused step and the listing answers 200 with an
//!   empty page — at every page size, and after resources have been created, since a
//!   configuration, a component or a recipe is not a paused step. A `nextToken` this store
//!   did not issue is not a position in that listing and is refused as the value of the
//!   parameter it arrived in.
//! * `SendWorkflowStepAction`'s `stepExecutionId` is `@required` with a `@pattern` of its
//!   own, and the operation models Image Builder's own parameter-value error — so a value
//!   outside that pattern is refused HTTP 400 `InvalidParameterValueException` naming the
//!   member and quoting the expression its value had to match, never the framework's generic
//!   validation error, which is in no SDK's imagebuilder error union.
//!
//! The rows span what the rules generalise over: the page size the listing is read at and
//! whether the store is empty; and, for the refused id, the way the value misses the pattern
//! (the prefix, the dashes, the group widths, a character that is not hex, the empty value,
//! trailing text) — with ids that DO match the pattern among the rows, so a refusal that
//! fired for everything would be caught.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The modelled `WorkflowStepExecutionId` pattern
/// (`smithy_query(query="shape",shape="WorkflowStepExecutionId")`), which a refusal of that
/// member quotes as the model writes it: there is no partition or Region field in it to
/// restate.
const STEP_EXECUTION_ID_PATTERN: &str =
    "^step-[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";

/// A component document with one build phase, so a staged component is created for its own
/// sake.
const BUILD_DOCUMENT: &str = concat!(
    "name: Probe\n",
    "schemaVersion: 1.0\n",
    "phases:\n",
    "  - name: build\n",
    "    steps:\n",
    "      - name: run\n",
    "        action: ExecuteBash\n",
    "        inputs:\n",
    "          commands:\n",
    "            - echo hi\n",
);

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// Status and parsed body of one call, signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The error shape a refusal names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence a refusal states.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The steps one listed page carries.
fn listed_steps(case: &str, body: &Value) -> Vec<Value> {
    body.get("steps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a steps list, got {body}"))
        .clone()
}

/// A well-formed image build version ARN, which every `SendWorkflowStepAction` row below
/// carries so the refusal it earns is about its step execution id.
fn image_build_version_arn() -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/probe-image/1.0.0/1")
}

/// A `SendWorkflowStepAction` body naming `step_execution_id`.
fn action_body(step_execution_id: &str, token: &str) -> Value {
    json!({
        "stepExecutionId": step_execution_id,
        "imageBuildVersionArn": image_build_version_arn(),
        "action": "RESUME",
        "clientToken": token,
    })
}

/// Create one configuration, one component and one recipe, so the listing is read against a
/// store that holds resources rather than against an empty one.
async fn stage_resources(router: &MultiServiceRouter) {
    for (what, uri, body) in [
        (
            "an infrastructure configuration",
            "/CreateInfrastructureConfiguration",
            json!({
                "name": "waiting-steps-config",
                "instanceProfileName": "EC2InstanceProfileForImageBuilder",
                "clientToken": "wstok-0001",
            }),
        ),
        (
            "a component",
            "/CreateComponent",
            json!({
                "name": "waiting-steps-component",
                "semanticVersion": "1.0.0",
                "platform": "Linux",
                "clientToken": "wstok-0002",
                "data": BUILD_DOCUMENT,
            }),
        ),
        (
            "an image recipe",
            "/CreateImageRecipe",
            json!({
                "name": "waiting-steps-recipe",
                "semanticVersion": "1.0.0",
                "parentImage": "ami-0123456789abcdef0",
                "clientToken": "wstok-0003",
            }),
        ),
    ] {
        let (status, created) = call(router, "PUT", uri, Some(&body)).await;
        assert_eq!(
            status, 200,
            "setup: creating {what} must succeed, got {status} with {created}"
        );
    }
}

#[tokio::test]
async fn an_account_whose_workflows_never_paused_has_no_waiting_step() {
    let router = emulator();

    // Nothing has been created yet, and then three resources have been: neither is a
    // workflow run that paused, so the listing reports no step either way — at every page
    // size the modelled range admits at its ends and in between.
    for (case, staged) in [("an empty store", false), ("a store holding resources", true)] {
        if staged {
            stage_resources(&router).await;
        }
        for body in [
            json!({}),
            json!({ "maxResults": 1 }),
            json!({ "maxResults": 25 }),
            json!({ "maxResults": 7 }),
        ] {
            let case = format!("{case} read with {body}");
            let (status, listed) =
                call(&router, "POST", "/ListWaitingWorkflowSteps", Some(&body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {listed}"
            );
            let steps = listed_steps(&case, &listed);
            assert!(
                steps.is_empty(),
                "case {case:?}: expected no waiting step, got {steps:?}"
            );
            assert!(
                listed.get("nextToken").is_none_or(Value::is_null),
                "case {case:?}: a complete page must carry no nextToken, got {listed}"
            );
        }
    }

    // A token this store did not issue is not a position in the listing.
    let (status, refused) = call(
        &router,
        "POST",
        "/ListWaitingWorkflowSteps",
        Some(&json!({ "nextToken": "not-a-token-this-store-issued" })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a token this store did not issue\": expected HTTP 400, got {status} with {refused}"
    );
    let code = error_code("a token this store did not issue", &refused);
    assert_eq!(
        code, INVALID_PARAMETER_VALUE,
        "case \"a token this store did not issue\": expected {INVALID_PARAMETER_VALUE:?}, got \
         {code:?}"
    );
    let expected = "The value supplied for parameter 'nextToken' is not valid. The provided token \
                    does not meet the required format.";
    let actual = error_message(&refused);
    assert_eq!(
        actual, expected,
        "case \"a token this store did not issue\": expected {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn a_step_execution_id_outside_its_pattern_is_refused_as_that_parameters_value() {
    let router = emulator();

    // Each row misses the modelled pattern a different way; all of them are refused as the
    // value of the member they arrived in, with the expression quoted as the model writes it.
    let rows = [
        ("the hex digits with no dashes", "step-0123456789abcdef0123456789abcdef"),
        ("no step- prefix", "0123abcd-ef01-2345-6789-abcdef012345"),
        ("another prefix", "workflow-0123abcd-ef01-2345-6789-abcdef012345"),
        ("too few hex digits in the first group", "step-0123abc-ef01-2345-6789-abcdef012345"),
        ("too many hex digits in the last group", "step-0123abcd-ef01-2345-6789-abcdef0123456"),
        ("a character that is not hex", "step-0123abcg-ef01-2345-6789-abcdef012345"),
        ("trailing text after a well-formed id", "step-0123abcd-ef01-2345-6789-abcdef012345x"),
        ("underscores for its dashes", "step-0123abcd_ef01_2345_6789_abcdef012345"),
        ("the prefix alone", "step-"),
    ];
    for (row, (case, step_execution_id)) in rows.into_iter().enumerate() {
        let (status, refused) = call(
            &router,
            "PUT",
            "/SendWorkflowStepAction",
            Some(&action_body(step_execution_id, &format!("sstok-{row}"))),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {step_execution_id:?}, got {status} with \
             {refused}"
        );
        let code = error_code(case, &refused);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the error code {INVALID_PARAMETER_VALUE:?}, got {code:?}"
        );
        let expected = format!(
            "The value supplied for parameter 'stepExecutionId' is not valid. stepExecutionId \
             must match pattern {STEP_EXECUTION_ID_PATTERN}"
        );
        let actual = error_message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }

    // An id that DOES match the pattern is past that refusal: whatever this service answers
    // for a step it holds nothing about, it is not the parameter-value refusal above.
    for (case, step_execution_id) in [
        ("lowercase hex", "step-0123abcd-ef01-2345-6789-abcdef012345"),
        ("uppercase hex, which the pattern admits too", "step-0123ABCD-EF01-2345-6789-ABCDEF012345"),
    ] {
        let (_, answered) = call(
            &router,
            "PUT",
            "/SendWorkflowStepAction",
            Some(&action_body(step_execution_id, "sstok-wellformed")),
        )
        .await;
        let message = error_message(&answered);
        assert!(
            !message.contains("stepExecutionId"),
            "case {case:?}: a well-formed id must be past the parameter refusal, got {answered}"
        );
    }
}
