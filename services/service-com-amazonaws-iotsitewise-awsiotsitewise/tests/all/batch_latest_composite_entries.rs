// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how `BatchGetAssetPropertyValue` resolves the
//! `(assetId, propertyId)` pair each of its entries carries, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: an entry addressed by `(assetId, propertyId)`
//! resolves against EVERY property the named asset carries — the ones
//! materialised from its asset model's own top-level definitions AND the ones
//! that model's composite models contribute, both addressed by the same pair —
//! so every such entry is answered as a success entry, carrying the latest
//! value stored for that property or no value at all when nothing has been
//! written to it. Only a pair naming no property of the asset is an error
//! entry, with the `ResourceNotFoundException` error code. The same resolution
//! decides the `completionStatus` of an entry a previous page already
//! processed, so a composite model's property is reported SUCCESS there too.
//!
//! Each table's rows differ along WHERE the property is declared (on the asset
//! model itself, on a composite model declared with it, on one added
//! afterwards, on one nested inside another, and on the component model a
//! component-model-based composite model reuses), along the property's data
//! type, and along whether anything has been written to it — with the asset's
//! own property as the control row: every row must be served the same way.
//!
//! The wire harness (`router`, `call`, …) lives in the `asset_lifecycle`
//! suite, the fixture spanning every place an asset carries a property in
//! `composite_model_property_streams`, the composite-model create in
//! `asset_model_composite_model` and the clock anchoring in `support`; this
//! suite reuses all four rather than keeping a copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, property_id, router};
use crate::asset_model_composite_model::create_composite;
use crate::batch_get_latest_values::{count, find_entry};
use crate::composite_model_property_streams::{
    assert_close, boiler, composite_property, numeric, write_samples,
};

/// Where `BatchGetAssetPropertyValue` is bound: `POST /properties/batch/latest`,
/// from `smithy_query(query="operation", operation="BatchGetAssetPropertyValue")`.
const BATCH_LATEST: &str = "/properties/batch/latest";

/// Where `BatchPutAssetPropertyValue` is bound, used here to write the values
/// the batch read must report.
const BATCH_PUT: &str = "/properties";

/// A `CustomID`-shaped id no create ever minted, for the rows that must be
/// refused.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The `BatchGetAssetPropertyValueErrorCode` an entry naming no property of the
/// asset reports.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The `entryId` the table gives the row at `at`.
fn entry_id(at: usize) -> String {
    format!("row{at}")
}

/// Whether the row at `at` has anything written to its property: every other
/// row, so each place a property can be declared is covered both with a stored
/// value and with none.
fn written(at: usize) -> bool {
    at.is_multiple_of(2)
}

