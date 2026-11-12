// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` WEBSOCKET data plane: what a client's RFC 6455 OPENING HANDSHAKE
//! against a deployed WebSocket api stage is answered with.
//!
//! A WebSocket api reports `wss://{api-id}.execute-api.{region}.amazonaws.com` as its
//! `ApiEndpoint`, with "the stage name … typically appended to this URI to form a complete
//! path to a deployed API stage" (`Api$ApiEndpoint`), so the FIRST request a client makes
//! against a deployed stage is the RFC 6455 opening handshake — a `GET` carrying
//! `Connection: Upgrade`, `Upgrade: websocket` and a `Sec-WebSocket-Key` — and not a
//! modelled operation. No service claimed it, so the emulator's front door resolved no
//! target service and answered `404` `UnknownService`: an "this API does not exist" shape
//! for the endpoint this very service reported, which a client cannot act on and cannot
//! tell apart from a stage that was never deployed.
//!
//! The rule these cases assert, one level above the recorded case (one WebSocket api,
//! `$request.body.action`, one stage, a handshake polled for a `101`):
//!
//! > An RFC 6455 opening handshake addressed to a stage of a WEBSOCKET api this service
//! > holds is answered BY THAT API'S OWN DATA PLANE, computed from the api's and the
//! > stage's stored records: a stage the endpoint does not serve (no stage of that name,
//! > or no deployment released to it) is `403` `{"message":"Forbidden"}`, and a DEPLOYED
//! > stage is `501` `NotImplemented` naming the absent capability — never the front door's
//! > `404 UnknownService`, and never a `101 Switching Protocols`, which would promise a
//! > socket this emulator cannot carry.
//!
//! Why `501` and not `101`: this emulator's front door serves one BUFFERED request per
//! call and hands a service no connection to switch protocols on, so after a `101` there
//! is no socket to carry frames, nothing to run `$connect` / the route-selection route /
//! `$disconnect` over, and no session a minted connection id could address. A `101` would
//! be a fabricated success — the client would see the upgrade and then read EOF — so the
//! handshake is refused with the status this emulator answers for a capability it does not
//! have, exactly as the Transcribe Streaming WebSocket transport refuses its own upgrade.
//!
//! The rows span what the rule generalises over: how the handshake addresses the api (its
//! own `execute-api` host, the emulator's own endpoint, a custom domain host), the field
//! spellings RFC 6455 §4.1 allows (`keep-alive, Upgrade`, `WebSocket`, lower case), the
//! form of the stage name, whether the stage carries a deployment, and the api's
//! `ProtocolType`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};

/// A `Sec-WebSocket-Key`: "a base64-encoded value that, when decoded, is 16 bytes in
/// length" (RFC 6455 §4.1) — the example key the RFC itself uses.
///
/// Shared with `websocket_connect_authorization`, which drives the same handshake against
/// a `$connect` route that carries an authorizer.
pub(crate) const CLIENT_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// The status a successful upgrade would carry. Asserted ABSENT: see the module docs.
const SWITCHING_PROTOCOLS: u16 = 101;

/// The status this emulator answers for a capability it does not have
/// (`crate::scaffolded`'s channel).
const NOT_IMPLEMENTED: u16 = 501;

/// The error code that status carries, in `x-amzn-errortype`.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The status and sentence an `execute-api` endpoint answers for an api or a stage it does
/// not serve.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The error code the front door answers when NO service claims a request — the answer
/// this suite exists to keep off the handshake path.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// A WebSocket api: `RouteSelectionExpression` "is required for WebSocket APIs"
/// (`CreateApi$RouteSelectionExpression`).
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

/// The integration a row's route fronts. `MOCK` "is not permitted for APIs with a protocol
/// type of HTTP", so an HTTP api gets the `HTTP_PROXY` upstream its protocol allows.
fn integration(protocol_type: &str) -> Value {
    if protocol_type == "HTTP" {
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/upstream",
            "integrationMethod": "ANY",
            "payloadFormatVersion": "1.0"
        })
    } else {
        json!({ "integrationType": "MOCK" })
    }
}

/// An api created, routed and (optionally) deployed to one stage.
struct Deployed {
    api_id: String,
    /// The host of the api's own `ApiEndpoint`, read back off the create response rather
    /// than spelled out here, so the handshake addresses the endpoint the service reported.
    endpoint_host: String,
    stage: String,
}

