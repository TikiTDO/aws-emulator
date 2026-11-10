// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue asset slice: `PutAsset` and the `GetAsset` that reads
//! back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test is the one `PutAsset` documents — "Creates or updates an asset in
//! Glue Data Catalog. If the asset already exists, this operation updates it; otherwise, a
//! new asset is created" — read as: the request's `Identifier` is the key, so a put under a
//! free identifier stores an asset stamped with the instant it was served and a put under
//! an identifier already stored REPLACES that asset's asset type, name, description and
//! forms while keeping the creation instant, a read of that identifier reports what the
//! latest put stored, and the asset type an asset names and the form type each of its form
//! entries names must be ones this account holds.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the form
//! and length of the identifier, whether a description is sent, the number and keys of the
//! forms, whether a form entry carries a form type, content, both or neither, whether the
//! asset type and the form types are addressed by minted identifier or by registered name,
//! what an update changes, and whether the put carried an idempotency token.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::{
    AssetEntry, ComAmazonawsGlueAwsglueState, CATALOG_RESOURCE_FORM_NAME, MAX_ASSETS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_catalog_form, call, emulator, emulator_on, error_code, ok};

/// The longest `Identifier` the model accepts: "len 1..1087" on `AssetId`
/// (`smithy_query shape=AssetId`), whose pattern `^[a-zA-Z0-9\-\:\/\.\_\*]+$` is what the
/// rows below keep to.
const MAX_ASSET_ID_LENGTH: usize = 1087;

/// Register a form type and return the `FormTypeId` the catalog assigned it, so an asset's
/// form entries can name a form type this account really holds.
async fn form_type(router: &MultiServiceRouter, name: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": format!("structure {name} {{ a: String }}") }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutFormType must report an Id, got {stored}"))
        .to_string()
}

