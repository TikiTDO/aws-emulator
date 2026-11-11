// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the HTTP STATUS each mutating operation of this service
//! answers with.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rule these cases assert: **the success status of an operation is the status API
//! Gateway answers it with, which for a handful of its writes is not the status the model's
//! `@http` trait declares** — a PATCH that rewrites an api, a route or a route response is
//! `201`, the two imports and the api-mapping create are `200`, the flush of a stage's
//! authorizer cache is `200`, and the custom domain teardown is `202 Accepted` — **while
//! every other operation answers its modelled `code`; and an answer that carries no content
//! states the length it has (`content-length: 0`)**, whatever that status is.
//!
//! The modelled codes each row is measured against come from the model itself
//! (`smithy_query(query="operation", operation="UpdateApi,UpdateRoute,UpdateRouteResponse,\
//! ImportApi,ReimportApi,CreateApiMapping,ResetAuthorizersCache,DeleteDomainName,\
//! DeleteApiMapping,DeleteRoute,DeleteVpcLink")`), and the statuses that differ from them
//! are single-homed in `crate::state::wire_status`.
//!
//! The rows span what the rule generalises over: the HTTP method (POST, PUT, PATCH, DELETE,
//! GET), the resource family (an api, a child of an api, a child of a child, a custom domain
//! name, a mapping under one, a VPC link), whether the answer carries a body or none, and —
//! on either side of the rule — the operations whose answer differs from their modelled code
//! and the operations whose answer is their modelled code. The recorded steps that filed the
//! issue (a `PATCH /v2/apis/{apiId}`, a `DELETE /v2/domainnames/{domainName}`) are two rows.
//!
//! NOTE on framing: what is pinned here is the length the SERVICE states, which the
//! in-process front door delivers unchanged. A client reached over HTTP/1.1 does not see it
//! on a `204`: the HTTP writer the vendored transport uses removes `content-length` from a
//! `204`, which is below anything this crate decides (see `crate::service` and
//! `empty_answer_framing.rs`). At any other status an empty answer carries — the `202`
//! teardown, the `200` cache flush — the header does reach the client, which the
//! raw-socket case below pins.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host_reading_headers, call_in_region, emulator};
// The framing rule's two readings live in the suite that owns it — one value and one header
// reading for both suites.
use crate::empty_answer_framing::{content_length, NO_CONTENT_LENGTH};

/// The region every request in this suite is signed for. Deliberately NOT the emulator's
/// `us-east-1` default, so nothing region-derived can be hardcoded.
const REGION: &str = "eu-west-2";

/// Send one request signed for [`REGION`] and answer `(status, body)`.
async fn call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(emulator, REGION, method, uri, body).await
}

/// [`call`] keeping the response headers, for the rows that assert on framing as well.
async fn call_reading_headers(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, http::HeaderMap, Value) {
    call_at_host_reading_headers(
        emulator,
        &format!("apigateway.{REGION}.amazonaws.com"),
        REGION,
        method,
        uri,
        body,
    )
    .await
}

/// An ACM certificate ARN, the only source `DomainNameConfiguration$CertificateArn`
/// documents, built through the emulator's own ARN helper.
fn certificate_arn(name: &str) -> String {
    engine::emulator_arn("acm", REGION, &format!("certificate/{name}"))
}

/// An OpenAPI definition `ImportApi`/`ReimportApi` can read: one path, fronted by an
/// `HTTP_PROXY` upstream.
fn definition(title: &str, path: &str) -> String {
    json!({
        "openapi": "3.0.1",
        "info": { "title": title, "version": "1.0" },
        "paths": {
            path: {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "ANY",
                        "uri": "https://example.com/upstream",
                        "payloadFormatVersion": "1.0"
                    }
                }
            }
        }
    })
    .to_string()
}

/// One row: a request, and the status this service answers it with.
struct Row {
    /// The operation and what the row varies, named in every assertion.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The status the answer carries.
    status: u16,
    /// Whether the answer carries NO content, and so states `content-length: 0`.
    empty: bool,
}

