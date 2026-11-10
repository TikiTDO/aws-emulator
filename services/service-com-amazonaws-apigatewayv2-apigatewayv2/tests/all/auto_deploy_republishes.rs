// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what an api's DEPLOYMENTS freeze and when the service
//! releases a new one by itself.
//!
//! The rules these cases assert:
//!
//! * **A deployment freezes the api's routes and integrations as they stood when it was
//!   released**, because a deployment is "an immutable representation of an API that can
//!   be called by users" (the `Deployment` shape's own doc). A stage serves the
//!   deployment associated with it ("A Deployment must be associated with a Stage for it
//!   to be callable over the internet"), so a stage pinned to an older deployment keeps
//!   answering for the configuration that deployment froze — a route added afterwards is
//!   `404` there and a route deleted afterwards is still served there.
//! * **Every change to the api's deployable configuration releases a NEW deployment to
//!   each of its stages whose `AutoDeploy` is on**: "Specifies whether updates to an API
//!   automatically trigger a new deployment" (`Stage$AutoDeploy`). The deployment the
//!   service releases by itself is marked as such ("Specifies whether a deployment was
//!   automatically released", `Deployment$AutoDeployed`) and the stage reports it through
//!   `DeploymentId` and `LastDeploymentStatusMessage`. A stage that does not auto-deploy
//!   is left on the deployment its caller chose.
//! * A call that changes NOTHING about that configuration — a read, or a write the
//!   service refused — releases nothing: the rule is about the configuration changing,
//!   not about a request arriving.
//!
//! The rows span what the rules generalise over: WHICH writer changes the configuration
//! (routes, integrations, the responses that belong to either, authorizers, models, the
//! api's own settings and its CORS configuration), the kind of change (a create, a patch,
//! a delete), and the protocol type of the api the writer is served under.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{message, send};

/// The region the control-plane requests are signed for, and so the region the api's own
/// endpoint host names.
const REGION: &str = "eu-west-1";

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`.
const DEPLOYED: &str = "DEPLOYED";

/// The status and sentence a request matching no route of the deployment its stage serves
/// earns.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The status a request a route's `HTTP_PROXY` integration fronts earns: the caller is
/// sent to the upstream rather than the gateway dialling it, so asserting it is how these
/// cases observe that the route was MATCHED and served.
const SERVED: u16 = 307;

/// The protocol the api one row is served under: the two values `ProtocolType` declares
/// (`smithy_query(query="shape", shape="ProtocolType")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Http,
    WebSocket,
}

/// What the row's call must do to the deployment an auto-deploying stage serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The api's deployable configuration changed, so the service released a new
    /// deployment to every auto-deploying stage.
    Released,
    /// Nothing about that configuration changed, so no deployment was released.
    Unchanged,
}

/// The api's children a row's call addresses, as the prepared api reports them.
struct Prepared {
    api_id: String,
    /// The host of the api's own `ApiEndpoint`, read back off `CreateApi`.
    endpoint_host: String,
    /// The integration the route targets.
    integration_id: String,
    /// A second integration nothing targets, so a row may delete one.
    spare_integration_id: String,
    route_id: String,
    /// The route's route key, so a row may ask for a key that is already taken.
    route_key: String,
    /// The route response of the route (a WebSocket api only).
    route_response_id: String,
    /// The deployment the pinned stage serves.
    pinned_deployment_id: String,
}

