// SPDX-License-Identifier: CC0-1.0

//! What a WEBSOCKET route's integration is given about the call it serves, over a real
//! connection: the `$context` a MOCK integration's mapping templates resolve, and the
//! `@connections` management API the function an `AWS_PROXY` route fronts reaches through the
//! SDK its runtime supplies.
//!
//! The rule both tables below are slices of: **one call of a WebSocket connection carries ONE
//! context — what the gateway reports about that call — and every reader of a call reads the
//! same one: a mapping template resolves `$context.<member>` for each member the call reports
//! (and `$stageVariables.<name>` for the serving stage's own variables), and a function the
//! route fronts is handed that context as its event's `requestContext` and can call the
//! `@connections` API named in it, so a payload it posts to a connection id is delivered to
//! THAT connection.**
//!
//! Nothing here is a value this emulator fills in for the test: each assertion is on a value
//! the gateway computed for the call (the route that served it, the stage, the api, the
//! address the client dialled) or on bytes a RUNNING function pushed over the socket, which
//! is why AWS Lambda and the `@connections` management API are mounted beside this service on
//! one engine.
//!
//! The suites that own the steps before the integration are not restated here: the
//! handshake's own statuses are `websocket_transport_data_plane.rs`, the event each call hands
//! a function is `websocket_lambda_routes.rs`, and the `@connections` calls a backend makes
//! over the api's own socket are `websocket_endpoint_connections.rs`.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use crate::http_api_backend_dispatch::given_function;
use crate::websocket_transport_data_plane::{handshake_at, ClientSocket};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The stage every api below is deployed to.
const STAGE: &str = "prod";

/// The variables that stage carries, which `$stageVariables` names.
const STAGE_VARIABLE: (&str, &str) = ("tenant", "acme");

/// The source address a connection on the core's loopback listener is attributed to.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// How long a case waits for one frame before failing: generous for a loaded test machine —
/// a frame may be the answer of a function this emulator has to run first — and bounded so a
/// case that will never be answered fails instead of hanging the suite.
const FRAME_DEADLINE: Duration = Duration::from_secs(30);

/// How often a case looks again for a state the call it already made owes — the record of a
/// connection the client closed.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The status each `@connections` operation answers for a connection that no longer exists
/// (`GoneException`, `@httpError(410)`).
const GONE: u16 = 410;

/// This service, the `@connections` management API and AWS Lambda on ONE engine: a
/// connection this api's socket carries is the connection that API addresses, and a route's
/// function is run by that Lambda.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One request against `host`, signed for `service`: its status and parsed body.
async fn signed(
    emulator: &MultiServiceRouter,
    host: &str,
    service: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("the body serializes")),
        None => bytes::Bytes::new(),
    };
    let request = smithy_server_runtime::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization(service))
        .body(payload)
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The characters a `ConnectionId` may carry into a URI path segment without being encoded:
/// the RFC 3986 unreserved set, which is what an SDK client leaves alone. A minted id is
/// base64, whose alphabet carries `/`, `+` and `=`.
const UNRESERVED: percent_encoding::AsciiSet = percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// What one `@connections` operation on `connection_id` answered, addressed at the
/// documented `…/{stage}/@connections/{id}` path with the id encoded the way an SDK client
/// encodes a path label.
async fn connections_call(
    emulator: &MultiServiceRouter,
    method: &str,
    connection_id: &str,
) -> (u16, Value) {
    let addressed = percent_encoding::utf8_percent_encode(connection_id, &UNRESERVED).to_string();
    signed(
        emulator,
        &format!("execute-api.{REGION}.amazonaws.com"),
        "execute-api",
        method,
        &format!("/{STAGE}/@connections/{addressed}"),
        None,
    )
    .await
}

/// One control-plane request against THIS service.
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    signed(
        emulator,
        &format!("apigateway.{REGION}.amazonaws.com"),
        "apigateway",
        method,
        uri,
        body,
    )
    .await
}

/// One control-plane request that must answer `201`.
async fn created(
    emulator: &MultiServiceRouter,
    case: &str,
    what: &str,
    uri: &str,
    body: Value,
) -> Value {
    let (status, answered) = control(emulator, "POST", uri, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: {what} must answer 201, got {status} {answered}"
    );
    answered
}

/// The member `name` of `body`, as text.
fn text(case: &str, what: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: {what} answered no {name}: {body}"))
        .to_string()
}

/// One api, created with the socket its `ApiEndpoint` announces.
struct Api {
    api_id: String,
    /// The `host:port` of the api's own socket, read off the announced `ApiEndpoint` so a
    /// case dials the address the service reported.
    authority: String,
}