/// Register an asset type over the form types `forms` names and return the `AssetTypeId`
/// the catalog minted for it, so an asset can be an instance of an asset type this account
/// really holds.
async fn asset_type(
    router: &MultiServiceRouter,
    name: &str,
    forms: &[(&str, &str)],
) -> String {
    let mut map = Map::new();
    for (form_name, form_type_id) in forms {
        map.insert(
            (*form_name).to_string(),
            json!({ "FormTypeIdentifier": form_type_id }),
        );
    }
    let stored = ok(
        router,
        "PutAssetType",
        &json!({ "Name": name, "Forms": Value::Object(map) }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutAssetType must report an Id, got {stored}"))
        .to_string()
}

/// The `GetAsset` response for the asset `identifier` names.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetAsset", &json!({ "Identifier": identifier })).await
}

/// A timestamp member of a response, as its epoch-second value.
fn instant(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: the response must carry {member}, got {body}"))
}

/// An identifier of exactly `length` characters that satisfies `AssetId`'s pattern.
fn identifier_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// The forms a `GetAsset` reports MINUS the catalog's own: the forms the asset's own puts
/// stored, which is what a row's request names.
///
/// Every read of an asset reports the catalog's own form on it beside them
/// ([`CATALOG_RESOURCE_FORM_NAME`]), carrying the catalog's own document for the asset,
/// which this asserts on each read it is used on ([`assert_catalog_form`]) so a row
/// comparing the stored forms against the request it sent does not restate it.
fn stored_forms(case: &str, read: &Value) -> Value {
    let mut forms = read
        .get("Forms")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: GetAsset must report Forms, got {read}"));
    let catalog_form = forms.remove(CATALOG_RESOURCE_FORM_NAME);
    assert_catalog_form(case, read, catalog_form.as_ref());
    Value::Object(forms)
}

// ---------------------------------------------------------------------------
// Put, then read back
// ---------------------------------------------------------------------------

/// What `PutAsset` persists is what `GetAsset` reports: the put answers with the
/// identifier it stored the asset under plus the name, description, creation instant and
/// forms it stored, a read of that identifier reports the same members beside the asset
/// type the asset is an instance of, and each asset is its own record.
#[tokio::test]
async fn put_assets_are_read_back_from_the_store() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let audit_form = form_type(&router, "AuditForm").await;
    let order_asset_type = asset_type(
        &router,
        "OrderAssetType",
        &[("Details", &order_form), ("Audit", &audit_form)],
    )
    .await;

    let long_identifier = identifier_of_length(MAX_ASSET_ID_LENGTH);
    let cases: Vec<(&str, Value)> = vec![
        (
            // The shape a lifecycle client sends: the required members alone.
            "one form, no description",
            json!({
                "Identifier": "order-1",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 1",
                "Forms": {
                    "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"one\"}" },
                },
            }),
        ),
        (
            "a description beside two forms",
            json!({
                "Identifier": "order-2",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 2",
                "Description": "the second order",
                "Forms": {
                    "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"two\"}" },
                    "Audit": { "FormTypeId": &audit_form, "Content": "{\"a\":\"audited\"}" },
                },
            }),
        ),
        (
            "two form entries naming ONE form type",
            json!({
                "Identifier": "order-3",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 3",
                "Forms": {
                    "Primary": { "FormTypeId": &order_form, "Content": "{\"a\":\"p\"}" },
                    "Secondary": { "FormTypeId": &order_form, "Content": "{\"a\":\"s\"}" },
                },
            }),
        ),
        (
            "a form entry carrying content and no form type",
            json!({
                "Identifier": "order-4",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 4",
                "Forms": { "Freeform": { "Content": "{\"a\":\"free\"}" } },
            }),
        ),
        (
            "a form entry carrying a form type and no content",
            json!({
                "Identifier": "order-5",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 5",
                "Forms": { "Details": { "FormTypeId": &order_form } },
            }),
        ),
        (
            "an empty form map, which the model puts no lower bound on",
            json!({
                "Identifier": "order-6",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 6",
                "Forms": {},
            }),
        ),
        (
            "an asset type named by the name it was registered under",
            json!({
                "Identifier": "order-7",
                "AssetTypeId": "OrderAssetType",
                "Name": "Order 7",
                "Forms": { "Details": { "FormTypeId": "OrderForm", "Content": "{}" } },
            }),
        ),
        (
            "an identifier of one character",
            json!({
                "Identifier": "o",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 8",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
            }),
        ),
        (
            // Every character class `AssetId`'s pattern allows beside the alphanumerics.
            "an identifier carrying the punctuation the pattern allows",
            json!({
                "Identifier": "arn:aws:glue:us-east-1:000000000000/asset.9_x*",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 9",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
            }),
        ),
        (
            "an identifier at the modelled length bound",
            json!({
                "Identifier": &long_identifier,
                "AssetTypeId": &order_asset_type,
                "Name": "Order 10",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
            }),
        ),
        (
            "an idempotency token sent with the put",
            json!({
                "Identifier": "order-11",
                "AssetTypeId": &order_asset_type,
                "Name": "Order 11",
                "Description": "sent with a token",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
                "ClientToken": "a-client-token",
            }),
        ),
    ];

    for (case, request) in &cases {
        let identifier = request["Identifier"].as_str().expect("the row has one");
        let sent_forms = &request["Forms"];

        let stored = ok(&router, "PutAsset", request).await;
        assert_eq!(
            (
                stored.get("Id"),
                stored.get("Name"),
                stored.get("Description"),
                stored.get("Forms"),
            ),
            (
                request.get("Identifier"),
                request.get("Name"),
                request.get("Description"),
                Some(sent_forms),
            ),
            "case {case}: PutAsset must report the asset it stored, got {stored}"
        );
        let created_at = instant(case, &stored, "CreatedAt");

        // The identifier the put was addressed with addresses the record it wrote, and the
        // read reports every member of it — including the asset type the put named, which
        // the put's own response does not carry.
        let read = get(&router, identifier).await;
        assert_eq!(
            (
                read.get("Id"),
                read.get("Name"),
                read.get("Description"),
                read.get("AssetTypeId"),
                &stored_forms(case, &read),
            ),
            (
                request.get("Identifier"),
                request.get("Name"),
                request.get("Description"),
                request.get("AssetTypeId"),
                sent_forms,
            ),
            "case {case}: GetAsset must report the asset the put stored, got {read}"
        );
        assert_eq!(
            instant(case, &read, "CreatedAt"),
            created_at,
            "case {case}: the read must report the creation instant the put stamped, got {read}"
        );
        // Nothing has updated the record, so it was last updated when it was created.
        assert_eq!(
            instant(case, &read, "UpdatedAt"),
            created_at,
            "case {case}: an asset no put has updated must report its creation instant as \
             its update instant, got {read}"
        );

        // Reading it again reports the same record: the instants are stored, not stamped
        // per read.
        let again = get(&router, identifier).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one asset must report one record, got {again}"
        );
    }

    // Each record is still its own after every put has landed: no put overwrote another's.
    for (case, request) in &cases {
        let identifier = request["Identifier"].as_str().expect("the row has one");
        let read = get(&router, identifier).await;
        assert_eq!(
            (read.get("Name"), Some(&stored_forms(case, &read))),
            (request.get("Name"), request.get("Forms")),
            "case {case}: each asset keeps its own name and forms, got {read}"
        );
    }
}

