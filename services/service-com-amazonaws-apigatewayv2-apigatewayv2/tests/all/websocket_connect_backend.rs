// SPDX-License-Identifier: CC0-1.0

//! The DATA MAPPING a WEBSOCKET route's integration applies to the call it fronts, and the
//! one route of a WEBSOCKET api that may DECLARE request parameters at all.
//!
//! The rule both tables below are slices of: **a WEBSOCKET route's integration passes the
//! call to its backend as its `RequestParameters` map it — each
//! `integration.request.{location}.{name}` key carrying either a request parameter of the
//! call, written `route.request.{location}.{name}` or `method.request.{location}.{name}`, or
//! a static value "enclosed within single quotes" — so the function reads the destination
//! name the integration chose, while an entry whose source the call never carried maps
//! nothing at all; and a ROUTE may DECLARE `Route$RequestParameters` only on `$connect`, the
//! route that serves the connection request, every other route of such an api being refused
//! `400` `BadRequestException`.**
//!
//! Nothing in the first table is a value this emulator fills in: the mapping is asserted
//! through the DECISION of a function that ran, which is why AWS Lambda is mounted beside
//! this service on one engine — the `$connect` handler admits the connection only when the
//! event it was handed carries what the mapping was supposed to put there, so a completed
//! handshake IS the assertion and a refused one is its negation.
//!
//! The rest of the WebSocket lifecycle — the event each call is stated in, the routes a frame
//! is dispatched to, the `$disconnect` call and the `REQUEST` authorizer in front of
//! `$connect` — is `websocket_lambda_routes.rs`'s, and the handshake's own statuses are
//! `websocket_transport_data_plane.rs`'s; neither is restated here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_in_region, emulator, error_code};
use crate::http_api_backend_dispatch::{emulator_with_lambda, given_function};
use crate::websocket_handshake_data_plane::CLIENT_KEY;
use crate::websocket_transport_data_plane::{dial_loopback, ClientSocket};

/// The Region every request below is signed for — the one the Lambda harness creates its
/// functions in, so the api and the function it names live in the same Region.
const REGION: &str = "us-east-1";

/// The stage every api below deploys.
const STAGE: &str = "prod";

/// The header a case's client sends on the handshake, and its value.
const SOURCE_HEADER: &str = "X-Client-Trace";
const SOURCE_VALUE: &str = "trace-7";

/// The query-string parameter a case's client sends, and its value.
const SOURCE_QUERY: &str = "token";
const SOURCE_QUERY_VALUE: &str = "t-9";

/// The status a completed handshake carries (RFC 6455 §4.1), and the one a gateway answers
/// for a connection request whose function refused it.
const SWITCHING_PROTOCOLS: u16 = 101;
const FORBIDDEN: u16 = 403;

/// The sentence a route that may not declare request parameters is refused with. Fixed text:
/// it names nothing of the request.
const CONNECT_ONLY_MESSAGE: &str =
    "Request parameters are only supported for the $connect route in WEBSOCKET APIs";

/// A WebSocket api whose messages select a route by their `action` member.
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

/// One control-plane request against this service, signed for [`REGION`].
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

/// Create one resource under an api and answer the created body, asserting the create was
/// accepted.
async fn created(
    emulator: &MultiServiceRouter,
    case: &str,
    uri: &str,
    body: Value,
    what: &str,
) -> Value {
    let (status, created) = control(emulator, "POST", uri, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: {what} must answer 201, got {status} {created}"
    );
    created
}

/// A `$connect` handler that admits the connection only when `condition` holds of the event
/// it was handed, and refuses it with a `403` otherwise.
///
/// A condition that cannot even be evaluated — a member the event does not carry — raises,
/// which Lambda reports as a function error and the gateway answers as a connection request
/// that produced no reply: a handshake that does not complete either way, which is what
/// every negative row below asserts.
fn deciding_handler(condition: &str) -> String {
    format!(
        "def lambda_handler(event, context):\n\
         \x20   return {{'statusCode': 200 if ({condition}) else 403}}\n"
    )
}

