// SPDX-License-Identifier: CC0-1.0

//! `TestInvokeMethod` beside the `execute-api` data plane: the two halves of ONE stored
//! configuration.
//!
//! The RULES these cases assert, over sets of inputs rather than one recorded call:
//!
//! * a simulated invocation reports, for a given simulated request, exactly what a real
//!   request to the API's deployed stage is served for the same stored records — the
//!   status, the body, the media type the body is rendered with and every mapped response
//!   header — because performing the integration is one function both halves reach, so a
//!   developer cannot be shown one answer by the test call and another by the endpoint;
//! * a simulated invocation applies NEITHER the method's `authorizationType` NOR its
//!   `apiKeyRequired`: it is reached through the control plane by the API's own owner and
//!   carries no end-caller identity and no API key, so it reports the integration's
//!   response for a method the data plane refuses to an anonymous, keyless caller;
//! * a simulated invocation is a READ: the data-plane answer taken before it and the one
//!   taken after it are the same answer, so nothing a later caller sees is changed by
//!   simulating a call (no authorization granted, no key metered, no response consumed).
//!
//! The rows differ along what those rules generalise over: the integration configuration
//! (a `MOCK` that answers, one whose template cannot be evaluated, one that refuses an
//! unmapped media type, and a type naming a backend that cannot be called), the media type
//! the request declares and the one it accepts, the verb, whether the method declares a
//! mapped response header, and the method's authorization — each documented
//! `authorizationType` the data plane can decide on its own, plus the API-key gate.
//!
//! Every case drives the real wire path — the control plane through this crate's
//! `restJson1` wire harness ([`Gateway`]) and the data plane through the same emulator on
//! the api's own invoke host — so what is compared is what a client sees on each side.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{deploy, invoke, put_method, Invoked};
use crate::integration_response::put_integration_response;
use crate::method_integration::{aws_integration_uri, put_integration};
use crate::method_response::{put_method_response, response_header};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};
use crate::test_invoke_method::{test_invoke, Simulated};

/// The stage every case deploys to and invokes; one is enough, because no row turns on the
/// stage's name (`stage_invoke_shared_stage_names` is the suite that varies it).
const STAGE: &str = "prod";

