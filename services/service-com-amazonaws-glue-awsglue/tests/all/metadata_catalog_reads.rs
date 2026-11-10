// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the Glue business-catalog READS report: the instants a
//! glossary and a glossary term carry, the identifier shape the catalog mints for them, the
//! glossary a page of terms was listed under, the asset an association was addressed to,
//! every collection a read of an asset reports, how a form's content is rendered, and the
//! identifier form an asset type reads back under.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a READ of a business-catalog entity reports every member the
//! catalog holds for it, computed from the stored record — the instants the server stamped,
//! every collection the entity can carry (EMPTY rather than absent where it carries
//! nothing), the document a form's content holds, and the entity's identity in the form
//! this surface hands it out in. Nothing a read reports is invented per call, so two reads
//! of one entity agree.
//!
//! The suites are tables whose rows differ along what the rule generalises over: which
//! entity is read, how many entries a collection holds, where the insignificant whitespace
//! of a submitted document sits, the form and length of an identifier, and which of an
//! entity's identifier forms the call addressed it by.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::MAX_ASSET_FORMS;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_catalog_form, call, emulator, error_code, ok, qualified_asset_type_id};

/// How many characters the identifier the catalog mints for a glossary or a glossary term
/// carries, and the alphabet it draws them from: 14 lowercase base-36 characters. `GlossaryId`
/// and `GlossaryTermId` carry no pattern of their own (`smithy_query shape=GlossaryId`), so
/// the shape asserted here is the one the catalog hands out, not a modelled constraint.
const CATALOG_ENTITY_ID_LENGTH: usize = 14;

/// The form name the catalog's OWN form on an asset carries, which every read of an asset
/// reports beside the forms a put stored.
const CATALOG_RESOURCE_FORM_NAME: &str = "catalog::Resource";

/// Create a glossary and answer the whole response.
async fn create_glossary(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "CreateGlossary", request).await
}

/// The `Id` member of a response, which must be a string.
fn id_of(case: &str, body: &Value) -> String {
    body.get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Id, got {body}"))
        .to_string()
}

/// A timestamp member of a response, as its epoch-second value.
fn instant(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: the response must carry {member}, got {body}"))
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
    let mut forms = Map::new();
    forms.insert(
        "Details".to_string(),
        json!({ "FormTypeIdentifier": form_type_id }),
    );
    let stored = ok(
        router,
        "PutAssetType",
        &json!({ "Name": name, "Forms": Value::Object(forms) }),
    )
    .await;
    id_of(name, &stored)
}

/// The `GetAsset` response for the asset `identifier` names.
async fn get_asset(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetAsset", &json!({ "Identifier": identifier })).await
}

// ---------------------------------------------------------------------------
// The instants a glossary and a glossary term carry
// ---------------------------------------------------------------------------