/// Create an api, an integration its protocol allows, a route over it and a stage;
/// release a deployment to that stage when `deploy` is set.
async fn deploy_api(
    emulator: &MultiServiceRouter,
    case: &str,
    api: Value,
    stage: &str,
    deploy: bool,
) -> Deployed {
    let protocol_type = as_str(&api, "protocolType");
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(api)).await;
    assert_eq!(status, 201, "case {case:?}: CreateApi must answer 201, got {created}");
    let api_id = as_str(&created, "apiId");
    let announced = as_str(&created, "apiEndpoint");
    crate::api_endpoint_scheme::assert_announced_endpoint(
        case,
        "CreateApi",
        &protocol_type,
        &api_id,
        crate::api_lifecycle::REGION,
        &announced,
    );
    // The host this suite addresses the handshake at is the api's own `execute-api` host —
    // the one AWS publishes and this emulator's FRONT DOOR claims, which is what these cases
    // are about. A WEBSOCKET api's own socket is announced instead (`announced` above) and is
    // driven over a real connection in `websocket_transport_data_plane`.
    let endpoint_host = format!(
        "{api_id}.execute-api.{}.amazonaws.com",
        crate::api_lifecycle::REGION
    );

    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(integration(&protocol_type)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    let (status, route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": route_key(&protocol_type),
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(status, 201, "case {case:?}: CreateRoute must answer 201, got {route}");

    let (status, created_stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {created_stage}"
    );

    if deploy {
        let (status, deployment) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateDeployment must answer 201, got {deployment}"
        );
        let (status, fetched) = call(
            emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: GetStage must answer 200, got {fetched}");
        assert!(
            !as_str(&fetched, "deploymentId").is_empty(),
            "case {case:?}: the stage must carry the released deployment, got {fetched}"
        );
    }

    Deployed {
        api_id,
        endpoint_host,
        stage: stage.to_string(),
    }
}

/// The answer to one request: its status, its `x-amzn-errortype`, the `sec-websocket-accept`
/// and `location` headers it carries (or `None`), its `content-type`, its body as text, and
/// its body parsed as JSON.
///
/// Both readings of the body are kept because this service's data planes answer in two
/// envelopes: a `{"message": …}` JSON body for every refusal, and an HTML document for a
/// published portal's page (`portal_data_plane`). A body that is not JSON parses to
/// `Value::Null` and is read through [`Answer::text`].
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) accept: Option<String>,
    /// Where a redirect sends the caller — the header the HTTP data plane answers an
    /// `HTTP_PROXY` upstream with (`http_api_jwt_data_plane`).
    pub(crate) location: Option<String>,
    pub(crate) content_type: String,
    pub(crate) text: String,
    pub(crate) body: Value,
    /// Every response header, for a case that asserts on a field a BACKEND set
    /// (`http_api_backend_dispatch`), including one the answer carries more than once.
    pub(crate) headers: http::HeaderMap,
    /// The body as the BYTES it arrived as, for a payload that is not text
    /// (`http_api_backend_dispatch`).
    pub(crate) bytes: bytes::Bytes,
}

impl Answer {
    /// The `message` member of an `execute-api` body, or the `__type` of a front-door
    /// envelope — whichever the answer carries — for an assertion message.
    pub(crate) fn summary(&self) -> String {
        let body = if self.body.is_null() {
            // A document rather than an envelope: enough of it to identify the page.
            self.text.chars().take(200).collect()
        } else {
            self.body.to_string()
        };
        format!(
            "{} {body} ({:?}, x-amzn-errortype: {:?})",
            self.status, self.content_type, self.error_type
        )
    }

    /// The first value the answer carries for `name`, or `None` for a field it does not
    /// carry.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
    }

    /// EVERY value the answer carries for `name`, in order: a field a backend set more than
    /// once (`set-cookie`) is sent once per value.
    pub(crate) fn all_headers(&self, name: &str) -> Vec<String> {
        self.headers
            .get_all(name)
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
            .collect()
    }
}

/// Send one request with arbitrary host, path and headers, and no credential — which is
/// what a WebSocket client's opening handshake is.
pub(crate) async fn send(
    emulator: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_with_body(emulator, host, method, path, headers, Vec::new()).await
}

/// [`send`] carrying a request BODY: what a client POSTing a payload to a route of a
/// deployed HTTP api stage sends (`http_api_payload_quota`). [`send`] is this with no body
/// at all, so both reach the emulator through one request builder.
pub(crate) async fn send_with_body(
    emulator: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("build the handshake request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
    };
    let error_type = header("x-amzn-errortype");
    let accept = header("sec-websocket-accept");
    let location = header("location");
    let content_type = header("content-type").unwrap_or_default();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        accept,
        location,
        content_type,
        text,
        body,
        headers,
        bytes,
    }
}

