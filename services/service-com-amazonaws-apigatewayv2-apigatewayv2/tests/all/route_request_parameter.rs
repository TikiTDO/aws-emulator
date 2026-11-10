// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteRouteRequestParameter`.
//!
//! The rule these cases assert: a DELETE on
//! `/v2/apis/{ApiId}/routes/{RouteId}/requestparameters/{RequestParameterKey}` removes
//! EXACTLY the one addressed entry from the stored route's request-parameter map and
//! answers 204 with no body, leaving every other entry — and every other member of the
//! route — as it was; each label of that path addresses a resource, so a level naming
//! something the store does not hold is the modelled `NotFoundException` (404) and
//! changes nothing.
//!
//! The rows vary what the rule generalises over: how many parameters the route carries,
//! the form of the key deleted (header, querystring, body), its position in the key order
//! (only, first, middle, last), and the constraint the entry carries (`required` true,
//! false, absent). The recorded case that filed the issue — one key dropped from a
//! two-key WebSocket route — is one row. Every route here is the `$connect` route, because
//! that is the only route of a WEBSOCKET api that may declare request parameters at all
//! (`crate::state::route_request_parameters`), and each row creates an api of its own.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// One row: the route to create, the key to delete, and the map that must remain.
struct Row {
    case: &'static str,
    /// The `CreateRoute` request, whose `requestParameters` is the starting map.
    route: Value,
    /// The `RequestParameterKey` the DELETE addresses.
    delete: &'static str,
    /// The `requestParameters` a later `GetRoute` must report.
    remaining: fn() -> Value,
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only parameter of a route, a header key",
            route: json!({
                "routeKey": "$connect",
                "requestParameters": {
                    "route.request.header.Authorization": { "required": true }
                }
            }),
            delete: "route.request.header.Authorization",
            remaining: || json!({}),
        },
        Row {
            case: "the first of two keys in key order",
            route: json!({
                "routeKey": "$connect",
                "requestParameters": {
                    "route.request.header.Authorization": { "required": true },
                    "route.request.querystring.token": { "required": false }
                }
            }),
            delete: "route.request.header.Authorization",
            remaining: || json!({ "route.request.querystring.token": { "required": false } }),
        },
        Row {
            case: "the last of two keys in key order",
            route: json!({
                "routeKey": "$connect",
                "requestParameters": {
                    "route.request.header.Authorization": { "required": true },
                    "route.request.querystring.token": { "required": true }
                }
            }),
            delete: "route.request.querystring.token",
            remaining: || json!({ "route.request.header.Authorization": { "required": true } }),
        },
        Row {
            case: "the middle key of three, a querystring key",
            route: json!({
                "routeKey": "$connect",
                "requestParameters": {
                    "route.request.body.action": { "required": true },
                    "route.request.header.X-Trace": { "required": false },
                    "route.request.querystring.token": { "required": true }
                }
            }),
            // `header` sorts between `body` and `querystring`, so this is the middle one.
            delete: "route.request.header.X-Trace",
            remaining: || {
                json!({
                    "route.request.body.action": { "required": true },
                    "route.request.querystring.token": { "required": true }
                })
            },
        },
        Row {
            case: "a body key whose entry carries no constraint member",
            route: json!({
                "routeKey": "$connect",
                "requestParameters": {
                    "route.request.body.action": {},
                    "route.request.body.payload": { "required": true }
                }
            }),
            delete: "route.request.body.action",
            remaining: || json!({ "route.request.body.payload": { "required": true } }),
        },
    ]
}

/// Create one api and answer its `ApiId`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one route under `api_id` and answer its `RouteId`.
async fn create_route(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
    );
    as_str(&created, "routeId")
}

