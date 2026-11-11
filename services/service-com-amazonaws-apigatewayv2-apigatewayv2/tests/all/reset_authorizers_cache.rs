// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ResetAuthorizersCache`.
//!
//! The rule these cases assert: **a call that empties a stage's authorizer cache addresses
//! three resources in turn (`DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers`)
//! — the api, its stage, and that stage's authorizer cache — so it is served only once all
//! three resolve in this account's records, and each one that does not is refused with the
//! modelled `NotFoundException` (HTTP 404) naming the resource that resolved to nothing: an
//! `ApiId` the account holds no api under, a `StageName` that api holds no stage under, or a
//! stage of an api whose `ProtocolType` is not `HTTP`, since the cache is "Supported only
//! for HTTP APIs" and so exists on no other kind of api. A request whose three resources all
//! resolve is answered `200 OK` — the status API Gateway answers this flush with — carrying
//! no content and stating the length of that nothing (`content-length: 0`), however often it
//! is repeated and whatever the stage held.** Emptying the cache is not a change to the
//! stage RESOURCE, so the stage a later `GetStage` reports is byte-for-byte the one that was
//! there before.
//!
//! Errors and the modelled status from the model: `smithy_query(query="operation",
//! operation="ResetAuthorizersCache")` — `NotFoundException` `httpError` 404,
//! `TooManyRequestsException` 429, success `code: 204` over `smithy.api#Unit`; the status the
//! service really answers a flush with is `crate::state::FLUSHED_CACHE_OK_STATUS`, and the
//! suite that pins it beside every other status this service answers is
//! `mutating_success_status.rs`. The protocol restriction is that operation's own
//! documentation ("Resets all authorizer cache entries on a stage. Supported only for HTTP
//! APIs").
//!
//! The rows span what the rule generalises over: the FORM of the stage name (the reserved
//! `$default` that serves an api's root, and named stages), whether the api carries an
//! authorizer at all, whether the stage has a deployment released to it, and how many times
//! the same call is repeated. The refusal rows span which of the three addressed resources
//! is the one that does not resolve, the api's `ProtocolType`, and the form of the value
//! that misses.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::DEFAULT_STAGE_NAME;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, call_reading_headers, emulator, error_code};
use crate::empty_answer_framing::{content_length, NO_CONTENT_LENGTH};

/// The status this operation is answered with: the one API Gateway puts on the wire for a
/// flush, which is not the `code: 204` the model declares for it.
const FLUSHED_STATUS: u16 = 200;

/// The `ProtocolType` whose stages carry an authorizer cache at all.
const HTTP_PROTOCOL: &str = "HTTP";

/// The other `ProtocolType` this control plane serves, whose stages carry none.
const WEBSOCKET_PROTOCOL: &str = "WEBSOCKET";

/// One served row: the HTTP api to build and the stage on it the cache is emptied for.
struct Served {
    case: &'static str,
    stage_name: &'static str,
    /// Whether the api carries a `REQUEST` authorizer — the kind whose decisions a stage's
    /// authorizer cache would hold.
    with_authorizer: bool,
    /// Whether a deployment is released to the stage before the cache is emptied.
    deployed: bool,
}

fn served_rows() -> Vec<Served> {
    vec![
        Served {
            case: "an HTTP api's $default stage, no authorizer, no deployment",
            stage_name: DEFAULT_STAGE_NAME,
            with_authorizer: false,
            deployed: false,
        },
        Served {
            case: "an HTTP api's named stage carrying a REQUEST authorizer and a deployment",
            stage_name: "prod",
            with_authorizer: true,
            deployed: true,
        },
        Served {
            case: "an HTTP api's $default stage carrying a REQUEST authorizer",
            stage_name: DEFAULT_STAGE_NAME,
            with_authorizer: true,
            deployed: false,
        },
        Served {
            case: "an HTTP api's stage name carrying a hyphen and an underscore",
            stage_name: "blue_green-2",
            with_authorizer: false,
            deployed: true,
        },
    ]
}

