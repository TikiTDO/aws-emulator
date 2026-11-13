// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a DELETE of a child resource does to the STORE.
//!
//! The rule: a delete that answers its modelled success has REMOVED the record, so every
//! later call that resolves that resource — its own reader, its update, the collection
//! reader that reports its kind, a projection read through it — is refused `404`
//! `NotFoundException` with the sentence that KIND of resource is reported with, while
//! its siblings under the same parent are untouched. A client that confirms a deletion by
//! reading until 404 therefore converges, and the key the deleted resource held is free
//! again, so a recreate-after-delete flow never meets the record it removed.
//!
//! The rows span what the rule generalises over: the kind of resource, where it is
//! addressed (under an integration, under a route, under an api, under a portal product,
//! at the account), the operations that address it after the delete (read, update,
//! collection read, projection read, a repeat delete), and whether a repeat delete is
//! refused or answers the same empty success. The recorded calls that filed the issue —
//! one `GetIntegrationResponse` after `DeleteIntegrationResponse`, and the same for a
//! route response, a model, a portal product and a product page — are one row each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The portal-products collection URI (`CreatePortalProduct`'s modelled
/// `POST /v2/portalproducts`).
const PRODUCTS: &str = "/v2/portalproducts";

/// AWS's sentence for a `RouteResponseId` the addressed route holds no response under.
/// Written out here rather than taken from the service, so the table asserts the rule.
const ROUTE_RESPONSE_NOT_FOUND: &str = "Invalid route response specified";

/// AWS's sentence for a product page that resolves to nothing: the whole sentence is
/// fixed text, with no variable part.
const PRODUCT_PAGE_NOT_FOUND: &str = "ProductPage not found";

/// AWS's sentence for a `RouteId` the addressed api holds no route under — the kind whose
/// delete was already applied to the store, so the table spans a kind this rule did not
/// have to change.
const ROUTE_NOT_FOUND: &str = "Invalid route identifier specified";

/// AWS's sentence for an `IntegrationResponseId` the addressed integration holds no
/// response under: it names both ids the request addressed the response with.
fn integration_response_not_found(integration_response_id: &str, integration_id: &str) -> String {
    format!(
        "IntegrationResponse with ID {integration_response_id} is not present in \
         integration with ID {integration_id}"
    )
}

/// AWS's sentence for a `ModelId` the addressed api holds no model under: the
/// unknown-identifier sentence extended with the id the request supplied.
fn model_not_found(model_id: &str) -> String {
    format!("Invalid model identifier specified: {model_id}")
}

/// AWS's sentence for a `PortalProductId` the account holds no product under: a product is
/// an account-level resource, so the account the lookup was made in is named beside the id.
fn portal_product_not_found(portal_product_id: &str) -> String {
    format!(
        "Product not found in account {} with id {portal_product_id}",
        ::engine::emulator_account_id()
    )
}

