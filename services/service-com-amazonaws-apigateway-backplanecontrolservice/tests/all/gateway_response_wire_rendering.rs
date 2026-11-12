// SPDX-License-Identifier: CC0-1.0

//! What the `execute-api` edge SENDS for a refused request: the gateway response's status,
//! its headers and the exact bytes of its body.
//!
//! A gateway response is the unit of customization for everything the api itself answers
//! (`PutGatewayResponse` takes a `responseType`), so these cases assert the rules by which
//! one reaches the wire, each over a table of inputs:
//!
//! * the `responseParameters` a customization declares are written on the response the type
//!   is served with, over a table spanning both mapping-expression spellings, the type's own
//!   customization and the class default standing in for it, and a mapping that names no
//!   static value;
//! * every refusal whose exception class is known names it in `x-amzn-ErrorType`;
//! * the default body a gateway response carries is the bytes API Gateway sends, which for
//!   the two responses the REQUEST VALIDATOR produces carry a space after the member
//!   separator and for the bodies the edge writes carry none.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};
use crate::stage_edge_admission::invoke_bytes;
use crate::stage_request_validation::{create_validator, order_schema, create_model};

/// The stage every case deploys to.
const STAGE: &str = "wire";

/// The media type the methods here declare and the requests send.
const JSON: &str = "application/json";