/// A read of a glossary or of a glossary term reports the two instants the catalog stamped
/// on it: both are the create's instant to begin with, two reads report the same pair (they
/// are stored, not stamped per read), and an update that changes the record moves the
/// update instant while leaving the creation instant where it is.
///
/// The rows differ along which entity is read and along which members its create carried.
#[tokio::test]
async fn a_read_of_a_glossary_or_a_term_reports_the_instants_the_catalog_stamped() {
    let router = emulator();

    // (case, the CreateGlossary request)
    let glossaries: Vec<(&str, Value)> = vec![
        ("a glossary with a description", json!({ "Name": "SalesGlossary", "Description": "sales terms" })),
        ("a glossary with no description", json!({ "Name": "BareGlossary" })),
        (
            "a glossary created under an idempotency token",
            json!({ "Name": "TokenedGlossary", "ClientToken": "a-client-token" }),
        ),
    ];

    for (case, request) in &glossaries {
        let created = create_glossary(&router, request).await;
        let id = id_of(case, &created);

        let read = ok(&router, "GetGlossary", &json!({ "Identifier": &id })).await;
        let created_at = instant(case, &read, "CreatedAt");
        assert_eq!(
            instant(case, &read, "UpdatedAt"),
            created_at,
            "case {case}: a glossary no update has changed must report its creation instant \
             as its update instant, got {read}"
        );

        let again = ok(&router, "GetGlossary", &json!({ "Identifier": &id })).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one glossary must report one record, got {again}"
        );

        // An update that names a member moves the update instant and nothing else.
        let updated = ok(
            &router,
            "UpdateGlossary",
            &json!({ "Identifier": &id, "Description": "an updated description" }),
        )
        .await;
        assert_eq!(
            updated.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the update must report the glossary it changed, got {updated}"
        );
        let after = ok(&router, "GetGlossary", &json!({ "Identifier": &id })).await;
        assert_eq!(
            instant(case, &after, "CreatedAt"),
            created_at,
            "case {case}: an update must not move the creation instant, got {after}"
        );
        assert!(
            instant(case, &after, "UpdatedAt") >= created_at,
            "case {case}: the update instant must not precede the creation instant, got \
             {after}"
        );
    }

    // A glossary term carries the same pair, for the same reason.
    let glossary = create_glossary(&router, &json!({ "Name": "TermHoldingGlossary" })).await;
    let glossary_id = id_of("the term's glossary", &glossary);

    // (case, the CreateGlossaryTerm members beside the glossary and the name)
    let terms: Vec<(&str, Value)> = vec![
        ("a term with both descriptions", json!({ "ShortDescription": "short", "LongDescription": "long" })),
        ("a term with a short description only", json!({ "ShortDescription": "short" })),
        ("a term with neither description", json!({})),
    ];

    for (index, (case, extra)) in terms.iter().enumerate() {
        let mut request = extra.clone();
        request["GlossaryIdentifier"] = json!(&glossary_id);
        request["Name"] = json!(format!("Term{index}"));
        let created = ok(&router, "CreateGlossaryTerm", &request).await;
        let id = id_of(case, &created);

        let read = ok(&router, "GetGlossaryTerm", &json!({ "Identifier": &id })).await;
        let created_at = instant(case, &read, "CreatedAt");
        assert_eq!(
            instant(case, &read, "UpdatedAt"),
            created_at,
            "case {case}: a term no update has changed must report its creation instant as \
             its update instant, got {read}"
        );
        let again = ok(&router, "GetGlossaryTerm", &json!({ "Identifier": &id })).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one term must report one record, got {again}"
        );
    }
}

// ---------------------------------------------------------------------------
// The identifier the catalog mints
// ---------------------------------------------------------------------------