/// One row of the table: a resource of one kind, its sibling under the same parent, and
/// every call that must stop resolving it once it is deleted.
struct Row {
    kind: &'static str,
    /// The resource the delete removes.
    uri: String,
    /// The identifier it carries, as its collection reader reports it.
    id: String,
    /// A sibling of the same kind under the same parent: the delete must leave it alone.
    sibling_uri: String,
    /// The sibling's identifier, so the collection reader is shown to still report it.
    sibling_id: String,
    /// The sentence every later call addressing the deleted resource is refused with.
    message: String,
    /// A patch this kind's update takes, where that update is implemented.
    patch: Option<Value>,
    /// Further reads that resolve the same record (a projection of it, a sub-resource of
    /// it), as (operation, URI).
    extra_reads: Vec<(&'static str, String)>,
    /// The collection reader that reports this kind, and the member its entries carry the
    /// identifier under, where that reader is implemented.
    collection: Option<(String, &'static str)>,
    /// The status a SECOND delete of the same resource answers: a refusal where the delete
    /// reports what it removed, the same empty success where it is idempotent.
    repeat_delete_status: u16,
}

/// Send one request that must be accepted with `expected`, and answer its body.
async fn accepted(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let (status, answered) = call(emulator, method, uri, body).await;
    assert_eq!(
        status, expected,
        "case {case:?}: expected {expected} from {method} {uri}, got {status} {answered}"
    );
    answered
}

/// Assert one call is refused `404 NotFoundException` with `message`.
async fn assert_not_found(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    message: &str,
) {
    let (status, answered) = call(emulator, method, uri, body).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404 from {method} {uri}, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "NotFoundException",
        "case {case:?}: expected NotFoundException from {method} {uri}, got {answered}"
    );
    assert_eq!(
        as_str(&answered, "message"),
        message,
        "case {case:?}: expected the refusal {message:?} from {method} {uri}, got {answered}"
    );
}

/// One WebSocket api — the protocol that carries route responses, integration responses
/// and models — answering its `ApiId`.
async fn websocket_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let api = accepted(
        emulator,
        name,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            // "This property is required for WebSocket APIs"
            // (`CreateApi$RouteSelectionExpression`).
            "routeSelectionExpression": "$request.body.action"
        })),
        201,
    )
    .await;
    as_str(&api, "apiId")
}