#[tokio::test]
async fn an_entry_naming_any_property_the_asset_carries_is_answered_from_the_store() {
    let router = router();
    let (asset_id, rows) = boiler(&router, "BatchLatest").await;
    assert!(
        rows.len() >= 4,
        "the fixture must span every place an asset carries a property, got {} rows",
        rows.len()
    );

    for (at, row) in rows.iter().enumerate() {
        if written(at) {
            write_samples(&router, &asset_id, row).await;
        }
    }

    // One request carrying every row, plus the two pairs that name nothing: a
    // propertyId no property of this asset stands under, and an assetId no
    // asset stands under.
    let mut entries: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(at, row)| {
            json!({
                "entryId": entry_id(at),
                "assetId": asset_id,
                "propertyId": row.property_id,
            })
        })
        .collect();
    entries.push(json!({
        "entryId": "absent-property",
        "assetId": asset_id,
        "propertyId": ABSENT_ID,
    }));
    entries.push(json!({
        "entryId": "absent-asset",
        "assetId": ABSENT_ID,
        "propertyId": rows[0].property_id,
    }));

    let request = json!({ "entries": entries });
    let (status, body) = call(&router, "POST", BATCH_LATEST, Some(request.clone())).await;
    assert_eq!(
        status, 200,
        "BatchGetAssetPropertyValue got {status} {body}"
    );
    assert_eq!(
        count(&body["skippedEntries"]),
        0,
        "no previous page processed anything, so skippedEntries must be empty: {body}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "one page answered every entry, so there must be no nextToken: {body}"
    );
    assert_eq!(
        count(&body["successEntries"]) + count(&body["errorEntries"]),
        entries.len(),
        "every entry must be answered exactly once, got {body}"
    );

    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let id = entry_id(at);
        assert!(
            find_entry(&body["errorEntries"], &id).is_none(),
            "case {case:?}: this pair names a property of the asset, so it must not be an error entry, got {body}"
        );
        let success = find_entry(&body["successEntries"], &id).unwrap_or_else(|| {
            panic!("case {case:?}: expected a success entry {id}, got {body}")
        });
        if written(at) {
            // The latest value of the series is the newer of the two samples
            // the write put on this property.
            assert_close(
                case,
                "the latest reading",
                numeric(&success["assetPropertyValue"]["value"]),
                row.readings[1],
            );
        } else {
            assert!(
                success.get("assetPropertyValue").is_none(),
                "case {case:?}: nothing has been written to this property, so it has no current value, got {success}"
            );
        }
    }

    for (case, id) in [
        (
            "a propertyId the asset carries no property under",
            "absent-property",
        ),
        ("an assetId no asset stands under", "absent-asset"),
    ] {
        assert!(
            find_entry(&body["successEntries"], id).is_none(),
            "case {case:?}: {id} names nothing, so it must not be a success entry, got {body}"
        );
        let failed = find_entry(&body["errorEntries"], id)
            .unwrap_or_else(|| panic!("case {case:?}: expected an error entry {id}, got {body}"));
        assert_eq!(
            failed["errorCode"].as_str(),
            Some(NOT_FOUND),
            "case {case:?}: expected the error code {NOT_FOUND}, got {failed}"
        );
        assert!(
            failed["errorMessage"]
                .as_str()
                .is_some_and(|message| message.contains(ABSENT_ID)),
            "case {case:?}: the message must name the id that resolved to nothing, got {failed}"
        );
    }

    // Reporting changes nothing a later caller sees: the same request answers
    // the same way.
    let (again_status, again) = call(&router, "POST", BATCH_LATEST, Some(request)).await;
    assert_eq!(
        again_status, status,
        "case \"read twice\": expected {status} again, got {again_status} {again}"
    );
    assert_eq!(
        again, body,
        "case \"read twice\": a read must not change what a later read sees, got {again} after {body}"
    );
}

/// How long a value this suite writes is, in bytes.
///
/// The handler pages a result set on its byte budget, so three values of this
/// size cannot share one page: two pages are what the skipped-entry assertions
/// below need.
const VALUE_BYTES: usize = 400 * 1024;