/// Create a WEBSOCKET api whose frames select a route by their `action` member.
async fn given_api(emulator: &MultiServiceRouter, case: &str, name: &str) -> Api {
    let created = created(
        emulator,
        case,
        "CreateApi",
        "/v2/apis",
        json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    let announced = text(case, "CreateApi", &created, "apiEndpoint");
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a WEBSOCKET api must announce a wss:// socket, got {announced:?}")
        })
        .to_string();
    Api {
        api_id: text(case, "CreateApi", &created, "apiId"),
        authority,
    }
}

/// Route `route_key` of `api_id` to a `MOCK` integration whose selected response renders
/// `template`, so what the client reads back IS that template rendered against the call.
async fn given_mock_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    template: &str,
) {
    let integration = created(
        emulator,
        case,
        "CreateIntegration",
        &format!("/v2/apis/{api_id}/integrations"),
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    let integration_id = text(case, "CreateIntegration", &integration, "integrationId");
    created(
        emulator,
        case,
        "CreateIntegrationResponse",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
        json!({
            "integrationResponseKey": "$default",
            "responseTemplates": { "$default": template }
        }),
    )
    .await;
    given_route(emulator, case, api_id, route_key, &integration_id, true).await;
}

/// Route `route_key` of `api_id` to the function `function_arn` through an `AWS_PROXY`
/// integration, two-way when `two_way`.
async fn given_lambda_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    function_arn: &str,
    two_way: bool,
) {
    let integration = created(
        emulator,
        case,
        "CreateIntegration",
        &format!("/v2/apis/{api_id}/integrations"),
        json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": function_arn,
            "integrationMethod": "POST"
        }),
    )
    .await;
    let integration_id = text(case, "CreateIntegration", &integration, "integrationId");
    given_route(emulator, case, api_id, route_key, &integration_id, two_way).await;
}

/// Create the route and, when `two_way`, the `RouteResponse` that makes it two-way — "to
/// define a two-way route, create a route response".
async fn given_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    integration_id: &str,
    two_way: bool,
) {
    let route = created(
        emulator,
        case,
        "CreateRoute",
        &format!("/v2/apis/{api_id}/routes"),
        json!({
            "routeKey": route_key,
            "target": format!("integrations/{integration_id}")
        }),
    )
    .await;
    if two_way {
        let route_id = text(case, "CreateRoute", &route, "routeId");
        created(
            emulator,
            case,
            "CreateRouteResponse",
            &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
            json!({ "routeResponseKey": "$default" }),
        )
        .await;
    }
}

/// Create the stage every case dials, with the variables `$stageVariables` names, and
/// release a deployment to it.
async fn given_stage(emulator: &MultiServiceRouter, case: &str, api_id: &str) {
    let (name, value) = STAGE_VARIABLE;
    created(
        emulator,
        case,
        "CreateStage",
        &format!("/v2/apis/{api_id}/stages"),
        json!({ "stageName": STAGE, "stageVariables": { name: value } }),
    )
    .await;
    created(
        emulator,
        case,
        "CreateDeployment",
        &format!("/v2/apis/{api_id}/deployments"),
        json!({ "stageName": STAGE }),
    )
    .await;
}

/// Send one text frame.
async fn send(case: &str, socket: &mut ClientSocket, message: &str) {
    socket
        .send(Message::Text(message.to_string().into()))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the frame must be sent, got {error}"));
}

/// The next frame this socket is sent, as its text; a socket that is sent none within
/// [`FRAME_DEADLINE`] answers `None`.
async fn next_frame(case: &str, socket: &mut ClientSocket) -> Option<String> {
    match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(Some(Ok(Message::Text(frame)))) => Some(frame.as_str().to_string()),
        Ok(Some(Ok(Message::Binary(frame)))) => Some(String::from_utf8_lossy(&frame).to_string()),
        Ok(Some(Ok(other))) => panic!("case {case:?}: unexpected frame {other:?}"),
        Ok(Some(Err(error))) => panic!("case {case:?}: the socket failed: {error}"),
        Ok(None) => panic!("case {case:?}: the socket closed instead of answering"),
        Err(_elapsed) => None,
    }
}

/// Send one frame and read the frame it is answered with.
async fn exchanged(case: &str, socket: &mut ClientSocket, message: &str) -> String {
    send(case, socket, message).await;
    next_frame(case, socket)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: {message} must be answered on the same socket"))
}

