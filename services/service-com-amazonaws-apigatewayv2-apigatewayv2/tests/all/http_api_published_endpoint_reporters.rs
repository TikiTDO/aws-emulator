// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the invoke address EVERY reporter of an api publishes, and for
//! each spelling of the invoke URL a client builds from it.
//!
//! The rule, one level above the recorded case that filed the issue (a client that read
//! `ApiEndpoint` off `CreateApi` and could not reach the address it was given):
//!
//! > A value this service publishes for a client to DIAL is an address this emulator itself
//! > answers on, so EVERY operation that reports an api's `ApiEndpoint` reports that one
//! > address, and every invoke URL a client builds from it — the endpoint with the stage
//! > name appended, and the endpoint on its own for the stage that serves the api's root
//! > ("the stage name is typically appended to this URI to form a complete path to a
//! > deployed API stage", `Api$ApiEndpoint`) — reaches the route the api deployed.
//!
//! The rows span what the rule generalises over: the endpoint the control-plane request was
//! addressed at (a loopback literal with a port, a loopback NAME with another port, and one
//! of AWS's own service hosts, each signed for a different Region), the OPERATION that
//! published the value (`CreateApi`, `GetApi`, `GetApis`, `UpdateApi`, `ReimportApi`) and the
//! spelling of the invoke URL the client writes (the endpoint alone, with a trailing slash,
//! with a route path, with a trailing slash on that path, and each of those with the root
//! stage's own name appended). The recorded case — one HTTP api read off `CreateApi` and
//! invoked at the endpoint with a trailing slash — is ONE row of each.
//!
//! `http_api_published_invoke_address` holds the same member's rule for `CreateApi`, `GetApi`,
//! `GetApis` and `ImportApi` over the endpoint forms; this suite holds the two halves it does
//! not: the api MUTATORS that report the member back (`UpdateApi`, `ReimportApi`), which must
//! not publish an address other than the one the api is served on, and the spellings of the
//! invoke URL that address an api's ROOT — the path the recorded client built.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::websocket_handshake_data_plane::{send, Answer};

/// The status an `HTTP_PROXY` route answers: the upstream is a URI the CALLER can reach, so
/// the caller is sent there — `307`, with the integration's own `IntegrationUri` in
/// `Location`. Reaching it proves the request resolved all the way to the route's
/// integration, which is what an address being SERVED means.
const SERVED: u16 = 307;

/// The front door's envelope for a request no service claimed. The defect this suite pins is
/// an address nothing serves, so this is never a legitimate answer here.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The upstream every row's `HTTP_PROXY` integration fronts.
const UPSTREAM: &str = "https://upstream.example.com/items";

/// The reserved stage "that serves an api's root" (`$default`), reached with no stage segment
/// in the path at all — and, because the member says the stage name is only "typically"
/// appended, reachable with its own name as the first segment too.
const DEFAULT_STAGE: &str = "$default";

/// The two routes every row deploys: the api's ROOT, which is what a client that appends
/// nothing to the published endpoint addresses, and one nested path.
const ROOT_ROUTE_KEY: &str = "GET /";
const NESTED_ROUTE_KEY: &str = "GET /items";
const NESTED_ROUTE_PATH: &str = "/items";

/// Which address a row must be announced on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Announced {
    /// The emulator's own endpoint, with the api behind the reserved data-plane path.
    EmulatorEndpoint,
    /// The `{api-id}.execute-api.{region}.amazonaws.com` host `Api$ApiEndpoint` documents,
    /// which this data plane claims host-style.
    ExecuteApiHost,
}