/// `PutGatewayResponse` for `response_type`, carrying `members`, asserting the modelled
/// 201.
async fn customize(gateway: &Gateway, api: &str, response_type: &str, members: Value) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            Some(members),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutGatewayResponse({response_type}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A `POST` method on a child of the root with a `MOCK` integration answering 200, so a
/// case can address a route that EXISTS as well as one that does not.
async fn mock_route(gateway: &Gateway, api: &str, root: &str, route: &str, method: Value) {
    let resource = resource_under(gateway, api, root, route).await;
    put_method(gateway, api, &resource, "POST", method).await;
    put_integration(
        gateway,
        api,
        &resource,
        "POST",
        json!({
            "type": "MOCK",
            "requestTemplates": { JSON: r#"{"statusCode": 200}"# },
        }),
    )
    .await;
    method_response(gateway, api, &resource, "POST", "200", json!({})).await;
    integration_response(
        gateway,
        api,
        &resource,
        "POST",
        "200",
        json!({ "responseTemplates": { JSON: r#"{"integration":"ran"}"# } }),
    )
    .await;
}

#[tokio::test]
async fn a_gateway_responses_response_parameters_are_written_on_the_response_it_serves() {
    struct Case {
        name: &'static str,
        /// The response type the customization is stored under.
        response_type: &'static str,
        /// Its `responseParameters`.
        parameters: Value,
        /// The headers the answer is expected to carry, by lowercased name.
        expected: &'static [(&'static str, &'static str)],
        /// The header names the answer must NOT carry.
        absent: &'static [&'static str],
    }

    // Rows differ along what the rule generalises over: which mapping-expression spelling
    // the key uses, whether the customization is of the refusal's own type or of the class
    // default standing in for it, how many headers it declares, and whether the value is
    // the documented static literal.
    let cases = [
        Case {
            name: "the refusal's own response type",
            response_type: "MISSING_AUTHENTICATION_TOKEN",
            parameters: json!({ "gatewayresponse.header.X-Gw": "'own-type'" }),
            expected: &[("x-gw", "own-type")],
            absent: &[],
        },
        Case {
            name: "the class default standing in for it",
            response_type: "DEFAULT_4XX",
            parameters: json!({ "gatewayresponse.header.X-Gw": "'default4xx'" }),
            expected: &[("x-gw", "default4xx")],
            absent: &[],
        },
        Case {
            name: "the multi-value spelling of the mapping expression",
            response_type: "DEFAULT_4XX",
            parameters: json!({ "gatewayresponse.headers.X-Gw": "'multi'" }),
            expected: &[("x-gw", "multi")],
            absent: &[],
        },
        Case {
            name: "several headers at once",
            response_type: "DEFAULT_4XX",
            parameters: json!({
                "gatewayresponse.header.Access-Control-Allow-Origin": "'*'",
                "gatewayresponse.header.X-Gw": "'several'",
            }),
            expected: &[("access-control-allow-origin", "*"), ("x-gw", "several")],
            absent: &[],
        },
        Case {
            // A mapping that reads a value off the REQUEST names nothing on a response the
            // api itself produced, so no header is written rather than one guessed at.
            name: "a mapping that is no static value",
            response_type: "DEFAULT_4XX",
            parameters: json!({ "gatewayresponse.header.X-Gw": "method.request.header.Origin" }),
            expected: &[],
            absent: &["x-gw"],
        },
        Case {
            // A customization of a type the refusal is not answered through, and of the
            // OTHER class default, reaches nothing.
            name: "a customization of another class",
            response_type: "DEFAULT_5XX",
            parameters: json!({ "gatewayresponse.header.X-Gw": "'default5xx'" }),
            expected: &[],
            absent: &["x-gw"],
        },
    ];

    for case in cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "gateway-response-parameters").await;
        mock_route(
            &gateway,
            &api,
            &root,
            "orders",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        customize(
            &gateway,
            &api,
            case.response_type,
            json!({ "responseParameters": case.parameters }),
        )
        .await;
        deploy(&gateway, &api, STAGE).await;

        // A route the deployment holds no method for: the refusal API Gateway answers with
        // `MISSING_AUTHENTICATION_TOKEN`, which is a 4xx and so is also covered by
        // `DEFAULT_4XX`.
        let answered = invoke_bytes(
            &gateway,
            &api,
            "GET",
            &format!("/{STAGE}/nowhere"),
            &[],
            Bytes::new(),
        )
        .await;
        assert_eq!(
            answered.status, 403,
            "case {:?}: expected 403 for an unmatched route, got {} with {:?}",
            case.name,
            answered.status,
            answered.text()
        );
        // The exception class the endpoint reports for this refusal, which is what an SDK
        // classifies a 403 by.
        assert_eq!(
            answered.header("x-amzn-errortype"),
            Some("MissingAuthenticationTokenException"),
            "case {:?}: expected the error type header, got {:?}",
            case.name,
            answered.header("x-amzn-errortype")
        );
        for (name, value) in case.expected {
            assert_eq!(
                answered.header(name),
                Some(*value),
                "case {:?}: expected the header {name}: {value}, got {:?}",
                case.name,
                answered.header(name)
            );
        }
        for name in case.absent {
            assert_eq!(
                answered.header(name),
                None,
                "case {:?}: expected no {name} header, got {:?}",
                case.name,
                answered.header(name)
            );
        }
    }
}

#[tokio::test]
async fn the_default_gateway_response_body_is_the_bytes_api_gateway_sends() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "gateway-response-bodies").await;
    create_model(&gateway, &api, "Order", order_schema()).await;
    let validator = create_validator(&gateway, &api, "both", true, true).await;
    mock_route(
        &gateway,
        &api,
        &root,
        "orders",
        json!({
            "authorizationType": "NONE",
            "requestValidatorId": validator,
            "requestModels": { JSON: "Order" },
            "requestParameters": { "method.request.header.X-Required": true },
        }),
    )
    .await;
    deploy(&gateway, &api, STAGE).await;

    struct Case {
        name: &'static str,
        path: &'static str,
        headers: &'static [(&'static str, &'static str)],
        payload: &'static str,
        status: u16,
        /// The body VERBATIM, separator included.
        body: &'static str,
    }

    // Rows differ along which gateway response answers the request: the two the request
    // VALIDATOR produces, whose default body carries a space after the member separator,
    // and the refusals the EDGE itself writes, whose bodies carry none.
    let cases = [
        Case {
            name: "a request missing a required header",
            path: "orders",
            headers: &[("content-type", JSON)],
            payload: r#"{"id":"a1","qty":2}"#,
            status: 400,
            body: r#"{"message": "Missing required request parameters: [X-Required]"}"#,
        },
        Case {
            // The required header is matched case-insensitively, as a header name compares:
            // sent lowercased, it is carried, so the refusal below is the BODY one.
            name: "a payload the model does not admit",
            path: "orders",
            headers: &[("content-type", JSON), ("x-required", "yes")],
            payload: r#"{"id":"a1"}"#,
            status: 400,
            body: r#"{"message": "Invalid request body"}"#,
        },
        Case {
            name: "a route the deployment holds no method for",
            path: "nowhere",
            headers: &[],
            payload: "",
            status: 403,
            body: r#"{"message":"Missing Authentication Token"}"#,
        },
        Case {
            name: "a stage the api does not hold",
            path: "orders",
            headers: &[("content-type", JSON)],
            payload: "",
            status: 403,
            body: r#"{"message":"Forbidden"}"#,
        },
    ];

    for case in cases {
        // The last row addresses a stage that was never created; every other row addresses
        // the deployed one.
        let stage = if case.name == "a stage the api does not hold" {
            "undeployed"
        } else {
            STAGE
        };
        let answered = invoke_bytes(
            &gateway,
            &api,
            "POST",
            &format!("/{stage}/{}", case.path),
            case.headers,
            Bytes::from(case.payload.to_owned()),
        )
        .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected {}, got {} with {:?}",
            case.name,
            case.status,
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.text(),
            case.body,
            "case {:?}: expected the body {:?} byte for byte, got {:?}",
            case.name,
            case.body,
            answered.text()
        );
    }
}