/// A put under an identifier the store already holds UPDATES that asset: its asset type,
/// name, description and whole set of forms become the ones the later put sent, the
/// creation instant is the one the first put stamped, the update instant moves to the
/// later put's, and no other asset changes.
///
/// The rows differ along what the update changes — the name, more forms, fewer forms, a
/// re-keyed form, a re-pointed form type, the asset type, a description added, a
/// description dropped, and the identical request.
#[tokio::test]
async fn a_put_under_a_stored_identifier_updates_that_asset() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let audit_form = form_type(&router, "AuditForm").await;
    let order_asset_type =
        asset_type(&router, "OrderAssetType", &[("Details", &order_form)]).await;
    let audited_asset_type =
        asset_type(&router, "AuditedAssetType", &[("Audit", &audit_form)]).await;

    // A bystander asset, to prove an update is scoped to the identifier it names.
    let bystander = ok(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "bystander",
            "AssetTypeId": &order_asset_type,
            "Name": "Bystander",
            "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"b\"}" } },
        }),
    )
    .await;

    let first = |identifier: &str| {
        json!({
            "Identifier": identifier,
            "AssetTypeId": &order_asset_type,
            "Name": "Before",
            "Description": "before the update",
            "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
        })
    };

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the name changed",
            first("renamed"),
            json!({
                "Identifier": "renamed",
                "AssetTypeId": &order_asset_type,
                "Name": "After",
                "Description": "before the update",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
            }),
        ),
        (
            "more form entries",
            first("growing"),
            json!({
                "Identifier": "growing",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Description": "before the update",
                "Forms": {
                    "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" },
                    "Audit": { "FormTypeId": &audit_form, "Content": "{\"a\":\"2\"}" },
                },
            }),
        ),
        (
            "fewer form entries",
            json!({
                "Identifier": "shrinking",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Forms": {
                    "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" },
                    "Audit": { "FormTypeId": &audit_form, "Content": "{\"a\":\"2\"}" },
                },
            }),
            json!({
                "Identifier": "shrinking",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Forms": { "Audit": { "FormTypeId": &audit_form, "Content": "{\"a\":\"2\"}" } },
            }),
        ),
        (
            "the form entry re-keyed",
            first("rekeyed"),
            json!({
                "Identifier": "rekeyed",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Description": "before the update",
                "Forms": { "Summary": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
            }),
        ),
        (
            "one form entry re-pointed at another form type",
            first("repointed"),
            json!({
                "Identifier": "repointed",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Description": "before the update",
                "Forms": { "Details": { "FormTypeId": &audit_form, "Content": "{\"a\":\"1\"}" } },
            }),
        ),
        (
            "the asset type changed",
            first("retyped"),
            json!({
                "Identifier": "retyped",
                "AssetTypeId": &audited_asset_type,
                "Name": "Before",
                "Description": "before the update",
                "Forms": { "Audit": { "FormTypeId": &audit_form, "Content": "{\"a\":\"1\"}" } },
            }),
        ),
        (
            "a description added where the first put sent none",
            json!({
                "Identifier": "described",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
            }),
            json!({
                "Identifier": "described",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Description": "added by the update",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
            }),
        ),
        (
            // A put SETS the asset, so a member the later put omits is no longer held.
            "the description dropped",
            first("undescribed"),
            json!({
                "Identifier": "undescribed",
                "AssetTypeId": &order_asset_type,
                "Name": "Before",
                "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
            }),
        ),
        (
            "the identical request",
            first("unchanged"),
            first("unchanged"),
        ),
    ];

    for (case, before, after) in &cases {
        let identifier = before["Identifier"].as_str().expect("the row has one");
        let created = ok(&router, "PutAsset", before).await;
        let created_at = instant(case, &created, "CreatedAt");

        let updated = ok(&router, "PutAsset", after).await;
        assert_eq!(
            (
                updated.get("Id"),
                updated.get("Name"),
                updated.get("Description"),
                updated.get("Forms"),
            ),
            (
                after.get("Identifier"),
                after.get("Name"),
                after.get("Description"),
                after.get("Forms"),
            ),
            "case {case}: an update must report the asset it stored, got {updated}"
        );
        assert_eq!(
            instant(case, &updated, "CreatedAt"),
            created_at,
            "case {case}: an update must keep the creation instant the first put stamped, \
             got {updated}"
        );

        // The identifier still resolves, and resolves to the updated record: the update
        // replaced the asset rather than adding a second one.
        let read = get(&router, identifier).await;
        assert_eq!(
            (
                read.get("AssetTypeId"),
                read.get("Name"),
                read.get("Description"),
                Some(&stored_forms(case, &read)),
            ),
            (
                after.get("AssetTypeId"),
                after.get("Name"),
                after.get("Description"),
                after.get("Forms"),
            ),
            "case {case}: a read after the update must report what the update stored, got \
             {read}"
        );
        let updated_at = instant(case, &read, "UpdatedAt");
        assert!(
            updated_at >= created_at,
            "case {case}: the update instant must not precede the creation instant, got \
             {updated_at} against {created_at}"
        );
    }

    // Nothing an update did reached the asset it did not name.
    let read = get(&router, "bystander").await;
    assert_eq!(
        (read.get("Name"), Some(&stored_forms("bystander", &read))),
        (bystander.get("Name"), bystander.get("Forms")),
        "case bystander: an update must change only the asset it names, got {read}"
    );
}