/// The resources every row addresses, created ahead of the table so each row is one call.
struct Fixture {
    /// An HTTP api holding a route, an integration and a stage.
    api: String,
    route: String,
    integration: String,
    stage: String,
    /// A WEBSOCKET api holding a route and a response of that route.
    socket_api: String,
    socket_route: String,
    route_response: String,
    /// The custom domain name the mapping rows address, and the api mapping the delete row
    /// removes, which points at the quick-created api's `$default` stage.
    domain: String,
    api_mapping: String,
    mapped_api: String,
    /// A second custom domain name, for the patch row: a patch moves a domain into its
    /// updating state, which is not what the teardown row is about.
    patched_domain: String,
    /// An api imported from a definition, for the reimport row.
    imported_api: String,
    /// A VPC link, whose teardown the model itself binds to `202`.
    vpc_link: String,
}

/// Create one resource, answering the member of the created record `member` names.
async fn created(
    emulator: &MultiServiceRouter,
    case: &str,
    uri: &str,
    request: Value,
    status: u16,
    member: &str,
) -> String {
    let (answered, body) = call(emulator, "POST", uri, Some(request)).await;
    assert_eq!(
        answered, status,
        "seeding {case}: expected {status}, got {answered} {body}"
    );
    as_str(&body, member)
}

