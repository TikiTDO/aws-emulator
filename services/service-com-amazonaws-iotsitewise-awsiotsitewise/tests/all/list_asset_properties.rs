// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssetProperties`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListAssetProperties` enumerates the properties the
//! named asset actually holds in the shared engine store — its own (base)
//! properties under the default `BASE` filter, and under `ALL` also every property
//! the composite models of its asset model contribute — in one stable order, cut
//! into pages of at most `maxResults` (default 50) joined by a resumable
//! `nextToken`, reporting each summary's members from the stored property, and
//! refuses a label or a token that names nothing stored.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, hex_token, router};

/// Fixed property ids, so the expected enumeration order and every page token are
/// known to the test without asking the emulator. Each is a lowercase hyphenated
/// UUID with RFC 4122's version-4 nibble, which is what the modelled `ID` pattern
/// admits.
const IDS: [&str; 5] = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
    "55555555-5555-4555-8555-555555555555",
];

/// The id of the composite model the `ALL` filter reaches into.
const COMPOSITE_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

/// The name of that composite model, which is also the first segment of the path
/// of every property it contributes.
const COMPOSITE_NAME: &str = "HighTemp";

/// The ids of the two properties that composite model defines.
const COMPOSITE_PROPERTY_IDS: [&str; 2] = [
    "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
];

/// The external id the fixture asset carries, so the `externalId:` label form of
/// `assetId` is exercised.
const ASSET_EXTERNAL_ID: &str = "fixture_asset";

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "99999999-9999-4999-8999-999999999999";

/// The asset model's own property definitions: one row per `PropertyType` member a
/// definition may carry (attribute | measurement | transform | metric), spanning
/// the `PropertyDataType` values a model property can hold and both the presence
/// and the absence of `externalId` and `unit`.
///
/// Source: `smithy_query(query="shape", shape="PropertyType,PropertyDataType")` —
/// `PropertyDataType` is STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO |
/// ANNOTATION | JSON, of which the last three are not model-property types, and
/// `STRUCT` is the one that also carries `dataTypeSpec`.
fn base_definitions() -> Value {
    json!([
        {
            "id": IDS[0], "name": "Serial", "dataType": "STRING",
            "type": {"attribute": {"defaultValue": "unset"}},
        },
        {
            "id": IDS[1], "name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius",
            "type": {"measurement": {}},
        },
        {
            "id": IDS[2], "name": "Running", "dataType": "BOOLEAN",
            "type": {"measurement": {}},
        },
        {
            "id": IDS[3], "name": "AlarmState", "dataType": "STRUCT",
            "dataTypeSpec": "AWS/ALARM_STATE", "type": {"measurement": {}},
        },
        {
            "id": IDS[4], "name": "Fahrenheit", "externalId": "fahrenheit_of_celsius",
            "dataType": "DOUBLE", "unit": "Fahrenheit",
            "type": {"transform": {
                "expression": "celsius * 9 / 5 + 32",
                "variables": [{"name": "celsius", "value": {"propertyId": IDS[1]}}],
            }},
        },
    ])
}

/// The base property names, in definition order — the order the asset's own
/// properties are materialised and enumerated in.
const BASE_NAMES: [&str; 5] = [
    "Serial",
    "Temperature",
    "Running",
    "AlarmState",
    "Fahrenheit",
];

/// The property names the composite model defines, in definition order.
const COMPOSITE_NAMES: [&str; 2] = ["State", "Threshold"];

