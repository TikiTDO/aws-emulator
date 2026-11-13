// SPDX-License-Identifier: CC0-1.0

//! A product REST endpoint page's content is GENERATED from the REST endpoint it
//! documents, and the page reports what that generation ended in.
//!
//! The rule these cases assert:
//!
//! > A write to a `ProductRestEndpointPage` starts the generation of the page's content
//! > from the REST endpoint its `RestEndpointIdentifier` names, so the page reports
//! > `IN_PROGRESS` until that generation comes due and then the status it settled in: a
//! > page whose endpoint the account HOLDS — a REST API of the API Gateway service, under
//! > one of that API's stages — settles `AVAILABLE` with no `StatusException`, and one
//! > whose endpoint resolves to nothing settles `FAILED` carrying the `StatusException`
//! > that says why. The outcome is decided by the WRITE, so a REST API created after that
//! > write does not move an already-generated page until the next write re-generates it.
//!
//! Every REST API and stage a row names was created through the service that OWNS them
//! (`apigateway:CreateRestApi`, `apigateway:PutMethod`, `apigateway:CreateDeployment`),
//! mounted beside this one on the same shared engine exactly as the router mounts them, so
//! nothing here passes off an identifier of its own as one the account holds.
//!
//! The rows differ along what the rule generalises over: whether the account holds the
//! named REST API at all, whether it holds the named STAGE of an API it does hold, which
//! method and path of that API the page documents (neither takes part in the resolution),
//! and whether the endpoint came to exist before or after the page's last write.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, carries_nothing};
use crate::product_rest_endpoint_page_lifecycle::{invoke_url, pages_of, product, REGION};

/// The span the service generates a page's content over, read from the service itself
/// rather than copied, plus the slack a test needs to be sure it has passed.
const SPAN: Duration =
    service_com_amazonaws_apigatewayv2_apigatewayv2::state::PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN;

/// How long a case waits before reading a settled page.
const PAST_THE_SPAN: Duration = SPAN.saturating_add(Duration::from_millis(100));

/// The `StatusException$Exception` a page whose endpoint resolved to nothing reports: the
/// class of error API Gateway states that fact under, which is the same `BadRequestException`
/// a create naming such an endpoint is refused with.
const UNRESOLVED_EXCEPTION: &str = "BadRequestException";

/// The `StatusException$Message` of that failure: API Gateway's own sentence for a
/// `restEndpointIdentifier` that names no stage or no REST API.
const UNRESOLVED_MESSAGE: &str = "Invalid restEndpointIdentifier stage or restApiId";

/// This service and the API Gateway (v1) service that owns REST APIs, on ONE shared engine
/// — the arrangement the router assembles, and the one the resolution under test reads the
/// account's REST APIs through.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these tests; the requests still carry the SigV4
    // credential scope, which is what routes them and names the Region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// Create one REST API with `stage` deployed on it, through the service that owns REST
/// APIs, and answer its `RestApiId`.
///
/// A stage serves a DEPLOYMENT, and a deployment is the snapshot of the API's methods, so
/// the API gets one method on its root resource first — the shortest path a client takes to
/// a stage it can invoke.
async fn rest_api_with_stage(emulator: &MultiServiceRouter, name: &str, stage: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/restapis",
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateRestApi, got {status} {created}"
    );
    let rest_api_id = as_str(&created, "id");
    let root = as_str(&created, "rootResourceId");

    let (status, method) = call(
        emulator,
        "PUT",
        &format!("/restapis/{rest_api_id}/resources/{root}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from PutMethod, got {status} {method}"
    );

    deploy_stage(emulator, &rest_api_id, stage).await;
    rest_api_id
}