/// Create one api of `protocol_type` and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, name: &str, protocol_type: &str) -> String {
    let mut request = json!({ "name": name, "protocolType": protocol_type });
    if protocol_type == WEBSOCKET_PROTOCOL {
        // A WEBSOCKET api dispatches by this expression, which its create requires.
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Give `api_id` one `REQUEST` authorizer — the kind whose answers a stage caches.
async fn create_request_authorizer(emulator: &MultiServiceRouter, case: &str, api_id: &str) {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "gate",
            "authorizerType": "REQUEST",
            "identitySource": ["$request.header.Authorization"],
            "authorizerUri":
                "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                 arn:aws:lambda:us-east-1:000000000000:function:gate/invocations",
            "authorizerPayloadFormatVersion": "2.0",
            "authorizerResultTtlInSeconds": 300
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAuthorizer, got {status} {created}"
    );
}

/// Create one stage named `stage_name` on `api_id`, releasing a deployment to it when
/// `deployed`.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
    deployed: bool,
) {
    let mut request = json!({ "stageName": stage_name });
    if deployed {
        let (status, route) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": "$default" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRoute, got {status} {route}"
        );
        let (status, deployment) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/deployments"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateDeployment, got {status} {deployment}"
        );
        request["deploymentId"] = json!(as_str(&deployment, "deploymentId"));
    }
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
}

/// The URI the authorizer cache of one stage is addressed at.
fn cache_uri(api_id: &str, stage_name: &str) -> String {
    format!("/v2/apis/{api_id}/stages/{stage_name}/cache/authorizers")
}

/// Read one stage back, asserting the read succeeded.
async fn read_stage(emulator: &MultiServiceRouter, case: &str, api_id: &str, stage_name: &str) -> Value {
    let (status, stage) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetStage, got {status} {stage}"
    );
    stage
}

#[tokio::test]
async fn a_resolved_api_and_stage_answer_an_empty_framed_200_and_leave_the_stage_alone() {
    let emulator = emulator();

    for (index, row) in served_rows().into_iter().enumerate() {
        let case = row.case;
        let api_id = create_api(
            &emulator,
            case,
            &format!("reset-cache-{index}"),
            HTTP_PROTOCOL,
        )
        .await;
        if row.with_authorizer {
            create_request_authorizer(&emulator, case, &api_id).await;
        }
        create_stage(&emulator, case, &api_id, row.stage_name, row.deployed).await;

        let before = read_stage(&emulator, case, &api_id, row.stage_name).await;

        // Repeating the call changes nothing about the answer: the stage holds the empty
        // cache this asks for after the first call, and AWS answers the same either way.
        for round in 0..3 {
            let (status, headers, body) = call_reading_headers(
                &emulator,
                "DELETE",
                &cache_uri(&api_id, row.stage_name),
                None,
            )
            .await;
            assert_eq!(
                status, FLUSHED_STATUS,
                "case {case:?} (round {round}): expected {FLUSHED_STATUS} from \
                 ResetAuthorizersCache, got {status} {body}"
            );
            assert_eq!(
                body,
                Value::Null,
                "case {case:?} (round {round}): expected an answer carrying no body, got {body}"
            );
            assert_eq!(
                content_length(&headers).as_deref(),
                Some(NO_CONTENT_LENGTH),
                "case {case:?} (round {round}): expected the answer to state content-length \
                 {NO_CONTENT_LENGTH}, got {:?}",
                content_length(&headers)
            );
        }

        // Emptying the cache is not a change to the stage resource, so the stage reported
        // afterwards is the one that was there before — `LastUpdatedDate` included.
        let after = read_stage(&emulator, case, &api_id, row.stage_name).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected ResetAuthorizersCache to leave the stage untouched, got \
             {after} after {before}"
        );
    }
}

