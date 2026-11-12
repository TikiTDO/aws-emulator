// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a DEPLOYED stage does with a method's `RequestValidator`
//! before the integration runs, and for the usage-plan throttle a metered call is admitted
//! by.
//!
//! Every case drives the real wire path — an unsigned request to the api's own invoke host
//! through the multi-service router, with the control plane driven through the same
//! emulator ([`Gateway`]) — so what is asserted is what a client of a published invoke URL
//! sees.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a method whose `requestValidatorId` names a validator is checked exactly as far as
//!   that validator DECLARES: `validateRequestBody` refuses a payload the `Model` the
//!   method maps to the request's content type does not admit, with the `BAD_REQUEST_BODY`
//!   gateway response, and `validateRequestParameters` refuses a request that omits a
//!   request parameter the method declares required, with `BAD_REQUEST_PARAMETERS` naming
//!   the parameters that were missing. The rows differ along the validator's mode (all
//!   four), along the way a payload fails one schema (absent property, wrong type, value
//!   outside a bound, undeclared property, not a JSON document at all, empty) and along
//!   which and how many declared parameters a request omits;
//! * a request a usage plan meters is admitted at the plan's `throttle` and shed with the
//!   `THROTTLED` gateway response once the plan has no request left for the key at this
//!   instant. The rows differ along the plan's `burstLimit`/`rateLimit` pair, along
//!   whether the plan declares a throttle at all, and along whether the method requires
//!   the key that the plan meters.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::execute_api_data_plane::{integration_response, method_response, put_integration, put_method};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};

/// The region the invoke host is addressed in; the host's region label is not matched
/// against a list, so one region drives the data plane.
const REGION: &str = "us-east-1";

/// The stage every case deploys to.
///
/// `pub(crate)` with the helpers below for the sibling
/// `request_validator_precedes_the_integration` suite, which drives the same validator
/// path against the backend integration types rather than a `MOCK`: one harness, so the
/// two suites cannot stand one validator up two ways.
pub(crate) const STAGE: &str = "test";

/// The media type the methods under test declare a model for, and the one every request
/// below sends its payload as.
pub(crate) const JSON: &str = "application/json";

/// One answered invoke: the status, the headers asserted on and the body verbatim.
pub(crate) struct Invoked {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: String,
}

impl Invoked {
    /// The `message` member of a JSON body — what every API Gateway gateway response
    /// carries — or the body verbatim when it carries none.
    pub(crate) fn message(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|body| body.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| self.body.clone())
    }
}

/// Issue one unsigned request to `api`'s invoke endpoint at `path`, carrying `headers` and
/// `body` — what a client of a published invoke URL sends.
pub(crate) async fn invoke(
    gateway: &Gateway,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Invoked {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    invoke_at(gateway, &host, verb, path, headers, body).await
}

/// [`invoke`] against an arbitrary host and path, for the cases that vary WHICH addressing
/// names the api.
async fn invoke_at(
    gateway: &Gateway,
    host: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Invoked {
    let mut builder = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body.to_owned()))
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked {
        status,
        error_type,
        body,
    }
}