/// Open an RFC 6455 handshake against the api's stage, carrying [`SOURCE_HEADER`] and
/// [`SOURCE_QUERY`]: the established socket, or the status a refusal carried.
///
/// The RFC 6455 §4.1 fields are written here rather than left to the client library, because
/// a request built field by field — the only way to carry the header a case maps — is sent
/// as written.
async fn handshake(case: &str, authority: &str) -> Result<ClientSocket, u16> {
    let stream = dial_loopback(case, authority).await.unwrap_or_else(|error| {
        panic!("case {case:?}: the announced socket {authority:?} must accept a connection, got {error}")
    });
    let request = http::Request::builder()
        .uri(format!(
            "ws://{authority}/{STAGE}?{SOURCE_QUERY}={SOURCE_QUERY_VALUE}"
        ))
        .header("host", authority)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", CLIENT_KEY)
        .header(SOURCE_HEADER, SOURCE_VALUE)
        .body(())
        .expect("the handshake request builds");
    match tokio_tungstenite::client_async(request, stream).await {
        Ok((socket, response)) => {
            assert_eq!(
                response.status().as_u16(),
                SWITCHING_PROTOCOLS,
                "case {case:?}: a completed handshake must answer 101"
            );
            Ok(socket)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(error) => panic!("case {case:?}: the handshake failed before any answer: {error}"),
    }
}

#[tokio::test]
async fn an_integrations_request_parameters_map_onto_what_the_connect_function_reads() {
    // Rows differ along WHAT the entry maps and WHERE: each destination location, each
    // spelling and each form a source takes, a destination colliding with a field the client
    // sent, and — the negative rows — a source the handshake never carried, a destination
    // this data plane does not read, and a function that refuses outright.
    let cases: &[(&str, Value, &str, bool)] = &[
        (
            "a header destination carrying the client's header",
            json!({ "integration.request.header.X-Forwarded-Trace": "route.request.header.X-Client-Trace" }),
            "event['headers']['X-Forwarded-Trace'] == 'trace-7'",
            true,
        ),
        (
            "the same source written as the model documents it",
            json!({ "integration.request.header.X-Forwarded-Trace": "method.request.header.x-client-trace" }),
            "event['headers']['X-Forwarded-Trace'] == 'trace-7'",
            true,
        ),
        (
            "a header destination carrying the handshake's query parameter",
            json!({ "integration.request.header.X-Token": "route.request.querystring.token" }),
            "event['headers']['X-Token'] == 't-9'",
            true,
        ),
        (
            "a header destination carrying a static value",
            json!({ "integration.request.header.X-Fixed": "'fixed-value'" }),
            "event['headers']['X-Fixed'] == 'fixed-value'",
            true,
        ),
        (
            "a query-string destination carrying the client's header",
            json!({ "integration.request.querystring.trace": "route.request.header.X-Client-Trace" }),
            "event['queryStringParameters']['trace'] == 'trace-7'",
            true,
        ),
        (
            "a mapped header is stated in the multi-valued map as well",
            json!({ "integration.request.header.X-Forwarded-Trace": "'one'" }),
            "event['multiValueHeaders']['X-Forwarded-Trace'] == ['one']",
            true,
        ),
        (
            "a destination naming a field the client sent replaces that value",
            json!({ "integration.request.header.X-Client-Trace": "'replaced'" }),
            "event['headers']['X-Client-Trace'] == 'replaced'",
            true,
        ),
        (
            "the handshake's own fields travel on beside the mapped ones",
            json!({ "integration.request.header.X-Fixed": "'fixed-value'" }),
            // Under the name the CLIENT sent: the client library writes its fields
            // lowercased, and the event reports the spelling it was sent with.
            "event['headers']['x-client-trace'] == 'trace-7' and \
             event['queryStringParameters']['token'] == 't-9'",
            true,
        ),
        (
            "an integration that maps nothing passes the call through",
            json!({}),
            "event['headers']['x-client-trace'] == 'trace-7'",
            true,
        ),
        // The negative rows: an entry that maps nothing must leave the destination OFF, and
        // a function that refuses must fail the handshake — so the rows above are not
        // passing for want of any decision at all.
        (
            "a source the handshake never carried maps nothing",
            json!({ "integration.request.header.X-Absent": "route.request.header.X-Never-Sent" }),
            "'X-Absent' in event['headers']",
            false,
        ),
        (
            "a key naming no integration request parameter maps nothing",
            json!({ "x-not-a-destination": "'v'" }),
            "'x-not-a-destination' in event['headers']",
            false,
        ),
        (
            "a function that answers a non-2xx status fails the handshake",
            json!({ "integration.request.header.X-Fixed": "'fixed-value'" }),
            "False",
            false,
        ),
    ];
    for (case, request_parameters, condition, admitted) in cases {
        let (emulator, engine) = emulator_with_lambda();
        let api = created(
            &emulator,
            case,
            "/v2/apis",
            websocket_api("mapped-connect"),
            "CreateApi",
        )
        .await;
        let api_id = as_str(&api, "apiId");
        let announced = as_str(&api, "apiEndpoint");
        let authority = announced
            .strip_prefix("wss://")
            .unwrap_or_else(|| {
                panic!("case {case:?}: a WEBSOCKET api announces a wss:// socket, got {announced:?}")
            })
            .to_string();
        let function = given_function(
            &emulator,
            &engine,
            case,
            "connect-handler",
            &deciding_handler(condition),
        )
        .await;
        let integration = created(
            &emulator,
            case,
            &format!("/v2/apis/{api_id}/integrations"),
            json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": function,
                "requestParameters": request_parameters
            }),
            "CreateIntegration",
        )
        .await;
        created(
            &emulator,
            case,
            &format!("/v2/apis/{api_id}/routes"),
            json!({
                "routeKey": "$connect",
                "target": format!("integrations/{}", as_str(&integration, "integrationId")),
                "requestParameters": {
                    format!("route.request.header.{SOURCE_HEADER}"): { "required": true }
                }
            }),
            "CreateRoute",
        )
        .await;
        created(
            &emulator,
            case,
            &format!("/v2/apis/{api_id}/stages"),
            json!({ "stageName": STAGE }),
            "CreateStage",
        )
        .await;
        created(
            &emulator,
            case,
            &format!("/v2/apis/{api_id}/deployments"),
            json!({ "stageName": STAGE }),
            "CreateDeployment",
        )
        .await;

        let opened = handshake(case, &authority).await;
        match (admitted, &opened) {
            (true, Ok(_)) => {}
            (false, Err(status)) => assert_eq!(
                *status, FORBIDDEN,
                "case {case:?}: expected the status the function answered ({FORBIDDEN}), got \
                 {status}"
            ),
            (true, Err(status)) => panic!(
                "case {case:?}: expected the handshake to complete, got {status}: the \
                 condition {condition:?} did not hold of the $connect event"
            ),
            (false, Ok(_)) => panic!(
                "case {case:?}: expected the handshake to be refused, got \
                 {SWITCHING_PROTOCOLS}: the condition {condition:?} held of the $connect \
                 event"
            ),
        }
    }
}