#[tokio::test]
async fn an_api_stage_or_cache_that_does_not_resolve_is_not_found() {
    let emulator = emulator();

    // Two HTTP apis, each with one stage, so a miss is a miss on the value the request named
    // rather than on an empty account — plus a WEBSOCKET api whose stages carry no authorizer
    // cache at all.
    let first = create_api(&emulator, "the first api", "reset-cache-first", HTTP_PROTOCOL).await;
    create_stage(&emulator, "the first api", &first, "prod", false).await;
    let second =
        create_api(&emulator, "the second api", "reset-cache-second", HTTP_PROTOCOL).await;
    create_stage(&emulator, "the second api", &second, DEFAULT_STAGE_NAME, false).await;
    let socket = create_api(
        &emulator,
        "the websocket api",
        "reset-cache-socket",
        WEBSOCKET_PROTOCOL,
    )
    .await;
    for stage_name in [DEFAULT_STAGE_NAME, "live"] {
        create_stage(&emulator, "the websocket api", &socket, stage_name, false).await;
    }
    // What that WEBSOCKET api's stages look like before anything is asked of them, so the
    // refusals below can be shown to have left them alone.
    let socket_before = read_stage(&emulator, "the websocket api", &socket, "live").await;

    // (case, the ApiId, the StageName, the resource the refusal must name)
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "an ApiId of the minted form that no api carries",
            "aaaaaaaaaa".to_string(),
            "prod".to_string(),
            "API",
        ),
        (
            "an ApiId that is not of the minted form",
            "no-such-api".to_string(),
            DEFAULT_STAGE_NAME.to_string(),
            "API",
        ),
        (
            "a held api and a stage name it holds no stage under",
            first.clone(),
            "nosuchstage".to_string(),
            "stage",
        ),
        (
            "a held api and the OTHER api's stage name",
            first.clone(),
            DEFAULT_STAGE_NAME.to_string(),
            "stage",
        ),
        (
            "a held api and a stage name carrying hyphens and underscores",
            first.clone(),
            "blue_green-2".to_string(),
            "stage",
        ),
        (
            "the other api and a named stage it holds no stage under",
            second.clone(),
            "prod".to_string(),
            "stage",
        ),
        // The api and the stage both resolve here; the cache is what does not, because the
        // sub-resource is supported only for HTTP apis.
        (
            "a WEBSOCKET api's named stage, which carries no authorizer cache",
            socket.clone(),
            "live".to_string(),
            "authorizers cache",
        ),
        (
            "a WEBSOCKET api's $default stage, which carries no authorizer cache",
            socket.clone(),
            DEFAULT_STAGE_NAME.to_string(),
            "authorizers cache",
        ),
        // A stage the WEBSOCKET api does not hold is still reported as the missing STAGE:
        // the resources are resolved in the order the URI names them.
        (
            "a WEBSOCKET api and a stage name it holds no stage under",
            socket.clone(),
            "nosuchstage".to_string(),
            "stage",
        ),
    ];

    for (case, api_id, stage_name, resource) in rows {
        let (status, body) =
            call(&emulator, "DELETE", &cache_uri(&api_id, &stage_name), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from ResetAuthorizersCache, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        let message = as_str(&body, "message");
        assert!(
            message.contains(resource),
            "case {case:?}: expected the refusal to name the {resource} that resolved to \
             nothing, got {message:?}"
        );
    }

    // The WEBSOCKET api's stage is exactly as it was: a refused call wrote nothing.
    let socket_after = read_stage(&emulator, "the websocket api", &socket, "live").await;
    assert_eq!(
        socket_after, socket_before,
        "case \"the websocket api after the refusals\": expected a refused \
         ResetAuthorizersCache to leave the stage untouched, got {socket_after} after \
         {socket_before}"
    );

    // Both HTTP stages are still there and still servable: a refusal removed nothing.
    for (case, api_id, stage_name) in [
        ("the first api after the refusals", &first, "prod"),
        (
            "the second api after the refusals",
            &second,
            DEFAULT_STAGE_NAME,
        ),
    ] {
        let (status, body) = call(&emulator, "DELETE", &cache_uri(api_id, stage_name), None).await;
        assert_eq!(
            status, FLUSHED_STATUS,
            "case {case:?}: expected {FLUSHED_STATUS}, got {status} {body}"
        );
    }
}