/// One row: the endpoint the control-plane calls are addressed at, the Region they are signed
/// for, and the address the api must be announced on.
struct Row {
    case: &'static str,
    /// The `Host` the control-plane request carries — the endpoint the client is configured
    /// with.
    host: &'static str,
    region: &'static str,
    announced: Announced,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a client configured with the emulator's loopback \
                   endpoint",
            host: "127.0.0.1:39461",
            region: "us-west-2",
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by its loopback NAME on another port",
            host: "localhost:9297",
            region: "ap-south-1",
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "a client that resolves AWS's own hosts: the documented execute-api host",
            host: "apigateway.eu-north-1.amazonaws.com",
            region: "eu-north-1",
            announced: Announced::ExecuteApiHost,
        },
    ]
}

/// The announced endpoint split into the authority a client dials and the path it carries
/// ahead of the stage — empty for an endpoint that is a bare host.
fn announced_parts(announced: &str) -> (String, String) {
    let without_scheme = announced
        .split_once("://")
        .map_or(announced, |(_scheme, rest)| rest);
    match without_scheme.find('/') {
        Some(at) => (
            without_scheme[..at].to_string(),
            without_scheme[at..].to_string(),
        ),
        None => (without_scheme.to_string(), String::new()),
    }
}

/// Assert `announced` is the address this emulator serves this api on, in the form this row's
/// client is announced it, and answer the authority and base path a client builds its invoke
/// URL from.
fn assert_announced(row: &Row, reporter: &str, api_id: &str, announced: &str) -> (String, String) {
    assert!(
        announced.starts_with("https://"),
        "case {:?}: {reporter} must announce an https endpoint for an HTTP api, got \
         {announced:?}",
        row.case
    );
    let (authority, base_path) = announced_parts(announced);
    match row.announced {
        Announced::EmulatorEndpoint => {
            assert_eq!(
                authority, row.host,
                "case {:?}: {reporter} must announce the endpoint the request reached, got \
                 {announced:?}",
                row.case
            );
            assert!(
                !authority.ends_with("amazonaws.com"),
                "case {:?}: {reporter} must not announce an AWS host no client of this \
                 emulator resolves, got {announced:?}",
                row.case
            );
            assert_eq!(
                base_path,
                format!("/_aws/execute-api/{api_id}"),
                "case {:?}: {reporter} must name the api in the reserved data-plane path, got \
                 {announced:?}",
                row.case
            );
        }
        Announced::ExecuteApiHost => {
            assert_eq!(
                authority,
                format!("{api_id}.execute-api.{}.amazonaws.com", row.region),
                "case {:?}: {reporter} must announce the api's own execute-api host, got \
                 {announced:?}",
                row.case
            );
            assert!(
                base_path.is_empty(),
                "case {:?}: the documented endpoint carries no path of its own, got \
                 {announced:?}",
                row.case
            );
        }
    }
    (authority, base_path)
}

/// The OpenAPI definition `ReimportApi` PUTS over the api: the same two routes, each fronting
/// the same upstream, so what the reimport is asserted on is the ENDPOINT it reports and not a
/// changed route set.
fn definition(name: &str) -> String {
    let operation = json!({
        "get": {
            "x-amazon-apigateway-integration": {
                "type": "http_proxy",
                "httpMethod": "GET",
                "uri": UPSTREAM,
                "payloadFormatVersion": "1.0"
            }
        }
    });
    json!({
        "openapi": "3.0.1",
        "info": { "title": name, "version": "1.0" },
        "paths": { "/": operation, NESTED_ROUTE_PATH: operation }
    })
    .to_string()
}