/// What a rendered value must be: the one text the call's own records give it, or the shape
/// of a value this run mints.
#[derive(Debug)]
enum Expect {
    /// Exactly this text, which the api's records decide.
    Exact(String),
    /// A non-empty value this call minted (an id of the call itself).
    NonEmpty,
    /// A number, which is what a template renders an instant of the call as.
    Digits,
}

/// Every `$context` member a WebSocket call reports resolves in the MOCK integration's
/// mapping template that reads it, with the value the call itself has.
#[tokio::test]
async fn a_mock_routes_template_resolves_the_context_of_the_call_it_serves() {
    let case = "a frame answered by a MOCK integration's response template";
    let (emulator, _engine) = emulator();
    let api = given_api(&emulator, case, "socket-call-context").await;

    // One route per reference under test, each fronting a MOCK integration whose selected
    // response is that reference alone — so the frame the client reads back IS the rendered
    // value. The rows span what the rule generalises over: each member the gateway reports
    // about a message call, a NESTED member (reached through the dotted path its nesting
    // gives it), the serving stage's own variables, and a template mixing the context with
    // the frame's payload. `routeKey` differs per row by construction: it is the route that
    // served the call.
    let rows: Vec<(&str, String, Expect)> = vec![
        (
            "the connection the frame arrived on",
            "$context.connectionId".to_string(),
            Expect::NonEmpty,
        ),
        (
            "the route that served the call",
            "$context.routeKey".to_string(),
            Expect::Exact("ctx-route-key".to_string()),
        ),
        (
            "the kind of call",
            "$context.eventType".to_string(),
            Expect::Exact("MESSAGE".to_string()),
        ),
        (
            "its direction",
            "$context.messageDirection".to_string(),
            Expect::Exact("IN".to_string()),
        ),
        (
            "the serving stage",
            "$context.stage".to_string(),
            Expect::Exact(STAGE.to_string()),
        ),
        (
            "the api",
            "$context.apiId".to_string(),
            Expect::Exact(api.api_id.clone()),
        ),
        (
            "the address the client dialled",
            "$context.domainName".to_string(),
            Expect::Exact(api.authority.clone()),
        ),
        (
            "a nested member of the context",
            "$context.identity.sourceIp".to_string(),
            Expect::Exact(LOOPBACK_SOURCE_IP.to_string()),
        ),
        (
            "the id of this call",
            "$context.requestId".to_string(),
            Expect::NonEmpty,
        ),
        (
            "when the connection was made",
            "$context.connectedAt".to_string(),
            Expect::Digits,
        ),
        (
            "a variable of the serving stage",
            format!("$stageVariables.{}", STAGE_VARIABLE.0),
            Expect::Exact(STAGE_VARIABLE.1.to_string()),
        ),
        (
            "the context beside the frame's own payload",
            r#"{"route":"$context.routeKey","sent":$input.json('$.action')}"#.to_string(),
            Expect::Exact(
                r#"{"route":"ctx-with-payload","sent":"ctx-with-payload"}"#.to_string(),
            ),
        ),
    ];
    // The route key each row's frame selects: the row's own name for the members whose
    // value does not depend on it, and a spelled-out key for the two rows that assert on
    // the route itself.
    let route_keys: Vec<String> = rows
        .iter()
        .enumerate()
        .map(|(at, (_what, template, _expected))| {
            if template.contains("routeKey") && template.starts_with('{') {
                "ctx-with-payload".to_string()
            } else if template.contains("routeKey") {
                "ctx-route-key".to_string()
            } else {
                format!("ctx-member-{at}")
            }
        })
        .collect();

    for ((_what, template, _expected), route_key) in rows.iter().zip(&route_keys) {
        given_mock_route(&emulator, case, &api.api_id, route_key, template).await;
    }
    given_stage(&emulator, case, &api.api_id).await;

    let mut socket: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must be completed, got {status}")
        });

    let mut rendered_connection_id = String::new();
    for ((what, template, expected), route_key) in rows.iter().zip(&route_keys) {
        let frame = exchanged(case, &mut socket, &json!({ "action": route_key }).to_string()).await;
        match expected {
            Expect::Exact(value) => assert_eq!(
                &frame, value,
                "case {case:?}: the template {template:?} must render {what} as {value:?}, got \
                 {frame:?}"
            ),
            Expect::NonEmpty => assert!(
                !frame.trim().is_empty(),
                "case {case:?}: the template {template:?} must render {what}, got {frame:?}"
            ),
            Expect::Digits => assert!(
                !frame.is_empty() && frame.chars().all(|glyph| glyph.is_ascii_digit()),
                "case {case:?}: the template {template:?} must render {what} as a number, got \
                 {frame:?}"
            ),
        }
        if template == "$context.connectionId" {
            rendered_connection_id = frame;
        }
    }

    // The id the template rendered is the connection's own, not a value this emulator made
    // up for the template: the `@connections` management API reports that connection.
    let (status, reported) = connections_call(&emulator, "GET", &rendered_connection_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnection on the rendered id {rendered_connection_id:?} must answer \
         200, got {status} {reported}"
    );
    assert!(
        reported.get("connectedAt").is_some(),
        "case {case:?}: the reported connection must carry connectedAt, got {reported}"
    );
}

