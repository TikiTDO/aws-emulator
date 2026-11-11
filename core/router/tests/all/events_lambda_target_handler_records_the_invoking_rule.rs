// SPDX-License-Identifier: CC0-1.0

//! What an EventBridge rule's Lambda target can RECORD depends on the services this router
//! mounts: a service token the AWS SDK a running handler imports answers to has to name a
//! service THIS emulator serves — asserted against the production service set
//! (`aws_emulator_router::build_router` on one engine, exactly as the entrypoint assembles it).
//!
//! The rule pinned here: the call seam a handler's SDK stands on dispatches into this same
//! process (`service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/aws_api.rs`), so
//! a token that seam binds requests for has to be answered by the service that owns those
//! requests: a token the SDK answers to but the router mounts nothing for would leave a
//! handler's call reaching the router's own `404 UnknownService` instead of a service — and a
//! target's handler that cannot reach the service it records its work in records nothing,
//! whatever the rule and the permission did. Each row therefore asserts both halves — that the
//! runtime's SDK catalog names the token, and that a request bound as that seam binds it is
//! answered by the SERVICE, with that service's own modelled refusal for the resource the row
//! asks about.
//!
//! The rows differ along which service of the seam is named: the configuration store a handler
//! records its work in, the queue it writes to and the bus it publishes on — three of the
//! services the seam binds, each asked for a resource that is not there, because the refusal for
//! an absent resource is an answer only that service can give. What the recording itself then
//! carries is pinned where the rule's own decision is, in
//! `services/service-com-amazonaws-eventbridge-awsevents/tests/all/lambda_target_handler_records_the_invoking_rule.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::aws_api::reachable_services;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The error code the ROUTER answers when no mounted service claims a request, which is what
/// no row may see.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The emulator as it is ASSEMBLED, on one engine: the same registrations the entrypoint
/// serves, so a token answered here is answered by the deployed emulator too.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // The suite signs nothing: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router
}

/// One row: the SDK token a handler builds its client with, the wire facts the call seam binds
/// its requests by, the call it makes and the refusal that service answers it with.
struct Row {
    case: &'static str,
    /// The token `boto3.client(...)` is given, which the runtime's SDK catalog has to name.
    token: &'static str,
    /// The service's SigV4 signing name, which is the credential scope that routes the call.
    signing_name: &'static str,
    /// The leading label of its endpoint host.
    endpoint_prefix: &'static str,
    /// The `X-Amz-Target` prefix and the AWS JSON version its protocol names.
    target_prefix: &'static str,
    json_version: &'static str,
    operation: &'static str,
    input: fn() -> Value,
    /// The error code that service answers for the absent resource the row asks about.
    refused_with: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the configuration store a handler records its work in",
            token: "ssm",
            signing_name: "ssm",
            endpoint_prefix: "ssm",
            target_prefix: "AmazonSSM",
            json_version: "1.1",
            operation: "GetParameter",
            input: || json!({ "Name": "/emulator/no-such-parameter" }),
            refused_with: "ParameterNotFound",
        },
        Row {
            case: "the queue a handler writes to",
            token: "sqs",
            signing_name: "sqs",
            endpoint_prefix: "sqs",
            target_prefix: "AmazonSQS",
            json_version: "1.0",
            operation: "GetQueueUrl",
            input: || json!({ "QueueName": "no-such-queue" }),
            refused_with: "QueueDoesNotExist",
        },
        Row {
            case: "the bus a handler publishes on",
            token: "events",
            signing_name: "events",
            endpoint_prefix: "events",
            target_prefix: "AWSEvents",
            json_version: "1.1",
            operation: "DescribeRule",
            input: || json!({ "Name": "no-such-rule" }),
            refused_with: "ResourceNotFoundException",
        },
    ]
}

/// RULE: a token the runtime's SDK answers to names a service this router mounts, so the call a
/// handler makes is answered by that service — over the three services these rows name.
#[tokio::test]
async fn each_service_token_these_rows_name_is_mounted_by_this_router() {
    let router = router();
    let catalog = reachable_services();

    for row in rows() {
        let case = row.case;
        assert!(
            catalog.contains(&row.token),
            "case {case:?}: expected the runtime's SDK catalog to name {:?}, got {catalog:?}",
            row.token
        );

        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header(
                "host",
                format!("{}.{REGION}.amazonaws.com", row.endpoint_prefix),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{}/aws4_request, \
                     SignedHeaders=host, Signature=not-verified",
                    row.signing_name
                ),
            )
            .header(
                "content-type",
                format!("application/x-amz-json-{}", row.json_version),
            )
            .header(
                "x-amz-target",
                format!("{}.{}", row.target_prefix, row.operation),
            )
            .body(Bytes::from((row.input)().to_string()))
            .expect("the test request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        assert!(
            !code.contains(UNKNOWN_SERVICE),
            "case {case:?}: expected a mounted service to claim the call, got {status} with \
             {parsed}"
        );
        assert!(
            code.ends_with(row.refused_with),
            "case {case:?}: expected the service's own {:?} for the absent resource, got \
             {status} with {parsed}",
            row.refused_with
        );
    }
}