/// Create the fixture asset model — the base definitions plus one composite model
/// of its own two properties — and return its id.
async fn create_fixture_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelProperties": base_definitions(),
            "assetModelCompositeModels": [{
                "id": COMPOSITE_ID,
                "name": COMPOSITE_NAME,
                "type": "AWS/ALARM",
                "properties": [
                    {
                        "id": COMPOSITE_PROPERTY_IDS[0], "name": COMPOSITE_NAMES[0],
                        "dataType": "STRUCT", "dataTypeSpec": "AWS/ALARM_STATE",
                        "type": {"measurement": {}},
                    },
                    {
                        "id": COMPOSITE_PROPERTY_IDS[1], "name": COMPOSITE_NAMES[1],
                        "dataType": "DOUBLE", "unit": "Celsius",
                        "type": {"attribute": {"defaultValue": "80"}},
                    },
                ],
            }],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {body}");
    body["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {body}"))
        .to_string()
}

/// The fixture: the model above plus one asset of it, as `(model_id, asset_id)`.
async fn model_and_asset(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let model_id = create_fixture_model(router, name).await;
    let (status, body) = call(
        router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": format!("{name}-1"),
            "assetModelId": model_id,
            "assetExternalId": ASSET_EXTERNAL_ID,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset({name}): got {status} {body}");
    let asset_id = body["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset({name}): no assetId in {body}"))
        .to_string();
    (model_id, asset_id)
}

/// `GET /assets/{label}/properties{query}`.
async fn list(router: &MultiServiceRouter, label: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{label}/properties{query}"),
        None,
    )
    .await
}

/// The summaries a `ListAssetProperties` body reports.
fn summaries(body: &Value) -> Vec<Value> {
    body["assetPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetPropertySummaries in {body}"))
        .clone()
}

/// The property name of each summary, taken from the last segment of its `path`:
/// `AssetPropertySummary` models no `name` member, so the path is where the name a
/// listing reports lives.
fn listed_names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["path"]
                .as_array()
                .and_then(|path| path.last())
                .and_then(|segment| segment["name"].as_str())
                .unwrap_or_else(|| panic!("a summary without a named path in {summary}"))
                .to_string()
        })
        .collect()
}

/// The names the `ALL` filter must enumerate, in order.
fn all_names() -> Vec<String> {
    BASE_NAMES
        .iter()
        .chain(COMPOSITE_NAMES.iter())
        .map(|name| name.to_string())
        .collect()
}

/// The names the `BASE` filter must enumerate, in order.
fn base_names() -> Vec<String> {
    BASE_NAMES.iter().map(|name| name.to_string()).collect()
}

