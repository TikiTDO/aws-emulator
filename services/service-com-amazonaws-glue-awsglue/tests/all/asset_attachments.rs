// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the attachments a Glue asset carries and for the catalog's own form
//! on it: `PutAttachment` and `DeleteAttachment` write them, and the `GetAsset` that reads
//! the asset back reports them.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rules under test:
//!
//! * an attachment written onto an asset is HELD on that asset and reported by a read of it
//!   as the entry its attachment name keys, carrying the form type the write named and the
//!   document the write supplied, and as one entry of the metadata-form list beside the map
//!   — so the write round-trips and an asset holding one attachment is distinguishable from
//!   one holding none ("Additional attachments on the asset for more context, keyed by
//!   attachment name", `smithy_query shape=GetAssetOutput`);
//! * the catalog's own form on an asset carries, as its content, the catalog's own resource
//!   document for that asset: its name, and its description when it has one.
//!
//! The suites are tables whose rows differ along what the rules generalise over: the form
//! and length of the attachment name, the document the write supplies and where its
//! insignificant whitespace sits, how many attachments the asset holds, which form type an
//! attachment names, and what the asset's own name and description hold.

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::{
    CATALOG_RESOURCE_FORM_NAME, CATALOG_RESOURCE_FORM_TYPE_ID, MAX_ASSET_ATTACHMENTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_catalog_form, call, emulator, error_code, error_message, ok};

/// The `Id` member of a response, which must be a string.
fn id_of(case: &str, body: &Value) -> String {
    body.get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Id, got {body}"))
        .to_string()
}

/// Register a form type and answer the identifier the catalog assigned it.
async fn form_type(router: &MultiServiceRouter, name: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": format!("structure {name} {{ a: String }}") }),
    )
    .await;
    id_of(name, &stored)
}

/// Register an asset type over one form type and answer the identifier the put reported.
async fn asset_type(router: &MultiServiceRouter, name: &str, form_type_id: &str) -> String {
    let stored = ok(
        router,
        "PutAssetType",
        &json!({
            "Name": name,
            "Forms": { "Details": { "FormTypeIdentifier": form_type_id } },
        }),
    )
    .await;
    id_of(name, &stored)
}

/// Store an asset carrying one form and answer the identifier it was stored under.
async fn put_asset(
    router: &MultiServiceRouter,
    identifier: &str,
    asset_type_id: &str,
    form_type_id: &str,
) -> String {
    ok(
        router,
        "PutAsset",
        &json!({
            "Identifier": identifier,
            "AssetTypeId": asset_type_id,
            "Name": "Orders Table",
            "Description": "Order transactions",
            "Forms": { "Details": { "FormTypeId": form_type_id, "Content": "{\"a\":1}" } },
        }),
    )
    .await;
    identifier.to_string()
}

/// The `GetAsset` response for the asset `identifier` names.
async fn get_asset(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetAsset", &json!({ "Identifier": identifier })).await
}

/// The `Attachments` map of a read, which must be present even when it is empty.
fn attachments(case: &str, read: &Value) -> Map<String, Value> {
    read.get("Attachments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the read must report Attachments, got {read}"))
}

/// The `MetadataForms` list of a read, which must be present even when it is empty.
fn metadata_forms(case: &str, read: &Value) -> Vec<Value> {
    read.get("MetadataForms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the read must report MetadataForms, got {read}"))
}

/// The names a read reports attachments under, in name order.
///
/// The map is a JSON object, whose member order is not part of the answer — and whether
/// `serde_json` keeps the order it was parsed in or sorts it depends on a feature any crate
/// in the build may enable — so the names are sorted here and the rows below name them
/// sorted. The ORDER a read reports them in is asserted on the list beside the map
/// (`MetadataForms`), which is a list and does carry one.
fn attachment_names(case: &str, read: &Value) -> Vec<String> {
    let mut names: Vec<String> = attachments(case, read).keys().cloned().collect();
    names.sort();
    names
}

