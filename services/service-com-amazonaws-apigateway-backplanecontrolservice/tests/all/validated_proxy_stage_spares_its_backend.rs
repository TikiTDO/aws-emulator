// SPDX-License-Identifier: CC0-1.0

//! What a deployed method's `RequestValidator` does to a request whose integration names a
//! REAL backend: the whole path from the stage invoke URL through the `AWS_PROXY` Lambda
//! function to the item that function writes, across API Gateway, AWS Lambda and Amazon
//! DynamoDB on one engine.
//!
//! RULE: a deployed method whose `requestValidatorId` names a `RequestValidator` whose
//! `validateRequestBody` is set refuses a payload the `Model` the method's `requestModels`
//! maps to the request's content type does not admit — HTTP 400 with the `BAD_REQUEST_BODY`
//! gateway response's own body, `{"message": "Invalid request body"}`, a sentence with no
//! variable part — and it refuses it BEFORE the method's integration is performed, so the
//! backend that integration names does no work for that request at all; a payload that
//! content type's model admits, and a request whose content type the method maps no model
//! to, reach the integration, and the method response is the backend's own.
//!
//! The rows differ along what the rule generalises over: the WAY a payload fails the model
//! (a required property absent, every required property absent, a property of the wrong
//! type, a number below the declared bound, a property the schema does not declare, a
//! document that is not JSON, an empty document, a JSON value that is not an object), the
//! CONTENT TYPE the request declares (one the method maps a model to and one it does not),
//! and the validator's own MODE (a body-validating one and a parameters-only one, on two
//! routes of the same api in front of the same function).
//!
//! Only a suite that mounts all three services can pin the second half of that rule: the
//! refusal's status and body are API Gateway's, but "the backend did no work" is only
//! observable where that work would be — the DynamoDB item the function writes under the key
//! each request names, absent for every refused request and present for every served one,
//! re-read once more after the whole table has run so a write that merely arrived late would
//! still be caught.
//!
//! The wire harness and the `Model` schema are the sibling `proxy_binary_media_types` and
//! `stage_request_validation` suites', reused rather than stood up a second way; only the
//! emulator is this suite's own, because the rule needs DynamoDB mounted beside the other
//! two.

use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::proxy_binary_media_types::{
    apigw, authorization, dispatch, lambda, member, package, wait_until_active, Answer,
    FUNCTION_TIMEOUT, HANDLER, REGION, ROLE,
};
use crate::stage_request_validation::{order_schema, GOOD_BODY, JSON, STAGE};

/// The function both routes of the api are integrated with.
const FUNCTION: &str = "apigw-validated-proxy";

/// The table the handler writes, and the attribute its key is.
const TABLE: &str = "validated-proxy-marks";
const KEY: &str = "pk";

/// The model both methods declare for [`JSON`] payloads.
const MODEL: &str = "Order";

/// A content type the methods map NO model to, so there is nothing for the body half of a
/// validator to check a request sent under it against.
const UNMODELLED_CONTENT_TYPE: &str = "text/plain";

/// The header each request names its own key by, so one request's work is told from another's.
const CASE_HEADER: &str = "x-case";

/// The body an uncustomized `BAD_REQUEST_BODY` gateway response carries, verbatim: the
/// validator's own serialization, which writes a space after the member separator.
const INVALID_BODY: &str = r#"{"message": "Invalid request body"}"#;

/// The error shape a refusal through that gateway response names itself by.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// How long to wait between polls of another service's clock-owed transition, and how many
/// polls to make. Generous on purpose: nothing here is timed, the condition is.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 800;

/// The handler every served request runs: it writes ONE item keyed by the request's own
/// [`CASE_HEADER`] value, holding the body it was handed, and answers the proxy envelope the
/// client is served. An item under a key is therefore evidence that the function ran for that
/// request, and its absence that it did not.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    headers = event.get("headers") or {}
    marked = headers.get("X-Case") or headers.get("x-case") or "no-case"
    body = event.get("body")
    boto3.client("dynamodb").put_item(
        TableName=os.environ["TABLE"],
        Item={"pk": {"S": marked}, "body": {"S": body if body is not None else ""}},
    )
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"stored": marked}),
    }