/// One request, as a row spells it against the prepared api.
struct Request {
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// One row: the call, the api it is served under, the status it answers and whether it
/// changed the configuration the api's stages serve.
struct Row {
    case: &'static str,
    protocol: Protocol,
    request: fn(&Prepared) -> Request,
    status: u16,
    expect: Expect,
}

fn rows() -> Vec<Row> {
    vec![
        // The routes of the api: the recorded case that filed this is ONE of these — a
        // route ADDED to a deployed api.
        Row {
            case: "CreateRoute adds a route to the api",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "POST",
                uri: format!("/v2/apis/{}/routes", prepared.api_id),
                body: Some(json!({
                    "routeKey": "GET /added",
                    "target": format!("integrations/{}", prepared.integration_id)
                })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "UpdateRoute moves a route onto another key",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "PATCH",
                uri: format!("/v2/apis/{}/routes/{}", prepared.api_id, prepared.route_id),
                body: Some(json!({ "routeKey": "GET /moved" })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "DeleteRoute removes the api's only route",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "DELETE",
                uri: format!("/v2/apis/{}/routes/{}", prepared.api_id, prepared.route_id),
                body: None,
            },
            status: 204,
            expect: Expect::Released,
        },
        Row {
            case: "DeleteRouteRequestParameter drops a parameter off a route",
            protocol: Protocol::WebSocket,
            request: |prepared| Request {
                method: "DELETE",
                uri: format!(
                    "/v2/apis/{}/routes/{}/requestparameters/route.request.header.x-trace",
                    prepared.api_id, prepared.route_id
                ),
                body: None,
            },
            status: 204,
            expect: Expect::Released,
        },
        // The responses that belong to a route, which the route carries.
        Row {
            case: "CreateRouteResponse adds a response under a route",
            protocol: Protocol::WebSocket,
            request: |prepared| Request {
                method: "POST",
                uri: format!(
                    "/v2/apis/{}/routes/{}/routeresponses",
                    prepared.api_id, prepared.route_id
                ),
                body: Some(json!({ "routeResponseKey": "added" })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "UpdateRouteResponse moves a response onto another key",
            protocol: Protocol::WebSocket,
            request: |prepared| Request {
                method: "PATCH",
                uri: format!(
                    "/v2/apis/{}/routes/{}/routeresponses/{}",
                    prepared.api_id, prepared.route_id, prepared.route_response_id
                ),
                body: Some(json!({ "routeResponseKey": "moved" })),
            },
            status: 201,
            expect: Expect::Released,
        },
        // The integrations of the api, and the responses that belong to one.
        Row {
            case: "CreateIntegration adds an integration to the api",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "POST",
                uri: format!("/v2/apis/{}/integrations", prepared.api_id),
                body: Some(json!({
                    "integrationType": "HTTP_PROXY",
                    "integrationMethod": "GET",
                    "integrationUri": "https://added.example.com/items",
                    "payloadFormatVersion": "1.0"
                })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "UpdateIntegration patches the integration a route targets",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "PATCH",
                uri: format!(
                    "/v2/apis/{}/integrations/{}",
                    prepared.api_id, prepared.integration_id
                ),
                body: Some(json!({ "timeoutInMillis": 9000 })),
            },
            status: 200,
            expect: Expect::Released,
        },
        Row {
            case: "DeleteIntegration removes an integration no route targets",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "DELETE",
                uri: format!(
                    "/v2/apis/{}/integrations/{}",
                    prepared.api_id, prepared.spare_integration_id
                ),
                body: None,
            },
            status: 204,
            expect: Expect::Released,
        },
        Row {
            case: "CreateIntegrationResponse adds a response under an integration",
            protocol: Protocol::WebSocket,
            request: |prepared| Request {
                method: "POST",
                uri: format!(
                    "/v2/apis/{}/integrations/{}/integrationresponses",
                    prepared.api_id, prepared.integration_id
                ),
                body: Some(json!({ "integrationResponseKey": "$default" })),
            },
            status: 201,
            expect: Expect::Released,
        },
        // The api-level records its routes are served under.
        Row {
            case: "CreateAuthorizer adds an authorizer to the api",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "POST",
                uri: format!("/v2/apis/{}/authorizers", prepared.api_id),
                body: Some(json!({
                    "name": "gate",
                    "authorizerType": "JWT",
                    "identitySource": ["$request.header.Authorization"],
                    "jwtConfiguration": {
                        "audience": ["an-audience"],
                        "issuer": "https://issuer.example.com/"
                    }
                })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "CreateModel adds a model to the api",
            protocol: Protocol::WebSocket,
            request: |prepared| Request {
                method: "POST",
                uri: format!("/v2/apis/{}/models", prepared.api_id),
                body: Some(json!({
                    "name": "Order",
                    "contentType": "application/json",
                    "schema": json!({
                        "$schema": "http://json-schema.org/draft-04/schema#",
                        "title": "Order",
                        "type": "object"
                    })
                    .to_string()
                })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "UpdateApi patches the api's own settings",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "PATCH",
                uri: format!("/v2/apis/{}", prepared.api_id),
                body: Some(json!({ "description": "patched by the test" })),
            },
            status: 201,
            expect: Expect::Released,
        },
        Row {
            case: "DeleteCorsConfiguration clears the api's CORS configuration",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "DELETE",
                uri: format!("/v2/apis/{}/cors", prepared.api_id),
                body: None,
            },
            status: 204,
            expect: Expect::Released,
        },
        // And the two kinds of call that change nothing.
        Row {
            case: "a read of the api's routes changes nothing",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "GET",
                uri: format!("/v2/apis/{}/routes", prepared.api_id),
                body: None,
            },
            status: 200,
            expect: Expect::Unchanged,
        },
        Row {
            case: "a CreateRoute the service refused over a duplicate key changes nothing",
            protocol: Protocol::Http,
            request: |prepared| Request {
                method: "POST",
                uri: format!("/v2/apis/{}/routes", prepared.api_id),
                body: Some(json!({ "routeKey": prepared.route_key })),
            },
            status: 409,
            expect: Expect::Unchanged,
        },
    ]
}

/// The `CreateApi` request for an api of `protocol`: an HTTP api carries a CORS
/// configuration (so a row may clear one), a WebSocket api the route selection expression
/// its routes are dispatched by.
fn api_request(protocol: Protocol) -> Value {
    match protocol {
        Protocol::Http => json!({
            "name": "redeploy-http",
            "protocolType": "HTTP",
            "corsConfiguration": { "allowOrigins": ["https://app.example.com"] }
        }),
        Protocol::WebSocket => json!({
            "name": "redeploy-socket",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    }
}

/// The integration an api of `protocol` can hold: a `PayloadFormatVersion` is "required
/// for HTTP APIs" (`CreateIntegration$PayloadFormatVersion`), and `RequestTemplates` and
/// the responses an integration carries are supported only for WebSocket APIs.
fn integration_request(protocol: Protocol, upstream: &str) -> Value {
    match protocol {
        Protocol::Http => json!({
            "integrationType": "HTTP_PROXY",
            "integrationMethod": "GET",
            "integrationUri": upstream,
            "payloadFormatVersion": "1.0"
        }),
        Protocol::WebSocket => json!({
            "integrationType": "HTTP_PROXY",
            "integrationMethod": "GET",
            "integrationUri": upstream
        }),
    }
}

/// The route of an api of `protocol`: an HTTP api's key names a method and a path, a
/// WebSocket api's names one of its reserved keys, and the members a route may carry
/// (`RequestParameters`, `RouteResponseSelectionExpression`) are supported only for
/// WebSocket APIs — with `RequestParameters` accepted only on `$connect`, the route that
/// serves the connection request.
fn route_request(protocol: Protocol, integration_id: &str) -> (String, Value) {
    let target = format!("integrations/{integration_id}");
    match protocol {
        Protocol::Http => (
            "GET /items".to_string(),
            json!({ "routeKey": "GET /items", "target": target }),
        ),
        Protocol::WebSocket => (
            "$connect".to_string(),
            json!({
                "routeKey": "$connect",
                "target": target,
                "routeResponseSelectionExpression": "$default",
                "requestParameters": {
                    "route.request.header.x-trace": { "required": true }
                }
            }),
        ),
    }
}

/// Create an api of `protocol` with two integrations, one route, a deployment, a stage
/// PINNED to that deployment and a `$default` stage that AUTO-DEPLOYS.
async fn prepare(emulator: &MultiServiceRouter, case: &str, protocol: Protocol) -> Prepared {
    let (status, api) = call(emulator, "POST", "/v2/apis", Some(api_request(protocol))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let endpoint_host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let integration_id = create_integration(
        emulator,
        case,
        &api_id,
        protocol,
        "https://upstream.example.com/items",
    )
    .await;
    let spare_integration_id = create_integration(
        emulator,
        case,
        &api_id,
        protocol,
        "https://spare.example.com/items",
    )
    .await;

    let (route_key, request) = route_request(protocol, &integration_id);
    let (status, route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {route}"
    );
    let route_id = as_str(&route, "routeId");

    let route_response_id = if protocol == Protocol::WebSocket {
        let (status, response) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
            Some(json!({ "routeResponseKey": "$default" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRouteResponse must answer 201, got {status} {response}"
        );
        as_str(&response, "routeResponseId")
    } else {
        String::new()
    };

    let (status, deployment) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "description": "the deployment the pinned stage serves" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDeployment must answer 201, got {status} {deployment}"
    );
    let pinned_deployment_id = as_str(&deployment, "deploymentId");

    let (status, pinned) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "pinned", "deploymentId": pinned_deployment_id })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201 for the pinned stage, got {status} {pinned}"
    );

    let (status, auto) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201 for the auto-deploying stage, got \
         {status} {auto}"
    );

    Prepared {
        api_id,
        endpoint_host,
        integration_id,
        spare_integration_id,
        route_id,
        route_key,
        route_response_id,
        pinned_deployment_id,
    }
}

/// Create one integration under `api_id` and answer its `IntegrationId`.
async fn create_integration(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    protocol: Protocol,
    upstream: &str,
) -> String {
    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(integration_request(protocol, upstream)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
    );
    as_str(&integration, "integrationId")
}

/// The stage `stage_name` of `api_id`, read back through `GetStage`.
async fn stage(emulator: &MultiServiceRouter, case: &str, api_id: &str, stage_name: &str) -> Value {
    let (status, stage) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStage must answer 200 for {stage_name:?}, got {status} {stage}"
    );
    stage
}

#[tokio::test]
async fn a_change_to_the_apis_configuration_releases_a_deployment_to_every_auto_deploying_stage() {
    for row in rows() {
        let emulator = emulator();
        let case = row.case;
        let prepared = prepare(&emulator, case, row.protocol).await;

        // What each stage serves before the row's call.
        let before = stage(&emulator, case, &prepared.api_id, "$default").await;
        let released_before = as_str(&before, "deploymentId");
        assert!(
            !released_before.is_empty(),
            "case {case:?}: an auto-deploying stage carries a deployment of its own, got \
             {before}"
        );

        let request = (row.request)(&prepared);
        let (status, answered) = call(
            &emulator,
            request.method,
            &request.uri,
            request.body.clone(),
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {} {} to answer {}, got {status} {answered}",
            request.method, request.uri, row.status
        );

        let after = stage(&emulator, case, &prepared.api_id, "$default").await;
        let released_after = as_str(&after, "deploymentId");
        match row.expect {
            Expect::Released => {
                assert_ne!(
                    released_after, released_before,
                    "case {case:?}: the auto-deploying stage must be released a new \
                     deployment, got {after}"
                );
                // The stage reports the release, and the deployment it now serves is one
                // the SERVICE released.
                assert_eq!(
                    as_str(&after, "lastDeploymentStatusMessage"),
                    format!("Successfully deployed stage with deployment id '{released_after}'"),
                    "case {case:?}: the stage must report the deployment it was last \
                     released, got {after}"
                );
                let (status, deployment) = call(
                    &emulator,
                    "GET",
                    &format!(
                        "/v2/apis/{}/deployments/{released_after}",
                        prepared.api_id
                    ),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetDeployment must answer 200 for the deployment the \
                     stage now serves, got {status} {deployment}"
                );
                assert_eq!(
                    deployment.get("autoDeployed").and_then(Value::as_bool),
                    Some(true),
                    "case {case:?}: a deployment the service released by itself is \
                     autoDeployed, got {deployment}"
                );
                assert_eq!(
                    as_str(&deployment, "deploymentStatus"),
                    DEPLOYED,
                    "case {case:?}: the released deployment must be {DEPLOYED}, got \
                     {deployment}"
                );
            }
            Expect::Unchanged => assert_eq!(
                released_after, released_before,
                "case {case:?}: nothing about the api's configuration changed, so no \
                 deployment may be released, got {after}"
            ),
        }

        // Either way the stage that does NOT auto-deploy is left on the deployment its
        // caller chose: "Can't be updated if autoDeploy is enabled" names the only
        // deployment the service moves (`UpdateStage$DeploymentId`).
        let pinned = stage(&emulator, case, &prepared.api_id, "pinned").await;
        assert_eq!(
            as_str(&pinned, "deploymentId"),
            prepared.pinned_deployment_id,
            "case {case:?}: a stage that does not auto-deploy keeps the deployment it was \
             pinned to, got {pinned}"
        );
        assert!(
            pinned
                .get("lastDeploymentStatusMessage")
                .and_then(Value::as_str)
                .is_none(),
            "case {case:?}: a stage the service released nothing to reports no last \
             deployment status, got {pinned}"
        );
    }
}

#[tokio::test]
async fn a_pinned_stage_serves_the_configuration_its_deployment_froze() {
    let emulator = emulator();
    let case = "a stage pinned to a deployment, beside an auto-deploying stage";
    let prepared = prepare(&emulator, case, Protocol::Http).await;
    let api_id = &prepared.api_id;
    let host = &prepared.endpoint_host;

    // The route both stages' deployments froze is served through both of them: the
    // `$default` stage serves the api's root, a named stage is reached under its name
    // ("the stage name is typically appended to this URI", `Api$ApiEndpoint`).
    for (stage_name, path) in [("$default", "/items"), ("pinned", "/pinned/items")] {
        let answer = send(&emulator, host, "GET", path, &[]).await;
        assert_eq!(
            answer.status,
            SERVED,
            "case {case:?}: the {stage_name:?} stage must serve the route its deployment \
             froze, got {}",
            answer.summary()
        );
    }

    // A route added AFTER both deployments were released: the auto-deploying stage has the
    // new configuration released to it, the pinned stage keeps the frozen one.
    let (status, added) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "GET /added",
            "target": format!("integrations/{}", prepared.integration_id)
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {added}"
    );

    let answer = send(&emulator, host, "GET", "/added", &[]).await;
    assert_eq!(
        answer.status,
        SERVED,
        "case {case:?}: the auto-deploying stage must serve the route just added, got {}",
        answer.summary()
    );
    let answer = send(&emulator, host, "GET", "/pinned/added", &[]).await;
    assert_eq!(
        answer.status,
        NOT_FOUND,
        "case {case:?}: the pinned stage must not serve a route its deployment never \
         froze, got {}",
        answer.summary()
    );
    assert_eq!(
        message(&answer),
        NOT_FOUND_MESSAGE,
        "case {case:?}: a request matching no route of the served deployment is \
         {NOT_FOUND_MESSAGE:?}, got {}",
        answer.summary()
    );

    // And a route DELETED afterwards: gone from the auto-deploying stage, still served by
    // the pinned one, because the deployment it serves is immutable.
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/routes/{}", prepared.route_id),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteRoute must answer 204, got {status} {deleted}"
    );