/// Deploy the row's api: an `HTTP_PROXY` integration, the root route and the nested one that
/// front it, and the auto-deploying stage that serves the api's root. Answers the `ApiId` and
/// the `ApiEndpoint` `CreateApi` announced.
async fn deploy(emulator: &MultiServiceRouter, row: &Row, name: &str) -> (String, String) {
    let (status, api) = call_at_host(
        emulator,
        row.host,
        row.region,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateApi must answer 201, got {status} {api}",
        row.case
    );
    let api_id = as_str(&api, "apiId");
    let announced = as_str(&api, "apiEndpoint");

    let (status, integration) = call_at_host(
        emulator,
        row.host,
        row.region,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationMethod": "GET",
            "integrationUri": UPSTREAM,
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateIntegration must answer 201, got {status} {integration}",
        row.case
    );
    let integration_id = as_str(&integration, "integrationId");

    for route_key in [ROOT_ROUTE_KEY, NESTED_ROUTE_KEY] {
        let (status, route) = call_at_host(
            emulator,
            row.host,
            row.region,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateRoute for {route_key:?} must answer 201, got {status} {route}",
            row.case
        );
    }

    let (status, stage) = call_at_host(
        emulator,
        row.host,
        row.region,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": DEFAULT_STAGE, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateStage must answer 201, got {status} {stage}",
        row.case
    );
    assert!(
        stage.get("deploymentId").and_then(Value::as_str).is_some(),
        "case {:?}: an auto-deploying stage must carry a deployment, got {stage}",
        row.case
    );

    (api_id, announced)
}

/// Send one invoke to `authority` at `path` and assert the api's route SERVED it — and,
/// first, that the front door did not answer that nothing claims the address.
async fn assert_served(
    case: &str,
    emulator: &MultiServiceRouter,
    spelling: &str,
    authority: &str,
    path: &str,
) {
    let answer: Answer = send(emulator, authority, "GET", path, &[]).await;
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the published address must be one this emulator serves, but \
         {spelling} ({authority}{path}) is claimed by no service: {}",
        answer.summary()
    );
    assert_eq!(
        answer.status,
        SERVED,
        "case {case:?}: {spelling} ({authority}{path}) must reach the route's integration, \
         got {}",
        answer.summary()
    );
    assert_eq!(
        answer.location.as_deref(),
        Some(UPSTREAM),
        "case {case:?}: {spelling} must be sent to the upstream the integration names, got {}",
        answer.summary()
    );
}

/// Every spelling of the invoke URL a client builds from the published endpoint, as
/// `(what a failing assertion calls it, the path to send)`.
///
/// `base_path` is what the endpoint carries ahead of the stage, which is empty for the
/// documented execute-api host and the reserved data-plane path for the emulator's own
/// endpoint; an empty path is the root itself.
fn spellings(base_path: &str) -> Vec<(String, String)> {
    let at = |path: String| if path.is_empty() { "/".to_string() } else { path };
    vec![
        (
            "the endpoint on its own, for the stage that serves the api's root".to_string(),
            at(base_path.to_string()),
        ),
        (
            "the endpoint with a trailing slash".to_string(),
            at(format!("{base_path}/")),
        ),
        (
            format!("the endpoint with {NESTED_ROUTE_PATH:?} appended"),
            at(format!("{base_path}{NESTED_ROUTE_PATH}")),
        ),
        (
            format!("the endpoint with {NESTED_ROUTE_PATH:?} appended and a trailing slash"),
            at(format!("{base_path}{NESTED_ROUTE_PATH}/")),
        ),
        (
            "the endpoint with the root stage's own name appended".to_string(),
            at(format!("{base_path}/{DEFAULT_STAGE}")),
        ),
        (
            format!("the endpoint with the root stage's name and {NESTED_ROUTE_PATH:?}"),
            at(format!("{base_path}/{DEFAULT_STAGE}{NESTED_ROUTE_PATH}")),
        ),
    ]
}