/// Every kind's row, each with its resource and the sibling the delete must spare.
async fn rows(emulator: &MultiServiceRouter) -> Vec<Row> {
    let api_id = websocket_api(emulator, "delete-applies").await;

    // "MOCK: for integrating the route or method request with API Gateway as a 'loopback'
    // endpoint without invoking any backend" (`CreateIntegration$IntegrationType`), so it
    // names no endpoint.
    let integration = accepted(
        emulator,
        "the integration",
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({ "integrationType": "MOCK" })),
        201,
    )
    .await;
    let integration_id = as_str(&integration, "integrationId");
    let integration_responses =
        format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses");

    let mut integration_response_ids = Vec::new();
    for key in ["$default", "/400/"] {
        let created = accepted(
            emulator,
            key,
            "POST",
            &integration_responses,
            Some(json!({ "integrationResponseKey": key })),
            201,
        )
        .await;
        integration_response_ids.push(as_str(&created, "integrationResponseId"));
    }

    let route = accepted(
        emulator,
        "the route",
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "$default",
            "target": format!("integrations/{integration_id}")
        })),
        201,
    )
    .await;
    let route_id = as_str(&route, "routeId");
    let route_responses = format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses");

    let mut route_response_ids = Vec::new();
    for key in ["$default", "echo"] {
        let created = accepted(
            emulator,
            key,
            "POST",
            &route_responses,
            Some(json!({ "routeResponseKey": key })),
            201,
        )
        .await;
        route_response_ids.push(as_str(&created, "routeResponseId"));
    }

    // Two more routes, so the kind whose delete already worked is one row of this table
    // too: the one that is deleted holds no route responses of its own.
    let mut plain_route_ids = Vec::new();
    for key in ["removed", "kept"] {
        let created = accepted(
            emulator,
            key,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": key })),
            201,
        )
        .await;
        plain_route_ids.push(as_str(&created, "routeId"));
    }

    let models = format!("/v2/apis/{api_id}/models");
    let mut model_ids = Vec::new();
    for name in ["Removed", "Kept"] {
        let created = accepted(
            emulator,
            name,
            "POST",
            &models,
            Some(json!({
                "name": name,
                "contentType": "application/json",
                "schema": "{\"type\":\"object\"}"
            })),
            201,
        )
        .await;
        model_ids.push(as_str(&created, "modelId"));
    }

    let mut product_ids = Vec::new();
    for display_name in ["Removed product", "Kept product"] {
        let created = accepted(
            emulator,
            display_name,
            "POST",
            PRODUCTS,
            Some(json!({ "displayName": display_name })),
            201,
        )
        .await;
        product_ids.push(as_str(&created, "portalProductId"));
    }

    // The pages are created under the product this table KEEPS, so the page row and the
    // product row do not address one another.
    let pages = format!("{PRODUCTS}/{}/productpages", product_ids[1]);
    let mut page_ids = Vec::new();
    for title in ["Removed page", "Kept page"] {
        let created = accepted(
            emulator,
            title,
            "POST",
            &pages,
            Some(json!({ "displayContent": { "title": title, "body": "A body." } })),
            201,
        )
        .await;
        page_ids.push(as_str(&created, "productPageId"));
    }

    vec![
        Row {
            kind: "IntegrationResponse",
            uri: format!("{integration_responses}/{}", integration_response_ids[0]),
            id: integration_response_ids[0].clone(),
            sibling_uri: format!("{integration_responses}/{}", integration_response_ids[1]),
            sibling_id: integration_response_ids[1].clone(),
            message: integration_response_not_found(
                &integration_response_ids[0],
                &integration_id,
            ),
            // The patch names a status-code selection key, the form an integration
            // response of a status-selected integration is keyed by.
            patch: Some(json!({ "integrationResponseKey": "/500/" })),
            extra_reads: Vec::new(),
            collection: None,
            repeat_delete_status: 404,
        },
        Row {
            kind: "RouteResponse",
            uri: format!("{route_responses}/{}", route_response_ids[0]),
            id: route_response_ids[0].clone(),
            sibling_uri: format!("{route_responses}/{}", route_response_ids[1]),
            sibling_id: route_response_ids[1].clone(),
            message: ROUTE_RESPONSE_NOT_FOUND.to_string(),
            patch: Some(json!({ "routeResponseKey": "moved" })),
            extra_reads: Vec::new(),
            collection: Some((route_responses.clone(), "routeResponseId")),
            repeat_delete_status: 404,
        },
        Row {
            kind: "Model",
            uri: format!("{models}/{}", model_ids[0]),
            id: model_ids[0].clone(),
            sibling_uri: format!("{models}/{}", model_ids[1]),
            sibling_id: model_ids[1].clone(),
            message: model_not_found(&model_ids[0]),
            // `UpdateModel` is still a 501 stub with an issue of its own.
            patch: None,
            // The template is generated FROM the model, so it stops resolving with it.
            extra_reads: vec![(
                "GetModelTemplate",
                format!("{models}/{}/template", model_ids[0]),
            )],
            collection: Some((models.clone(), "modelId")),
            repeat_delete_status: 404,
        },
        Row {
            kind: "PortalProduct",
            uri: format!("{PRODUCTS}/{}", product_ids[0]),
            id: product_ids[0].clone(),
            sibling_uri: format!("{PRODUCTS}/{}", product_ids[1]),
            sibling_id: product_ids[1].clone(),
            message: portal_product_not_found(&product_ids[0]),
            patch: Some(json!({ "displayName": "Renamed product" })),
            // The sharing policy is a sub-resource of the product, so it goes with it.
            extra_reads: vec![(
                "GetPortalProductSharingPolicy",
                format!("{PRODUCTS}/{}/sharingpolicy", product_ids[0]),
            )],
            collection: Some((PRODUCTS.to_string(), "portalProductId")),
            // A product that is already gone is in the state the call asks for, so the
            // repeat answers the first delete's empty 204 (the rule the
            // `absent_portal_product_delete_idempotence` suite owns).
            repeat_delete_status: 204,
        },
        Row {
            kind: "ProductPage",
            uri: format!("{pages}/{}", page_ids[0]),
            id: page_ids[0].clone(),
            sibling_uri: format!("{pages}/{}", page_ids[1]),
            sibling_id: page_ids[1].clone(),
            message: PRODUCT_PAGE_NOT_FOUND.to_string(),
            patch: Some(json!({
                "displayContent": { "title": "Patched", "body": "Patched body." }
            })),
            extra_reads: Vec::new(),
            // `ListProductPages` is still a 501 stub with an issue of its own.
            collection: None,
            // A page that is already gone is in the state the call asks for, so the
            // repeat answers the first delete's empty 204 (the rule the
            // `absent_page_delete_idempotence` suite owns).
            repeat_delete_status: 204,
        },
        Row {
            kind: "Route",
            uri: format!("/v2/apis/{api_id}/routes/{}", plain_route_ids[0]),
            id: plain_route_ids[0].clone(),
            sibling_uri: format!("/v2/apis/{api_id}/routes/{}", plain_route_ids[1]),
            sibling_id: plain_route_ids[1].clone(),
            message: ROUTE_NOT_FOUND.to_string(),
            patch: Some(json!({ "routeKey": "patched" })),
            extra_reads: Vec::new(),
            collection: Some((format!("/v2/apis/{api_id}/routes"), "routeId")),
            repeat_delete_status: 404,
        },
    ]
}