#[tokio::test]
async fn deleting_a_request_parameter_removes_exactly_that_entry() {
    let emulator = emulator();

    for row in rows() {
        let api_id = create_api(&emulator, row.case, &websocket_api("socket-api")).await;
        let route_id = create_route(&emulator, row.case, &api_id, &row.route).await;

        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!(
                "/v2/apis/{api_id}/routes/{route_id}/requestparameters/{}",
                row.delete
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteRouteRequestParameter, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty 204 body, got {body}",
            row.case
        );

        // The store is what answers: read the route back through GetRoute.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRoute, got {status} {fetched}",
            row.case
        );
        let expected = (row.remaining)();
        assert_eq!(
            fetched.get("requestParameters"),
            Some(&expected),
            "case {:?}: expected the remaining parameters {expected}, got {fetched}",
            row.case
        );
        // Deleting a parameter touches nothing else on the route.
        assert_eq!(
            fetched.get("routeKey"),
            row.route.get("routeKey"),
            "case {:?}: expected the route key to be untouched, got {fetched}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_request_parameter_the_route_does_not_carry_is_not_found() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "the owning api", &websocket_api("owner-api")).await;
    let other_api = create_api(&emulator, "another api", &websocket_api("other-api")).await;
    let route_id = create_route(
        &emulator,
        "a route with one parameter",
        &api_id,
        &json!({
            "routeKey": "$connect",
            "requestParameters": { "route.request.header.Authorization": { "required": true } }
        }),
    )
    .await;
    // A route of an HTTP api: request parameters are "supported only for WebSocket
    // APIs", so it carries none and every key is absent.
    let http_api = create_api(
        &emulator,
        "an http api",
        &json!({ "name": "http-api", "protocolType": "HTTP" }),
    )
    .await;
    let http_route = create_route(
        &emulator,
        "an http route",
        &http_api,
        &json!({ "routeKey": "GET /things" }),
    )
    .await;

    for (case, uri) in [
        (
            "a key the route does not carry",
            format!("/v2/apis/{api_id}/routes/{route_id}/requestparameters/route.request.header.Nope"),
        ),
        (
            "the route's key, addressed through another api",
            format!(
                "/v2/apis/{other_api}/routes/{route_id}/requestparameters/route.request.header.Authorization"
            ),
        ),
        (
            "a route id nothing minted",
            format!(
                "/v2/apis/{api_id}/routes/nosuchrte0/requestparameters/route.request.header.Authorization"
            ),
        ),
        (
            "an api that does not exist",
            format!(
                "/v2/apis/nosuchapi00/routes/{route_id}/requestparameters/route.request.header.Authorization"
            ),
        ),
        (
            "a route of an HTTP api, which carries no request parameters",
            format!(
                "/v2/apis/{http_api}/routes/{http_route}/requestparameters/route.request.header.Authorization"
            ),
        ),
    ] {
        let (status, body) = call(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteRouteRequestParameter, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals removed anything: the route still carries its parameter.
    let (status, fetched) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetRoute, got {status} {fetched}"
    );
    assert_eq!(
        fetched.get("requestParameters"),
        Some(&json!({ "route.request.header.Authorization": { "required": true } })),
        "expected the refused deletes to have changed nothing, got {fetched}"
    );
}

#[tokio::test]
async fn deleting_the_same_parameter_twice_is_not_found_the_second_time() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "the owning api", &websocket_api("repeat-api")).await;
    let route_id = create_route(
        &emulator,
        "a route with two parameters",
        &api_id,
        &json!({
            "routeKey": "$connect",
            "requestParameters": {
                "route.request.header.Authorization": { "required": true },
                "route.request.querystring.token": { "required": false }
            }
        }),
    )
    .await;
    let uri = format!(
        "/v2/apis/{api_id}/routes/{route_id}/requestparameters/route.request.header.Authorization"
    );

    let (status, body) = call(&emulator, "DELETE", &uri, None).await;
    assert_eq!(status, 204, "first delete: expected 204, got {status} {body}");

    let (status, body) = call(&emulator, "DELETE", &uri, None).await;
    assert_eq!(
        status, 404,
        "second delete: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "second delete: expected NotFoundException, got {body}"
    );
    // The key level reports a fixed sentence, carrying nothing from the request.
    assert_eq!(
        as_str(&body, "message"),
        "Invalid request parameter specified",
        "second delete: expected the key-level message, got {body}"
    );

    // The other parameter survived both calls.
    let (status, fetched) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetRoute, got {status} {fetched}"
    );
    assert_eq!(
        fetched.get("requestParameters"),
        Some(&json!({ "route.request.querystring.token": { "required": false } })),
        "expected only the parameter that was never deleted, got {fetched}"
    );
}