#[tokio::test]
async fn the_filter_selects_the_stored_properties_the_listing_reports() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    // One row per way a caller can ask for the filter: absent (the documented BASE
    // default), BASE, and ALL — the only two values `ListAssetPropertiesFilter`
    // admits, per `smithy_query(query="shape", shape="ListAssetPropertiesFilter")`.
    let base = base_names();
    let all = all_names();
    let cases: [(&str, &Vec<String>); 3] = [
        ("?maxResults=250", &base),
        ("?maxResults=250&filter=BASE", &base),
        ("?maxResults=250&filter=ALL", &all),
    ];
    for (query, expected) in cases {
        let (status, body) = list(&router, &asset_id, query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: ListAssetProperties got {status} {body}"
        );
        assert_eq!(
            &listed_names(&body), expected,
            "case {query:?}: the listing must report exactly the stored properties, got {body}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {query:?}: a page that holds everything must carry no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn every_summary_member_comes_from_the_stored_asset_property() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    // Bind an alias and enable notifications on one base property, so the rows
    // below span a property that carries each per-asset member and ones that
    // carry neither.
    let alias = "/plant/1/boiler/temperature";
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{}", IDS[1]),
        Some(json!({"propertyAlias": alias, "propertyNotificationState": "ENABLED"})),
    )
    .await;
    assert_eq!(status, 200, "UpdateAssetProperty: got {status} {updated}");

    // The round trips: the implemented readers of the same stored records.
    let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    let (status, composite) = call(
        &router,
        "GET",
        &format!("/assets/{asset_id}/composite-models/{COMPOSITE_ID}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAssetCompositeModel: got {status} {composite}"
    );

    let (status, listed) = list(&router, &asset_id, "?maxResults=250&filter=ALL").await;
    assert_eq!(status, 200, "ListAssetProperties: got {status} {listed}");
    let rows = summaries(&listed);

    // Each case is one stored property and where it is held: `None` for a base
    // property, the composite model's id for one the composite model contributes.
    let described_base = described["assetProperties"].clone();
    let described_composite = composite["assetCompositeModelProperties"].clone();
    let cases: Vec<(&str, Option<&str>, &Value)> = BASE_NAMES
        .iter()
        .map(|name| (*name, None, &described_base))
        .chain(
            COMPOSITE_NAMES
                .iter()
                .map(|name| (*name, Some(COMPOSITE_ID), &described_composite)),
        )
        .collect();

    for (name, composite_id, held_in) in cases {
        let stored = held_in
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: no stored properties in {held_in}"))
            .iter()
            .find(|property| property["name"] == json!(name))
            .unwrap_or_else(|| panic!("case {name:?}: not stored in {held_in}"));
        let summary = rows
            .iter()
            .find(|summary| summary["id"] == stored["id"])
            .unwrap_or_else(|| panic!("case {name:?}: not listed in {listed}"));

        // Every member the summary shares with the stored property must be the
        // stored value, not a fresh one.
        for member in ["id", "externalId", "alias", "unit", "notification"] {
            assert_eq!(
                summary.get(member).unwrap_or(&Value::Null),
                stored.get(member).unwrap_or(&Value::Null),
                "case {name:?}/{member:?}: the summary disagrees with the stored property \
                 ({summary} vs {stored})"
            );
        }
        assert_eq!(
            summary.get("assetCompositeModelId").and_then(Value::as_str),
            composite_id,
            "case {name:?}: assetCompositeModelId must name the composite model that holds \
             the property, got {summary}"
        );

        // `path` is the structured path from the root of the asset: the asset
        // itself, the composite models the property sits inside, outermost first,
        // then the property.
        let asset_root = json!({
            "id": described["assetId"].clone(),
            "name": described["assetName"].clone(),
        });
        let expected_path: Vec<Value> = std::iter::once(asset_root)
            .chain(
                composite_id
                    .map(|id| json!({"id": id, "name": COMPOSITE_NAME}))
                    .into_iter(),
            )
            .chain(std::iter::once(json!({"id": stored["id"], "name": name})))
            .collect();
        assert_eq!(
            summary["path"].as_array(),
            Some(&expected_path),
            "case {name:?}: path must run from the asset root to the property, got {summary}"
        );
    }

    // The alias rows specifically: the property the update bound must report it,
    // and every other property must report none. `IDS` is index-aligned with
    // `BASE_NAMES`, so each row names the stored property without asking the
    // listing which one it is.
    for (at, name) in BASE_NAMES.iter().enumerate() {
        let want_alias = (*name == "Temperature").then_some(alias);
        let summary = rows
            .iter()
            .find(|summary| summary["id"] == json!(IDS[at]))
            .unwrap_or_else(|| panic!("case {name:?}: not listed in {listed}"));
        assert_eq!(
            summary.get("alias").and_then(Value::as_str),
            want_alias,
            "case {name:?}: the alias must be the one bound against the stored property, \
             got {summary}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_property_exactly_once_whatever_the_page_size() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;
    let expected = all_names();

    // Rows: page sizes from one-per-page to larger than the enumeration, including
    // the exact length, one either side of it, and the published upper bound of
    // `maxResults` (range 1..250, per
    // `smithy_query(query="operation", operation="ListAssetProperties")`).
    for max_results in [1_usize, 2, 3, 6, 7, 8, 250] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let resume = token
                .as_deref()
                .map(|token| format!("&nextToken={token}"))
                .unwrap_or_default();
            let (status, body) = list(
                &router,
                &asset_id,
                &format!("?filter=ALL&maxResults={max_results}{resume}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: page {pages} got {status} {body}"
            );
            let page = listed_names(&body);
            assert!(
                page.len() <= max_results,
                "case maxResults={max_results}: page {pages} holds {} summaries, more than \
                 asked: {body}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={max_results}: paging did not terminate after {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len().is_multiple_of(max_results),
                        "case maxResults={max_results}: a page followed by a nextToken must \
                         be full, saw {} summaries: {body}",
                        seen.len()
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={max_results}: the pages joined must be the whole enumeration once"
        );
        let want_pages = expected.len().div_ceil(max_results);
        assert_eq!(
            pages, want_pages,
            "case maxResults={max_results}: expected {want_pages} pages, walked {pages}"
        );
    }
}

#[tokio::test]
async fn the_next_token_names_the_first_property_of_the_next_page() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    // The token the handler issues after a first page of one is the hex of this
    // enumeration's resume key for the SECOND property — `<composite id>|<property
    // id>`, with the composite id empty for a base property. Computed here by hand,
    // so the assertion is about the token's content and not just that paging
    // round-trips.
    let (status, first) = list(&router, &asset_id, "?maxResults=1").await;
    assert_eq!(status, 200, "first page: got {status} {first}");
    assert_eq!(
        first["nextToken"].as_str().unwrap_or(""),
        hex_token(&format!("|{}", IDS[1])),
        "the nextToken must name the first property of the next page, got {first}"
    );

    // And the composite-model rows of an ALL listing carry the composite model's id
    // in their resume key, so resuming from one works.
    let resume = hex_token(&format!("{COMPOSITE_ID}|{}", COMPOSITE_PROPERTY_IDS[1]));
    let (status, tail) = list(
        &router,
        &asset_id,
        &format!("?filter=ALL&nextToken={resume}"),
    )
    .await;
    assert_eq!(status, 200, "resumed page: got {status} {tail}");
    assert_eq!(
        listed_names(&tail),
        vec![COMPOSITE_NAMES[1].to_string()],
        "resuming from the last property must answer exactly it, got {tail}"
    );
}

#[tokio::test]
async fn the_all_filter_reaches_a_composite_model_nested_inside_another() {
    let router = router();
    let (model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    // A composite model nested inside the fixture's one: the stored composite
    // models are a FLAT list linked by parent, at any depth, and `ALL` is "all
    // asset properties for a given asset model ID" — so a nested composite
    // model's property is listed too, with the whole chain in its path.
    let (status, created) = call(
        &router,
        "POST",
        &format!("/asset-models/{model_id}/composite-models"),
        Some(json!({
            "assetModelCompositeModelName": "Nested",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": COMPOSITE_ID,
            "assetModelCompositeModelProperties": [
                {"name": "Deep", "dataType": "STRING", "type": {"attribute": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel: got {status} {created}"
    );
    let nested_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelId in {created}"))
        .to_string();

    let (status, body) = list(&router, &asset_id, "?maxResults=250&filter=ALL").await;
    assert_eq!(status, 200, "ListAssetProperties: got {status} {body}");

    let mut expected = all_names();
    expected.push("Deep".to_string());
    assert_eq!(
        listed_names(&body), expected,
        "case \"nested composite model\": the ALL listing must reach every composite model's \
         properties, got {body}"
    );

    let deep = summaries(&body)
        .into_iter()
        .find(|summary| summary["assetCompositeModelId"] == json!(nested_id))
        .unwrap_or_else(|| panic!("case \"nested composite model\": not listed in {body}"));
    let path = deep["path"]
        .as_array()
        .unwrap_or_else(|| panic!("case \"nested composite model\": no path in {deep}"));
    assert_eq!(
        path.len(),
        4,
        "case \"nested composite model\": the path must run asset root, outer composite, \
         nested composite, property, got {deep}"
    );
    assert_eq!(
        path.first().map(|segment| segment["id"].clone()),
        Some(json!(asset_id)),
        "case \"nested composite model\": the path must start at the asset itself, got {deep}"
    );
    assert_eq!(
        path.get(1).map(|segment| segment["id"].clone()),
        Some(json!(COMPOSITE_ID)),
        "case \"nested composite model\": the outermost composite model follows the asset \
         root, got {deep}"
    );
    assert_eq!(
        path.get(2).map(|segment| segment["id"].clone()),
        Some(json!(nested_id)),
        "case \"nested composite model\": the nested composite model follows it, got {deep}"
    );
}

#[tokio::test]
async fn max_results_is_refused_outside_its_published_range() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    // `maxResults` carries the modelled range 1..250: each published bound is
    // accepted and the value just past each bound is refused.
    for (max_results, accepted) in [(0_i64, false), (1, true), (250, true), (251, false)] {
        let (status, body) = list(&router, &asset_id, &format!("?maxResults={max_results}")).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults={max_results}: an out-of-range page size must be refused, \
                 got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_label_or_token_that_names_nothing_stored_is_a_modelled_refusal() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    // Rows: the ways a request can name something this store does not hold.
    let stale_token = hex_token(&format!("|{ABSENT_ID}"));
    let composite_token = hex_token(&format!("{COMPOSITE_ID}|{}", COMPOSITE_PROPERTY_IDS[0]));
    let cases: [(&str, String, String, u16, &str); 5] = [
        (
            "unknown asset id",
            ABSENT_ID.to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown externalId label",
            "externalId:no_such_asset".to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "token for a property not in this enumeration",
            asset_id.clone(),
            format!("?nextToken={stale_token}"),
            400,
            "InvalidRequestException",
        ),
        (
            "token for a composite property the BASE filter does not enumerate",
            asset_id.clone(),
            format!("?nextToken={composite_token}"),
            400,
            "InvalidRequestException",
        ),
        (
            "token that is not a token this operation issued",
            asset_id.clone(),
            "?nextToken=zzz".to_string(),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, label, query, want_status, want_code) in cases {
        let (status, body) = list(&router, &label, &query).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected HTTP {want_status}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with(want_code),
            "case {case:?}: expected {want_code}, got {:?} in {body}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn an_external_id_label_lists_the_same_properties_and_the_read_changes_nothing() {
    let router = router();
    let (_model_id, asset_id) = model_and_asset(&router, "Boiler").await;

    let (status, by_id) = list(&router, &asset_id, "?maxResults=250&filter=ALL").await;
    assert_eq!(status, 200, "by raw id: got {status} {by_id}");

    // Rows: the other way to name the SAME stored asset, and the same request
    // repeated — a reporting operation must answer identically both times.
    let external_label = format!("externalId:{ASSET_EXTERNAL_ID}");
    let cases: [(&str, &str); 2] = [
        ("externalId label", external_label.as_str()),
        ("the raw id again", asset_id.as_str()),
    ];
    for (case, label) in cases {
        let (status, body) = list(&router, label, "?maxResults=250&filter=ALL").await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["assetPropertySummaries"], by_id["assetPropertySummaries"],
            "case {case:?}: must list the same properties as the first read, got {body}"
        );
    }
}

#[tokio::test]
async fn an_asset_of_a_model_with_no_properties_lists_an_empty_page() {
    let router = router();
    let (status, created) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "Bare", "assetModelProperties": []})),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel(Bare): got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelId in {created}"))
        .to_string();
    let asset_id = create_asset(&router, "Bare-1", &model_id).await;

    for filter in ["BASE", "ALL"] {
        let (status, body) = list(&router, &asset_id, &format!("?filter={filter}")).await;
        assert_eq!(status, 200, "case {filter:?}: got {status} {body}");
        assert_eq!(
            summaries(&body).len(),
            0,
            "case {filter:?}: an asset with no properties must list nothing, got {body}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {filter:?}: an empty listing must carry no nextToken, got {body}"
        );
    }
}