async fn fixture(emulator: &MultiServiceRouter) -> Fixture {
    let api = created(
        emulator,
        "the HTTP api",
        "/v2/apis",
        json!({ "name": "status-http-api", "protocolType": "HTTP" }),
        201,
        "apiId",
    )
    .await;
    let route = created(
        emulator,
        "a route of the HTTP api",
        &format!("/v2/apis/{api}/routes"),
        json!({ "routeKey": "GET /items" }),
        201,
        "routeId",
    )
    .await;
    let integration = created(
        emulator,
        "an integration of the HTTP api",
        &format!("/v2/apis/{api}/integrations"),
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/items",
            "payloadFormatVersion": "1.0"
        }),
        201,
        "integrationId",
    )
    .await;
    let stage = created(
        emulator,
        "a stage of the HTTP api",
        &format!("/v2/apis/{api}/stages"),
        json!({ "stageName": "prod" }),
        201,
        "stageName",
    )
    .await;

    let socket_api = created(
        emulator,
        "the WEBSOCKET api",
        "/v2/apis",
        json!({
            "name": "status-socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
        201,
        "apiId",
    )
    .await;
    let socket_route = created(
        emulator,
        "a route of the WEBSOCKET api",
        &format!("/v2/apis/{socket_api}/routes"),
        json!({ "routeKey": "$connect" }),
        201,
        "routeId",
    )
    .await;
    let route_response = created(
        emulator,
        "a response of the WEBSOCKET route",
        &format!("/v2/apis/{socket_api}/routes/{socket_route}/routeresponses"),
        json!({ "routeResponseKey": "$default" }),
        201,
        "routeResponseId",
    )
    .await;

    let domain = "mapped.example.com".to_string();
    let patched_domain = "patched.example.com".to_string();
    for name in [&domain, &patched_domain] {
        let (status, body) = call(
            emulator,
            "POST",
            "/v2/domainnames",
            Some(json!({
                "domainName": name,
                "domainNameConfigurations": [{ "certificateArn": certificate_arn(name) }]
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "seeding the domain name {name:?}: expected 201, got {status} {body}"
        );
    }
    // Quick create gives this api the `$default` stage a mapping points at.
    let mapped_api = created(
        emulator,
        "the quick-created api the mappings point at",
        "/v2/apis",
        json!({
            "name": "status-mapped-api",
            "protocolType": "HTTP",
            "target": "https://example.com/backend"
        }),
        201,
        "apiId",
    )
    .await;
    let api_mapping = created(
        emulator,
        "an api mapping of the domain name",
        &format!("/v2/domainnames/{domain}/apimappings"),
        json!({ "apiId": mapped_api, "stage": "$default", "apiMappingKey": "invoices" }),
        // The status this very table pins for `CreateApiMapping`.
        200,
        "apiMappingId",
    )
    .await;

    let (status, imported) = call(
        emulator,
        "PUT",
        "/v2/apis",
        Some(json!({ "body": definition("status-imported-api", "/pets") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the imported api: expected 200, got {status} {imported}"
    );
    let imported_api = as_str(&imported, "apiId");

    let vpc_link = created(
        emulator,
        "a VPC link",
        "/v2/vpclinks",
        json!({ "name": "status-vpc-link", "subnetIds": ["subnet-0a1b2c3d"] }),
        201,
        "vpcLinkId",
    )
    .await;

    Fixture {
        api,
        route,
        integration,
        stage,
        socket_api,
        socket_route,
        route_response,
        domain,
        api_mapping,
        mapped_api,
        patched_domain,
        imported_api,
        vpc_link,
    }
}

/// The table: every row's request, and the status this service answers it with. Ordered so
/// a row addresses a resource that is still there, with the teardowns last.
fn rows(f: &Fixture) -> Vec<Row> {
    let Fixture {
        api,
        route,
        integration,
        stage,
        socket_api,
        socket_route,
        route_response,
        domain,
        api_mapping,
        mapped_api,
        patched_domain,
        imported_api,
        vpc_link,
    } = f;
    vec![
        // The writes whose answer is NOT their modelled code.
        Row {
            case: "UpdateApi, a patch of an api (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/apis/{api}"),
            body: Some(json!({ "description": "the patched api" })),
            status: 201,
            empty: false,
        },
        Row {
            case: "UpdateRoute, a patch of a child of an api (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/apis/{api}/routes/{route}"),
            body: Some(json!({ "routeKey": "GET /patched" })),
            status: 201,
            empty: false,
        },
        Row {
            case: "UpdateRouteResponse, a patch of a child of a route (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/apis/{socket_api}/routes/{socket_route}/routeresponses/{route_response}"),
            body: Some(json!({ "routeResponseKey": "moved" })),
            status: 201,
            empty: false,
        },
        Row {
            case: "ImportApi, a PUT that mints an api (modelled 201)",
            method: "PUT",
            uri: "/v2/apis".to_string(),
            body: Some(json!({ "body": definition("status-second-import", "/orders") })),
            status: 200,
            empty: false,
        },
        Row {
            case: "ReimportApi, a PUT over an api that exists (modelled 201)",
            method: "PUT",
            uri: format!("/v2/apis/{imported_api}"),
            body: Some(json!({ "body": definition("status-reimported", "/orders") })),
            status: 200,
            empty: false,
        },
        Row {
            case: "CreateApiMapping, a POST that mints a child of a domain name \
                   (modelled 201)",
            method: "POST",
            uri: format!("/v2/domainnames/{domain}/apimappings"),
            body: Some(json!({ "apiId": api, "stage": stage, "apiMappingKey": "orders" })),
            status: 200,
            empty: false,
        },
        Row {
            // A flush, not a teardown: nothing the stage is configured with is removed, only
            // the decisions its authorizers had already returned. The answer has no members,
            // so this status is the whole signal the flush was carried out.
            case: "ResetAuthorizersCache, a flush of a stage's authorizer cache \
                   (modelled 204)",
            method: "DELETE",
            uri: format!("/v2/apis/{api}/stages/{stage}/cache/authorizers"),
            body: None,
            status: 200,
            empty: true,
        },
        // The writes whose answer IS their modelled code: the same methods over the same
        // resource families.
        Row {
            case: "CreateRoute, a POST that mints a child of an api (modelled 201)",
            method: "POST",
            uri: format!("/v2/apis/{api}/routes"),
            body: Some(json!({ "routeKey": "POST /items" })),
            status: 201,
            empty: false,
        },
        Row {
            case: "UpdateApiMapping, a patch of a child of a domain name (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/domainnames/{domain}/apimappings/{api_mapping}"),
            body: Some(json!({
                "apiId": mapped_api,
                "stage": "$default",
                "apiMappingKey": "receipts"
            })),
            status: 200,
            empty: false,
        },
        Row {
            case: "UpdateStage, a patch of a child of an api (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/apis/{api}/stages/{stage}"),
            body: Some(json!({ "description": "the patched stage" })),
            status: 200,
            empty: false,
        },
        Row {
            case: "UpdateIntegration, a patch of a child of an api (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/apis/{api}/integrations/{integration}"),
            body: Some(json!({ "description": "the patched integration" })),
            status: 200,
            empty: false,
        },
        Row {
            case: "UpdateDomainName, a patch of a domain name (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/domainnames/{patched_domain}"),
            body: Some(json!({
                "domainNameConfigurations": [{
                    "certificateArn": certificate_arn(patched_domain),
                    "endpointType": "REGIONAL"
                }]
            })),
            status: 200,
            empty: false,
        },
        Row {
            case: "UpdateVpcLink, a patch of a VPC link (modelled 200)",
            method: "PATCH",
            uri: format!("/v2/vpclinks/{vpc_link}"),
            body: Some(json!({ "name": "renamed-vpc-link" })),
            status: 200,
            empty: false,
        },
        Row {
            case: "GetApi, a read (modelled 200)",
            method: "GET",
            uri: format!("/v2/apis/{api}"),
            body: None,
            status: 200,
            empty: false,
        },
        // The teardowns, each answering with no content at all.
        Row {
            case: "DeleteRoute, a child of an api (modelled 204)",
            method: "DELETE",
            uri: format!("/v2/apis/{api}/routes/{route}"),
            body: None,
            status: 204,
            empty: true,
        },
        Row {
            case: "DeleteApiMapping, a child of a domain name (modelled 204)",
            method: "DELETE",
            uri: format!("/v2/domainnames/{domain}/apimappings/{api_mapping}"),
            body: None,
            status: 204,
            empty: true,
        },
        Row {
            // Its output is an empty STRUCTURE rather than `smithy.api#Unit`
            // (`smithy_query(query="operation", operation="DeleteVpcLink")`:
            // `output_shape: DeleteVpcLinkResponse`), so the protocol renders a document for
            // it and this answer carries content — the framing rule above does not reach it.
            case: "DeleteVpcLink, a teardown the model itself binds to 202",
            method: "DELETE",
            uri: format!("/v2/vpclinks/{vpc_link}"),
            body: None,
            status: 202,
            empty: false,
        },
        Row {
            case: "DeleteDomainName, a teardown that continues after the answer \
                   (modelled 204)",
            method: "DELETE",
            uri: format!("/v2/domainnames/{domain}"),
            body: None,
            status: 202,
            empty: true,
        },
        Row {
            case: "DeleteApi, a top-level resource (modelled 204)",
            method: "DELETE",
            uri: format!("/v2/apis/{socket_api}"),
            body: None,
            status: 204,
            empty: true,
        },
    ]
}