/// Write one `VALUE_BYTES`-long string value on `(asset_id, property)`.
async fn put_long_value(router: &MultiServiceRouter, asset_id: &str, case: &str, property: &str) {
    let (status, body) = call(
        router,
        "POST",
        BATCH_PUT,
        Some(json!({"entries": [{
            "entryId": "seed",
            "assetId": asset_id,
            "propertyId": property,
            "propertyValues": [{
                "value": {"stringValue": "x".repeat(VALUE_BYTES)},
                "timestamp": {"timeInSeconds": crate::support::anchor()},
                "quality": "GOOD",
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: the write got {status} {body}");
    assert_eq!(
        count(&body["errorEntries"]),
        0,
        "case {case:?}: a write to a property of this asset must be accepted, got {body}"
    );
}

#[tokio::test]
async fn an_entry_a_previous_page_processed_is_skipped_with_the_status_it_resolved_to() {
    let router = router();

    // An asset model with one STRING property of its own and one composite
    // model declared with it, plus a second composite model added afterwards:
    // three properties, in the three places an asset carries one, every one of
    // them addressed by `(assetId, propertyId)`.
    let (status, created) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "SkippedRecorder",
            "assetModelProperties": [
                {"name": "Own", "dataType": "STRING", "type": {"measurement": {}}},
            ],
            "assetModelCompositeModels": [
                {"name": "Declared", "type": "CUSTOM", "properties": [
                    {"name": "Tape", "dataType": "STRING", "type": {"measurement": {}}},
                ]},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {created}"))
        .to_string();

    let (status, added) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Added",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Reel", "dataType": "STRING", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel: got {status} {added}"
    );
    let added_id = added["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModelCompositeModel: no id in {added}"))
        .to_string();

    let asset_id = create_asset(&router, "SkippedRecorder-1", &model_id).await;
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");

    // The id of the composite model declared with the asset model, read back
    // from the service rather than assumed: the create answers only the model's
    // id and status. A CUSTOM composite model is the one the summaries name.
    let (status, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    let declared_id = model["assetModelCompositeModelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no composite models in {model}"))
        .iter()
        .find(|composite| composite["name"] == "Declared")
        .unwrap_or_else(|| panic!("no composite model named Declared in {model}"))["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the composite model Declared has no id in {model}"))
        .to_string();

    // Rows differ along where the property is declared; the first row is the
    // pair that names nothing, so a page later than the first carries a skipped
    // entry of each completion status.
    let rows = [
        (
            "a propertyId the asset carries no property under",
            "absent".to_string(),
            ABSENT_ID.to_string(),
        ),
        (
            "the asset's own property",
            "own".to_string(),
            property_id(&asset["assetProperties"], "Own"),
        ),
        (
            "a property of a composite model declared with the asset model",
            "tape".to_string(),
            composite_property(&router, &model_id, &declared_id, "Tape").await,
        ),
        (
            "a property of a composite model added to the model afterwards",
            "reel".to_string(),
            composite_property(&router, &model_id, &added_id, "Reel").await,
        ),
    ];

    for (case, id, property) in &rows {
        if id != "absent" {
            put_long_value(&router, &asset_id, case, property).await;
        }
    }

    let entries: Vec<Value> = rows
        .iter()
        .map(|(_, id, property)| {
            json!({"entryId": id, "assetId": asset_id, "propertyId": property})
        })
        .collect();

    let (status, first) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "page 0: got {status} {first}");
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("page 0: a result set past the page budget must be paged, got {first}")
        })
        .to_string();

    let (status, second) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries, "nextToken": token })),
    )
    .await;
    assert_eq!(status, 200, "page 1: got {status} {second}");

    // Every entry the first page answered is reported as skipped on the second,
    // with the completion status the pair it names resolves to — SUCCESS for a
    // property of the asset, wherever it is declared, and ERROR only for the
    // pair that names none.
    let answered: Vec<String> = ["successEntries", "errorEntries"]
        .iter()
        .flat_map(|list| first[*list].as_array().cloned().unwrap_or_default())
        .map(|entry| entry["entryId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        count(&second["skippedEntries"]),
        answered.len(),
        "page 1: expected {} skipped entries, got {second}",
        answered.len()
    );
    for (case, id, _) in &rows {
        if !answered.contains(id) {
            continue;
        }
        let skipped = find_entry(&second["skippedEntries"], id).unwrap_or_else(|| {
            panic!("case {case:?}: {id} was already processed, so it must be skipped: {second}")
        });
        if id == "absent" {
            assert_eq!(
                skipped["completionStatus"].as_str(),
                Some("ERROR"),
                "case {case:?}: this pair names no property, so its completionStatus is ERROR, got {skipped}"
            );
            assert_eq!(
                skipped["errorInfo"]["errorCode"].as_str(),
                Some(NOT_FOUND),
                "case {case:?}: expected the error code {NOT_FOUND}, got {skipped}"
            );
            assert!(
                skipped["errorInfo"]
                    .get("errorTimestamp")
                    .is_some_and(|stamp| !stamp.is_null()),
                "case {case:?}: an ERROR skipped entry carries the instant it failed, got {skipped}"
            );
        } else {
            assert_eq!(
                skipped["completionStatus"].as_str(),
                Some("SUCCESS"),
                "case {case:?}: this pair names a property of the asset, so its completionStatus is SUCCESS, got {skipped}"
            );
            assert!(
                skipped.get("errorInfo").is_none(),
                "case {case:?}: a SUCCESS skipped entry carries no errorInfo, got {skipped}"
            );
        }
    }

    // The entries the first page did not reach are answered whole on the
    // second, and the walk ends with the list.
    let remaining: Vec<&(&str, String, String)> = rows
        .iter()
        .filter(|(_, id, _)| !answered.contains(id))
        .collect();
    assert!(
        !remaining.is_empty(),
        "the page budget must have left at least one entry for the second page: {first}"
    );
    for (case, id, _) in remaining {
        let success = find_entry(&second["successEntries"], id).unwrap_or_else(|| {
            panic!("case {case:?}: expected a success entry {id} on page 1, got {second}")
        });
        assert_eq!(
            success["assetPropertyValue"]["value"]["stringValue"]
                .as_str()
                .map(str::len),
            Some(VALUE_BYTES),
            "case {case:?}: the stored value must be reported whole, got {id}"
        );
    }
    assert!(
        second.get("nextToken").is_none(),
        "page 1: every entry has been processed, so there must be no nextToken: {second}"
    );
}