/// The identifier the catalog mints for a glossary and for a glossary term has the shape
/// this surface hands out — [`CATALOG_ENTITY_ID_LENGTH`] lowercase base-36 characters — is
/// the address the entity answers to, is reported unchanged by every later read, and is its
/// own: no two entities share one.
///
/// The rows differ along the entity kind and along what the create carried, none of which
/// the identifier is derived from.
#[tokio::test]
async fn the_catalog_mints_its_own_identifier_shape_for_a_glossary_and_a_term() {
    let router = emulator();

    /// Whether `id` has the shape the catalog hands out.
    fn assert_catalog_shape(case: &str, id: &str) {
        assert_eq!(
            id.chars().count(),
            CATALOG_ENTITY_ID_LENGTH,
            "case {case}: expected an identifier of {CATALOG_ENTITY_ID_LENGTH} characters, \
             got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case}: expected lowercase base-36 characters, got {id:?}"
        );
    }

    let mut minted: Vec<String> = Vec::new();

    // (case, the CreateGlossary request)
    let glossaries: Vec<(&str, Value)> = vec![
        ("the shortest name the model admits", json!({ "Name": "A" })),
        ("a plain name", json!({ "Name": "OrdersGlossary" })),
        (
            "a name at the modelled length bound",
            json!({ "Name": "G".repeat(256) }),
        ),
        (
            "a name carrying spaces and punctuation",
            json!({ "Name": "Orders & Returns (2024)" }),
        ),
        (
            "a create carrying a description and a token",
            json!({ "Name": "DescribedGlossary", "Description": "d", "ClientToken": "t" }),
        ),
    ];

    for (case, request) in &glossaries {
        let created = create_glossary(&router, request).await;
        let id = id_of(case, &created);
        assert_catalog_shape(case, &id);

        let read = ok(&router, "GetGlossary", &json!({ "Identifier": &id })).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the identifier the create reported must address the record and be \
             reported unchanged, got {read}"
        );
        minted.push(id);
    }

    let glossary_id = minted[1].clone();
    // (case, the term's name)
    let terms: Vec<(&str, String)> = vec![
        ("the shortest term name", "T".to_string()),
        ("a plain term name", "Revenue".to_string()),
        (
            "a term name at the modelled length bound",
            "T".repeat(256),
        ),
    ];
    for (case, name) in &terms {
        let created = ok(
            &router,
            "CreateGlossaryTerm",
            &json!({ "GlossaryIdentifier": &glossary_id, "Name": name }),
        )
        .await;
        let id = id_of(case, &created);
        assert_catalog_shape(case, &id);

        let read = ok(&router, "GetGlossaryTerm", &json!({ "Identifier": &id })).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the identifier the create reported must address the term and be \
             reported unchanged, got {read}"
        );
        minted.push(id);
    }

    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "case every entity: two entities must not share one identifier, got {minted:?}"
    );
}

// ---------------------------------------------------------------------------
// The glossary a page of terms was listed under
// ---------------------------------------------------------------------------

/// Every page `ListGlossaryTerms` answers names the glossary it was listed under, echoed
/// from the request, so a client paging several glossaries attributes each page to its
/// glossary.
///
/// The rows differ along how many terms the glossary holds, whether the page is the first
/// or a later one, the page size, and whether the glossary identifier resolves at all.
#[tokio::test]
async fn a_page_of_glossary_terms_names_the_glossary_it_was_listed_under() {
    let router = emulator();

    // (case, how many terms the glossary holds)
    let rows: Vec<(&str, usize)> = vec![
        ("a glossary holding no term", 0),
        ("a glossary holding one term", 1),
        ("a glossary holding three terms", 3),
    ];

    let mut glossaries: Vec<(&str, String)> = Vec::new();
    for (case, term_count) in &rows {
        let created = create_glossary(
            &router,
            &json!({ "Name": format!("Glossary{term_count}") }),
        )
        .await;
        let glossary_id = id_of(case, &created);
        for index in 0..*term_count {
            ok(
                &router,
                "CreateGlossaryTerm",
                &json!({
                    "GlossaryIdentifier": &glossary_id,
                    "Name": format!("Term{term_count}_{index}"),
                }),
            )
            .await;
        }
        glossaries.push((case, glossary_id));
    }

    for (case, glossary_id) in &glossaries {
        // The whole listing, and then one term at a time, so a later page is covered too.
        for page_size in [None, Some(1), Some(1000)] {
            let mut request = json!({ "GlossaryIdentifier": glossary_id });
            if let Some(size) = page_size {
                request["MaxResults"] = json!(size);
            }
            let mut page = ok(&router, "ListGlossaryTerms", &request).await;
            let mut pages = 0;
            loop {
                pages += 1;
                assert_eq!(
                    page.get("GlossaryId").and_then(Value::as_str),
                    Some(glossary_id.as_str()),
                    "case {case} at page size {page_size:?}: page {pages} must name the \
                     glossary it was listed under, got {page}"
                );
                let Some(token) = page.get("NextToken").and_then(Value::as_str) else {
                    break;
                };
                let mut next = request.clone();
                next["NextToken"] = json!(token);
                page = ok(&router, "ListGlossaryTerms", &next).await;
            }
        }
    }

    // The operation models no not-found error, so a listing for a glossary this account
    // holds nothing under is served — and still names the glossary it was asked for.
    let page = ok(
        &router,
        "ListGlossaryTerms",
        &json!({ "GlossaryIdentifier": "no-such-glossary" }),
    )
    .await;
    assert_eq!(
        page.get("GlossaryId").and_then(Value::as_str),
        Some("no-such-glossary"),
        "case a glossary identifier that resolves to nothing: the page must still name it, \
         got {page}"
    );
}