    let answer = send(&emulator, host, "GET", "/items", &[]).await;
    assert_eq!(
        answer.status,
        NOT_FOUND,
        "case {case:?}: the auto-deploying stage must stop serving a deleted route, got {}",
        answer.summary()
    );
    let answer = send(&emulator, host, "GET", "/pinned/items", &[]).await;
    assert_eq!(
        answer.status,
        SERVED,
        "case {case:?}: the pinned stage must keep serving the route its deployment froze, \
         got {}",
        answer.summary()
    );

    // The pinned stage still serves the integration that deployment froze, too: the
    // upstream it redirects to is the one the api held when the deployment was released,
    // whatever the integration says now.
    let (status, patched) = call(
        &emulator,
        "PATCH",
        &format!(
            "/v2/apis/{api_id}/integrations/{}",
            prepared.integration_id
        ),
        Some(json!({ "integrationUri": "https://moved.example.com/items" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateIntegration must answer 200, got {status} {patched}"
    );
    let answer = send(&emulator, host, "GET", "/pinned/items", &[]).await;
    assert_eq!(
        answer.location.as_deref(),
        Some("https://upstream.example.com/items"),
        "case {case:?}: the pinned stage must serve the integration its deployment froze, \
         got {}",
        answer.summary()
    );
    let answer = send(&emulator, host, "GET", "/added", &[]).await;
    assert_eq!(
        answer.location.as_deref(),
        Some("https://moved.example.com/items"),
        "case {case:?}: the auto-deploying stage must serve the patched integration, got {}",
        answer.summary()
    );

    // Nothing of this depends on the region in the api's endpoint host: the host under
    // test is the one `CreateApi` announced for this api.
    assert!(
        host.contains(REGION),
        "case {case:?}: the api's announced endpoint must name the request's region, got \
         {host:?}"
    );
}