#[tokio::test]
async fn every_mutating_operation_answers_the_status_the_service_answers() {
    let emulator = emulator();
    let fixture = fixture(&emulator).await;

    for row in rows(&fixture) {
        let case = row.case;
        let (status, headers, body) =
            call_reading_headers(&emulator, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {} from {} {}, got {status} {body}",
            row.status, row.method, row.uri
        );
        if row.empty {
            assert_eq!(
                body,
                Value::Null,
                "case {case:?}: this answer carries no content, got {body}"
            );
            assert_eq!(
                content_length(&headers).as_deref(),
                Some(NO_CONTENT_LENGTH),
                "case {case:?}: an answer that carries no content states content-length \
                 {NO_CONTENT_LENGTH}, got {:?}",
                content_length(&headers)
            );
        } else {
            assert_ne!(
                body,
                Value::Null,
                "case {case:?}: this answer carries a document, got none at {status}"
            );
            assert_ne!(
                content_length(&headers).as_deref(),
                Some(NO_CONTENT_LENGTH),
                "case {case:?}: an answer that carries a document states no zero length, \
                 got {body}"
            );
        }
    }
}

/// RULE on the wire: the status and the framing a client reads off a real socket are the
/// ones the table above pins — with ONE exception the transport owns, recorded here.
///
/// The `202 Accepted` teardown carries no body and STATES `content-length: 0`, so a client
/// that closes the message out on an explicit zero length reads the same message end the
/// origin wrote; and an answer that DOES carry a document states that document's own length,
/// never the empty answer's zero. The `204` answers carry the status and no body; the
/// `content-length: 0` this service states for them does not reach the client, because the
/// HTTP/1.1 writer this emulator is served by removes `content-length` from a `204`
/// (RFC 9110 §15.3.5 forbids a body there) — that writer is below anything this crate
/// decides, so this case pins what the socket really delivers and `empty_answer_framing.rs`
/// pins what the service states.
#[tokio::test(flavor = "multi_thread")]
async fn on_the_wire_a_teardown_states_its_status_and_its_length() {
    let emulator = emulator();
    let fixture = fixture(&emulator).await;

    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(emulator.clone()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    /// What a row requires of the `content-length` the answer states.
    enum Framing {
        /// No body at all, and the length of that nothing stated.
        EmptyAndStated,
        /// No body at all, and nothing required of the header: the writer removes it from a
        /// `204`, which is a transport decision this crate does not own.
        EmptyAndUnstated,
        /// A document, whose own byte length the stated header must equal.
        TheDocumentsOwnLength,
    }

    for (case, method, path, expected_status, framing) in [
        (
            // The recorded step this suite's flush row comes from, measured where the bytes
            // are visible: a `200` with no content, whose stated zero length a client really
            // reads (unlike a `204`'s, which the writer below removes).
            "ResetAuthorizersCache, answered 200",
            "DELETE",
            format!(
                "/v2/apis/{}/stages/{}/cache/authorizers",
                fixture.api, fixture.stage
            ),
            "HTTP/1.1 200",
            Framing::EmptyAndStated,
        ),
        (
            "DeleteRoute, answered 204",
            "DELETE",
            format!("/v2/apis/{}/routes/{}", fixture.api, fixture.route),
            "HTTP/1.1 204",
            Framing::EmptyAndUnstated,
        ),
        (
            "DeleteDomainName, answered 202",
            "DELETE",
            format!("/v2/domainnames/{}", fixture.domain),
            "HTTP/1.1 202",
            Framing::EmptyAndStated,
        ),
        (
            // The other side of the framing rule, measured where the bytes are visible: an
            // answer that carries a document states that document's length, so the empty
            // answers' zero above is a length and not a constant.
            "GetApi, answered 200 with the api",
            "GET",
            format!("/v2/apis/{}", fixture.api),
            "HTTP/1.1 200",
            Framing::TheDocumentsOwnLength,
        ),
    ] {
        let answer = tokio::task::spawn_blocking(move || raw_request(port, method, &path))
            .await
            .expect("the socket exchange runs to completion");
        assert!(
            answer.starts_with(expected_status),
            "case {case:?}: expected {expected_status:?} on the wire, got {answer:?}"
        );
        let (head, body) = answer
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("case {case:?}: a response head and body, got {answer:?}"));
        let head = head.to_ascii_lowercase();
        match framing {
            Framing::EmptyAndStated | Framing::EmptyAndUnstated => {
                assert!(
                    body.is_empty(),
                    "case {case:?}: this answer carries no body, got {body:?}"
                );
                assert!(
                    !head.contains("transfer-encoding"),
                    "case {case:?}: an answer with no body is not chunked, got {head:?}"
                );
                if matches!(framing, Framing::EmptyAndStated) {
                    assert!(
                        head.contains(&format!("content-length: {NO_CONTENT_LENGTH}")),
                        "case {case:?}: expected the answer to state content-length \
                         {NO_CONTENT_LENGTH}, got {head:?}"
                    );
                }
            }
            Framing::TheDocumentsOwnLength => {
                assert!(
                    !body.is_empty(),
                    "case {case:?}: this answer carries a document, got none"
                );
                assert!(
                    head.contains(&format!("content-length: {}", body.len())),
                    "case {case:?}: expected the answer to state the document's own length \
                     {}, got {head:?}",
                    body.len()
                );
            }
        }
    }
}