/// A retry carrying the `ClientToken` of the put already stored is answered with that
/// record and writes nothing: the asset keeps the name, description and forms the first
/// put stored, and its update instant does not move. A put carrying ANOTHER token, or
/// none, is a fresh write and does update the record.
#[tokio::test]
async fn a_put_retried_under_its_own_token_is_idempotent() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let order_asset_type =
        asset_type(&router, "OrderAssetType", &[("Details", &order_form)]).await;

    let rows: Vec<(&str, &str, Option<&str>, bool)> = vec![
        ("the same token", "retried", Some("token-one"), true),
        ("another token", "retoken", Some("token-two"), false),
        ("no token at all", "untokened", None, false),
    ];

    for (case, identifier, retry_token, idempotent) in rows {
        let mut put = json!({
            "Identifier": identifier,
            "AssetTypeId": &order_asset_type,
            "Name": "First",
            "Description": "the first put",
            "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{\"a\":\"1\"}" } },
            "ClientToken": "token-one",
        });
        let created = ok(&router, "PutAsset", &put).await;
        let first_read = get(&router, identifier).await;
        let created_at = instant(case, &created, "CreatedAt");
        let first_update = instant(case, &first_read, "UpdatedAt");

        // The retry differs from the first put in every member the put SETS, so what it
        // answers with says whether the token made it a retry or a rewrite.
        put["Name"] = json!("Second");
        put["Description"] = json!("the second put");
        put["Forms"] = json!({ "Audit": { "Content": "{\"a\":\"2\"}" } });
        match retry_token {
            Some(token) => put["ClientToken"] = json!(token),
            None => {
                put.as_object_mut().expect("the request is an object").remove("ClientToken");
            }
        }

        let answered = ok(&router, "PutAsset", &put).await;
        let read = get(&router, identifier).await;
        let expected_name = if idempotent { "First" } else { "Second" };
        let expected_forms = if idempotent {
            &created["Forms"]
        } else {
            &put["Forms"]
        };
        assert_eq!(
            (
                answered.get("Name").and_then(Value::as_str),
                answered.get("Forms"),
            ),
            (Some(expected_name), Some(expected_forms)),
            "case {case}: expected the second put to answer with {expected_name:?}, got \
             {answered}"
        );
        assert_eq!(
            (
                read.get("Name").and_then(Value::as_str),
                Some(&stored_forms(case, &read))
            ),
            (Some(expected_name), Some(expected_forms)),
            "case {case}: expected the store to hold {expected_name:?}, got {read}"
        );
        assert_eq!(
            instant(case, &read, "CreatedAt"),
            created_at,
            "case {case}: the creation instant must not move, got {read}"
        );
        let second_update = instant(case, &read, "UpdatedAt");
        if idempotent {
            assert_eq!(
                second_update, first_update,
                "case {case}: a retry must not move the update instant, got {read}"
            );
        } else {
            assert!(
                second_update >= first_update,
                "case {case}: a rewrite's update instant must not precede the one it \
                 replaces, got {second_update} against {first_update}"
            );
        }
    }

    // A retry is served only while the request it retries is one this catalog would still
    // serve: the references are resolved before the token is read, so a retry naming a form
    // type the store no longer holds is refused like any other put naming an entity that
    // does not exist, and the stored asset is left as it was.
    let gone_form = form_type(&router, "GoneForm").await;
    let request = json!({
        "Identifier": "stale-retry",
        "AssetTypeId": &order_asset_type,
        "Name": "First",
        "Forms": { "Details": { "FormTypeId": &gone_form, "Content": "{\"a\":\"1\"}" } },
        "ClientToken": "token-three",
    });
    let created = ok(&router, "PutAsset", &request).await;
    ok(
        &router,
        "DeleteFormType",
        &json!({ "Identifier": &gone_form }),
    )
    .await;

    let (status, body) = call(&router, "PutAsset", &request).await;
    assert_eq!(
        status, 400,
        "case a retry whose form type is gone: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case a retry whose form type is gone: expected EntityNotFoundException, got {body}"
    );
    let read = get(&router, "stale-retry").await;
    assert_eq!(
        Some(&stored_forms("a retry whose form type is gone", &read)),
        created.get("Forms"),
        "case a retry whose form type is gone: the stored asset must be left as it was, got \
         {read}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A read for an identifier this account holds no asset under is refused with the modelled
/// `EntityNotFoundException`. The rows differ along how the identifier misses: never
/// stored, a prefix of a stored identifier, a stored identifier with one more character, a
/// stored identifier recased, and the identifier of a DIFFERENT resource kind (an asset
/// type and a form type), which this namespace does not hold.
#[tokio::test]
async fn reading_an_unknown_asset_is_refused() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let order_asset_type =
        asset_type(&router, "OrderAssetType", &[("Details", &order_form)]).await;
    ok(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "stored-asset",
            "AssetTypeId": &order_asset_type,
            "Name": "Stored",
            "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
        }),
    )
    .await;

    let rows: Vec<(&str, &str)> = vec![
        ("never stored", "no-such-asset"),
        ("a prefix of a stored identifier", "stored-asse"),
        ("a stored identifier with one more character", "stored-asset0"),
        ("a stored identifier recased", "Stored-Asset"),
        ("the identifier of an asset type", &order_asset_type),
        ("the identifier of a form type", &order_form),
    ];

    for (case, identifier) in rows {
        let (status, body) = call(&router, "GetAsset", &json!({ "Identifier": identifier })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {identifier:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
        );
    }

    // The stored identifier still resolves, so the misses above are misses and not a
    // broken lookup.
    let read = get(&router, "stored-asset").await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("Stored"),
        "case the stored identifier: expected the stored asset, got {read}"
    );
}