#[tokio::test]
async fn request_parameters_are_declared_only_on_the_connection_requests_route() {
    let emulator = emulator();
    let declared = json!({ "route.request.header.x-client-trace": { "required": true } });
    // Rows differ along the route key the declaration arrives on: the connection request's,
    // each other reserved key, a key of the api's own naming, and a route declaring an
    // empty map (which declares nothing and so is accepted anywhere).
    let cases: &[(&str, &str, Value, bool)] = &[
        (
            "the connection request's route",
            "$connect",
            declared.clone(),
            true,
        ),
        ("the close's route", "$disconnect", declared.clone(), false),
        ("the catch-all route", "$default", declared.clone(), false),
        (
            "a route of the api's own naming",
            "sendmessage",
            declared.clone(),
            false,
        ),
        (
            "a route declaring an empty map",
            "broadcast",
            json!({}),
            true,
        ),
    ];
    for (case, route_key, parameters, accepted) in cases {
        let api = created(
            &emulator,
            case,
            "/v2/apis",
            websocket_api(&format!("declaring-{}", route_key.replace('$', ""))),
            "CreateApi",
        )
        .await;
        let api_id = as_str(&api, "apiId");
        let (status, body) = control(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": route_key, "requestParameters": parameters })),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateRoute, got {status} {body}"
            );
            // Read it back through GetRoute: what the create reported is what a later
            // caller reads.
            let route_id = as_str(&body, "routeId");
            let (status, fetched) = control(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetRoute, got {status} {fetched}"
            );
            assert_eq!(
                fetched.get("requestParameters"),
                Some(parameters),
                "case {case:?}: expected GetRoute to report the declaration, got {fetched}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateRoute, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            CONNECT_ONLY_MESSAGE,
            "case {case:?}: expected the refusal's own sentence, got {body}"
        );
        // Nothing was written: the api still holds no route.
        let (status, routes) =
            control(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoutes, got {status} {routes}"
        );
        assert_eq!(
            routes.get("items").and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {case:?}: expected the refused create to have written nothing, got {routes}"
        );
    }
}

