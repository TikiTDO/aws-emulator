// SPDX-License-Identifier: CC0-1.0

//! The method's REQUEST VALIDATOR applied to a SIMULATED invocation: `TestInvokeMethod`
//! reports the refusal the validator earns the request, with the integration never
//! performed.
//!
//! `RequestValidator` validates a request before the integration is invoked, and that is a
//! property of the METHOD rather than of the stage the request arrived at — so the rule is
//! asserted on both halves of one stored configuration at once: every row drives
//! `TestInvokeMethod` AND a request to the api's deployed stage, and the two must answer
//! the same status and the same body.
//!
//! The rows differ along what the rule generalises over: which half of the validator is
//! declared, which part of the request is wrong (a missing required header, a missing
//! required query parameter, a payload the model's schema does not admit, a payload that is
//! not JSON at all), and the requests a validator that declares neither half admits.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};
use crate::stage_edge_admission::invoke_bytes;
use crate::stage_request_validation::{create_model, create_validator, order_schema, GOOD_BODY};
use crate::test_invoke_method::test_invoke;

/// The stage every row deploys to and invokes.
const STAGE: &str = "sim";

/// The media type the methods declare a model for and every request below sends.
const JSON: &str = "application/json";

/// The body the `MOCK` integration answers when the validator admits the request.
const SERVED: &str = r#"{"integration":"ran"}"#;

/// A `POST` method with a `MOCK` integration answering 200 with [`SERVED`], carrying
/// `method_request` — the validator, the models and the parameters the row declares.
async fn validated_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    route: &str,
    method_request: Value,
) -> String {
    let resource = resource_under(gateway, api, root, route).await;
    put_method(gateway, api, &resource, "POST", method_request).await;
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
        json!({ "responseTemplates": { JSON: SERVED } }),
    )
    .await;
    resource
}

#[tokio::test]
async fn a_simulated_invocation_is_refused_by_the_methods_request_validator() {
    struct Case {
        name: &'static str,
        /// `(validateRequestBody, validateRequestParameters)`.
        mode: (bool, bool),
        /// The query string the request carries, `?`-prefixed or empty.
        query: &'static str,
        /// Whether the request carries the required header.
        carries_header: bool,
        payload: &'static str,
        status: u16,
        /// The body both halves must answer, byte for byte.
        body: &'static str,
    }

    let cases = [
        Case {
            name: "a validator declaring both halves admits a complete request",
            mode: (true, true),
            query: "?tenant=acme",
            carries_header: true,
            payload: GOOD_BODY,
            status: 200,
            body: SERVED,
        },
        Case {
            name: "a missing required header",
            mode: (true, true),
            query: "?tenant=acme",
            carries_header: false,
            payload: GOOD_BODY,
            status: 400,
            body: r#"{"message": "Missing required request parameters: [X-Required]"}"#,
        },
        Case {
            name: "a missing required query parameter",
            mode: (false, true),
            query: "",
            carries_header: true,
            payload: GOOD_BODY,
            status: 400,
            body: r#"{"message": "Missing required request parameters: [tenant]"}"#,
        },
        Case {
            name: "a payload the model's schema does not admit",
            mode: (true, false),
            query: "",
            carries_header: false,
            payload: r#"{"id":"a1"}"#,
            status: 400,
            body: r#"{"message": "Invalid request body"}"#,
        },
        Case {
            name: "a payload outside the schema's bounds",
            mode: (true, false),
            query: "",
            carries_header: false,
            payload: r#"{"id":"a1","qty":0}"#,
            status: 400,
            body: r#"{"message": "Invalid request body"}"#,
        },
        Case {
            name: "a payload that is not a JSON document at all",
            mode: (true, false),
            query: "",
            carries_header: false,
            payload: "not json",
            status: 400,
            body: r#"{"message": "Invalid request body"}"#,
        },
        Case {
            name: "a validator declaring the body half only ignores a missing parameter",
            mode: (true, false),
            query: "",
            carries_header: false,
            payload: GOOD_BODY,
            status: 200,
            body: SERVED,
        },
        Case {
            name: "a validator declaring the parameter half only ignores the payload",
            mode: (false, true),
            query: "?tenant=acme",
            carries_header: true,
            payload: r#"{"id":"a1"}"#,
            status: 200,
            body: SERVED,
        },
        Case {
            name: "a validator declaring neither half admits everything",
            mode: (false, false),
            query: "",
            carries_header: false,
            payload: "not json",
            status: 200,
            body: SERVED,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "simulated-validator").await;
        create_model(&gateway, &api, "Order", order_schema()).await;
        let validator = create_validator(
            &gateway,
            &api,
            &format!("mode{index}"),
            case.mode.0,
            case.mode.1,
        )
        .await;
        let resource = validated_route(
            &gateway,
            &api,
            &root,
            &format!("orders{index}"),
            json!({
                "authorizationType": "NONE",
                "requestValidatorId": validator,
                "requestModels": { JSON: "Order" },
                "requestParameters": {
                    "method.request.header.X-Required": true,
                    "method.request.querystring.tenant": true,
                },
            }),
        )
        .await;
        deploy(&gateway, &api, STAGE).await;

        // The simulated request declares, as members, what the wire request below carries as
        // headers — including the required header in the LOWERCASE spelling, because a
        // header name is matched case-insensitively.
        let mut headers = json!({ "Content-Type": JSON });
        if case.carries_header {
            headers["x-required"] = json!("yes");
        }
        let simulated = test_invoke(
            &gateway,
            &api,
            &resource,
            "POST",
            json!({
                "pathWithQueryString": format!("/orders{index}{}", case.query),
                "headers": headers,
                "body": case.payload,
            }),
        )
        .await;
        assert_eq!(
            simulated.http_status, 200,
            "case {:?}: expected TestInvokeMethod itself to answer 200, got {} with {:?}",
            case.name, simulated.http_status, simulated.body
        );
        assert_eq!(
            simulated.status(),
            i64::from(case.status),
            "case {:?}: expected the simulated status {}, got {} with {:?}",
            case.name,
            case.status,
            simulated.status(),
            simulated.simulated_body()
        );
        assert_eq!(
            simulated.simulated_body(),
            case.body,
            "case {:?}: expected the simulated body {:?}, got {:?}",
            case.name,
            case.body,
            simulated.simulated_body()
        );

        // The same records, reached through the deployed stage: one validator, so both
        // halves answer the same thing.
        let mut wire: Vec<(&str, &str)> = vec![("content-type", JSON)];
        if case.carries_header {
            wire.push(("x-required", "yes"));
        }
        let answered = invoke_bytes(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/orders{index}{}", case.query),
            &wire,
            Bytes::from(case.payload.to_owned()),
        )
        .await;
        assert_eq!(
            answered.status, case.status,
            "case {:?}: expected the deployed stage to answer {}, got {} with {:?}",
            case.name,
            case.status,
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.text(),
            case.body,
            "case {:?}: expected the deployed stage's body {:?}, got {:?}",
            case.name,
            case.body,
            answered.text()
        );
    }
}