#[tokio::test]
async fn a_deleted_child_resource_stops_resolving_for_every_later_caller() {
    let emulator = emulator();

    for row in rows(&emulator).await {
        let case = row.kind;

        // The resource is addressable before the delete, so the refusals below are the
        // delete's work and not a fixture that never existed.
        accepted(&emulator, case, "GET", &row.uri, None, 200).await;

        let deleted = accepted(&emulator, case, "DELETE", &row.uri, None, 204).await;
        assert_eq!(
            deleted,
            Value::Null,
            "case {case:?}: expected an empty body from the delete, got {deleted}"
        );

        // Its own reader no longer resolves it, with that kind's sentence.
        assert_not_found(&emulator, case, "GET", &row.uri, None, &row.message).await;

        for (operation, uri) in &row.extra_reads {
            let case = format!("{case} read through {operation}");
            assert_not_found(&emulator, &case, "GET", uri, None, &row.message).await;
        }

        // An update of a resource that is gone is refused rather than resurrecting it.
        if let Some(patch) = &row.patch {
            let case = format!("{case} patched after the delete");
            assert_not_found(
                &emulator,
                &case,
                "PATCH",
                &row.uri,
                Some(patch.clone()),
                &row.message,
            )
            .await;
            // And the patch wrote nothing: the read still refuses.
            assert_not_found(&emulator, &case, "GET", &row.uri, None, &row.message).await;
        }

        // The collection reader reports the siblings and not the deleted record.
        if let Some((collection, member)) = &row.collection {
            let listed = accepted(&emulator, case, "GET", collection, None, 200).await;
            let reported: Vec<String> = listed["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| as_str(item, member))
                .collect();
            assert!(
                !reported.contains(&row.id),
                "case {case:?}: expected {} to be gone from {collection}, got {reported:?}",
                row.id
            );
            assert!(
                reported.contains(&row.sibling_id),
                "case {case:?}: expected the sibling {} to survive in {collection}, got \
                 {reported:?}",
                row.sibling_id
            );
        }

        // A repeat delete reports what it removed, which is nothing.
        let (status, answered) = call(&emulator, "DELETE", &row.uri, None).await;
        assert_eq!(
            status, row.repeat_delete_status,
            "case {case:?}: expected {} from the repeat delete, got {status} {answered}",
            row.repeat_delete_status
        );
        if row.repeat_delete_status == 404 {
            assert_eq!(
                as_str(&answered, "message"),
                row.message,
                "case {case:?}: expected the repeat delete to be refused {:?}, got {answered}",
                row.message
            );
        }

        // Nothing of the sibling under the same parent was touched.
        accepted(&emulator, case, "GET", &row.sibling_uri, None, 200).await;
    }
}

/// A selection key is unique within its parent, so the key a deleted response held is
/// free again: the recreate a client runs after confirming the delete is ACCEPTED, and
/// mints a record of its own rather than handing back the one that was removed.
#[tokio::test]
async fn the_key_a_deleted_response_held_is_free_for_a_recreate() {
    let emulator = emulator();
    let api_id = websocket_api(&emulator, "recreate-after-delete").await;

    let integration = accepted(
        &emulator,
        "the integration",
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({ "integrationType": "MOCK" })),
        201,
    )
    .await;
    let integration_id = as_str(&integration, "integrationId");
    let route = accepted(
        &emulator,
        "the route",
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({ "routeKey": "$default" })),
        201,
    )
    .await;
    let route_id = as_str(&route, "routeId");

    // (kind, the collection, the create request, the member the minted id is reported in)
    let kinds: Vec<(&str, String, Value, &str)> = vec![
        (
            "IntegrationResponse",
            format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
            json!({ "integrationResponseKey": "$default" }),
            "integrationResponseId",
        ),
        (
            "RouteResponse",
            format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
            json!({ "routeResponseKey": "$default" }),
            "routeResponseId",
        ),
    ];

    for (kind, collection, request, member) in kinds {
        let first = accepted(
            &emulator,
            kind,
            "POST",
            &collection,
            Some(request.clone()),
            201,
        )
        .await;
        let first_id = as_str(&first, member);

        accepted(
            &emulator,
            kind,
            "DELETE",
            &format!("{collection}/{first_id}"),
            None,
            204,
        )
        .await;

        // The same key again: accepted, because the record that held it is gone.
        let second = accepted(
            &emulator,
            kind,
            "POST",
            &collection,
            Some(request.clone()),
            201,
        )
        .await;
        let second_id = as_str(&second, member);
        assert_ne!(
            second_id, first_id,
            "case {kind:?}: expected the recreate to mint an id of its own, got {first_id} \
             twice"
        );

        // The new record reads back, and the removed one still does not.
        accepted(
            &emulator,
            kind,
            "GET",
            &format!("{collection}/{second_id}"),
            None,
            200,
        )
        .await;
        let (status, answered) = call(
            &emulator,
            "GET",
            &format!("{collection}/{first_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {kind:?}: expected the deleted {first_id} to stay gone, got {status} \
             {answered}"
        );
    }
}