/// A put naming an asset type this account does not hold, or a form entry naming a form
/// type it does not hold, is refused with the modelled `EntityNotFoundException`, and
/// nothing is stored under the identifier the refused put named. The rows differ along
/// which reference misses, how it misses, and where in the form map a missing reference
/// sits.
#[tokio::test]
async fn a_put_naming_an_unregistered_asset_type_or_form_type_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let order_form = form_type(&router, "OrderForm").await;
    let order_asset_type =
        asset_type(&router, "OrderAssetType", &[("Details", &order_form)]).await;

    let truncated_type = &order_asset_type[..order_asset_type.len() - 1];
    let extended_type = format!("{order_asset_type}0");
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "an asset type identifier that was never minted",
            "missing-type",
            json!({
                "Identifier": "missing-type",
                "AssetTypeId": "no-such-asset-type",
                "Name": "Order",
                "Forms": {},
            }),
        ),
        (
            "a prefix of a minted asset type identifier",
            "truncated-type",
            json!({
                "Identifier": "truncated-type",
                "AssetTypeId": truncated_type,
                "Name": "Order",
                "Forms": {},
            }),
        ),
        (
            "a minted asset type identifier with one more character",
            "extended-type",
            json!({
                "Identifier": "extended-type",
                "AssetTypeId": &extended_type,
                "Name": "Order",
                "Forms": {},
            }),
        ),
        (
            "an asset type named by a recased registered name",
            "recased-type",
            json!({
                "Identifier": "recased-type",
                "AssetTypeId": "orderassettype",
                "Name": "Order",
                "Forms": {},
            }),
        ),
        (
            "the identifier of a form type rather than an asset type",
            "wrong-kind-type",
            json!({
                "Identifier": "wrong-kind-type",
                "AssetTypeId": &order_form,
                "Name": "Order",
                "Forms": {},
            }),
        ),
        (
            "a form entry naming a form type that was never registered",
            "missing-form-type",
            json!({
                "Identifier": "missing-form-type",
                "AssetTypeId": &order_asset_type,
                "Name": "Order",
                "Forms": { "Details": { "FormTypeId": "no-such-form-type", "Content": "{}" } },
            }),
        ),
        (
            "one form entry of several missing, beside an entry that resolves",
            "partly-resolving",
            json!({
                "Identifier": "partly-resolving",
                "AssetTypeId": &order_asset_type,
                "Name": "Order",
                "Forms": {
                    "Details": { "FormTypeId": &order_form, "Content": "{}" },
                    "Audit": { "FormTypeId": "another-missing-form-type", "Content": "{}" },
                },
            }),
        ),
        (
            "a form entry naming a form type by a recased registered name",
            "recased-form-type",
            json!({
                "Identifier": "recased-form-type",
                "AssetTypeId": &order_asset_type,
                "Name": "Order",
                "Forms": { "Details": { "FormTypeId": "orderform", "Content": "{}" } },
            }),
        ),
    ];

    for (case, identifier, request) in &rows {
        let (status, body) = call(&router, "PutAsset", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );

        // A refused put stores nothing: the identifier it named is still free, so a later
        // put under it would create rather than update.
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let held = state.lock().assets.contains_key(*identifier);
        assert!(
            !held,
            "case {case}: a refused put must store no asset under {identifier:?}"
        );
    }

    // A put whose every reference resolves is still served, so the refusals above are the
    // reference checks and not a broken write path.
    let served = ok(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "resolving",
            "AssetTypeId": &order_asset_type,
            "Name": "Order",
            "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
        }),
    )
    .await;
    let read = get(&router, "resolving").await;
    assert_eq!(
        Some(&stored_forms("every reference resolves", &read)),
        served.get("Forms"),
        "case every reference resolves: expected the stored forms, got {read}"
    );
}