/// The RFC 6455 opening handshake, with the `Connection` and `Upgrade` fields spelled as
/// the case asks.
async fn handshake(
    emulator: &MultiServiceRouter,
    host: &str,
    path: &str,
    connection: &str,
    upgrade: &str,
) -> Answer {
    send(
        emulator,
        host,
        "GET",
        path,
        &[
            ("connection", connection),
            ("upgrade", upgrade),
            ("sec-websocket-key", CLIENT_KEY),
            ("sec-websocket-version", "13"),
        ],
    )
    .await
}

/// The route key a row's route carries: a WebSocket api's catch-all `$default` route, an
/// HTTP api's method and path.
///
/// `$default` rather than `$connect`, because these cases are about how a handshake RESOLVES
/// an api and a stage: a `$connect` route's integration serves the CONNECTION REQUEST, whose
/// own answer is `websocket_connect_authorization`'s and
/// `websocket_transport_data_plane`'s subject.
fn route_key(protocol_type: &str) -> &'static str {
    if protocol_type == "HTTP" {
        "ANY /{proxy+}"
    } else {
        "$default"
    }
}

/// The `message` member an `execute-api` body carries.
pub(crate) fn message(answer: &Answer) -> String {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One row of the deployed-stage table: how the handshake addresses the api, and how its
/// handshake fields are spelled.
struct Row {
    case: &'static str,
    /// The stage the api is deployed to — the segment the handshake path carries.
    stage: &'static str,
    /// How the handshake addresses the api: the host to send it to, built from the api's
    /// own endpoint host.
    host: fn(&Deployed) -> String,
    connection: &'static str,
    upgrade: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "the recorded case: the api's own execute-api host, the fields a client \
               library sends",
        stage: "prod",
        host: |deployed| deployed.endpoint_host.clone(),
        connection: "Upgrade",
        upgrade: "websocket",
    },
    Row {
        case: "the api's own execute-api host on an explicit port, a multi-token \
               Connection field and a mixed-case Upgrade",
        stage: "dev",
        host: |deployed| format!("{}:443", deployed.endpoint_host),
        connection: "keep-alive, Upgrade",
        upgrade: "WebSocket",
    },
    Row {
        case: "the emulator's own endpoint, which is where a client that cannot resolve \
               the execute-api host points its socket",
        stage: "sockets",
        host: |_| "127.0.0.1:9297".to_string(),
        connection: "upgrade",
        upgrade: "websocket",
    },
    Row {
        case: "a custom domain host fronting the api, with a stage name at an unusual \
               form",
        stage: "Stage_1-x",
        host: |_| "sockets.example.com".to_string(),
        connection: "Upgrade",
        upgrade: "websocket",
    },
];

