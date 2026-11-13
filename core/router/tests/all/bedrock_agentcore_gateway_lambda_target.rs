// SPDX-License-Identifier: CC0-1.0

//! A gateway target whose MCP tool handler is a Lambda function, across the two services
//! that own the two halves: AgentCore Control writes the target, Lambda owns the function.
//!
//! The rule pinned here: `CreateGatewayTarget` resolves a Lambda MCP target's function
//! against Lambda when the target is WRITTEN — the gateway has to invoke that function to
//! serve a tool call — so a target naming a function that does not exist is refused with
//! the operation's modelled validation error and nothing is persisted, while a target
//! naming one that does exist is written and reads back.
//!
//! The refusal IDENTIFIES THE MISSING RESOURCE: its message is the fixed two sentences
//! around the function ARN the request supplied, verbatim, so a caller can read which
//! function is absent out of it. `UpdateGatewayTarget` resolves the same reference for the
//! same reason and words the refusal the same way.
//!
//! Rows span what the rule generalises over: whether the function exists, which Region
//! and name its ARN carries, a qualified (`:version`) ARN, and target shapes that name no
//! function at all — for which Lambda is not consulted and nothing may be refused.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the Region the gateway's ARNs carry.
const REGION: &str = "us-east-1";

/// The settle span AgentCore Control gives a gateway's clock-owed `CREATING` -> `READY`
/// transition (`state::GATEWAY_SETTLE_DELAY`), plus slack. A gateway takes no target
/// until that transition is due, so every row here waits it out before adding one.
const PAST_GATEWAY_SETTLE: Duration = Duration::from_millis(900);

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN inside `RoleArn`'s pattern, for both services.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/mcp-exec";

/// A deployment package as `Code.ZipFile` carries it: base64 in a `restJson1` JSON body.
/// The content is irrelevant to this suite — only that Lambda accepts it and stores a
/// function — so it is a fixed, valid base64 constant rather than a built archive.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// AgentCore Control and Lambda over ONE shared engine, as `router/src/lib.rs` mounts
/// them, so the control plane's in-process callback reaches this very Lambda.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// One call to the service whose signing name is `signing_name`, at `uri`.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The `restJson1` error code (`__type`), or the empty string on a response with none.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// How a refusal of a target naming a function Lambda does not hold reads: fixed text
/// around the `lambdaArn` the request supplied, which is the only variable part.
fn function_not_found(lambda_arn: &str) -> String {
    format!("Lambda function not found: {lambda_arn}. Verify the function ARN is correct and retry.")
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create a Lambda function and return the ARN Lambda minted for it.
async fn create_function(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "lambda",
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE_ARN,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": {"ZipFile": ZIP_FILE},
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    let arn = text(&created, "FunctionArn");
    assert_eq!(
        arn,
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        "case {name:?}: expected the modelled function ARN, got {arn:?}"
    );
    arn
}

/// Create a gateway and return its `gatewayId`, once the gateway is READY to take a
/// target.
async fn create_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "bedrock-agentcore",
        "POST",
        "/gateways/",
        json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"}),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
    );
    tokio::time::sleep(PAST_GATEWAY_SETTLE).await;
    text(&created, "gatewayId")
}