/// The CLOSING handshake and what it leaves behind: a client's close frame is answered with
/// a close frame, and the id that connection was minted then addresses a connection that no
/// longer exists.
#[tokio::test]
async fn a_clients_close_frame_is_answered_with_a_close_frame() {
    let case = "a client closing a carried connection";
    let (emulator, _engine) = emulator();
    let api = given_api(&emulator, case, "socket-closing-handshake").await;
    // One route, so a socket can learn the id its connection was minted before it closes.
    given_mock_route(
        &emulator,
        case,
        &api.api_id,
        "whoami",
        "$context.connectionId",
    )
    .await;
    given_stage(&emulator, case, &api.api_id).await;

    // The rows span what the closing handshake generalises over: the normal closure a client
    // library sends, another status with a reason phrase, and a close frame carrying no
    // status at all (RFC 6455 §7.4.1's "no status code was actually present").
    let rows: [(&str, Option<CloseFrame>); 3] = [
        (
            "the normal closure",
            Some(CloseFrame {
                code: CloseCode::Normal,
                reason: String::new().into(),
            }),
        ),
        (
            "a status with a reason",
            Some(CloseFrame {
                code: CloseCode::Away,
                reason: "going away".to_string().into(),
            }),
        ),
        ("no status at all", None),
    ];
    for (what, close) in rows {
        let mut socket: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
            .await
            .unwrap_or_else(|status| {
                panic!("case {case:?}: the handshake must be completed, got {status}")
            });
        let connection_id = exchanged(case, &mut socket, r#"{"action":"whoami"}"#).await;
        socket
            .send(Message::Close(close))
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {what} must be sent, got {error}")
            });
        match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
            Ok(Some(Ok(Message::Close(_echo)))) => {}
            other => panic!(
                "case {what:?}: the close must be answered with a close frame, got {other:?}"
            ),
        }
        // The connection's record ends with it, which the `@connections` API reports: the
        // close is a call of the connection (the `$disconnect` route's), so the record goes
        // once that call is served rather than the instant the frame arrived.
        let deadline = std::time::Instant::now() + FRAME_DEADLINE;
        let mut last = (0u16, Value::Null);
        while std::time::Instant::now() < deadline {
            last = connections_call(&emulator, "GET", &connection_id).await;
            if last.0 == GONE {
                break;
            }
            tokio::time::sleep(LOOK_INTERVAL).await;
        }
        assert_eq!(
            last.0, GONE,
            "case {what:?}: the closed connection {connection_id:?} must answer {GONE}, got {} {}",
            last.0, last.1
        );
    }
}

/// every connection a frame names through the `@connections` API of the endpoint its event
/// reports.
fn pushing_handler() -> String {
    "import json\n\
     import boto3\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   call = json.loads(event.get(\"body\") or \"{}\")\n\
     \x20   reported = event[\"requestContext\"]\n\
     \x20   if call.get(\"action\") == \"whoami\":\n\
     \x20       return {\"statusCode\": 200,\n\
     \x20               \"body\": json.dumps({\"connectionId\": reported[\"connectionId\"],\n\
     \x20                                   \"routeKey\": reported[\"routeKey\"]})}\n\
     \x20   connections = boto3.client(\n\
     \x20       \"apigatewaymanagementapi\",\n\
     \x20       endpoint_url=\"https://%s/%s\" % (reported[\"domainName\"], reported[\"stage\"]),\n\
     \x20   )\n\
     \x20   if call.get(\"action\") == \"inspect\":\n\
     \x20       known = connections.get_connection(ConnectionId=call[\"target\"])\n\
     \x20       connections.post_to_connection(\n\
     \x20           ConnectionId=reported[\"connectionId\"],\n\
     \x20           Data=json.dumps({\"connectedAt\": str(known.get(\"ConnectedAt\"))}).encode(\n\
     \x20               \"utf-8\"\n\
     \x20           ),\n\
     \x20       )\n\
     \x20       return {\"statusCode\": 200}\n\
     \x20   for target in call.get(\"targets\") or []:\n\
     \x20       connections.post_to_connection(\n\
     \x20           ConnectionId=target, Data=call[\"message\"].encode(\"utf-8\")\n\
     \x20       )\n\
     \x20   return {\"statusCode\": 200}\n"
        .to_string()
}