/// Write one attachment onto an asset and answer the response.
async fn put_attachment(
    router: &MultiServiceRouter,
    asset: &str,
    name: &str,
    form_type_id: &str,
    content: &str,
) -> Value {
    ok(
        router,
        "PutAttachment",
        &json!({
            "AssetIdentifier": asset,
            "AttachmentName": name,
            "FormTypeId": form_type_id,
            "Content": content,
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// An attachment is read back off the asset it was written onto
// ---------------------------------------------------------------------------

/// An attachment written onto an asset is reported by a read of that asset: as the
/// `Attachments` entry its name keys, carrying the form type the write named and the
/// document the write supplied, and as one entry of the `MetadataForms` list beside it. The
/// asset's own forms are untouched by the write, and two reads of one asset agree.
///
/// The rows differ along the form and length of the attachment name, along the document the
/// write supplies and where its insignificant whitespace sits, and along which of two form
/// types the attachment names.
#[tokio::test]
async fn an_attachment_is_read_back_off_the_asset_it_was_written_onto() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let sensitivity = form_type(&router, "SensitivityForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;

    // The names keep to `AttachmentName`'s pattern `^[a-zA-Z][a-zA-Z0-9_]*$` and its
    // "len 1..256" (`smithy_query shape=AttachmentName`).
    let longest = format!("a{}", "x".repeat(255));
    // (case, attachment name, the form type it names, the content sent, the content reported)
    let rows: Vec<(&str, String, &str, &str, &str)> = vec![
        (
            "a plain name and a compact document",
            "sensitivity".to_string(),
            &sensitivity,
            r#"{"sensitivity":"HIGH","classification":"PII"}"#,
            r#"{"sensitivity":"HIGH","classification":"PII"}"#,
        ),
        (
            "a name carrying digits and an underscore, and a spaced document",
            "attach_sensitivity_2024".to_string(),
            &sensitivity,
            r#"{ "sensitivity" : "LOW" , "tags" : [ "a" , "b" ] }"#,
            r#"{"sensitivity":"LOW","tags":["a","b"]}"#,
        ),
        (
            "the shortest name the model admits, and a nested document",
            "a".to_string(),
            &details,
            "{\n  \"owner\": \"analytics\",\n  \"nested\": { \"deep\": true }\n}",
            r#"{"owner":"analytics","nested":{"deep":true}}"#,
        ),
        (
            "the longest name the model admits, and an empty document",
            longest.clone(),
            &details,
            "{ }",
            "{}",
        ),
        (
            "a document whose strings carry quotes and backslashes",
            "quoted".to_string(),
            &details,
            r#"{ "note": "a \"quoted\" value \\ here" }"#,
            r#"{"note":"a \"quoted\" value \\ here"}"#,
        ),
        (
            "text that is no JSON document at all",
            "plain".to_string(),
            &details,
            "sensitivity: HIGH",
            "sensitivity: HIGH",
        ),
    ];

    for (index, (case, name, form_type_id, sent, expected)) in rows.iter().enumerate() {
        let asset = put_asset(
            &router,
            &format!("attached-{index}"),
            &asset_type_id,
            &details,
        )
        .await;

        let written = put_attachment(&router, &asset, name, form_type_id, sent).await;
        assert_eq!(
            (
                written.get("AssetIdentifier").and_then(Value::as_str),
                written.get("AssetId").and_then(Value::as_str),
                written.get("AttachmentName").and_then(Value::as_str),
                written.get("FormTypeId").and_then(Value::as_str),
            ),
            (
                Some(asset.as_str()),
                Some(asset.as_str()),
                Some(name.as_str()),
                Some(*form_type_id),
            ),
            "case {case}: the write must report the attachment it stored, got {written}"
        );

        let read = get_asset(&router, &asset).await;
        let entry = json!({ "FormTypeId": form_type_id, "Content": expected });
        assert_eq!(
            attachments(case, &read).get(name.as_str()),
            Some(&entry),
            "case {case}: the read must report the attachment under its name, got {read}"
        );
        assert_eq!(
            metadata_forms(case, &read),
            vec![json!({
                "FormName": name,
                "FormTypeId": form_type_id,
                "Content": expected,
            })],
            "case {case}: the read must carry one metadata form per attachment, got {read}"
        );

        // The write touches the asset's own forms not at all: the form its put stored and
        // the catalog's own form are both still reported.
        let forms = read
            .get("Forms")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case}: the read must report Forms, got {read}"));
        assert_eq!(
            forms.get("Details").and_then(|form| form.get("Content")),
            Some(&json!("{\"a\":1}")),
            "case {case}: the asset's own form must survive the write, got {read}"
        );
        assert_catalog_form(case, &read, forms.get(CATALOG_RESOURCE_FORM_NAME));

        // Nothing is stamped per read: two reads of one asset report one record.
        let again = get_asset(&router, &asset).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one asset must agree, got {again}"
        );
    }
}

// ---------------------------------------------------------------------------
// How many attachments an asset holds, and what one name holds
// ---------------------------------------------------------------------------

/// An asset accumulates the attachments written onto it, keyed by name and in name order,
/// and ONE name holds ONE attachment: "if an attachment with the same name already exists,
/// it is overwritten", so a second write under a held name replaces that document and leaves
/// the count where it is, while a write under a free name adds an entry. A retry carrying
/// the idempotency token of the write already stored is answered with that record rather
/// than rewriting it.
///
/// The rows differ along how many attachments the asset holds as the suite walks the writes.
#[tokio::test]
async fn attachments_accumulate_by_name_and_one_name_holds_one_attachment() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let sensitivity = form_type(&router, "SensitivityForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;
    let asset = put_asset(&router, "accumulating", &asset_type_id, &details).await;

    // (case, attachment name, the form type, the content, the names the asset holds after)
    let writes: Vec<(&str, &str, &str, &str, Vec<&str>)> = vec![
        (
            "the first attachment",
            "sensitivity",
            &sensitivity,
            r#"{"v":1}"#,
            vec!["sensitivity"],
        ),
        (
            "a second attachment under another name",
            "audit",
            &details,
            r#"{"v":2}"#,
            vec!["audit", "sensitivity"],
        ),
        (
            "a third attachment under another name",
            "zeta",
            &details,
            r#"{"v":3}"#,
            vec!["audit", "sensitivity", "zeta"],
        ),
        (
            "a write under a name the asset already holds",
            "sensitivity",
            &details,
            r#"{"v":4}"#,
            vec!["audit", "sensitivity", "zeta"],
        ),
    ];

    for (case, name, form_type_id, content, held) in &writes {
        put_attachment(&router, &asset, name, form_type_id, content).await;

        let read = get_asset(&router, &asset).await;
        let reported = attachments(case, &read);
        assert_eq!(
            attachment_names(case, &read),
            *held,
            "case {case}: the asset must hold the attachments written onto it, got {read}"
        );
        assert_eq!(
            reported.get(*name),
            Some(&json!({ "FormTypeId": form_type_id, "Content": content })),
            "case {case}: the latest write under one name is what the read reports, got \
             {read}"
        );

        // The list beside the map carries the same entries, in the same order.
        let listed_forms = metadata_forms(case, &read);
        let listed: Vec<&str> = listed_forms
            .iter()
            .map(|entry| {
                entry
                    .get("FormName")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: every entry names its form, got {entry}"))
            })
            .collect();
        assert_eq!(
            listed, *held,
            "case {case}: the list must mirror the map, got {read}"
        );
    }

    // "A unique, case-sensitive identifier that you provide to ensure the idempotency of the
    // request": the retry of a tokened write is answered with the record that write stored,
    // so a retry carrying another document does NOT replace it — while the same write under
    // another token does.
    let tokened = json!({
        "AssetIdentifier": &asset,
        "AttachmentName": "tokened",
        "FormTypeId": &details,
        "Content": r#"{"token":"first"}"#,
        "ClientToken": "a-client-token",
    });
    ok(&router, "PutAttachment", &tokened).await;
    let mut retried = tokened.clone();
    retried["Content"] = json!(r#"{"token":"retried"}"#);
    ok(&router, "PutAttachment", &retried).await;
    let read = get_asset(&router, &asset).await;
    assert_eq!(
        attachments("a retried tokened write", &read)
            .get("tokened")
            .and_then(|entry| entry.get("Content")),
        Some(&json!(r#"{"token":"first"}"#)),
        "case a retried tokened write: the retry must not rewrite the attachment, got {read}"
    );

    let mut other_token = retried.clone();
    other_token["ClientToken"] = json!("another-client-token");
    ok(&router, "PutAttachment", &other_token).await;
    let read = get_asset(&router, &asset).await;
    assert_eq!(
        attachments("a write under another token", &read)
            .get("tokened")
            .and_then(|entry| entry.get("Content")),
        Some(&json!(r#"{"token":"retried"}"#)),
        "case a write under another token: the write must overwrite the attachment, got \
         {read}"
    );
}

// ---------------------------------------------------------------------------
// The ceiling on the attachments one asset may hold
// ---------------------------------------------------------------------------

/// A read reports every attachment TWICE — as the keyed entry and as the entry of the list
/// beside it — so the attachments one asset may hold are bounded: the write that fills the
/// asset to the ceiling is served and reported, the one past it is refused with the client
/// error the operation models and stores nothing, and a write that REWRITES a held name is
/// served however full the asset is.
#[tokio::test]
async fn the_attachments_one_asset_may_hold_are_bounded() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;
    let asset = put_asset(&router, "bounded", &asset_type_id, &details).await;

    for index in 0..MAX_ASSET_ATTACHMENTS {
        put_attachment(
            &router,
            &asset,
            &format!("a{index}"),
            &details,
            r#"{"v":0}"#,
        )
        .await;
    }
    let read = get_asset(&router, &asset).await;
    assert_eq!(
        attachments("attachments at the ceiling", &read).len(),
        MAX_ASSET_ATTACHMENTS,
        "case attachments at the ceiling: the read must report every attachment, got {read}"
    );

    let (status, body) = call(
        &router,
        "PutAttachment",
        &json!({
            "AssetIdentifier": &asset,
            "AttachmentName": "oneTooMany",
            "FormTypeId": &details,
            "Content": "{}",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one attachment too many: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one attachment too many: expected InvalidInputException, got {body}"
    );
    let read = get_asset(&router, &asset).await;
    assert!(
        !attachments("one attachment too many", &read).contains_key("oneTooMany"),
        "case one attachment too many: a refused write must store nothing, got {read}"
    );

    // A rewrite of a name the asset already holds replaces that record, so it is served at
    // the ceiling: nothing is added.
    put_attachment(&router, &asset, "a0", &details, r#"{"v":1}"#).await;
    let read = get_asset(&router, &asset).await;
    let reported = attachments("a rewrite at the ceiling", &read);
    assert_eq!(
        (
            reported.len(),
            reported.get("a0").and_then(|entry| entry.get("Content")),
        ),
        (MAX_ASSET_ATTACHMENTS, Some(&json!(r#"{"v":1}"#))),
        "case a rewrite at the ceiling: the rewrite must replace the one record, got {read}"
    );
}

// ---------------------------------------------------------------------------
// Deleting an attachment by its key
// ---------------------------------------------------------------------------

/// `DeleteAttachment` removes the one attachment its name keys and leaves every other
/// attachment the asset holds where it is, so a read after the delete reports neither the
/// entry nor its metadata form; deleting the last one leaves both collections EMPTY rather
/// than absent. A name the asset holds no attachment under, and an asset this account holds
/// none of, are refused with the modelled miss and delete nothing.
///
/// The rows differ along how many attachments the asset holds when the delete lands.
#[tokio::test]
async fn a_delete_removes_the_one_attachment_its_name_keys() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;
    let asset = put_asset(&router, "deleting", &asset_type_id, &details).await;

    for name in ["audit", "sensitivity", "zeta"] {
        put_attachment(&router, &asset, name, &details, r#"{"v":1}"#).await;
    }

    // (case, the name deleted, the names the asset holds afterwards)
    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "the middle of three",
            "sensitivity",
            vec!["audit", "zeta"],
        ),
        ("the last of two", "zeta", vec!["audit"]),
        ("the only one left", "audit", Vec::new()),
    ];

    for (case, name, held) in &rows {
        let deleted = ok(
            &router,
            "DeleteAttachment",
            &json!({ "AssetIdentifier": &asset, "AttachmentName": name }),
        )
        .await;
        assert_eq!(
            (
                deleted.get("AssetIdentifier").and_then(Value::as_str),
                deleted.get("Identifier").and_then(Value::as_str),
            ),
            (Some(asset.as_str()), Some(asset.as_str())),
            "case {case}: the delete must report the asset it was addressed to, got {deleted}"
        );

        let read = get_asset(&router, &asset).await;
        assert_eq!(
            attachment_names(case, &read),
            *held,
            "case {case}: the delete must remove the one attachment it named, got {read}"
        );
        assert_eq!(
            metadata_forms(case, &read).len(),
            held.len(),
            "case {case}: the list must mirror the map after the delete, got {read}"
        );

        // The same delete sent twice: the name resolves to nothing the second time.
        let (status, body) = call(
            &router,
            "DeleteAttachment",
            &json!({ "AssetIdentifier": &asset, "AttachmentName": name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a repeated delete must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(*name),
            "case {case}: the refusal must name the attachment, got {body}"
        );
    }

    // An asset this account holds no record of is refused the way every read of the asset
    // surface refuses one.
    let (status, body) = call(
        &router,
        "DeleteAttachment",
        &json!({ "AssetIdentifier": "never-stored", "AttachmentName": "audit" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a delete on an asset that does not exist: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case a delete on an asset that does not exist: expected EntityNotFoundException, \
         got {body}"
    );
}

// ---------------------------------------------------------------------------
// What an attachment write refuses
// ---------------------------------------------------------------------------

/// A write is refused when a reference it names resolves to nothing — the asset it attaches
/// to, or the form type the attachment is created against — and when it names one half of
/// the iterable-form target without the other; no asset this store holds is changed by a
/// refused write.
///
/// The rows differ along which reference is unresolvable and along which half of the target
/// pair the request carries.
#[tokio::test]
async fn an_attachment_write_refuses_references_that_resolve_to_nothing() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;
    let asset = put_asset(&router, "refusing", &asset_type_id, &details).await;

    // (case, the request, the error the write must be refused with)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an asset this account holds none of",
            json!({
                "AssetIdentifier": "never-stored",
                "AttachmentName": "audit",
                "FormTypeId": &details,
                "Content": "{}",
            }),
            "EntityNotFoundException",
        ),
        (
            "a form type that was never registered",
            json!({
                "AssetIdentifier": &asset,
                "AttachmentName": "audit",
                "FormTypeId": "NeverRegistered",
                "Content": "{}",
            }),
            "EntityNotFoundException",
        ),
        (
            "an iterable form named without an item",
            json!({
                "AssetIdentifier": &asset,
                "AttachmentName": "audit",
                "FormTypeId": &details,
                "Content": "{}",
                "IterableFormName": "columns",
            }),
            "InvalidInputException",
        ),
        (
            "an item named without an iterable form",
            json!({
                "AssetIdentifier": &asset,
                "AttachmentName": "audit",
                "FormTypeId": &details,
                "Content": "{}",
                "ItemIdentifier": "column-1",
            }),
            "InvalidInputException",
        ),
        (
            "an item inside an iterable form the asset does not carry",
            json!({
                "AssetIdentifier": &asset,
                "AttachmentName": "audit",
                "FormTypeId": &details,
                "Content": "{}",
                "IterableFormName": "columns",
                "ItemIdentifier": "column-1",
            }),
            "EntityNotFoundException",
        ),
    ];

    for (case, request, expected) in &rows {
        let (status, body) = call(&router, "PutAttachment", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(*expected),
            "case {case}: expected {expected}, got {body}"
        );

        // A refused write stores nothing: the asset still carries no attachment.
        let read = get_asset(&router, &asset).await;
        assert_eq!(
            attachments(case, &read),
            Map::new(),
            "case {case}: a refused write must store nothing, got {read}"
        );
        assert_eq!(
            metadata_forms(case, &read),
            Vec::<Value>::new(),
            "case {case}: a refused write must leave the list empty, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The document the catalog's own form on an asset carries
// ---------------------------------------------------------------------------

/// The catalog's own form on an asset carries, as its content, the catalog's own resource
/// document for that asset: a JSON object holding the asset's name, and its description when
/// the asset has one, name first. It is computed from the record, so a put that renames the
/// asset or drops its description moves the document with it, two reads report one document,
/// and the list a read answers with beside the map reports the same one.
///
/// The rows differ along what the asset's name and description hold: a description or none,
/// an empty description, text carrying quotes, backslashes, a newline and non-ASCII
/// characters.
#[tokio::test]
async fn the_catalogs_own_form_carries_the_assets_resource_document() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;

    // (case, the asset's Name, its Description, the document the catalog's form must carry)
    let rows: Vec<(&str, &str, Option<&str>, &str)> = vec![
        (
            "a name and a description",
            "Customer Table",
            Some("Raw customer records"),
            r#"{"name":"Customer Table","description":"Raw customer records"}"#,
        ),
        (
            "a name and no description",
            "behavioral-fidelity-asset",
            None,
            r#"{"name":"behavioral-fidelity-asset"}"#,
        ),
        (
            "an empty description, which the model puts no bound on",
            "Orders Table",
            Some(""),
            r#"{"name":"Orders Table","description":""}"#,
        ),
        (
            "a name carrying a quote and a backslash",
            r#"a "quoted" \ name"#,
            Some("the asset's description"),
            r#"{"name":"a \"quoted\" \\ name","description":"the asset's description"}"#,
        ),
        (
            "a description carrying a newline and a tab",
            "Multiline",
            Some("first\nsecond\tthird"),
            r#"{"name":"Multiline","description":"first\nsecond\tthird"}"#,
        ),
        (
            "non-ASCII text",
            "Tableau des ventes",
            Some("données brutes — 日本語"),
            r#"{"name":"Tableau des ventes","description":"données brutes — 日本語"}"#,
        ),
    ];

    for (index, (case, name, description, expected)) in rows.iter().enumerate() {
        let identifier = format!("resource-document-{index}");
        let mut request = json!({
            "Identifier": &identifier,
            "AssetTypeId": &asset_type_id,
            "Name": name,
            "Forms": {},
        });
        if let Some(description) = description {
            request["Description"] = json!(description);
        }
        ok(&router, "PutAsset", &request).await;

        let read = get_asset(&router, &identifier).await;
        let catalog_form = read
            .get("Forms")
            .and_then(|forms| forms.get(CATALOG_RESOURCE_FORM_NAME))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case}: the read must report the catalog's own form, got {read}")
            });
        assert_eq!(
            catalog_form,
            json!({
                "FormTypeId": CATALOG_RESOURCE_FORM_TYPE_ID,
                "Content": expected,
            }),
            "case {case}: expected the document {expected:?}, got {catalog_form}"
        );

        // The list a read answers with beside the map reports the same form.
        let listed = read
            .get("AssetForms")
            .and_then(Value::as_array)
            .and_then(|forms| forms.last())
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the read must report AssetForms, got {read}"));
        assert_eq!(
            listed,
            json!({
                "FormName": CATALOG_RESOURCE_FORM_NAME,
                "FormTypeId": CATALOG_RESOURCE_FORM_TYPE_ID,
                "Content": expected,
            }),
            "case {case}: the list and the map must agree about the catalog's own form, got \
             {read}"
        );

        let again = get_asset(&router, &identifier).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one asset must report one document, got {again}"
        );
    }

    // The document follows the record: a put that renames the asset and drops its
    // description moves it, and a put that gives it one back moves it again.
    let identifier = "renamed";
    for (case, request, expected) in [
        (
            "the first put",
            json!({
                "Identifier": identifier,
                "AssetTypeId": &asset_type_id,
                "Name": "Before",
                "Description": "the first description",
                "Forms": {},
            }),
            r#"{"name":"Before","description":"the first description"}"#,
        ),
        (
            "a put that renames the asset and sends no description",
            json!({
                "Identifier": identifier,
                "AssetTypeId": &asset_type_id,
                "Name": "After",
                "Forms": {},
            }),
            r#"{"name":"After"}"#,
        ),
        (
            "a put that gives it a description back",
            json!({
                "Identifier": identifier,
                "AssetTypeId": &asset_type_id,
                "Name": "After",
                "Description": "the second description",
                "Forms": {},
            }),
            r#"{"name":"After","description":"the second description"}"#,
        ),
    ] {
        ok(&router, "PutAsset", &request).await;
        let read = get_asset(&router, identifier).await;
        assert_eq!(
            read.get("Forms")
                .and_then(|forms| forms.get(CATALOG_RESOURCE_FORM_NAME))
                .and_then(|form| form.get("Content"))
                .and_then(Value::as_str),
            Some(expected),
            "case {case}: the document must follow the record, got {read}"
        );
    }
}