#[tokio::test]
async fn a_deployed_websocket_stage_answers_the_handshake_from_its_own_records() {
    for row in ROWS {
        let emulator = emulator();
        let deployed = deploy_api(
            &emulator,
            row.case,
            websocket_api("socket-api"),
            row.stage,
            true,
        )
        .await;
        let host = (row.host)(&deployed);
        let answer = handshake(
            &emulator,
            &host,
            &format!("/{}", row.stage),
            row.connection,
            row.upgrade,
        )
        .await;

        // The answer is this api's, not the front door's "no such service".
        assert_ne!(
            answer.error_type.as_deref(),
            Some(UNKNOWN_SERVICE),
            "case {:?}: the handshake must reach this api's data plane, got {}",
            row.case,
            answer.summary()
        );
        assert_ne!(
            answer.status, SWITCHING_PROTOCOLS,
            "case {:?}: a 101 would promise a socket this emulator cannot carry, got {}",
            row.case,
            answer.summary()
        );
        assert_eq!(
            answer.accept, None,
            "case {:?}: no upgrade happened, so no Sec-WebSocket-Accept is sent, got {:?}",
            row.case, answer.accept
        );
        assert_eq!(
            answer.status, NOT_IMPLEMENTED,
            "case {:?}: expected {NOT_IMPLEMENTED} for a deployed WebSocket stage, got {}",
            row.case,
            answer.summary()
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(NOT_IMPLEMENTED_CODE),
            "case {:?}: expected the error code {NOT_IMPLEMENTED_CODE}, got {}",
            row.case,
            answer.summary()
        );
        let reason = message(&answer);
        assert!(
            reason.contains("WebSocket") && reason.contains("ApiEndpoint"),
            "case {:?}: the refusal must say this endpoint carries no WebSocket transport \
             and name the one the api announces, rather than read as a misconfigured \
             stage, got {reason:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_stage_the_endpoint_does_not_serve_is_forbidden() {
    /// One row: what is wrong with the handshake's target, and the path it addresses.
    struct Case {
        case: &'static str,
        /// The stage created on the api.
        stage: &'static str,
        /// Whether a deployment is released to it.
        deploy: bool,
        /// The path the handshake addresses, from the created stage name.
        path: fn(&str) -> String,
    }

    const CASES: &[Case] = &[
        Case {
            case: "a stage with no deployment released to it",
            stage: "prod",
            deploy: false,
            path: |stage| format!("/{stage}"),
        },
        Case {
            case: "a stage name the api does not hold",
            stage: "prod",
            deploy: true,
            path: |_| "/never-created".to_string(),
        },
        Case {
            case: "no stage segment at all",
            stage: "prod",
            deploy: true,
            path: |_| "/".to_string(),
        },
    ];

    for case in CASES {
        let emulator = emulator();
        let deployed = deploy_api(
            &emulator,
            case.case,
            websocket_api("socket-api"),
            case.stage,
            case.deploy,
        )
        .await;
        // Addressed at the api's OWN endpoint host, so the api is identified even when
        // the path names no stage of it.
        let answer = handshake(
            &emulator,
            &deployed.endpoint_host,
            &(case.path)(&deployed.stage),
            "Upgrade",
            "websocket",
        )
        .await;
        assert_eq!(
            answer.status, FORBIDDEN,
            "case {:?}: expected {FORBIDDEN} for a stage this endpoint does not serve, \
             got {}",
            case.case,
            answer.summary()
        );
        assert_eq!(
            message(&answer),
            FORBIDDEN_MESSAGE,
            "case {:?}: expected the {FORBIDDEN_MESSAGE:?} body an execute-api endpoint \
             answers with, got {}",
            case.case,
            answer.summary()
        );
        assert_eq!(
            answer.accept, None,
            "case {:?}: no upgrade happened, so no Sec-WebSocket-Accept is sent",
            case.case
        );
        let _ = &deployed.api_id;
    }
}

#[tokio::test]
async fn what_the_socket_front_door_does_not_claim() {
    // Each row is a request the WebSocket data plane must NOT answer: it is not an
    // opening handshake, or it addresses no WEBSOCKET api of this service. None of them
    // may be claimed, so each is left to the front door — which, with only this service
    // registered, answers its own `UnknownService`. What is pinned is that this service
    // did not answer them, not the front door's wording.
    let emulator = emulator();
    let socket = deploy_api(
        &emulator,
        "the claim table's WebSocket api",
        websocket_api("socket-api"),
        "prod",
        true,
    )
    .await;
    let plain = deploy_api(
        &emulator,
        "the claim table's HTTP api",
        http_api("http-api"),
        "live",
        true,
    )
    .await;

    let socket_host = socket.endpoint_host.clone();
    let unclaimed: Vec<(&str, Answer)> = vec![
        (
            "an ordinary GET on the stage path, asking for no upgrade",
            send(
                &emulator,
                &socket_host,
                "GET",
                "/prod",
                &[("connection", "keep-alive")],
            )
            .await,
        ),
        (
            "an upgrade to another protocol",
            send(
                &emulator,
                &socket_host,
                "GET",
                "/prod",
                &[("connection", "Upgrade"), ("upgrade", "h2c")],
            )
            .await,
        ),
        (
            "the handshake fields on a POST, which RFC 6455 does not open a socket with",
            send(
                &emulator,
                &socket_host,
                "POST",
                "/prod",
                &[
                    ("connection", "Upgrade"),
                    ("upgrade", "websocket"),
                    ("sec-websocket-key", CLIENT_KEY),
                ],
            )
            .await,
        ),
        (
            "a handshake with no Sec-WebSocket-Key",
            send(
                &emulator,
                &socket_host,
                "GET",
                "/prod",
                &[("connection", "Upgrade"), ("upgrade", "websocket")],
            )
            .await,
        ),
        (
            "a handshake on the @connections path space, which ApiGatewayManagementApi \
             models",
            handshake(
                &emulator,
                &socket_host,
                "/prod/@connections/Lx9ZKcvPmjQCJhA=",
                "Upgrade",
                "websocket",
            )
            .await,
        ),
    ];

    for (case, answer) in unclaimed {
        assert_eq!(
            answer.error_type.as_deref(),
            Some(UNKNOWN_SERVICE),
            "case {case:?}: this service must not claim the request, got {}",
            answer.summary()
        );
        assert_ne!(
            answer.status, NOT_IMPLEMENTED,
            "case {case:?}: the WebSocket data plane must not answer this request, got {}",
            answer.summary()
        );
    }

    // A handshake on an `execute-api` endpoint whose api id no api of this emulator has is
    // answered by the HTTP data plane (`src/service/invoke.rs`), which owns that address:
    // the endpoint resolves to no api, so the answer is its `404` rather than this module's
    // capability refusal. What is pinned here is only that the WEBSOCKET plane did not
    // answer it.
    let on_no_api = handshake(
        &emulator,
        "zzz9999999.execute-api.eu-west-1.amazonaws.com",
        "/prod",
        "Upgrade",
        "websocket",
    )
    .await;
    assert_ne!(
        on_no_api.status, NOT_IMPLEMENTED,
        "a handshake on an endpoint that resolves to no api must not be answered by the \
         WebSocket data plane, got {}",
        on_no_api.summary()
    );
    assert_eq!(
        on_no_api.accept, None,
        "no upgrade happened, so no Sec-WebSocket-Accept is sent, got {}",
        on_no_api.summary()
    );

    // An upgrade request to an HTTP api's stage is an ordinary HTTP request to that api's
    // routes, so it belongs to the HTTP data plane (`src/service/invoke.rs`) and not to
    // this one: the api serves the host, nothing of it matches `GET /`, and the answer is
    // that data plane's `404` rather than this module's capability refusal. What is pinned
    // here is only that the WEBSOCKET plane did not answer it.
    let on_an_http_api = handshake(
        &emulator,
        &plain.endpoint_host,
        "/live",
        "Upgrade",
        "websocket",
    )
    .await;
    assert_ne!(
        on_an_http_api.status, NOT_IMPLEMENTED,
        "a handshake on an HTTP api's stage must not be answered by the WebSocket data \
         plane, got {}",
        on_an_http_api.summary()
    );
    assert_eq!(
        on_an_http_api.accept, None,
        "no upgrade happened, so no Sec-WebSocket-Accept is sent, got {}",
        on_an_http_api.summary()
    );
    let _ = (&socket.api_id, &plain.api_id, &plain.stage);
}

#[tokio::test]
async fn a_stage_name_two_websocket_apis_hold_names_no_one_api() {
    // A handshake whose host names no api is resolved by its stage segment. When two
    // WebSocket apis carry a stage of that name, the request identifies no ONE api, so
    // this service answers for none of them rather than guessing.
    let emulator = emulator();
    let first = deploy_api(&emulator, "the first api", websocket_api("first"), "prod", true).await;
    let second =
        deploy_api(&emulator, "the second api", websocket_api("second"), "prod", true).await;
    assert_ne!(
        first.api_id, second.api_id,
        "the two apis must be distinct for this case to mean anything"
    );

    let ambiguous = handshake(&emulator, "127.0.0.1:9297", "/prod", "Upgrade", "websocket").await;
    assert_eq!(
        ambiguous.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "a stage name two apis hold identifies no api, got {}",
        ambiguous.summary()
    );

    // Each api is still reachable through its OWN endpoint host, where the host names it.
    for deployed in [&first, &second] {
        let answer = handshake(
            &emulator,
            &deployed.endpoint_host,
            "/prod",
            "Upgrade",
            "websocket",
        )
        .await;
        assert_eq!(
            answer.status, NOT_IMPLEMENTED,
            "api {}: its own endpoint host identifies it, got {}",
            deployed.api_id,
            answer.summary()
        );
    }
}

#[tokio::test]
async fn the_handshake_changes_nothing_a_later_caller_sees() {
    let emulator = emulator();
    let deployed = deploy_api(
        &emulator,
        "a read must not change what a later caller sees",
        websocket_api("socket-api"),
        "prod",
        true,
    )
    .await;
    let uri = format!("/v2/apis/{}/stages/{}", deployed.api_id, deployed.stage);
    let (status, before) = call(&emulator, "GET", &uri, None).await;
    assert_eq!(status, 200, "GetStage must answer 200, got {before}");

    let first = handshake(
        &emulator,
        &deployed.endpoint_host,
        "/prod",
        "Upgrade",
        "websocket",
    )
    .await;
    let second = handshake(
        &emulator,
        &deployed.endpoint_host,
        "/prod",
        "Upgrade",
        "websocket",
    )
    .await;
    assert_eq!(
        (first.status, message(&first)),
        (second.status, message(&second)),
        "two identical handshakes must be answered identically, got {} then {}",
        first.summary(),
        second.summary()
    );

    let (status, after) = call(&emulator, "GET", &uri, None).await;
    assert_eq!(status, 200, "GetStage must answer 200, got {after}");
    assert_eq!(
        after, before,
        "the handshake changed the stage record a later caller reads"
    );
}