/// The store holds a bounded number of assets: the put at the ceiling succeeds, the one
/// that would add a record past it is refused, an UPDATE of a stored asset still succeeds
/// at the ceiling (it adds no record), and freeing a slot lets a create succeed again — so
/// the ceiling bounds what is stored rather than latching on how many puts were ever made.
#[tokio::test]
async fn a_put_that_would_grow_the_store_past_the_asset_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let order_form = form_type(&router, "OrderForm").await;
    let order_asset_type =
        asset_type(&router, "OrderAssetType", &[("Details", &order_form)]).await;

    // Reaching the ceiling over the wire would take a thousand requests and would exercise
    // the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_ASSETS - 1 {
            let identifier = format!("seeded-{index}");
            store.assets.insert(
                identifier.clone(),
                AssetEntry::new(
                    identifier,
                    order_asset_type.clone(),
                    "Seeded".to_string(),
                    None,
                    std::collections::BTreeMap::new(),
                    None,
                    std::time::SystemTime::now(),
                ),
            );
        }
    }

    let last = json!({
        "Identifier": "last-asset",
        "AssetTypeId": &order_asset_type,
        "Name": "Last",
        "Forms": {},
    });
    let past = json!({
        "Identifier": "one-too-many",
        "AssetTypeId": &order_asset_type,
        "Name": "One too many",
        "Forms": {},
    });

    ok(&router, "PutAsset", &last).await;

    let (status, body) = call(&router, "PutAsset", &past).await;
    assert_eq!(
        status,
        400,
        "case asset {} of {MAX_ASSETS}: expected 400, got {status} with {body}",
        MAX_ASSETS + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one asset too many: expected InvalidInputException, got {body}"
    );

    // An update stores no new record, so it is served at the ceiling.
    let updated = ok(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "last-asset",
            "AssetTypeId": &order_asset_type,
            "Name": "Last, updated",
            "Forms": { "Details": { "FormTypeId": &order_form, "Content": "{}" } },
        }),
    )
    .await;
    assert_eq!(
        updated.get("Name").and_then(Value::as_str),
        Some("Last, updated"),
        "case an update at the ceiling: expected the update to be served, got {updated}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().assets.remove("seeded-0");
    }
    ok(&router, "PutAsset", &past).await;
}
