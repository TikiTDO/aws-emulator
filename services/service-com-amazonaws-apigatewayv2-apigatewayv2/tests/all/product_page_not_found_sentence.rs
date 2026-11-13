// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SENTENCE a `NotFoundException` carries when a
//! `ProductPageId` addressed under a portal product resolves to no page.
//!
//! The rule: every operation addressed at ONE product page
//! (`/v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}`) refuses a page
//! that resolves to nothing with HTTP 404, error code `NotFoundException` and the
//! portal-plane sentence that names the resource TYPE alone — fixed text for the whole
//! class, with no variable part: it carries neither the page id, nor the product id, nor
//! the account the lookup was made in. The reader (`GetProductPage`) and the writer
//! (`UpdateProductPage`) share one sentence, so the wording cannot drift between them.
//!
//! The rows span the ways a page id resolves to nothing — a page that was created and
//! then deleted, an id nothing ever minted (a short one, a single character, one at the
//! modelled 30-character maximum, a mixed-case one outside the minted alphabet), an id
//! this account minted under ANOTHER product, a live id addressed under a product nothing
//! minted, and (for the reader, the only operation that models the member) a live page
//! whose `ResourceOwnerAccountId` names another account. The id forms come from
//! `GetProductPage$ProductPageId`'s reported shape `__stringMin10Max30PatternAZ09`
//! (len 10..30, pattern `^[a-z0-9]+$`); a request may address the URI with any string,
//! which is why forms outside that shape are rows too.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The portal products collection (`/v2/portalproducts`).
const PRODUCTS: &str = "/v2/portalproducts";

/// AWS's sentence for a product page that resolves to nothing: the whole sentence is
/// fixed text naming the resource type, with nothing interpolated.
const PRODUCT_PAGE_NOT_FOUND: &str = "ProductPage not found";

/// The product pages collection of one product.
fn pages_of(product_id: &str) -> String {
    format!("{PRODUCTS}/{product_id}/productpages")
}