/// One bodiless request written straight onto a socket, with everything the server answered
/// read back as text. The credential scope is what routes the request to this service, so it
/// carries one even though the test router verifies no signature.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the framing these cases are about.
fn raw_request(port: u16, method: &str, path: &str) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         host: localhost\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential=AKIDTESTAPIGWV2/20261021/{REGION}/apigateway/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000\r\n\
         content-length: 0\r\n\
         connection: close\r\n\
         \r\n"
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("read the whole answer");
    String::from_utf8_lossy(&answer).to_string()
}

/// RULE, the other half: the status a write answers with says nothing about what it wrote —
/// every one of the operations above is still read back through the service's own readers.
#[tokio::test]
async fn the_status_a_write_answers_does_not_change_what_it_wrote() {
    let emulator = emulator();
    let fixture = fixture(&emulator).await;

    // The patch answered `201` wrote the member it carried, and a later read reports it.
    let (status, patched) = call(
        &emulator,
        "PATCH",
        &format!("/v2/apis/{}", fixture.api),
        Some(json!({ "description": "the patched api" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"UpdateApi\": expected 201, got {status} {patched}"
    );
    let (status, read) = call(&emulator, "GET", &format!("/v2/apis/{}", fixture.api), None).await;
    assert_eq!(
        status, 200,
        "case \"GetApi after UpdateApi\": expected 200, got {status} {read}"
    );
    assert_eq!(
        as_str(&read, "description"),
        "the patched api",
        "case \"GetApi after UpdateApi\": expected the patched description, got {read}"
    );

    // The reimport answered `200` replaced the definition the api carried.
    let (status, reimported) = call(
        &emulator,
        "PUT",
        &format!("/v2/apis/{}", fixture.imported_api),
        Some(json!({ "body": definition("status-reimported", "/orders") })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ReimportApi\": expected 200, got {status} {reimported}"
    );
    let (status, routes) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{}/routes", fixture.imported_api),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetRoutes after ReimportApi\": expected 200, got {status} {routes}"
    );
    let keys: Vec<String> = routes["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|route| as_str(route, "routeKey"))
        .collect();
    assert_eq!(
        keys,
        vec!["GET /orders".to_string()],
        "case \"GetRoutes after ReimportApi\": expected exactly the reimported path, got \
         {routes}"
    );

    // The create answered `200` minted a mapping its own reader reports.
    let (status, mapping) = call(
        &emulator,
        "POST",
        &format!("/v2/domainnames/{}/apimappings", fixture.domain),
        Some(json!({ "apiId": fixture.api, "stage": fixture.stage, "apiMappingKey": "orders" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"CreateApiMapping\": expected 200, got {status} {mapping}"
    );
    let mapping_id = as_str(&mapping, "apiMappingId");
    let (status, fetched) = call(
        &emulator,
        "GET",
        &format!(
            "/v2/domainnames/{}/apimappings/{mapping_id}",
            fixture.domain
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GetApiMapping after CreateApiMapping\": expected 200, got {status} {fetched}"
    );
    assert_eq!(
        fetched, mapping,
        "case \"GetApiMapping after CreateApiMapping\": expected the created record, got \
         {fetched}"
    );

    // And the teardown answered `202` really removed the domain name: its own reader no
    // longer reports it, nor does it report the mapping that was addressed under it.
    let (status, accepted) = call(
        &emulator,
        "DELETE",
        &format!("/v2/domainnames/{}", fixture.domain),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"DeleteDomainName\": expected 202, got {status} {accepted}"
    );
    for (case, uri) in [
        (
            "GetDomainName after DeleteDomainName",
            format!("/v2/domainnames/{}", fixture.domain),
        ),
        (
            "GetApiMapping after DeleteDomainName",
            format!(
                "/v2/domainnames/{}/apimappings/{mapping_id}",
                fixture.domain
            ),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 once the domain name is gone, got {status} {body}"
        );
    }
}