/// The connection id a socket learns from the two-way `whoami` route, which is the id the
/// function was given for that connection.
async fn whoami(case: &str, socket: &mut ClientSocket) -> String {
    let frame = exchanged(case, socket, r#"{"action":"whoami"}"#).await;
    let reply: Value = serde_json::from_str(&frame).unwrap_or_else(|error| {
        panic!("case {case:?}: the whoami reply must be the function's JSON body, got {frame:?} ({error})")
    });
    assert_eq!(
        reply.get("routeKey").and_then(Value::as_str),
        Some("whoami"),
        "case {case:?}: the reply must come from the whoami route, got {frame:?}"
    );
    reply
        .get("connectionId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: the reply must carry a connectionId, got {frame:?}"))
        .to_string()
}

/// A payload a route's function posts through the `@connections` API is delivered to the
/// connection it named, and to no other.
#[tokio::test]
async fn a_routes_function_posts_to_the_connections_it_names() {
    let case = "a route's function fanning a payload out over @connections";
    let (emulator, engine) = emulator();
    let function = given_function(&emulator, &engine, case, "socket-push", &pushing_handler()).await;
    let api = given_api(&emulator, case, "socket-push-api").await;
    // `whoami` is TWO-WAY, so the function's own answer reaches the client that asked; the
    // pushing routes are one-way, so the only frames they produce are the ones the function
    // posted.
    for (route_key, two_way) in [("whoami", true), ("push", false), ("inspect", false)] {
        given_lambda_route(&emulator, case, &api.api_id, route_key, &function, two_way).await;
    }
    given_stage(&emulator, case, &api.api_id).await;

    let mut first: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the first handshake must be completed, got {status}")
        });
    let mut second: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the second handshake must be completed, got {status}")
        });
    let first_id = whoami(case, &mut first).await;
    let second_id = whoami(case, &mut second).await;
    assert_ne!(
        first_id, second_id,
        "case {case:?}: each connection must be minted its own id, got {first_id:?} twice"
    );

    // The rows span what the fan-out generalises over: a payload posted to the connection
    // that sent the frame, one posted to the OTHER connection, and one posted to both. Each
    // row carries its own marker and each socket is read in row order, so a payload
    // delivered to a connection it was not posted to is read as the wrong marker by the
    // next row that reads that socket.
    let rows: [(&str, Vec<&String>, &str); 3] = [
        ("the sender's own connection", vec![&first_id], "first-only"),
        ("another connection", vec![&second_id], "second-only"),
        ("both connections", vec![&first_id, &second_id], "both"),
    ];
    for (what, targets, marker) in rows {
        let message = json!({
            "action": "push",
            "targets": targets,
            "message": marker,
        })
        .to_string();
        let expects_first = targets.contains(&&first_id);
        send(case, &mut first, &message).await;
        if expects_first {
            let delivered = next_frame(case, &mut first)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: {what} must be delivered to the sender"));
            assert_eq!(
                delivered, marker,
                "case {case:?}: {what} must be delivered the payload {marker:?}, got \
                 {delivered:?}"
            );
        }
        if targets.contains(&&second_id) {
            let delivered = next_frame(case, &mut second).await.unwrap_or_else(|| {
                panic!("case {case:?}: {what} must be delivered to the connection it named")
            });
            assert_eq!(
                delivered, marker,
                "case {case:?}: {what} must be delivered the payload {marker:?}, got \
                 {delivered:?}"
            );
        }
    }

    // The other operations of the same API are reachable the same way: the function reads
    // the connection it was asked about and posts what that read reported.
    send(
        case,
        &mut first,
        &json!({ "action": "inspect", "target": second_id }).to_string(),
    )
    .await;
    let reported = next_frame(case, &mut first)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the inspected connection must be reported back"));
    let reported: Value = serde_json::from_str(&reported).unwrap_or_else(|error| {
        panic!("case {case:?}: the pushed payload must be the function's JSON, got {reported:?} ({error})")
    });
    assert!(
        reported
            .get("connectedAt")
            .and_then(Value::as_str)
            .is_some_and(|instant| !instant.trim().is_empty() && instant != "None"),
        "case {case:?}: GetConnection must report when the connection was made, got {reported}"
    );
}