/// An MCP Lambda target body naming `lambda_arn`.
fn lambda_target(name: &str, lambda_arn: &str) -> Value {
    json!({
        "name": name,
        "targetConfiguration": {
            "mcp": {
                "lambda": {
                    "lambdaArn": lambda_arn,
                    "toolSchema": {
                        "inlinePayload": [{
                            "name": "echo",
                            "description": "echo a string back",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                }
            }
        }
    })
}

async fn create_target(
    router: &MultiServiceRouter,
    gateway_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "bedrock-agentcore",
        "POST",
        &format!("/gateways/{gateway_id}/targets/"),
        body,
    )
    .await
}

/// Reconfigure a stored target. The body carries the credential provider the operation
/// reports as required, so the row is judged on the configuration it sends.
async fn update_target(
    router: &MultiServiceRouter,
    gateway_id: &str,
    target_id: &str,
    mut body: Value,
) -> (u16, Value) {
    body["credentialProviderConfigurations"] =
        json!([{"credentialProviderType": "GATEWAY_IAM_ROLE"}]);
    call(
        router,
        "bedrock-agentcore",
        "PUT",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        body,
    )
    .await
}

#[tokio::test]
async fn a_gateway_target_requires_the_lambda_function_it_names_to_exist() {
    let router = router();
    let gateway_id = create_gateway(&router, "mcpgateway").await;

    // The functions this suite resolves against, one per Region/name shape.
    let present = create_function(&router, "mcp-tools").await;
    let other_name = create_function(&router, "mcp-tools-v2").await;

    // (case, lambdaArn, expected status, expected error code)
    //
    // Every row is the SAME request but for the function ARN, so the function's
    // existence is the only thing that can decide the answer.
    let rows: [(&str, String, u16, &str); 8] = [
        ("a function that exists", present.clone(), 202, ""),
        ("another function that exists", other_name, 202, ""),
        (
            "the same function, qualified with $LATEST",
            format!("{present}:$LATEST"),
            202,
            "",
        ),
        (
            // A version qualifier Lambda has not published: Lambda reports no such
            // function, so the target is refused for the same grounded reason as a
            // name that was never created.
            "the same function, qualified with a version nothing published",
            format!("{present}:7"),
            400,
            "ValidationException",
        ),
        (
            "a function name no function has",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:not-created"),
            400,
            "ValidationException",
        ),
        (
            // The ARN's Region chooses which Lambda endpoint is asked. This emulator's
            // Lambda holds one store rather than one per Region, so the same name is
            // found through either endpoint; what decides is whether a function of that
            // name was created at all.
            "the same function name through another Region's endpoint",
            format!("arn:aws:lambda:eu-west-1:{ACCOUNT}:function:mcp-tools"),
            202,
            "",
        ),
        (
            "a name nothing created, through another Region's endpoint",
            format!("arn:aws:lambda:eu-west-1:{ACCOUNT}:function:absent-elsewhere"),
            400,
            "ValidationException",
        ),
        (
            "a function whose name differs by one character",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:mcp-tool"),
            400,
            "ValidationException",
        ),
    ];

    for (index, (case, lambda_arn, expected_status, expected_code)) in
        rows.into_iter().enumerate()
    {
        let body = lambda_target(&format!("target{index}"), &lambda_arn);
        let (status, answer) = create_target(&router, &gateway_id, body).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (expected_status, expected_code),
            "case {case:?} ({lambda_arn}): expected HTTP {expected_status} {expected_code:?}, got \
             {status} {} ({answer})",
            error_code(&answer)
        );
        if !expected_code.is_empty() {
            // The refusal names the function that is missing, by the ARN the row sent.
            let expected_message = function_not_found(&lambda_arn);
            assert_eq!(
                text(&answer, "message"),
                expected_message,
                "case {case:?} ({lambda_arn}): expected the refusal {expected_message:?}, got {}",
                text(&answer, "message")
            );
        }
    }
}

/// `UpdateGatewayTarget` resolves the function a new configuration names against Lambda
/// too, and refuses an absent one in the same words — and a configuration naming a
/// function that is there is written.
#[tokio::test]
async fn reconfiguring_a_target_requires_the_lambda_function_it_names_to_exist() {
    let router = router();
    let gateway_id = create_gateway(&router, "reconfigure").await;
    let present = create_function(&router, "mcp-update").await;

    let (status, created) = create_target(
        &router,
        &gateway_id,
        lambda_target("updatable", &present),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: expected HTTP 202 from CreateGatewayTarget, got {status} ({created})"
    );
    let target_id = text(&created, "targetId");

    // (case, the lambdaArn the update sends) — every ARN here names no function.
    let absent: [(&str, String); 3] = [
        (
            "a function name nothing created",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:never-updated"),
        ),
        (
            "the stored function qualified with an unpublished version",
            format!("{present}:12"),
        ),
        (
            "a name nothing created, through another Region's endpoint",
            format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:function:absent-elsewhere"),
        ),
    ];

    for (case, lambda_arn) in absent {
        let (status, refused) = update_target(
            &router,
            &gateway_id,
            &target_id,
            json!({"targetConfiguration": lambda_target("updatable", &lambda_arn)
                ["targetConfiguration"]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({lambda_arn}): expected HTTP 400 ValidationException, got {status} {} \
             ({refused})",
            error_code(&refused)
        );
        let expected_message = function_not_found(&lambda_arn);
        assert_eq!(
            text(&refused, "message"),
            expected_message,
            "case {case:?} ({lambda_arn}): expected the refusal {expected_message:?}, got {}",
            text(&refused, "message")
        );
    }

    // The stored configuration survived every refusal, and a function that IS there is
    // accepted.
    let (status, fetched) = call(
        &router,
        "bedrock-agentcore",
        "GET",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetGatewayTarget after the refused updates\": expected HTTP 200, got {status} \
         ({fetched})"
    );
    assert_eq!(
        fetched["targetConfiguration"], created["targetConfiguration"],
        "case \"GetGatewayTarget after the refused updates\": the stored configuration must be \
         the one created, got {} for {}",
        fetched["targetConfiguration"], created["targetConfiguration"]
    );

    let other = create_function(&router, "mcp-update-v2").await;
    let (status, updated) = update_target(
        &router,
        &gateway_id,
        &target_id,
        json!({"targetConfiguration": lambda_target("updatable", &other)["targetConfiguration"]}),
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"an update naming a function that exists\": expected HTTP 202, got {status} \
         ({updated})"
    );
}

/// A refused target is not persisted, and an accepted one reads back.
#[tokio::test]
async fn only_an_accepted_lambda_target_is_persisted() {
    let router = router();
    let gateway_id = create_gateway(&router, "roundtrip").await;
    let lambda_arn = create_function(&router, "mcp-roundtrip").await;

    // The refusal first: nothing it names may survive it.
    let (status, refused) = create_target(
        &router,
        &gateway_id,
        lambda_target(
            "missing-function",
            &format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:never-created"),
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a target naming a missing function\": expected HTTP 400 ValidationException, got \
         {status} {} ({refused})",
        error_code(&refused)
    );
    let absent_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:never-created");
    let expected_message = function_not_found(&absent_arn);
    assert_eq!(
        text(&refused, "message"),
        expected_message,
        "case \"a target naming a missing function\": expected the refusal \
         {expected_message:?}, got {}",
        text(&refused, "message")
    );
    // The member whose value failed is still reported, where the shape declares a member
    // complaint belongs: the `fieldList`, not the message.
    assert_eq!(
        refused["fieldList"][0]["name"],
        json!("targetConfiguration.mcp.lambda.lambdaArn"),
        "case \"a target naming a missing function\": expected the failing member path in the \
         fieldList, got {}",
        refused["fieldList"]
    );

    let (status, accepted) = create_target(
        &router,
        &gateway_id,
        lambda_target("present-function", &lambda_arn),
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"a target naming a function that exists\": expected HTTP 202, got {status} \
         ({accepted})"
    );
    let target_id = text(&accepted, "targetId");

    // The gateway holds exactly the accepted target: the read of the refused one is the
    // modelled 404, and the accepted one reads back as it was stored.
    let (status, fetched) = call(
        &router,
        "bedrock-agentcore",
        "GET",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetGatewayTarget after CreateGatewayTarget\": expected HTTP 200, got {status} \
         ({fetched})"
    );
    assert_eq!(
        fetched["targetConfiguration"], accepted["targetConfiguration"],
        "case \"GetGatewayTarget after CreateGatewayTarget\": the target read back must be the \
         one created, got {} for {}",
        fetched["targetConfiguration"], accepted["targetConfiguration"]
    );
    assert_eq!(
        text(&fetched, "name"),
        "present-function",
        "case \"GetGatewayTarget after CreateGatewayTarget\": expected the stored name, got {}",
        text(&fetched, "name")
    );
}

/// A target shape that names no Lambda function is not resolved against Lambda at all, so
/// it is written whatever Lambda holds.
#[tokio::test]
async fn a_target_that_names_no_function_is_not_resolved_against_lambda() {
    let router = router();
    let gateway_id = create_gateway(&router, "nolambda").await;

    // (case, body) — one row per non-Lambda target shape the model declares.
    let rows: [(&str, Value); 3] = [
        (
            "an MCP target with an inline OpenAPI schema",
            json!({
                "name": "openapi",
                "targetConfiguration": {
                    "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
                }
            }),
        ),
        (
            "an MCP server endpoint target",
            json!({
                "name": "mcpserver",
                "targetConfiguration": {
                    "mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}
                }
            }),
        ),
        (
            "an MCP target with an inline Smithy model",
            json!({
                "name": "smithymodel",
                "targetConfiguration": {
                    "mcp": {"smithyModel": {"inlinePayload": "{\"smithy\":\"2.0\"}"}}
                }
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, answer) = create_target(&router, &gateway_id, body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 (no function to resolve), got {status} ({answer})"
        );
    }
}