// ---------------------------------------------------------------------------
// The asset an association was addressed to
// ---------------------------------------------------------------------------

/// `AssociateGlossaryTerms` reports the asset it was addressed to under both names this
/// surface carries it under, taken from the request, so a caller correlates the answer with
/// the asset it just changed.
///
/// The rows differ along the form and length of the asset identifier and along how many
/// terms the association names.
#[tokio::test]
async fn an_association_reports_the_asset_it_was_addressed_to() {
    let router = emulator();
    let details = form_type(&router, "OrderForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;
    let glossary = create_glossary(&router, &json!({ "Name": "AssociationGlossary" })).await;
    let glossary_id = id_of("the association's glossary", &glossary);

    let mut terms: Vec<String> = Vec::new();
    for index in 0..3 {
        let created = ok(
            &router,
            "CreateGlossaryTerm",
            &json!({ "GlossaryIdentifier": &glossary_id, "Name": format!("Term{index}") }),
        )
        .await;
        terms.push(id_of("a term to associate", &created));
    }

    let longest = "a".repeat(1087);
    // (case, the asset identifier, how many terms the association names)
    let rows: Vec<(&str, String, usize)> = vec![
        ("a plain identifier", "orders-asset".to_string(), 1),
        (
            "an identifier carrying dots, slashes and colons",
            "catalog:default/orders.table_1".to_string(),
            2,
        ),
        ("a one-character identifier", "a".to_string(), 3),
        (
            "an identifier at the modelled length bound",
            longest.clone(),
            1,
        ),
    ];

    for (case, identifier, term_count) in &rows {
        ok(
            &router,
            "PutAsset",
            &json!({
                "Identifier": identifier,
                "AssetTypeId": &asset_type_id,
                "Name": "Orders",
                "Forms": {},
            }),
        )
        .await;

        let named: Vec<String> = terms.iter().take(*term_count).cloned().collect();
        let answered = ok(
            &router,
            "AssociateGlossaryTerms",
            &json!({ "AssetIdentifier": identifier, "GlossaryTermIdentifiers": &named }),
        )
        .await;
        assert_eq!(
            (
                answered.get("Identifier").and_then(Value::as_str),
                answered.get("AssetIdentifier").and_then(Value::as_str),
            ),
            (Some(identifier.as_str()), Some(identifier.as_str())),
            "case {case}: the association must report the asset it was addressed to, got \
             {answered}"
        );
        assert_eq!(
            answered
                .get("GlossaryTerms")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(*term_count),
            "case {case}: the association must report the terms the asset carries, got \
             {answered}"
        );

        // Re-sending the same association reports the same asset: the member is the
        // request's, not a function of what changed.
        let repeated = ok(
            &router,
            "AssociateGlossaryTerms",
            &json!({ "AssetIdentifier": identifier, "GlossaryTermIdentifiers": &named }),
        )
        .await;
        assert_eq!(
            repeated.get("Identifier").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {case}: a repeated association must report the same asset, got {repeated}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every collection a read of an asset reports
// ---------------------------------------------------------------------------

/// A read of an asset reports every collection the asset can carry — its forms as both the
/// map and the list, its attachments, its iterable forms, its metadata forms and its
/// glossary terms — EMPTY where the asset carries nothing rather than leaving the member
/// out, beside the catalog's own form on the asset and the name of the asset type it is an
/// instance of.
///
/// The rows differ along how many forms the asset carries, whether it carries a glossary
/// term association, and whether its asset type was addressed by name or by the identifier
/// the put reported. The ceiling on the forms one asset may hold ([`MAX_ASSET_FORMS`]) is
/// exercised at the bound and one past it.
#[tokio::test]
async fn a_read_of_an_asset_reports_every_collection_it_can_carry() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let audit = form_type(&router, "AuditForm").await;
    let asset_type_id = asset_type(&router, "OrderAssetType", &details).await;
    let glossary = create_glossary(&router, &json!({ "Name": "AssetGlossary" })).await;
    let term = ok(
        &router,
        "CreateGlossaryTerm",
        &json!({
            "GlossaryIdentifier": id_of("the asset's glossary", &glossary),
            "Name": "Revenue",
        }),
    )
    .await;
    let term_id = id_of("the asset's term", &term);

    // (case, the asset identifier, the forms the put sends, whether a term is associated,
    //  how the asset type is addressed)
    let rows: Vec<(&str, &str, Value, bool, String)> = vec![
        (
            "an asset carrying no form at all",
            "formless",
            json!({}),
            false,
            asset_type_id.clone(),
        ),
        (
            "an asset carrying one form",
            "one-form",
            json!({ "Details": { "FormTypeId": &details, "Content": "{\"a\":1}" } }),
            false,
            asset_type_id.clone(),
        ),
        (
            "an asset carrying two forms and a term",
            "two-forms",
            json!({
                "Details": { "FormTypeId": &details, "Content": "{\"a\":1}" },
                "Audit": { "FormTypeId": &audit, "Content": "{\"b\":2}" },
            }),
            true,
            "OrderAssetType".to_string(),
        ),
        (
            "an asset carrying a form with no content",
            "no-content",
            json!({ "Details": { "FormTypeId": &details } }),
            true,
            asset_type_id.clone(),
        ),
    ];

    for (case, identifier, forms, associate, asset_type_address) in &rows {
        ok(
            &router,
            "PutAsset",
            &json!({
                "Identifier": identifier,
                "AssetTypeId": asset_type_address,
                "Name": "Orders",
                "Forms": forms,
            }),
        )
        .await;
        if *associate {
            ok(
                &router,
                "AssociateGlossaryTerms",
                &json!({
                    "AssetIdentifier": identifier,
                    "GlossaryTermIdentifiers": [&term_id],
                }),
            )
            .await;
        }

        let read = get_asset(&router, identifier).await;

        // The collections no implemented operation writes are reported EMPTY, never absent.
        assert_eq!(
            read.get("Attachments"),
            Some(&json!({})),
            "case {case}: an asset with no attachment must report the empty map, got {read}"
        );
        assert_eq!(
            read.get("IterableForms"),
            Some(&json!({})),
            "case {case}: an asset with no iterable form must report the empty map, got \
             {read}"
        );
        assert_eq!(
            read.get("MetadataForms"),
            Some(&json!([])),
            "case {case}: an asset with no metadata form must report the empty list, got \
             {read}"
        );

        // The terms the asset carries, as a list that is empty rather than absent.
        let expected_terms = if *associate {
            vec![Value::String(term_id.clone())]
        } else {
            Vec::new()
        };
        assert_eq!(
            read.get("GlossaryTerms").and_then(Value::as_array),
            Some(&expected_terms),
            "case {case}: the read must report the terms the asset carries, got {read}"
        );

        // The forms map carries the asset's own forms and the catalog's own form, which
        // names the catalog's own form type and carries the catalog's own document for the
        // asset.
        let reported_forms = read
            .get("Forms")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case}: the read must report Forms, got {read}"));
        assert_catalog_form(case, &read, reported_forms.get(CATALOG_RESOURCE_FORM_NAME));
        let sent_names: BTreeSet<&String> = forms
            .as_object()
            .expect("the row sends a map")
            .keys()
            .collect();
        let reported_names: BTreeSet<&String> = reported_forms
            .keys()
            .filter(|name| name.as_str() != CATALOG_RESOURCE_FORM_NAME)
            .collect();
        assert_eq!(
            reported_names, sent_names,
            "case {case}: the read must report the forms the put stored, got {read}"
        );

        // The same forms, as the list the read answers with beside the map: the asset's own
        // forms and then the catalog's.
        let listed = read
            .get("AssetForms")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: the read must report AssetForms, got {read}"));
        assert_eq!(
            listed.len(),
            sent_names.len() + 1,
            "case {case}: the list must carry one entry per reported form, got {read}"
        );
        assert_eq!(
            listed
                .last()
                .and_then(|entry| entry.get("FormName"))
                .and_then(Value::as_str),
            Some(CATALOG_RESOURCE_FORM_NAME),
            "case {case}: the catalog's own form must be the last entry, got {read}"
        );
        for entry in listed {
            let name = entry
                .get("FormName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: every entry names its form, got {entry}"));
            assert_eq!(
                entry.get("FormTypeId"),
                reported_forms[name].get("FormTypeId"),
                "case {case}: the list and the map must agree about {name}, got {entry}"
            );
            assert_eq!(
                entry.get("Content"),
                reported_forms[name].get("Content"),
                "case {case}: the list and the map must agree about {name}, got {entry}"
            );
        }

        // The asset type the asset is an instance of, by name, whichever form the put
        // addressed it by.
        assert_eq!(
            read.get("Type").and_then(Value::as_str),
            Some("OrderAssetType"),
            "case {case}: the read must name the asset type the asset is an instance of, \
             got {read}"
        );
    }

    // One put decides what every later read of that asset serialises — the forms map and the
    // list beside it — so the forms one asset may hold are bounded: the put at the ceiling is
    // served and reported in full, and the one past it is refused with the client error the
    // operation models, storing nothing.
    let at_ceiling: Map<String, Value> = (0..MAX_ASSET_FORMS)
        .map(|index| {
            (
                format!("Form{index}"),
                json!({ "FormTypeId": &details, "Content": "{}" }),
            )
        })
        .collect();
    let mut past_ceiling = at_ceiling.clone();
    past_ceiling.insert(
        "OneTooMany".to_string(),
        json!({ "FormTypeId": &details, "Content": "{}" }),
    );

    ok(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "bounded",
            "AssetTypeId": &asset_type_id,
            "Name": "Orders",
            "Forms": Value::Object(at_ceiling),
        }),
    )
    .await;
    let read = get_asset(&router, "bounded").await;
    assert_eq!(
        read.get("Forms").and_then(Value::as_object).map(Map::len),
        Some(MAX_ASSET_FORMS + 1),
        "case forms at the ceiling: the read must report every form and the catalog's own, \
         got {read}"
    );

    let (status, body) = call(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "one-form-too-many",
            "AssetTypeId": &asset_type_id,
            "Name": "Orders",
            "Forms": Value::Object(past_ceiling),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one form too many: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one form too many: expected InvalidInputException, got {body}"
    );
    let (status, body) = call(
        &router,
        "GetAsset",
        &json!({ "Identifier": "one-form-too-many" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one form too many: a refused put must store nothing, got {status} with {body}"
    );

    // `AssetFormKey` carries no pattern, so a put MAY store a form under the name the
    // catalog's own form carries. What a read reports for a form the asset holds is then
    // that form: a stored form is never replaced by the catalog's.
    ok(
        &router,
        "PutAsset",
        &json!({
            "Identifier": "shadowing",
            "AssetTypeId": &asset_type_id,
            "Name": "Orders",
            "Forms": {
                CATALOG_RESOURCE_FORM_NAME: { "FormTypeId": &details, "Content": "{\"a\":1}" },
            },
        }),
    )
    .await;
    let read = get_asset(&router, "shadowing").await;
    assert_eq!(
        read.get("Forms")
            .and_then(|forms| forms.get(CATALOG_RESOURCE_FORM_NAME)),
        Some(&json!({ "FormTypeId": &details, "Content": "{\"a\":1}" })),
        "case a form stored under the catalog's own form name: the read must report the \
         stored form, got {read}"
    );
    assert_eq!(
        read.get("AssetForms").and_then(Value::as_array).map(Vec::len),
        Some(1),
        "case a form stored under the catalog's own form name: the list must carry that one \
         form alone, got {read}"
    );
}

// ---------------------------------------------------------------------------
// A form's content
// ---------------------------------------------------------------------------

/// A read of an asset reports a form's content as the JSON DOCUMENT the put supplied,
/// rendered without the whitespace between its tokens: member order, member names and the
/// spelling of every scalar are the document's own, and whitespace inside a string is part
/// of a token and survives. Text that is no JSON document at all is reported exactly as it
/// was stored. The put that accepted the text answers with the text as sent.
///
/// The rows differ along where the insignificant whitespace sits — between members, inside
/// an array, around a nested object, as newlines and indentation — and along what the
/// document holds: strings carrying spaces and punctuation, escaped quotes and backslashes,
/// members out of alphabetical order, number spellings, an empty object and a non-document.
#[tokio::test]
async fn a_forms_content_is_reported_as_the_document_it_carries() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;
    let asset_type_id = asset_type(&router, "ContentAssetType", &details).await;

    // (case, the content the put sends, the content the read must report)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a document already without whitespace",
            r#"{"owner":"analytics-team","rowCount":1000}"#,
            r#"{"owner":"analytics-team","rowCount":1000}"#,
        ),
        (
            "a space after every colon and comma",
            r#"{"owner": "analytics-team", "rowCount": 1000}"#,
            r#"{"owner":"analytics-team","rowCount":1000}"#,
        ),
        (
            "newlines and indentation",
            "{\n  \"owner\": \"analytics-team\",\n  \"rowCount\": 1000\n}",
            r#"{"owner":"analytics-team","rowCount":1000}"#,
        ),
        (
            "whitespace around a nested object and an array",
            r#"{ "tags" : [ "a" , "b" ] , "nested" : { "deep" : true } }"#,
            r#"{"tags":["a","b"],"nested":{"deep":true}}"#,
        ),
        (
            "members out of alphabetical order",
            r#"{ "zeta": 1, "alpha": 2 }"#,
            r#"{"zeta":1,"alpha":2}"#,
        ),
        (
            "strings carrying spaces, colons and commas",
            r#"{ "note": "owner: analytics, team" }"#,
            r#"{"note":"owner: analytics, team"}"#,
        ),
        (
            "a string carrying an escaped quote and a backslash",
            r#"{ "note": "a \"quoted\" value \\ here" }"#,
            r#"{"note":"a \"quoted\" value \\ here"}"#,
        ),
        (
            "number spellings the document chose",
            r#"{ "a": 1.50, "b": 1e3, "c": -0.0 }"#,
            r#"{"a":1.50,"b":1e3,"c":-0.0}"#,
        ),
        ("an empty object", "{ }", "{}"),
        ("an empty array", "[ ]", "[]"),
        (
            "a bare JSON string",
            "\"  padded  \"",
            "\"  padded  \"",
        ),
        (
            "text that is no JSON document at all",
            "owner: analytics-team",
            "owner: analytics-team",
        ),
    ];

    for (index, (case, sent, expected)) in rows.iter().enumerate() {
        let identifier = format!("content-{index}");
        let stored = ok(
            &router,
            "PutAsset",
            &json!({
                "Identifier": &identifier,
                "AssetTypeId": &asset_type_id,
                "Name": "Content",
                "Forms": { "Details": { "FormTypeId": &details, "Content": sent } },
            }),
        )
        .await;

        // The put answers with the text it accepted.
        assert_eq!(
            stored
                .get("Forms")
                .and_then(|forms| forms.get("Details"))
                .and_then(|form| form.get("Content"))
                .and_then(Value::as_str),
            Some(*sent),
            "case {case}: the put must answer with the content it accepted, got {stored}"
        );

        let read = get_asset(&router, &identifier).await;
        let reported = read
            .get("Forms")
            .and_then(|forms| forms.get("Details"))
            .and_then(|form| form.get("Content"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: the read must report the content, got {read}"));
        assert_eq!(
            reported, *expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
    }

    // Two puts sending ONE document with different spacing store one asset: a read of
    // either reports the same content.
    let spaced = r#"{ "owner" : "analytics-team" }"#;
    let compact = r#"{"owner":"analytics-team"}"#;
    for (case, content) in [("the spaced form", spaced), ("the compact form", compact)] {
        ok(
            &router,
            "PutAsset",
            &json!({
                "Identifier": "one-document",
                "AssetTypeId": &asset_type_id,
                "Name": "Content",
                "Forms": { "Details": { "FormTypeId": &details, "Content": content } },
            }),
        )
        .await;
        let read = get_asset(&router, "one-document").await;
        assert_eq!(
            read.get("Forms")
                .and_then(|forms| forms.get("Details"))
                .and_then(|form| form.get("Content"))
                .and_then(Value::as_str),
            Some(compact),
            "case {case}: one document must read back one way, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The identifier an asset type reads back under
// ---------------------------------------------------------------------------

/// A read of an asset type reports its identity in the catalog-qualified form
/// `glue_<accountId>::<name>`, while the put that registers it reports the bare name — and
/// BOTH forms address the record, so a client that kept either reaches the same asset type
/// through the read and the delete.
///
/// The rows differ along the form and length of the asset type's name.
#[tokio::test]
async fn an_asset_type_reads_back_under_its_catalog_qualified_id() {
    let router = emulator();
    let details = form_type(&router, "DetailsForm").await;

    let mut longest = String::from("A");
    longest.push_str(&"x".repeat(127));
    // The names keep to `AssetTypeName`'s pattern `^(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$`
    // and its "len 1..128" (`smithy_query operation=PutAssetType`).
    let names: Vec<(&str, String)> = vec![
        ("the shortest name the model admits", "A".to_string()),
        ("a plain name", "OrderAssetType".to_string()),
        ("a name carrying digits", "OrderAsset2024".to_string()),
        ("a name carrying an underscore", "order_asset".to_string()),
        ("a name opening with an underscore", "_orderAsset1".to_string()),
        ("the longest name the model admits", longest),
    ];

    for (case, name) in &names {
        let reported = asset_type(&router, name, &details).await;
        assert_eq!(
            &reported, name,
            "case {case}: the put must report the name it registered, got {reported:?}"
        );

        let qualified = qualified_asset_type_id(name);
        // Both forms address the record, and each read reports the qualified form.
        for address in [name.as_str(), qualified.as_str()] {
            let read = ok(&router, "GetAssetType", &json!({ "Identifier": address })).await;
            assert_eq!(
                (
                    read.get("Id").and_then(Value::as_str),
                    read.get("Name").and_then(Value::as_str),
                ),
                (Some(qualified.as_str()), Some(name.as_str())),
                "case {case}: a read addressed by {address:?} must report the qualified id, \
                 got {read}"
            );
        }

        // The delete resolves the qualified form too, and the record is gone afterwards.
        ok(
            &router,
            "DeleteAssetType",
            &json!({ "Identifier": &qualified }),
        )
        .await;
        let (status, body) = call(
            &router,
            "GetAssetType",
            &json!({ "Identifier": &qualified }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a deleted asset type must miss under its qualified id, got \
             {status} with {body}"
        );
    }

    // A qualified identifier for a name this account holds no asset type under resolves to
    // nothing, so the form itself is not what makes a read succeed.
    let (status, body) = call(
        &router,
        "GetAssetType",
        &json!({ "Identifier": qualified_asset_type_id("NeverRegistered") }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a qualified identifier naming nothing: expected 400, got {status} with {body}"
    );
}