/// Create one portal product and answer its identifier.
async fn product(emulator: &MultiServiceRouter, display_name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        PRODUCTS,
        Some(json!({ "displayName": display_name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {display_name:?}: expected 201 from CreatePortalProduct, got {status} {created}"
    );
    as_str(&created, "portalProductId")
}

/// Create one product page under `product_id` and answer its identifier.
async fn page(emulator: &MultiServiceRouter, product_id: &str, title: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &pages_of(product_id),
        Some(json!({ "displayContent": { "title": title, "body": "A body." } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {title:?}: expected 201 from CreateProductPage, got {status} {created}"
    );
    as_str(&created, "productPageId")
}

/// One way of addressing a product page that resolves to nothing.
struct Case {
    label: String,
    /// The page's URI, `{PRODUCTS}/{product}/productpages/{page}`, with any query string
    /// the case addresses it with already appended.
    uri: String,
    /// Whether only the reader can be addressed this way: `ResourceOwnerAccountId` is
    /// modelled on `GetProductPage` alone.
    reader_only: bool,
}

/// The account another account's resource owner is named with: an id this emulator does
/// not hold resources for, so a page read on its behalf resolves to nothing.
const ANOTHER_ACCOUNT: &str = "111122223333";

/// The two operations addressed at ONE page, with the body each sends.
fn addressing_operations() -> Vec<(&'static str, &'static str, Option<Value>)> {
    vec![
        ("GetProductPage", "GET", None),
        (
            "UpdateProductPage",
            "PATCH",
            Some(json!({
                "displayContent": { "title": "Patched", "body": "A patched body." }
            })),
        ),
    ]
}

#[tokio::test]
async fn every_operation_addressing_an_unresolvable_product_page_answers_one_fixed_sentence() {
    let emulator = emulator();
    let account = engine::emulator_account_id();

    let product_id = product(&emulator, "page-not-found-sentence").await;
    let other_product_id = product(&emulator, "page-not-found-sentence-other").await;

    // The recorded case: a page that WAS minted under this product and then deleted.
    let deleted_page_id = page(&emulator, &product_id, "Deleted").await;
    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("{}/{deleted_page_id}", pages_of(&product_id)),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "expected 204 from DeleteProductPage, got {status} {body}"
    );

    // A page of the OTHER product: an id this account minted, which still names no page
    // of the product under test.
    let elsewhere_page_id = page(&emulator, &other_product_id, "Elsewhere").await;

    // A live page of the product under test, so a refused lookup can be shown to have
    // left the product's own pages readable.
    let live_page_id = page(&emulator, &product_id, "Live").await;

    let mut cases = vec![
        Case {
            label: "a page this product's delete removed".to_string(),
            uri: format!("{}/{deleted_page_id}", pages_of(&product_id)),
            reader_only: false,
        },
        Case {
            label: "a page minted under another product".to_string(),
            uri: format!("{}/{elsewhere_page_id}", pages_of(&product_id)),
            reader_only: false,
        },
        Case {
            label: "a live page under a product nothing minted".to_string(),
            uri: format!("{}/{live_page_id}", pages_of("zzzzzzzzzz")),
            reader_only: false,
        },
        Case {
            label: "a live page whose resource owner is another account".to_string(),
            uri: format!(
                "{}/{live_page_id}?resourceOwnerAccountId={ANOTHER_ACCOUNT}",
                pages_of(&product_id)
            ),
            reader_only: true,
        },
    ];

    // Ids nothing minted, spanning the forms a client can put in the URI: an
    // eleven-character lowercase-alphanumeric one, a single character, the longest the
    // model's page-id member reports (len 10..30), and a mixed-case one outside the
    // minted alphabet (`^[a-z0-9]+$`).
    for unminted in [
        "nosuchpage0".to_string(),
        "z".to_string(),
        "9".repeat(30),
        "MixedCase1".to_string(),
    ] {
        cases.push(Case {
            label: format!("a page id nothing minted ({unminted:?})"),
            uri: format!("{}/{unminted}", pages_of(&product_id)),
            reader_only: false,
        });
    }

    let operations = addressing_operations();
    for case in &cases {
        for (operation, method, body) in &operations {
            if case.reader_only && *operation != "GetProductPage" {
                continue;
            }
            let label = format!("{operation} on {}", case.label);
            let (status, answered) = call(&emulator, method, &case.uri, body.clone()).await;
            assert_eq!(
                status, 404,
                "case {label:?}: expected 404 from {method} {uri}, got {status} {answered}",
                uri = case.uri
            );
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {label:?}: expected NotFoundException, got {answered}"
            );
            let message = as_str(&answered, "message");
            assert_eq!(
                message, PRODUCT_PAGE_NOT_FOUND,
                "case {label:?}: expected the refusal to carry \
                 {PRODUCT_PAGE_NOT_FOUND:?}, got {message:?}"
            );
            // The sentence is fixed text for the whole class, so what came back over the
            // wire interpolates none of the request's or the store's values.
            for hidden in [
                account,
                ANOTHER_ACCOUNT,
                product_id.as_str(),
                other_product_id.as_str(),
                deleted_page_id.as_str(),
                elsewhere_page_id.as_str(),
                live_page_id.as_str(),
            ] {
                assert!(
                    !message.contains(hidden),
                    "case {label:?}: expected a sentence with no variable part, but \
                     {message:?} carries {hidden:?}"
                );
            }
        }
    }

    // Every row above was a refusal, not a blanket 404: the product's live page still
    // reads back, unchanged by the refused lookups.
    let (status, read) = call(
        &emulator,
        "GET",
        &format!("{}/{live_page_id}", pages_of(&product_id)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 reading the live page back, got {status} {read}"
    );
    assert_eq!(
        as_str(&read, "productPageId"),
        live_page_id,
        "expected GetProductPage to report the live page, got {read}"
    );
    assert_eq!(
        read.get("displayContent")
            .and_then(|content| content.get("title"))
            .and_then(Value::as_str),
        Some("Live"),
        "expected the refused lookups to have written nothing to the live page, got {read}"
    );
}