/// `CreateModel` of `name` with `schema`, asserting the modelled 201.
pub(crate) async fn create_model(gateway: &Gateway, api: &str, name: &str, schema: Value) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/models"),
            Some(json!({
                "name": name,
                "contentType": JSON,
                "schema": schema.to_string(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateModel({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateRequestValidator` in the mode `(validateRequestBody, validateRequestParameters)`
/// names, answering its id.
pub(crate) async fn create_validator(
    gateway: &Gateway,
    api: &str,
    name: &str,
    body: bool,
    parameters: bool,
) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/requestvalidators"),
            Some(json!({
                "name": name,
                "validateRequestBody": body,
                "validateRequestParameters": parameters,
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateRequestValidator({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateRequestValidator")
}

/// The body the `MOCK` integration of every method below answers with, so a served call is
/// distinguishable from a refused one by its body alone.
fn served_body(route: &str) -> String {
    format!(r#"{{"integration":"{route}"}}"#)
}

/// A `POST` method on a child resource of the root, carrying `request_models`,
/// `request_parameters` and `request_validator`, with a `MOCK` integration that answers
/// 200 with [`served_body`]: the one integration type that needs no backend.
async fn mock_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    route: &str,
    method_request: Value,
) {
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
        json!({ "responseTemplates": { JSON: served_body(route) } }),
    )
    .await;
}

/// `CreateDeployment` of `api` into [`STAGE`], asserting the modelled 201.
pub(crate) async fn deploy(gateway: &Gateway, api: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": STAGE })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The draft-04 object schema every validator case validates against: two required
/// properties, a bounded integer and no undeclared property. The dialect is the one
/// `CreateModel$schema` documents a model is written in ("this should be JSON schema
/// draft 4 model").
pub(crate) fn order_schema() -> Value {
    json!({
        "$schema": "http://json-schema.org/draft-04/schema#",
        "title": "Order",
        "type": "object",
        "required": ["id", "qty"],
        "properties": {
            "id": { "type": "string" },
            "qty": { "type": "integer", "minimum": 1 },
        },
        "additionalProperties": false,
    })
}

/// The payload that satisfies [`order_schema`].
pub(crate) const GOOD_BODY: &str = r#"{"id":"a1","qty":2}"#;

/// The four modes a `RequestValidator` can declare, as
/// `(route, validateRequestBody, validateRequestParameters)`.
const MODES: &[(&str, bool, bool)] = &[
    ("body", true, false),
    ("params", false, true),
    ("both", true, true),
    ("none", false, false),
];

#[tokio::test]
async fn each_validator_mode_refuses_only_what_it_declares() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "validator-modes").await;
    create_model(&gateway, &api, "Order", order_schema()).await;

    for (route, body, parameters) in MODES {
        let validator = create_validator(&gateway, &api, route, *body, *parameters).await;
        mock_route(
            &gateway,
            &api,
            &root,
            route,
            json!({
                "authorizationType": "NONE",
                "requestModels": { JSON: "Order" },
                "requestParameters": { "method.request.querystring.tenant": true },
                "requestValidatorId": validator,
            }),
        )
        .await;
    }
    deploy(&gateway, &api).await;

    // One row per (mode, request) cell: the request a mode declares nothing about is
    // SERVED, and the one it does declare about is refused through the gateway response for
    // that half of the request.
    struct Cell {
        /// The validator mode's route.
        route: &'static str,
        /// What the request carries: a payload and a query string.
        payload: &'static str,
        query: &'static str,
        /// The status and the `message` the stage must answer with.
        status: u16,
        message: &'static str,
    }
    let served_for = |route: &str| served_body(route);
    let cells = [
        // The body-only validator screens the payload and says nothing about parameters.
        Cell { route: "body", payload: GOOD_BODY, query: "?tenant=acme", status: 200, message: "" },
        Cell { route: "body", payload: r#"{"id":"a1"}"#, query: "?tenant=acme", status: 400, message: "Invalid request body" },
        Cell { route: "body", payload: GOOD_BODY, query: "", status: 200, message: "" },
        // The parameters-only validator does the opposite.
        Cell { route: "params", payload: GOOD_BODY, query: "?tenant=acme", status: 200, message: "" },
        Cell { route: "params", payload: r#"{"id":"a1"}"#, query: "?tenant=acme", status: 200, message: "" },
        Cell { route: "params", payload: GOOD_BODY, query: "", status: 400, message: "Missing required request parameters: [tenant]" },
        // Both halves declared: each wrong request earns its own refusal.
        Cell { route: "both", payload: GOOD_BODY, query: "?tenant=acme", status: 200, message: "" },
        Cell { route: "both", payload: r#"{"id":"a1"}"#, query: "?tenant=acme", status: 400, message: "Invalid request body" },
        Cell { route: "both", payload: GOOD_BODY, query: "", status: 400, message: "Missing required request parameters: [tenant]" },
        // A validator that declares neither refuses nothing, so every request is served.
        Cell { route: "none", payload: GOOD_BODY, query: "?tenant=acme", status: 200, message: "" },
        Cell { route: "none", payload: r#"{"id":"a1"}"#, query: "?tenant=acme", status: 200, message: "" },
        Cell { route: "none", payload: GOOD_BODY, query: "", status: 200, message: "" },
    ];

    for cell in cells {
        let case = format!(
            "{}/{} with payload {} and query {:?}",
            STAGE, cell.route, cell.payload, cell.query
        );
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{}{}", cell.route, cell.query),
            &[("content-type", JSON)],
            cell.payload,
        )
        .await;
        assert_eq!(
            answered.status, cell.status,
            "case {case}: expected {}, got {} with {:?}",
            cell.status, answered.status, answered.body
        );
        if cell.status == 200 {
            assert_eq!(
                answered.body,
                served_for(cell.route),
                "case {case}: expected the integration's own body, got {:?}",
                answered.body
            );
        } else {
            assert_eq!(
                answered.message(),
                cell.message,
                "case {case}: expected the message {:?}, got {:?}",
                cell.message,
                answered.message()
            );
            assert_eq!(
                answered.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case}: expected the refusal to name its error shape, got {:?}",
                answered.error_type
            );
        }
    }
}

#[tokio::test]
async fn a_body_validator_refuses_every_payload_the_model_does_not_admit() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "validator-bodies").await;
    create_model(&gateway, &api, "Order", order_schema()).await;
    let validator = create_validator(&gateway, &api, "bodyonly", true, false).await;
    mock_route(
        &gateway,
        &api,
        &root,
        "orders",
        json!({
            "authorizationType": "NONE",
            "requestModels": { JSON: "Order" },
            "requestValidatorId": validator,
        }),
    )
    .await;
    deploy(&gateway, &api).await;

    // Rows differ along HOW the payload fails the one schema — which is what the rule
    // generalises over — plus the payloads that satisfy it.
    let rows: &[(&str, &str, u16)] = &[
        ("every declared property, in range", GOOD_BODY, 200),
        ("the other order of the same members", r#"{"qty":7,"id":"zz"}"#, 200),
        ("a required property absent", r#"{"id":"a1"}"#, 400),
        ("both required properties absent", "{}", 400),
        ("a property of the wrong type", r#"{"id":"a1","qty":"two"}"#, 400),
        ("a number below the declared minimum", r#"{"id":"a1","qty":0}"#, 400),
        ("a property the schema does not declare", r#"{"id":"a1","qty":1,"extra":true}"#, 400),
        ("a payload that is not a JSON document", "not json at all", 400),
        ("an empty payload", "", 400),
        ("a JSON value that is not an object", "[1,2,3]", 400),
    ];
    for (case, payload, status) in rows {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/orders"),
            &[("content-type", JSON)],
            payload,
        )
        .await;
        assert_eq!(
            answered.status, *status,
            "case {case}: expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        if *status == 400 {
            assert_eq!(
                answered.message(),
                "Invalid request body",
                "case {case}: expected the BAD_REQUEST_BODY sentence, got {:?}",
                answered.message()
            );
        }
    }

    // A content type the method maps NO model to declares nothing to validate against, so
    // the very payload the JSON model rejects is served when it is sent as another type.
    let answered = invoke(
        &gateway,
        &api,
        "POST",
        &format!("/{STAGE}/orders"),
        &[("content-type", "text/plain")],
        "not json at all",
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case an unmodelled content type: expected 200, got {} with {:?}",
        answered.status, answered.body
    );

    // The validator is the METHOD's, not the addressing's: the same two payloads answer the
    // same way whichever addressing names the api — the api's own invoke host, the
    // emulator's data-plane path prefix, or the api id as the first path segment. A client
    // that cannot resolve the invoke host reaches the stage through one of the latter two,
    // and a request validator that only ran for one of them would screen nothing for it.
    let emulator_host = "127.0.0.1:9297";
    let addressings: &[(&str, &str, String)] = &[
        (
            "the api's own invoke host",
            &format!("{api}.execute-api.{REGION}.amazonaws.com"),
            format!("/{STAGE}/orders"),
        ),
        (
            "the data-plane path prefix",
            emulator_host,
            format!("/_aws/execute-api/{api}/{STAGE}/orders"),
        ),
        (
            "the api id as the first path segment",
            emulator_host,
            format!("/{api}/{STAGE}/orders"),
        ),
    ];
    for (case, host, path) in addressings {
        for (payload, status) in [(GOOD_BODY, 200u16), (r#"{"id":"a1"}"#, 400)] {
            let answered = invoke_at(
                &gateway,
                host,
                "POST",
                path,
                &[("content-type", JSON)],
                payload,
            )
            .await;
            assert_eq!(
                answered.status, status,
                "case {case} with payload {payload}: expected {status}, got {} with {:?}",
                answered.status, answered.body
            );
            if status == 200 {
                assert_eq!(
                    answered.body,
                    served_body("orders"),
                    "case {case}: expected the integration's own body, got {:?}",
                    answered.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_parameter_validator_names_every_required_parameter_the_request_omits() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "validator-parameters").await;
    let validator = create_validator(&gateway, &api, "paramsonly", false, true).await;
    mock_route(
        &gateway,
        &api,
        &root,
        "orders",
        json!({
            "authorizationType": "NONE",
            // Two required parameters in two locations, plus one of each that is NOT
            // required: an optional parameter can never be missing.
            "requestParameters": {
                "method.request.querystring.tenant": true,
                "method.request.header.x-trace": true,
                "method.request.querystring.page": false,
                "method.request.header.x-optional": false,
            },
            "requestValidatorId": validator,
        }),
    )
    .await;
    deploy(&gateway, &api).await;

    // Rows differ along which declared parameters the request carries, and along the
    // blank value that is no value at all.
    struct Carried {
        case: &'static str,
        /// The query string the request is sent with.
        query: &'static str,
        /// The request headers beside the content type.
        headers: &'static [(&'static str, &'static str)],
        /// The status the stage must answer, and the `message` of a refusal.
        status: u16,
        message: &'static str,
    }
    let rows = [
        Carried {
            case: "both required parameters carried",
            query: "?tenant=acme",
            headers: &[("x-trace", "t-1")],
            status: 200,
            message: "",
        },
        Carried {
            case: "every declared parameter carried",
            query: "?tenant=acme&page=2",
            headers: &[("x-trace", "t-1"), ("x-optional", "o")],
            status: 200,
            message: "",
        },
        Carried {
            case: "the required query parameter omitted",
            query: "?page=2",
            headers: &[("x-trace", "t-1")],
            status: 400,
            message: "Missing required request parameters: [tenant]",
        },
        Carried {
            case: "the required header omitted",
            query: "?tenant=acme",
            headers: &[],
            status: 400,
            message: "Missing required request parameters: [x-trace]",
        },
        Carried {
            case: "both required parameters omitted",
            query: "",
            headers: &[],
            status: 400,
            message: "Missing required request parameters: [x-trace, tenant]",
        },
        Carried {
            case: "the required query parameter present but blank",
            query: "?tenant=",
            headers: &[("x-trace", "t-1")],
            status: 400,
            message: "Missing required request parameters: [tenant]",
        },
        Carried {
            case: "only the optional parameters carried",
            query: "?page=2",
            headers: &[("x-optional", "o")],
            status: 400,
            message: "Missing required request parameters: [x-trace, tenant]",
        },
    ];
    for row in rows {
        // Every row declares the payload's content type; what it varies is the rest.
        let mut headers: Vec<(&str, &str)> = vec![("content-type", JSON)];
        headers.extend_from_slice(row.headers);
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/orders{}", row.query),
            &headers,
            GOOD_BODY,
        )
        .await;
        assert_eq!(
            answered.status, row.status,
            "case {}: expected {}, got {} with {:?}",
            row.case, row.status, answered.status, answered.body
        );
        if row.status == 400 {
            assert_eq!(
                answered.message(),
                row.message,
                "case {}: expected the message {:?}, got {:?}",
                row.case,
                row.message,
                answered.message()
            );
        }
    }
}

#[tokio::test]
async fn a_model_that_references_another_model_of_the_api_is_resolved() {
    // A `Model`'s schema may be composed of references to the API's OTHER models, which
    // API Gateway identifies by the model's own schema URL, so the referenced model's
    // constraints are part of what the validator applies.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "validator-references").await;
    create_model(
        &gateway,
        &api,
        "Line",
        json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object",
            "required": ["sku"],
            "properties": { "sku": { "type": "string" } },
        }),
    )
    .await;
    create_model(
        &gateway,
        &api,
        "Basket",
        json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object",
            "required": ["line"],
            "properties": {
                "line": {
                    "$ref": format!(
                        "https://apigateway.amazonaws.com/restapis/{api}/models/Line"
                    ),
                },
            },
        }),
    )
    .await;
    let validator = create_validator(&gateway, &api, "basket", true, false).await;
    mock_route(
        &gateway,
        &api,
        &root,
        "baskets",
        json!({
            "authorizationType": "NONE",
            "requestModels": { JSON: "Basket" },
            "requestValidatorId": validator,
        }),
    )
    .await;
    deploy(&gateway, &api).await;

    // Rows differ along which of the two models' constraints the payload breaks.
    let rows: &[(&str, &str, u16)] = &[
        ("a payload satisfying both models", r#"{"line":{"sku":"a1"}}"#, 200),
        ("the referenced model's required property absent", r#"{"line":{}}"#, 400),
        ("the referenced model's property of the wrong type", r#"{"line":{"sku":7}}"#, 400),
        ("the referencing model's required property absent", "{}", 400),
    ];
    for (case, payload, status) in rows {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/baskets"),
            &[("content-type", JSON)],
            payload,
        )
        .await;
        assert_eq!(
            answered.status, *status,
            "case {case}: expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
    }

    // A reference to anything OUTSIDE the API's own models resolves to nothing, so the
    // validator the method declares cannot be applied at all: the call is answered through
    // the API's `API_CONFIGURATION_ERROR` gateway response — the same way this data plane
    // answers an integration it cannot perform — rather than served as if the check had
    // passed, and nothing of whatever the reference named reaches the caller.
    let outside: &[(&str, &str)] = &[
        ("a file-scheme reference", "file:///etc/hostname"),
        ("an http reference", "http://127.0.0.1/schema.json"),
        ("a reference to a model the api does not hold", "Absent"),
    ];
    for (case, reference) in outside {
        let (api, root) = create_api(&gateway, "validator-outside-references").await;
        create_model(
            &gateway,
            &api,
            "Outside",
            json!({
                "$schema": "http://json-schema.org/draft-04/schema#",
                "type": "object",
                "required": ["line"],
                "properties": { "line": { "$ref": reference } },
            }),
        )
        .await;
        let validator = create_validator(&gateway, &api, "outside", true, false).await;
        mock_route(
            &gateway,
            &api,
            &root,
            "outside",
            json!({
                "authorizationType": "NONE",
                "requestModels": { JSON: "Outside" },
                "requestValidatorId": validator,
            }),
        )
        .await;
        deploy(&gateway, &api).await;

        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/outside"),
            &[("content-type", JSON)],
            "{}",
        )
        .await;
        assert_eq!(
            answered.status, 500,
            "case {case}: expected the unapplicable validator to be answered through \
             API_CONFIGURATION_ERROR, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.message(),
            "Internal server error",
            "case {case}: expected the API_CONFIGURATION_ERROR sentence and nothing of what \
             the reference named, got {:?}",
            answered.body
        );
    }
}

/// `PutGatewayResponse` of `response_type` with `template` as its `application/json`
/// response template, asserting the modelled 201.
async fn customize_gateway_response(
    gateway: &Gateway,
    api: &str,
    response_type: &str,
    template: &str,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            Some(json!({ "responseTemplates": { JSON: template } })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutGatewayResponse({response_type}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_refusals_gateway_response_template_resolves_the_errors_own_context() {
    // Rows differ along what the customized template carries — a literal only, the quoted
    // and the unquoted spelling of a resolvable `$context` reference, a reference nothing
    // resolves, and a DIRECTIVE, which has to be executed rather than resolved — and along
    // which half of the request the refusal came from, since each half is answered through
    // its own gateway response type.
    struct Row {
        case: &'static str,
        /// The gateway response type customized, and its `application/json` template.
        response_type: &'static str,
        template: &'static str,
        /// The request: a payload and a query string, chosen to earn that response type.
        payload: &'static str,
        query: &'static str,
        /// Fragments the served body must carry, verbatim.
        carries: &'static [&'static str],
    }
    let rows = [
        Row {
            case: "a literal template",
            response_type: "BAD_REQUEST_BODY",
            template: r#"{"which":"body-marker"}"#,
            payload: r#"{"id":"a1"}"#,
            query: "?tenant=acme",
            carries: &[r#""which":"body-marker""#],
        },
        Row {
            case: "the message, the response type and the validation detail",
            response_type: "BAD_REQUEST_BODY",
            template: r#"{"which":"body-marker","message":$context.error.messageString,"type":"$context.error.responseType","detail":"$context.error.validationErrorString"}"#,
            payload: r#"{"id":"a1"}"#,
            query: "?tenant=acme",
            carries: &[
                r#""which":"body-marker""#,
                r#""message":"Invalid request body""#,
                r#""type":"BAD_REQUEST_BODY""#,
                // The validation's own finding, which must name the property the payload
                // omitted — a detail no canned sentence could carry.
                "qty",
            ],
        },
        Row {
            case: "the parameters refusal's own response type",
            response_type: "BAD_REQUEST_PARAMETERS",
            template: r#"{"which":"params-marker","message":$context.error.messageString,"type":"$context.error.responseType"}"#,
            payload: GOOD_BODY,
            query: "",
            carries: &[
                r#""which":"params-marker""#,
                r#""message":"Missing required request parameters: [tenant]""#,
                r#""type":"BAD_REQUEST_PARAMETERS""#,
            ],
        },
        Row {
            case: "a reference nothing here resolves",
            response_type: "BAD_REQUEST_BODY",
            template: r#"{"which":"body-marker","at":"$context.somethingElse"}"#,
            payload: r#"{"id":"a1"}"#,
            query: "?tenant=acme",
            // Left exactly as the template wrote it, which is what the template language
            // itself does with a reference it cannot resolve.
            carries: &[r#""at":"$context.somethingElse""#],
        },
        Row {
            case: "a template carrying a directive",
            response_type: "BAD_REQUEST_BODY",
            template: "#set($which = \"body-marker\"){\"which\":\"$which\"}",
            payload: r#"{"id":"a1"}"#,
            query: "?tenant=acme",
            // Not rendered at all: the caller gets API Gateway's own default body for the
            // response type instead of a body this emulator guessed at — and that default
            // body is the VALIDATOR's own rendering, which carries a space after the member
            // separator.
            carries: &[r#"{"message": "Invalid request body"}"#],
        },
    ];

    for row in rows {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "gateway-response-templates").await;
        create_model(&gateway, &api, "Order", order_schema()).await;
        let validator = create_validator(&gateway, &api, "both", true, true).await;
        mock_route(
            &gateway,
            &api,
            &root,
            "orders",
            json!({
                "authorizationType": "NONE",
                "requestModels": { JSON: "Order" },
                "requestParameters": { "method.request.querystring.tenant": true },
                "requestValidatorId": validator,
            }),
        )
        .await;
        customize_gateway_response(&gateway, &api, row.response_type, row.template).await;
        deploy(&gateway, &api).await;

        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/orders{}", row.query),
            &[("content-type", JSON)],
            row.payload,
        )
        .await;
        assert_eq!(
            answered.status, 400,
            "case {}: expected 400, got {} with {:?}",
            row.case, answered.status, answered.body
        );
        for fragment in row.carries {
            assert!(
                answered.body.contains(fragment),
                "case {}: expected the body to carry {fragment:?}, got {:?}",
                row.case,
                answered.body
            );
        }
        // Whatever the template, the served body is a JSON document a client can read.
        let parsed: Result<Value, _> = serde_json::from_str(&answered.body);
        assert!(
            parsed.is_ok(),
            "case {}: expected the rendered body to be JSON, got {:?}",
            row.case,
            answered.body
        );
    }
}

/// `CreateApiKey(enabled)`, answering `(id, value)`.
async fn create_api_key(gateway: &Gateway, name: &str) -> (String, String) {
    let answer = gateway
        .call(
            "POST",
            "/apikeys",
            Some(json!({ "name": name, "enabled": true })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateApiKey({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    (
        string_member(&answer.body, "id", "CreateApiKey"),
        string_member(&answer.body, "value", "CreateApiKey"),
    )
}

/// `CreateUsagePlan` over `api`'s [`STAGE`] with `throttle` (none when `Value::Null`),
/// then `CreateUsagePlanKey` adding `key_id` to it.
async fn create_usage_plan(gateway: &Gateway, api: &str, name: &str, throttle: Value, key_id: &str) {
    let mut request = json!({
        "name": name,
        "apiStages": [{ "apiId": api, "stage": STAGE }],
    });
    if !throttle.is_null() {
        request["throttle"] = throttle;
    }
    let answer = gateway.call("POST", "/usageplans", Some(request)).await;
    assert_eq!(
        answer.status, 201,
        "case CreateUsagePlan({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let plan = string_member(&answer.body, "id", "CreateUsagePlan");
    let added = gateway
        .call(
            "POST",
            &format!("/usageplans/{plan}/keys"),
            Some(json!({ "keyId": key_id, "keyType": "API_KEY" })),
        )
        .await;
    assert_eq!(
        added.status, 201,
        "case CreateUsagePlanKey({name}): expected 201, got {} with {:?}",
        added.status, added.body
    );
}

/// One usage-plan throttle case: the plan's limits, whether the method requires the key,
/// and how many of a burst of requests the plan must admit.
struct Throttled {
    case: &'static str,
    /// The plan's `throttle`, or `Value::Null` for a plan that declares none.
    throttle: Value,
    /// Whether the method under test sets `apiKeyRequired`.
    api_key_required: bool,
    /// How many requests of the burst must be served before the plan sheds one.
    admitted: usize,
}

#[tokio::test]
async fn a_usage_plan_throttle_sheds_the_burst_it_does_not_admit() {
    // Rows differ along the plan's burst limit — what the token bucket's capacity is —
    // along a plan that declares no throttle at all, and along a method that requires no
    // key, which the plan therefore meters nothing for. The RATE is a thousandth of a
    // request per second throughout, so no refill can fall inside the burst however long
    // this test's requests take on the machine running it: what each row asserts is the
    // capacity the plan admits, not the clock.
    let rows = [
        Throttled {
            case: "a burst of one",
            throttle: json!({ "burstLimit": 1, "rateLimit": 0.001 }),
            api_key_required: true,
            admitted: 1,
        },
        Throttled {
            case: "a burst of three",
            throttle: json!({ "burstLimit": 3, "rateLimit": 0.001 }),
            api_key_required: true,
            admitted: 3,
        },
        Throttled {
            case: "a plan with no throttle",
            throttle: Value::Null,
            api_key_required: true,
            admitted: 6,
        },
        Throttled {
            case: "a method that requires no api key",
            throttle: json!({ "burstLimit": 1, "rateLimit": 0.001 }),
            api_key_required: false,
            admitted: 6,
        },
    ];
    /// How many requests each row sends: more than any row's limit admits.
    const BURST: usize = 6;

    for row in rows {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "usage-plan-throttle").await;
        mock_route(
            &gateway,
            &api,
            &root,
            "metered",
            json!({
                "authorizationType": "NONE",
                "apiKeyRequired": row.api_key_required,
            }),
        )
        .await;
        deploy(&gateway, &api).await;
        let (key_id, key_value) = create_api_key(&gateway, "throttled").await;
        create_usage_plan(&gateway, &api, "plan", row.throttle.clone(), &key_id).await;

        let mut statuses: Vec<u16> = Vec::new();
        let mut last = Invoked {
            status: 0,
            error_type: None,
            body: String::new(),
        };
        for _ in 0..BURST {
            last = invoke(
                &gateway,
                &api,
                "POST",
                &format!("/{STAGE}/metered"),
                &[("content-type", JSON), ("x-api-key", &key_value)],
                GOOD_BODY,
            )
            .await;
            statuses.push(last.status);
        }
        let served = statuses.iter().take_while(|status| **status == 200).count();
        assert_eq!(
            served, row.admitted,
            "case {}: expected the plan to admit {} of {BURST} requests, got {statuses:?}",
            row.case, row.admitted
        );
        if row.admitted < BURST {
            assert!(
                statuses[row.admitted..].iter().all(|status| *status == 429),
                "case {}: expected every request past the limit to be shed, got {statuses:?}",
                row.case
            );
            assert_eq!(
                last.message(),
                "Too Many Requests",
                "case {}: expected the THROTTLED sentence, got {:?}",
                row.case,
                last.message()
            );
            assert_eq!(
                last.error_type.as_deref(),
                Some("TooManyRequestsException"),
                "case {}: expected the refusal to name its error shape, got {:?}",
                row.case,
                last.error_type
            );
        }
    }
}