/// A portal product OWNS its pages, so deleting the product takes them with it: a page of
/// a product that is gone resolves to nothing, reported as a missing page.
#[tokio::test]
async fn deleting_a_portal_product_takes_the_pages_it_owns_with_it() {
    let emulator = emulator();

    let product = accepted(
        &emulator,
        "the product",
        "POST",
        PRODUCTS,
        Some(json!({ "displayName": "Payments" })),
        201,
    )
    .await;
    let product_id = as_str(&product, "portalProductId");
    let pages = format!("{PRODUCTS}/{product_id}/productpages");

    // The rows span the number of pages one product holds.
    let mut page_ids = Vec::new();
    for title in ["Getting started", "Reference", "Pricing"] {
        let created = accepted(
            &emulator,
            title,
            "POST",
            &pages,
            Some(json!({ "displayContent": { "title": title, "body": "A body." } })),
            201,
        )
        .await;
        page_ids.push(as_str(&created, "productPageId"));
    }

    accepted(
        &emulator,
        "the product",
        "DELETE",
        &format!("{PRODUCTS}/{product_id}"),
        None,
        204,
    )
    .await;

    for page_id in &page_ids {
        assert_not_found(
            &emulator,
            &format!("page {page_id} of a deleted product"),
            "GET",
            &format!("{pages}/{page_id}"),
            None,
            PRODUCT_PAGE_NOT_FOUND,
        )
        .await;
    }

    // And a create under the product that is gone names the PRODUCT that resolved to
    // nothing, not a page.
    assert_not_found(
        &emulator,
        "a page created under a deleted product",
        "POST",
        &pages,
        Some(json!({ "displayContent": { "title": "New", "body": "A body." } })),
        &portal_product_not_found(&product_id),
    )
    .await;
}