/// Deploy `stage` on `rest_api_id` through the service that owns REST APIs, and read the
/// stage back there: what this suite calls "a stage the account holds" is what that service
/// reports, not what this one assumed.
async fn deploy_stage(emulator: &MultiServiceRouter, rest_api_id: &str, stage: &str) {
    let (status, deployment) = call(
        emulator,
        "POST",
        &format!("/restapis/{rest_api_id}/deployments"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "stage {stage:?}: expected 201 from CreateDeployment, got {status} {deployment}"
    );
    let (status, read) = call(
        emulator,
        "GET",
        &format!("/restapis/{rest_api_id}/stages/{stage}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "stage {stage:?}: the owner of REST APIs must report the stage it was asked to \
         create, got {status} {read}"
    );
}

/// Take `stage` of `rest_api_id` away again, there too, and confirm it is gone.
async fn delete_stage(emulator: &MultiServiceRouter, rest_api_id: &str, stage: &str) {
    let uri = format!("/restapis/{rest_api_id}/stages/{stage}");
    let (status, body) = call(emulator, "DELETE", &uri, None).await;
    assert_eq!(
        status, 202,
        "stage {stage:?}: expected the owner of REST APIs to accept DeleteStage, got \
         {status} {body}"
    );
    let (status, body) = call(emulator, "GET", &uri, None).await;
    assert_eq!(
        status, 404,
        "stage {stage:?}: expected the deleted stage to resolve to nothing, got {status} \
         {body}"
    );
}

/// Create one page documenting `parts` under `collection` and answer `(page uri, created)`.
async fn page(
    emulator: &MultiServiceRouter,
    case: &str,
    collection: &str,
    parts: Value,
) -> (String, Value) {
    let (status, created) = call(
        emulator,
        "POST",
        collection,
        Some(json!({ "restEndpointIdentifier": { "identifierParts": parts } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductRestEndpointPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productRestEndpointPageId");
    (format!("{collection}/{page_id}"), created)
}

/// Read one page back, asserting the read succeeded.
async fn read(emulator: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    let (status, body) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetProductRestEndpointPage, got {status} {body}"
    );
    body
}

/// Assert `document` reports `expected_status`, with a `StatusException` exactly when that
/// status is `FAILED` and with the sentence that failure states.
fn assert_settled(case: &str, document: &Value, expected_status: &str) {
    assert_eq!(
        as_str(document, "status"),
        expected_status,
        "case {case:?}: expected the page to report {expected_status}, got {document}"
    );
    if expected_status == "FAILED" {
        let exception = document
            .get("statusException")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            exception,
            json!({ "exception": UNRESOLVED_EXCEPTION, "message": UNRESOLVED_MESSAGE }),
            "case {case:?}: expected the failure to say why, got {document}"
        );
    } else {
        assert!(
            carries_nothing(document, "statusException"),
            "case {case:?}: a page that did not fail has nothing to explain, got {document}"
        );
    }
}

/// One row: the REST endpoint a page documents, and the status its generation settles in.
struct Row {
    case: &'static str,
    /// The stage to deploy on the row's own REST API, or `None` for a row whose page names
    /// a REST API the account holds nothing under.
    deployed_stage: Option<&'static str>,
    /// The `stage` the page's identifier names.
    identified_stage: &'static str,
    /// The `method` and `path` the page's identifier names: the endpoint of the API it
    /// documents, which takes no part in whether the API and stage resolve.
    method: &'static str,
    path: &'static str,
    /// The `Status` the page must settle in.
    settles_in: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a REST API and a stage the account holds",
            deployed_stage: Some("prod"),
            identified_stage: "prod",
            method: "GET",
            path: "/orders",
            settles_in: "AVAILABLE",
        },
        Row {
            case: "another endpoint of that same stage: the method and path take no part",
            deployed_stage: Some("prod"),
            identified_stage: "prod",
            method: "POST",
            path: "/accounts/{accountId}/orders/{proxy+}",
            settles_in: "AVAILABLE",
        },
        Row {
            case: "a stage name the account's REST API has no stage under",
            deployed_stage: Some("prod"),
            identified_stage: "beta",
            method: "GET",
            path: "/orders",
            settles_in: "FAILED",
        },
        Row {
            case: "a REST API id the account holds nothing under",
            deployed_stage: None,
            identified_stage: "prod",
            method: "GET",
            path: "/orders",
            settles_in: "FAILED",
        },
        Row {
            case: "a REST API the account holds and a stage named like its default one",
            deployed_stage: Some("v1"),
            identified_stage: "$default",
            method: "DELETE",
            path: "/orders/{orderId}",
            settles_in: "FAILED",
        },
    ]
}

#[tokio::test]
async fn a_page_settles_in_the_status_the_generation_of_its_content_ended_in() {
    let emulator = emulator();
    let product_id = product(&emulator, "Generation").await;
    let collection = pages_of(&product_id);

    // A REST API id the account holds nothing under, in the form the owner of REST APIs
    // mints (ten lowercase alphanumerics), so what fails is the resolution and not the
    // shape of the identifier.
    const UNKNOWN_REST_API: &str = "nosuchapi0";

    for row in rows() {
        let case = row.case;
        let rest_api_id = match row.deployed_stage {
            Some(stage) => rest_api_with_stage(&emulator, case, stage).await,
            None => UNKNOWN_REST_API.to_string(),
        };
        let (uri, created) = page(
            &emulator,
            case,
            &collection,
            json!({
                "restApiId": rest_api_id,
                "stage": row.identified_stage,
                "method": row.method,
                "path": row.path
            }),
        )
        .await;

        // The content is still being generated when the create answers, whichever way the
        // generation is going to end, so a caller acting straight after it sees the
        // transient status and no explanation of a failure that has not happened yet.
        assert_settled(case, &created, "IN_PROGRESS");

        // The endpoint a page reports is the invoke URL of the STAGE its identifier names,
        // whether or not that stage resolves.
        assert_eq!(
            created
                .get("displayContent")
                .and_then(|content| content.get("endpoint"))
                .and_then(Value::as_str),
            Some(invoke_url(&rest_api_id, row.identified_stage, REGION).as_str()),
            "case {case:?}: expected the stage invoke URL, got {created}"
        );

        tokio::time::sleep(PAST_THE_SPAN).await;
        let settled = read(&emulator, case, &uri).await;
        assert_settled(case, &settled, row.settles_in);

        // A read reports: two reads of one settled page answer the same record.
        let again = read(&emulator, case, &uri).await;
        assert_eq!(
            again, settled,
            "case {case:?}: reading a settled page must not move it, got {again} after \
             {settled}"
        );
    }
}

#[tokio::test]
async fn the_generation_is_resolved_by_the_write_and_an_update_resolves_it_again() {
    let emulator = emulator();
    let product_id = product(&emulator, "Re-generation").await;
    let collection = pages_of(&product_id);

    // One REST API with one stage, both the account's own, and the stage name it does NOT
    // hold a stage under yet.
    let rest_api_id = rest_api_with_stage(&emulator, "the account's REST API", "prod").await;

    // A page whose stage the API has none of: nothing to generate its content from.
    let late = "a page created before the stage it documents was deployed";
    let (late_uri, _) = page(
        &emulator,
        late,
        &collection,
        json!({
            "restApiId": rest_api_id,
            "stage": "beta",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;
    // A page whose stage the API already has: its content is generated from a real
    // endpoint.
    let early = "a page created while the stage it documents was deployed";
    let (early_uri, _) = page(
        &emulator,
        early,
        &collection,
        json!({
            "restApiId": rest_api_id,
            "stage": "prod",
            "method": "GET",
            "path": "/orders"
        }),
    )
    .await;
    tokio::time::sleep(PAST_THE_SPAN).await;
    assert_settled(late, &read(&emulator, late, &late_uri).await, "FAILED");
    assert_settled(early, &read(&emulator, early, &early_uri).await, "AVAILABLE");

    // Now the account's REST API gains the one stage and loses the other. Each page was
    // resolved by the write that generated its content, so NEITHER moves: nothing
    // re-generated either one.
    deploy_stage(&emulator, &rest_api_id, "beta").await;
    delete_stage(&emulator, &rest_api_id, "prod").await;
    let late_before = read(&emulator, late, &late_uri).await;
    let early_before = read(&emulator, early, &early_uri).await;
    assert_settled(late, &late_before, "FAILED");
    assert_settled(early, &early_before, "AVAILABLE");

    // An update re-generates a page's content against the endpoint as it is NOW: each page
    // is back to IN_PROGRESS when its patch answers — a client waiting for the re-resolve
    // to clear observes it — and settles in what the generation that patch started ended
    // in, which is the other way round for each of them. The patch names no endpoint
    // (`RestEndpointIdentifier` is not an input of the update), so what changed is only
    // when the content was generated.
    for (case, uri, before, settles_in) in [
        (late, &late_uri, &late_before, "AVAILABLE"),
        (early, &early_uri, &early_before, "FAILED"),
    ] {
        let (status, patched) = call(
            &emulator,
            "PATCH",
            uri,
            Some(json!({ "tryItState": "ENABLED" })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductRestEndpointPage, got {status} \
             {patched}"
        );
        assert_settled(case, &patched, "IN_PROGRESS");
        tokio::time::sleep(PAST_THE_SPAN).await;
        let regenerated = read(&emulator, case, uri).await;
        assert_settled(case, &regenerated, settles_in);
        assert_eq!(
            regenerated.get("restEndpointIdentifier"),
            before.get("restEndpointIdentifier"),
            "case {case:?}: the page still documents the endpoint it was created for, got \
             {regenerated}"
        );
    }
}