#[tokio::test]
async fn a_patch_that_leaves_request_parameters_off_the_connection_requests_route_is_refused() {
    let emulator = emulator();
    let declared = json!({ "route.request.header.x-client-trace": { "required": true } });
    let api = created(
        &emulator,
        "patching a declaration",
        "/v2/apis",
        websocket_api("patch-declaring"),
        "CreateApi",
    )
    .await;
    let api_id = as_str(&api, "apiId");
    let connect = created(
        &emulator,
        "the connection request's route",
        &format!("/v2/apis/{api_id}/routes"),
        json!({ "routeKey": "$connect", "requestParameters": declared.clone() }),
        "CreateRoute",
    )
    .await;
    let other = created(
        &emulator,
        "another route of the api",
        &format!("/v2/apis/{api_id}/routes"),
        json!({ "routeKey": "sendmessage" }),
        "CreateRoute",
    )
    .await;

    // Rows differ along HOW the merged route would end up declaring parameters off
    // `$connect`: adding them to another route, and moving the declaring route onto another
    // key; and — the accepted rows — patching the declaring route's own map, and giving the
    // declaration up in the same patch that moves the key.
    let cases: &[(&str, &str, Value, bool)] = &[
        (
            "parameters added to another route",
            &as_str(&other, "routeId"),
            json!({ "requestParameters": declared.clone() }),
            false,
        ),
        (
            "the declaring route moved onto another key",
            &as_str(&connect, "routeId"),
            json!({ "routeKey": "broadcast" }),
            false,
        ),
        (
            "the declaring route's own map replaced",
            &as_str(&connect, "routeId"),
            json!({ "requestParameters": { "route.request.querystring.token": { "required": false } } }),
            true,
        ),
        (
            "the declaration given up in the patch that moves the key",
            &as_str(&connect, "routeId"),
            json!({ "routeKey": "relay", "requestParameters": {} }),
            true,
        ),
    ];
    for (case, route_id, patch, accepted) in cases {
        let (status, body) = control(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            Some(patch.clone()),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from UpdateRoute, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateRoute, got {status} {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            CONNECT_ONLY_MESSAGE,
            "case {case:?}: expected the refusal's own sentence, got {body}"
        );
    }
}