/// One stored configuration, the simulated and real request made against it, and what both
/// halves must answer.
struct Parity {
    /// What makes this row its own case.
    case: &'static str,
    /// The path part of the resource this row stands its method up on, which is also the
    /// path the data plane addresses it at.
    path_part: &'static str,
    /// The verb the method is put under, and the verb the data-plane request carries.
    verb: &'static str,
    /// The `PutIntegration` request this row's method carries.
    integration: Value,
    /// The `PutMethodResponse` requests, by status code.
    method_responses: Vec<(&'static str, Value)>,
    /// The `PutIntegrationResponse` requests, by status code.
    integration_responses: Vec<(&'static str, Value)>,
    /// The request headers BOTH halves carry: as `TestInvokeMethod`'s `headers` members and
    /// as the data-plane request's own headers.
    headers: Vec<(&'static str, &'static str)>,
    /// The status both halves must answer.
    status: u16,
    /// The body both halves must answer.
    body: &'static str,
    /// The headers the SIMULATION must report, by name and value — the media type the body
    /// was rendered with, plus the mapped response header a row that declares one must
    /// carry. Asserted on their own as well as cross-checked against the wire, so a
    /// simulation that reported no header at all is a failure rather than a vacuous pass.
    reported_headers: Vec<(&'static str, &'static str)>,
}

/// `PutIntegration` through the crate's shared helper, asserted at the modelled 201.
async fn integration(gateway: &Gateway, api: &str, resource: &str, verb: &str, request: Value) {
    let answer = put_integration(gateway, api, resource, verb, request).await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration({resource}/{verb}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutIntegrationResponse` through the crate's shared helper, asserted at the modelled 201.
async fn integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let answer = put_integration_response(gateway, api, resource, verb, status, request).await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegrationResponse({resource}/{verb}/{status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `PutMethodResponse` through the crate's shared helper, asserted at the modelled 201.
async fn method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) {
    let answer = put_method_response(gateway, api, resource, verb, status, request).await;
    assert_eq!(
        answer.status, 201,
        "case PutMethodResponse({resource}/{verb}/{status}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `headers` member of a `TestInvokeMethod` request carrying `headers`, which is where a
/// simulated request declares what a real one carries on the wire.
fn simulated_request(headers: &[(&str, &str)]) -> Value {
    let map: serde_json::Map<String, Value> = headers
        .iter()
        .map(|(name, value)| ((*name).to_string(), json!(value)))
        .collect();
    json!({ "headers": Value::Object(map) })
}

/// Assert the simulated invocation and the real one are the SAME answer: the status, the
/// body, and every header the simulation reports with the value the wire carried for it.
fn assert_same_answer(case: &str, simulated: &Simulated, answered: &Invoked) {
    assert_eq!(
        simulated.http_status, 200,
        "case {case:?}: expected TestInvokeMethod to answer 200, got {} with {:?}",
        simulated.http_status, simulated.body
    );
    assert_eq!(
        simulated.status(),
        i64::from(answered.status),
        "case {case:?}: expected the simulated status to be the served {}, got {:?}",
        answered.status,
        simulated.body
    );
    assert_eq!(
        simulated.simulated_body(),
        answered.body,
        "case {case:?}: expected the simulated body to be the served {:?}, got {:?}",
        answered.body,
        simulated.simulated_body()
    );
    let reported = simulated.headers();
    let reported = reported
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a headers object, got {reported:?}"));
    for (name, value) in reported {
        let served = answered.header(&name.to_ascii_lowercase());
        assert_eq!(
            served,
            value.as_str(),
            "case {case:?}: expected the served {name:?} to be {value:?}, got {served:?}"
        );
    }
}

#[tokio::test]
async fn a_simulated_invocation_reports_what_the_deployed_stage_serves() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-parity").await;

    // Rows differ along the integration configuration, the media type declared and the one
    // accepted, the verb, and whether a mapped response header is declared — a served
    // answer and each way an integration cannot be performed among them.
    let cases: Vec<Parity> = vec![
        Parity {
            case: "a MOCK whose json request template names 200",
            path_part: "pets",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": {
                    "application/json": "{\"statusCode\": 200}",
                    "application/xml": "{\"statusCode\": 404}"
                }
            }),
            method_responses: vec![
                (
                    "200",
                    json!({ "responseParameters": { response_header("X-Pets"): true } }),
                ),
                ("404", json!({})),
            ],
            integration_responses: vec![
                (
                    "200",
                    json!({
                        "responseTemplates": {
                            "application/json": "{\"pets\":[]}",
                            "text/plain": "no pets"
                        },
                        "responseParameters": { response_header("X-Pets"): "'none'" }
                    }),
                ),
                (
                    "404",
                    json!({
                        "responseTemplates": { "application/json": "{\"message\":\"no such pet\"}" }
                    }),
                ),
            ],
            headers: vec![("Content-Type", "application/json")],
            status: 200,
            body: "{\"pets\":[]}",
            reported_headers: vec![
                ("Content-Type", "application/json"),
                ("X-Pets", "none"),
            ],
        },
        Parity {
            case: "a MOCK whose xml request template names 404",
            path_part: "pets-xml",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": {
                    "application/json": "{\"statusCode\": 200}",
                    "application/xml": "{\"statusCode\": 404}"
                }
            }),
            method_responses: vec![("200", json!({})), ("404", json!({}))],
            integration_responses: vec![
                (
                    "200",
                    json!({ "responseTemplates": { "application/json": "{\"pets\":[]}" } }),
                ),
                (
                    "404",
                    json!({
                        "responseTemplates": { "application/json": "{\"message\":\"no such pet\"}" }
                    }),
                ),
            ],
            headers: vec![("Content-Type", "application/xml")],
            status: 404,
            body: "{\"message\":\"no such pet\"}",
            reported_headers: vec![("Content-Type", "application/json")],
        },
        Parity {
            case: "an accepted text/plain selects that response template",
            path_part: "pets-text",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
            }),
            method_responses: vec![("200", json!({}))],
            integration_responses: vec![(
                "200",
                json!({
                    "responseTemplates": {
                        "application/json": "{\"pets\":[]}",
                        "text/plain": "no pets"
                    }
                }),
            )],
            headers: vec![("Content-Type", "application/json"), ("Accept", "text/plain")],
            status: 200,
            body: "no pets",
            // The accepted template decides the BODY; with no `Content-Type` mapping
            // declared, the media type stays `application/json` on both sides.
            reported_headers: vec![("Content-Type", "application/json")],
        },
        Parity {
            case: "a POST method answering its own status",
            path_part: "created",
            verb: "POST",
            integration: json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 201}" }
            }),
            method_responses: vec![("201", json!({}))],
            integration_responses: vec![(
                "201",
                json!({ "responseTemplates": { "application/json": "{\"created\":true}" } }),
            )],
            headers: vec![("Content-Type", "application/json")],
            status: 201,
            body: "{\"created\":true}",
            reported_headers: vec![("Content-Type", "application/json")],
        },
        Parity {
            case: "an integration naming a Lambda backend that cannot be called",
            path_part: "proxy",
            verb: "GET",
            integration: json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": aws_integration_uri(
                    "lambda:path/2015-03-31/functions/\
                     arn:aws:lambda:us-east-1:123456789012:function:handler/invocations"
                )
            }),
            method_responses: vec![("200", json!({}))],
            integration_responses: vec![],
            headers: vec![("Content-Type", "application/json")],
            status: 500,
            body: "{\"message\":\"Internal server error\"}",
            reported_headers: vec![("Content-Type", "application/json")],
        },
        Parity {
            case: "a MOCK whose response template is a Velocity template",
            path_part: "velocity",
            verb: "GET",
            integration: json!({ "type": "MOCK" }),
            method_responses: vec![("200", json!({}))],
            integration_responses: vec![(
                "200",
                json!({
                    "responseTemplates": {
                        "application/json": "#set($root = $input.path('$'))\n{\"id\":\"$root.id\"}"
                    }
                }),
            )],
            headers: vec![("Content-Type", "application/json")],
            status: 500,
            body: "{\"message\":\"Internal server error\"}",
            reported_headers: vec![("Content-Type", "application/json")],
        },
        Parity {
            case: "a MOCK that passes no unmapped media type through",
            path_part: "strict",
            verb: "GET",
            integration: json!({
                "type": "MOCK",
                "passthroughBehavior": "NEVER",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" }
            }),
            method_responses: vec![("200", json!({}))],
            integration_responses: vec![(
                "200",
                json!({ "responseTemplates": { "application/json": "{\"pets\":[]}" } }),
            )],
            headers: vec![("Content-Type", "text/csv")],
            status: 415,
            body: "{\"message\":\"Unsupported Media Type\"}",
            reported_headers: vec![("Content-Type", "application/json")],
        },
    ];

    // Every row's records are written first, and ONE deployment then carries them all: the
    // data plane serves what a deployment holds, so the stage has to be deployed after the
    // last method is put.
    let mut resources: Vec<String> = Vec::with_capacity(cases.len());
    for row in &cases {
        let resource = resource_under(&gateway, &api, &root, row.path_part).await;
        put_method(
            &gateway,
            &api,
            &resource,
            row.verb,
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        integration(&gateway, &api, &resource, row.verb, row.integration.clone()).await;
        for (status, request) in &row.method_responses {
            method_response(&gateway, &api, &resource, row.verb, status, request.clone()).await;
        }
        for (status, request) in &row.integration_responses {
            integration_response(&gateway, &api, &resource, row.verb, status, request.clone()).await;
        }
        resources.push(resource);
    }
    deploy(&gateway, &api, STAGE).await;

    for (row, resource) in cases.iter().zip(&resources) {
        let case = row.case;
        let path = format!("/{STAGE}/{}", row.path_part);
        // Before: what the endpoint serves with no simulation having happened.
        let before = invoke(&gateway, &api, row.verb, &path, &row.headers).await;
        assert_eq!(
            before.status, row.status,
            "case {case:?}: expected the stage to serve {}, got {} with {:?}",
            row.status, before.status, before.body
        );
        assert_eq!(
            before.body, row.body,
            "case {case:?}: expected the stage to serve the body {:?}, got {:?}",
            row.body, before.body
        );

        let simulated = test_invoke(
            &gateway,
            &api,
            resource,
            row.verb,
            simulated_request(&row.headers),
        )
        .await;
        assert_same_answer(case, &simulated, &before);
        // The headers the simulation must REPORT, named on their own: the cross-check above
        // iterates what the simulation reported, so a simulation that dropped a mapped
        // header (or reported none at all) would pass it vacuously.
        let reported = simulated.headers();
        for (name, value) in &row.reported_headers {
            assert_eq!(
                reported.get(*name).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected the simulation to report {name:?} = {value:?}, \
                 got {reported:?}"
            );
        }

        // After: the simulation is a read, so the endpoint answers the same request the
        // same way, and a repeated simulation reports the same answer again.
        let after = invoke(&gateway, &api, row.verb, &path, &row.headers).await;
        assert_eq!(
            after.status, before.status,
            "case {case:?}: expected the served status to be unchanged by the simulation ({}), \
             got {}",
            before.status, after.status
        );
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the served body to be unchanged by the simulation ({:?}), \
             got {:?}",
            before.body, after.body
        );
        let repeated = test_invoke(
            &gateway,
            &api,
            resource,
            row.verb,
            simulated_request(&row.headers),
        )
        .await;
        assert_same_answer(case, &repeated, &after);
    }
}

/// One method whose authorization the data plane decides on its own, and what each half
/// answers an anonymous, keyless caller.
struct Gated {
    /// What makes this row its own case.
    case: &'static str,
    /// The path part of the resource, which is also the path the data plane addresses and
    /// the marker the served body carries.
    path_part: &'static str,
    /// Whether the method's `authorizationType` is `CUSTOM`, in which case it names the
    /// API's authorizer — the one value of the member that needs a second record.
    custom: bool,
    /// The `PutMethod` request, less the `authorizerId` a `CUSTOM` row has filled in.
    method: Value,
    /// The status the data plane answers an anonymous, keyless caller.
    refused_status: u16,
    /// The `message` of the gateway response the data plane answers that caller with, or
    /// `None` for a row the data plane serves.
    refused_message: Option<&'static str>,
}

/// `CreateAuthorizer` of a `TOKEN` authorizer reading its identity source from a request
/// header, answering its id. The `CUSTOM` rows name it, which is what makes them a method
/// whose authorization the data plane can decide (refusing the call before the authorizer
/// would be reached) while the simulation does not decide it at all.
async fn create_authorizer(gateway: &Gateway, api: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": "token-authorizer",
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": aws_integration_uri(
                    "lambda:path/2015-03-31/functions/\
                     arn:aws:lambda:us-east-1:123456789012:function:authorize/invocations"
                ),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateAuthorizer: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateAuthorizer")
}

#[tokio::test]
async fn the_simulation_applies_neither_the_authorization_type_nor_the_api_key_gate() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "test-invoke-authorization").await;
    let authorizer = create_authorizer(&gateway, &api).await;

    // Rows differ along the method's authorization: the open case, each
    // `Method$authorizationType` the data plane can decide without a backend, and the API
    // key gate. Every row carries the SAME MOCK integration, so what differs in the
    // data-plane answer is the authorization alone — and the simulated answer is the
    // integration's response in every row.
    let cases: Vec<Gated> = vec![
        Gated {
            case: "an open method",
            path_part: "open",
            custom: false,
            method: json!({ "authorizationType": "NONE" }),
            refused_status: 200,
            refused_message: None,
        },
        Gated {
            case: "an AWS_IAM method reached with no credential",
            path_part: "iam",
            custom: false,
            method: json!({ "authorizationType": "AWS_IAM" }),
            refused_status: 403,
            refused_message: Some("Missing Authentication Token"),
        },
        Gated {
            case: "a CUSTOM method whose identity source is absent",
            path_part: "custom",
            custom: true,
            method: json!({ "authorizationType": "CUSTOM" }),
            refused_status: 401,
            refused_message: Some("Unauthorized"),
        },
        Gated {
            case: "a method requiring an API key reached with none",
            path_part: "keyed",
            custom: false,
            method: json!({ "authorizationType": "NONE", "apiKeyRequired": true }),
            refused_status: 403,
            refused_message: Some("Forbidden"),
        },
    ];

    let mut resources: Vec<String> = Vec::with_capacity(cases.len());
    for row in &cases {
        let resource = resource_under(&gateway, &api, &root, row.path_part).await;
        let mut request = row.method.clone();
        if row.custom {
            request["authorizerId"] = json!(authorizer);
        }
        put_method(&gateway, &api, &resource, "GET", request).await;
        integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
        method_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
        integration_response(
            &gateway,
            &api,
            &resource,
            "GET",
            "200",
            json!({
                "responseTemplates": {
                    "application/json": format!("{{\"served\":\"{}\"}}", row.path_part)
                }
            }),
        )
        .await;
        resources.push(resource);
    }
    deploy(&gateway, &api, STAGE).await;

    for (row, resource) in cases.iter().zip(&resources) {
        let case = row.case;
        let path = format!("/{STAGE}/{}", row.path_part);
        let served_body = format!("{{\"served\":\"{}\"}}", row.path_part);

        // The data plane applies the method's authorization to an anonymous, keyless
        // caller.
        let answered = invoke(&gateway, &api, "GET", &path, &[]).await;
        assert_eq!(
            answered.status, row.refused_status,
            "case {case:?}: expected the stage to answer {}, got {} with {:?}",
            row.refused_status, answered.status, answered.body
        );
        match row.refused_message {
            Some(message) => assert_eq!(
                answered.message().as_deref(),
                Some(message),
                "case {case:?}: expected the gateway response message {message:?}, got {:?}",
                answered.body
            ),
            None => assert_eq!(
                answered.body, served_body,
                "case {case:?}: expected the stage to serve {served_body:?}, got {:?}",
                answered.body
            ),
        }

        // The simulation does not: it reports the integration's own response for every row,
        // whatever the data plane answered.
        let simulated = test_invoke(&gateway, &api, resource, "GET", json!({})).await;
        assert_eq!(
            simulated.http_status, 200,
            "case {case:?}: expected TestInvokeMethod to answer 200, got {} with {:?}",
            simulated.http_status, simulated.body
        );
        assert_eq!(
            simulated.status(),
            200,
            "case {case:?}: expected the simulated status 200, got {:?}",
            simulated.body
        );
        assert_eq!(
            simulated.simulated_body(),
            served_body,
            "case {case:?}: expected the simulated body {served_body:?}, got {:?}",
            simulated.simulated_body()
        );

        // And it granted nothing: the same anonymous, keyless request is answered exactly
        // as it was before the simulation.
        let again = invoke(&gateway, &api, "GET", &path, &[]).await;
        assert_eq!(
            again.status, answered.status,
            "case {case:?}: expected the served status to be unchanged by the simulation ({}), \
             got {}",
            answered.status, again.status
        );
        assert_eq!(
            again.body, answered.body,
            "case {case:?}: expected the served body to be unchanged by the simulation ({:?}), \
             got {:?}",
            answered.body, again.body
        );
    }
}