"#;

/// API Gateway, AWS Lambda and Amazon DynamoDB over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the function the integration invokes and the table
/// that function writes are the ones this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a control-plane request, and an invoke of the published
    // stage URL carries no credential at all, as a client of a public method sends none.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// One Amazon DynamoDB call over its `awsJson1_0` wire.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    dispatch(
        router,
        "POST",
        &format!("dynamodb.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &format!("DynamoDB_20120810.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// Poll `DescribeTable` until DynamoDB first reports the table `ACTIVE`, which is when it
/// serves the writes the handler makes ("you can perform read and write operations only on an
/// ACTIVE table"). The span a created table spends `CREATING` is that service's own, so
/// nothing here sleeps a fixed span on its behalf.
async fn wait_until_table_active(router: &MultiServiceRouter) {
    let mut last = Value::Null;
    for _ in 0..POLLS {
        let described = dynamodb(router, "DescribeTable", json!({ "TableName": TABLE })).await;
        last = described.json();
        if last.pointer("/Table/TableStatus").and_then(Value::as_str) == Some("ACTIVE") {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup CreateTable: the table never became ACTIVE, last read {last}");
}

/// The item the handler wrote under `key`, or `None` when nothing is stored under it — which
/// is what a request whose integration was never performed leaves behind.
async fn stored_item(router: &MultiServiceRouter, case: &str, key: &str) -> Option<Value> {
    let read = dynamodb(
        router,
        "GetItem",
        json!({
            "TableName": TABLE,
            "Key": { KEY: { "S": key } },
            // "Determines the read consistency model": a strongly consistent read, so what is
            // asserted is what the table holds at this instant rather than a stale copy.
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: expected HTTP 200 from GetItem for {key:?}, got {} with {:?}",
        read.status,
        read.text()
    );
    read.json().get("Item").cloned()
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com` — carrying no credential at all.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    route: &str,
    content_type: &str,
    key: &str,
    payload: &str,
) -> Answer {
    dispatch(
        router,
        "POST",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}/{route}"),
        &[("content-type", content_type), (CASE_HEADER, key)],
        Bytes::from(payload.to_owned()),
    )
    .await
}

/// One route of the api: the validator mode its method declares, and whether that mode
/// checks the request's BODY.
struct Route {
    route: &'static str,
    /// The mode the route's `RequestValidator` declares, as
    /// `(validateRequestBody, validateRequestParameters)`.
    validates: (bool, bool),
}

/// The two routes, differing in nothing but the validator their method names: both declare
/// [`MODEL`] for [`JSON`] and both are integrated with the same function, so what separates
/// them is the mode alone.
const ROUTES: &[Route] = &[
    Route {
        route: "validated",
        validates: (true, false),
    },
    Route {
        route: "unvalidated",
        validates: (false, true),
    },
];

/// One request under test: what it sends, and whether the BODY half of a validator admits it.
struct Row {
    case: &'static str,
    content_type: &'static str,
    payload: &'static str,
    /// Whether a body-validating validator admits this request — either because the payload
    /// satisfies the model, or because the method maps no model to the content type it was
    /// sent under, so the body half has nothing to check it against.
    admitted: bool,
}

/// The rows, spanning the way a payload meets or fails [`order_schema`] and the content type
/// it is sent under.
const ROWS: &[Row] = &[
    Row {
        case: "a payload the model admits",
        content_type: JSON,
        payload: GOOD_BODY,
        admitted: true,
    },
    Row {
        // The schema's `minimum` is 1, so this is the lowest value the bound admits.
        case: "a payload at the declared minimum",
        content_type: JSON,
        payload: r#"{"id":"at-the-bound","qty":1}"#,
        admitted: true,
    },
    Row {
        case: "a required property absent",
        content_type: JSON,
        payload: r#"{"id":"no-qty"}"#,
        admitted: false,
    },
    Row {
        case: "every required property absent",
        content_type: JSON,
        payload: "{}",
        admitted: false,
    },
    Row {
        case: "a property of the wrong type",
        content_type: JSON,
        payload: r#"{"id":"wrong-type","qty":"two"}"#,
        admitted: false,
    },
    Row {
        case: "a number below the declared minimum",
        content_type: JSON,
        payload: r#"{"id":"below-the-bound","qty":0}"#,
        admitted: false,
    },
    Row {
        case: "a property the schema does not declare",
        content_type: JSON,
        payload: r#"{"id":"undeclared","qty":1,"extra":true}"#,
        admitted: false,
    },
    Row {
        case: "a payload that is not a JSON document",
        content_type: JSON,
        payload: "not json at all",
        admitted: false,
    },
    Row {
        case: "an empty payload",
        content_type: JSON,
        payload: "",
        admitted: false,
    },
    Row {
        case: "a JSON value that is not an object",
        content_type: JSON,
        payload: "[1,2,3]",
        admitted: false,
    },
    Row {
        // The method maps a model to `application/json` alone, so a request sent under
        // another content type has no schema to be checked against, however the JSON model
        // would have read it.
        case: "a payload under a content type the method maps no model to",
        content_type: UNMODELLED_CONTENT_TYPE,
        payload: "not json at all",
        admitted: true,
    },
];

/// Stand the whole estate up: the table, the function that writes it, the grant that admits
/// the invocation, and the api carrying one route per [`ROUTES`] entry in front of that
/// function, deployed to [`STAGE`]. Answers the api id.
async fn deployed(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{ "AttributeName": KEY, "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": KEY, "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "setup CreateTable: expected HTTP 200, got {} with {:?}",
        created.status,
        created.text()
    );
    wait_until_table_active(router).await;

    let function = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": { "Variables": { "TABLE": TABLE } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        function.status,
        201,
        "setup CreateFunction: expected HTTP 201, got {} with {:?}",
        function.status,
        function.text()
    );
    let function_arn = member("CreateFunction", &function.json(), "FunctionArn");
    wait_until_active(router, FUNCTION).await;

    // "Grants a principal permission to use a function" (`AddPermission`): the grant that
    // admits the invocation API Gateway makes. It names no `SourceArn`, so every method of
    // the api below is covered and what a row observes is the validator's decision alone.
    let granted = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigateway-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
        })),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "setup AddPermission: expected HTTP 201, got {} with {:?}",
        granted.status,
        granted.text()
    );

    let api = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "validated-proxy-stage" })),
        201,
    )
    .await;
    let root = member("CreateRestApi", &api, "rootResourceId");
    let api = member("CreateRestApi", &api, "id");

    // The model both methods declare. "For application/json models, this should be JSON
    // schema draft 4 model" (`CreateModel$schema`), which is the dialect `order_schema`
    // declares.
    apigw(
        router,
        "CreateModel",
        "POST",
        &format!("/restapis/{api}/models"),
        Some(json!({
            "name": MODEL,
            "contentType": JSON,
            "schema": order_schema().to_string(),
        })),
        201,
    )
    .await;
    // The record the validator consumes is the one `CreateModel` stored: read back before any
    // invoke, so a route that refuses a payload is known to refuse it against that schema.
    let stored = apigw(
        router,
        "GetModel",
        "GET",
        &format!("/restapis/{api}/models/{MODEL}"),
        None,
        200,
    )
    .await;
    assert_eq!(
        member("GetModel", &stored, "schema"),
        order_schema().to_string(),
        "setup GetModel: expected the stored schema verbatim, got {stored}"
    );

    for route in ROUTES {
        let validator = apigw(
            router,
            "CreateRequestValidator",
            "POST",
            &format!("/restapis/{api}/requestvalidators"),
            Some(json!({
                "name": route.route,
                "validateRequestBody": route.validates.0,
                "validateRequestParameters": route.validates.1,
            })),
            201,
        )
        .await;
        let validator = member("CreateRequestValidator", &validator, "id");
        let resource = apigw(
            router,
            "CreateResource",
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": route.route })),
            201,
        )
        .await;
        let resource = member("CreateResource", &resource, "id");
        apigw(
            router,
            "PutMethod",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/POST"),
            Some(json!({
                "authorizationType": "NONE",
                "requestModels": { JSON: MODEL },
                "requestValidatorId": validator,
            })),
            201,
        )
        .await;
        apigw(
            router,
            "PutIntegration",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/POST/integration"),
            Some(json!({
                "type": "AWS_PROXY",
                // `integrationHttpMethod` travels under its `jsonName`, which is what
                // `smithy_query(query="operation",operation="PutIntegration")` reports.
                "httpMethod": "POST",
                "uri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {function_arn}/invocations"
                ),
            })),
            201,
        )
        .await;
    }

    apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    api
}

#[tokio::test]
async fn a_body_validating_stage_refuses_a_bad_payload_before_its_lambda_backend_runs() {
    let router = emulator();
    let api = deployed(&router).await;

    // What every request left behind, as `(case, key, whether the function must have run)`,
    // swept once more after the whole table has run.
    let mut expected: Vec<(String, String, bool)> = Vec::new();

    for route in ROUTES {
        for (index, row) in ROWS.iter().enumerate() {
            let refused = route.validates.0 && !row.admitted;
            let case = format!(
                "the {} route with {} (validateRequestBody={})",
                route.route, row.case, route.validates.0
            );
            let key = format!("{}-{index}", route.route);
            let answered = invoke(
                &router,
                &api,
                route.route,
                row.content_type,
                &key,
                row.payload,
            )
            .await;

            if refused {
                assert_eq!(
                    answered.status,
                    400,
                    "case {case:?}: expected the validator's 400, got {} with {:?}",
                    answered.status,
                    answered.text()
                );
                assert_eq!(
                    answered.text(),
                    INVALID_BODY,
                    "case {case:?}: expected the BAD_REQUEST_BODY body verbatim, got {:?}",
                    answered.text()
                );
                assert_eq!(
                    answered.header("x-amzn-errortype").as_deref(),
                    Some(BAD_REQUEST_EXCEPTION),
                    "case {case:?}: expected the refusal to name its error shape, got {:?}",
                    answered.header("x-amzn-errortype")
                );
                // The integration was never performed, so the backend did none of the work a
                // served request makes it do.
                assert_eq!(
                    stored_item(&router, &case, &key).await,
                    None,
                    "case {case:?}: expected the refused request to leave no item under \
                     {key:?}, got one"
                );
            } else {
                assert_eq!(
                    answered.status,
                    200,
                    "case {case:?}: expected the function's own 200, got {} with {:?}",
                    answered.status,
                    answered.text()
                );
                assert_eq!(
                    answered.json(),
                    json!({ "stored": key }),
                    "case {case:?}: expected the handler's own result for {key:?}, got {:?}",
                    answered.text()
                );
                assert_eq!(
                    stored_item(&router, &case, &key).await,
                    Some(json!({ KEY: { "S": key }, "body": { "S": row.payload } })),
                    "case {case:?}: expected the table to hold what the handler wrote under \
                     {key:?}"
                );
            }
            expected.push((case, key, !refused));
        }
    }

    // Every refused request is still backed by nothing, and every served one by the item its
    // run wrote: a write that arrived after the answer did would show up here.
    for (case, key, ran) in &expected {
        let held = stored_item(&router, case, key).await;
        assert_eq!(
            held.is_some(),
            *ran,
            "case {case:?}: after the whole table, expected the item under {key:?} to be \
             {}, got {held:?}",
            if *ran { "held" } else { "absent" }
        );
    }
}