/// One reporter's answer: the operation that published the endpoint, and the body it answered.
async fn reporters(
    emulator: &MultiServiceRouter,
    row: &Row,
    api_id: &str,
    name: &str,
) -> Vec<(&'static str, String)> {
    let mut reported: Vec<(&'static str, String)> = Vec::new();

    let (status, fetched) = call_at_host(
        emulator,
        row.host,
        row.region,
        "GET",
        &format!("/v2/apis/{api_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: GetApi must answer 200, got {status} {fetched}",
        row.case
    );
    reported.push(("GetApi", as_str(&fetched, "apiEndpoint")));

    let (status, listed) =
        call_at_host(emulator, row.host, row.region, "GET", "/v2/apis", None).await;
    assert_eq!(
        status, 200,
        "case {:?}: GetApis must answer 200, got {status} {listed}",
        row.case
    );
    let items = listed["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {:?}: GetApis served no items, got {listed}", row.case));
    assert_eq!(
        items.len(),
        1,
        "case {:?}: this emulator holds exactly the row's api, got {listed}",
        row.case
    );
    reported.push(("GetApis", as_str(&items[0], "apiEndpoint")));

    let (status, updated) = call_at_host(
        emulator,
        row.host,
        row.region,
        "PATCH",
        &format!("/v2/apis/{api_id}"),
        Some(json!({ "description": "published-address check" })),
    )
    .await;
    // The success status each mutating operation answers with is `mutating_success_status`'s
    // subject (a PATCH that rewrites an api is `201`, the imports are `200`); what is
    // asserted here is only the endpoint such an answer reports.
    assert_eq!(
        status, 201,
        "case {:?}: UpdateApi must answer 201, got {status} {updated}",
        row.case
    );
    reported.push(("UpdateApi", as_str(&updated, "apiEndpoint")));

    let (status, reimported) = call_at_host(
        emulator,
        row.host,
        row.region,
        "PUT",
        &format!("/v2/apis/{api_id}"),
        Some(json!({ "body": definition(name) })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: ReimportApi must answer 200, got {status} {reimported}",
        row.case
    );
    reported.push(("ReimportApi", as_str(&reimported, "apiEndpoint")));

    reported
}

/// Every operation that reports an api's `ApiEndpoint` reports the one address this emulator
/// serves that api on — the mutators included, so a patch or a reimport cannot leave a client
/// holding an address nothing answers.
#[tokio::test]
async fn every_reporter_of_an_api_publishes_the_address_it_is_served_on() {
    for (index, row) in rows().into_iter().enumerate() {
        // One emulator per row, so GetApis lists exactly this row's api.
        let emulator = emulator();
        let case = row.case;
        let name = format!("published-reporters-{index}");
        let (api_id, announced) = deploy(&emulator, &row, &name).await;
        assert_announced(&row, "CreateApi", &api_id, &announced);
        for (reporter, reported) in reporters(&emulator, &row, &api_id, &name).await {
            assert_eq!(
                reported, announced,
                "case {case:?}: {reporter} must report the address CreateApi announced \
                 ({announced:?}), got {reported:?}"
            );
            assert_announced(&row, reporter, &api_id, &reported);
        }
    }
}

/// Every invoke URL a client builds from the published endpoint reaches the api's route,
/// including the api's ROOT — the endpoint with nothing appended and the endpoint with a
/// trailing slash, which is the URL the recorded client built.
#[tokio::test]
async fn every_invoke_spelling_built_from_the_published_endpoint_reaches_the_route() {
    for (index, row) in rows().into_iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let name = format!("published-spellings-{index}");
        let (api_id, announced) = deploy(&emulator, &row, &name).await;
        let (authority, base_path) = assert_announced(&row, "CreateApi", &api_id, &announced);
        for (spelling, path) in spellings(&base_path) {
            assert_served(case, &emulator, &spelling, &authority, &path).await;
        }
        // And the same spellings still reach the route after a reimport PUT the definition
        // over the api: the endpoint it reports is the one it is served on.
        let (status, reimported) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "PUT",
            &format!("/v2/apis/{api_id}"),
            Some(json!({ "body": definition(&name) })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ReimportApi must answer 200, got {status} {reimported}"
        );
        let (authority, base_path) =
            assert_announced(&row, "ReimportApi", &api_id, &as_str(&reimported, "apiEndpoint"));
        for (spelling, path) in spellings(&base_path) {
            assert_served(
                case,
                &emulator,
                &format!("{spelling}, after the reimport"),
                &authority,
                &path,
            )
            .await;
        }
    }
}
