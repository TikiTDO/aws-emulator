// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the composite models an ASSET carries, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: an asset instantiated from an asset model carries
//! an INSTANCE of every composite model that model holds, so each composite
//! model is reported on the asset — a custom one in
//! `assetCompositeModelSummaries`, a built-in one in `assetCompositeModels` —
//! and the properties it contributes are materialised on
//! the asset — addressable by property id exactly as the model's own top-level
//! definitions are (ingestible, readable, describable), each under the `path`
//! that reaches it from the root of the asset, and kept OUT of the asset's base
//! `assetProperties`.
//!
//! The rows span the ways a composite model comes by its definitions (inline
//! ones of its own, the ones of a component model it reuses, none at all), the
//! number and data types of what it contributes, and whether it sits at the top
//! of the asset or nested inside another composite model.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, error_code, router};
use crate::asset_model_composite_model::{create_composite, PAST_THE_OWED_TRANSITION};
use crate::support::now_seconds;

/// Where `BatchPutAssetPropertyValue` is bound: `POST /properties`.
const BATCH_PUT: &str = "/properties";

/// One composite model of the fixture: what it is created with, and what the
/// asset's instance of it must carry.
struct Row {
    /// What this row varies — the case name every assertion carries.
    case: &'static str,
    /// The `CreateAssetModelCompositeModel` request body, bar the parent link,
    /// which is filled in from the row above it when `nested` is set.
    body: Value,
    /// Whether the composite model is nested inside the row created before it,
    /// in which case the asset reports it through the composite model it sits
    /// in rather than at the top level.
    nested: bool,
    /// `(property name, PropertyDataType)` for each property the composite model
    /// contributes, in the order the model reports them.
    contributes: Vec<(&'static str, &'static str)>,
}

/// A reading of `data_type` for the ingest and read-back of one property.
///
/// The pairing of `PropertyDataType` to `Variant` member is the model's:
/// `smithy_query(query="shape", shape="PropertyDataType,Variant")` — of the
/// values STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION |
/// JSON only the first four have a `Variant` member carrying a scalar reading,
/// which is what the rows below contribute.
fn reading(data_type: &str) -> Value {
    match data_type {
        "DOUBLE" => json!({"doubleValue": 42.5}),
        "INTEGER" => json!({"integerValue": 7}),
        "STRING" => json!({"stringValue": "running"}),
        "BOOLEAN" => json!({"booleanValue": true}),
        other => panic!("no reading for data type {other}"),
    }
}

/// The names in an `assetProperties` / `assetModelCompositeModelProperties`
/// list, in order.
fn names_of(properties: &Value) -> Vec<&str> {
    properties
        .as_array()
        .map(|list| {
            list.iter()
                .map(|property| property["name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

/// Create the asset model and its composite models, and return
/// `(assetModelId, the rows paired with the composite model id each got)`.
async fn fixture(router: &MultiServiceRouter) -> (String, Vec<(Row, String)>) {
    let reusable = component_model(
        router,
        "Pump",
        json!([
            {"name": "Flow", "dataType": "DOUBLE", "unit": "m3/s", "type": {"measurement": {}}},
            {
                "name": "Serial", "dataType": "STRING", "externalId": "pump_serial",
                "type": {"attribute": {"defaultValue": "unset"}},
            },
        ]),
    )
    .await;

    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Plant",
            "assetModelProperties": [
                {"name": "Uptime", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel(Plant): got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel(Plant): no assetModelId in {created}"))
        .to_string();

    // One row per way of coming by definitions, and per shape of what is
    // contributed: one inline property, several inline properties of differing
    // data types, a reused component model's two, none at all, and a nested
    // composite model with one of its own.
    let rows = vec![
        Row {
            case: "one inline definition of its own",
            body: json!({
                "assetModelCompositeModelName": "Setpoints",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelDescription": "the operator's setpoints",
                "assetModelCompositeModelExternalId": "setpoints",
                "assetModelCompositeModelProperties": [
                    {
                        "name": "Target", "dataType": "DOUBLE", "unit": "Celsius",
                        "type": {"attribute": {"defaultValue": "20"}},
                    },
                ],
            }),
            nested: false,
            contributes: vec![("Target", "DOUBLE")],
        },
        Row {
            case: "several inline definitions, of differing data types",
            body: json!({
                "assetModelCompositeModelName": "Readings",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelProperties": [
                    {"name": "Cycles", "dataType": "INTEGER", "unit": "Count", "type": {"measurement": {}}},
                    {"name": "Label", "dataType": "STRING", "type": {"measurement": {}}},
                    {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
                ],
            }),
            nested: false,
            contributes: vec![
                ("Cycles", "INTEGER"),
                ("Label", "STRING"),
                ("Running", "BOOLEAN"),
            ],
        },
        Row {
            case: "a reused component model's definitions",
            body: json!({
                "assetModelCompositeModelName": "Reused",
                "assetModelCompositeModelType": "CUSTOM",
                "composedAssetModelId": &reusable,
            }),
            nested: false,
            contributes: vec![("Flow", "DOUBLE"), ("Serial", "STRING")],
        },
        Row {
            case: "no definitions at all",
            body: json!({
                "assetModelCompositeModelName": "Anomaly",
                "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
            }),
            nested: false,
            contributes: Vec::new(),
        },
        Row {
            case: "nested inside the composite model created before it",
            body: json!({
                "assetModelCompositeModelName": "Nested",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelProperties": [
                    {"name": "Depth", "dataType": "INTEGER", "type": {"measurement": {}}},
                ],
            }),
            nested: true,
            contributes: vec![("Depth", "INTEGER")],
        },
    ];

    let mut created_rows: Vec<(Row, String)> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut body = row.body.clone();
        if row.nested {
            let parent = created_rows
                .last()
                .map(|(_, id)| id.clone())
                .expect("a composite model to nest inside");
            body["parentAssetModelCompositeModelId"] = json!(parent);
        }
        let case = row.case;
        let (status, created) = create_composite(router, &model_id, body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateAssetModelCompositeModel got {status} {created}"
        );
        let composite_id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no composite model id in {created}"))
            .to_string();
        created_rows.push((row, composite_id));
    }
    (model_id, created_rows)
}

/// Create a `COMPONENT_MODEL` carrying `properties` and return its id.
async fn component_model(router: &MultiServiceRouter, name: &str, properties: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {created}"))
        .to_string()
}

/// `GET /assets/{asset_id}`, with `query` appended.
async fn describe_asset(
    router: &MultiServiceRouter,
    asset_id: &str,
    query: &str,
) -> (u16, Value) {
    call(router, "GET", &format!("/assets/{asset_id}{query}"), None).await
}

/// `GET /assets/{asset_id}/composite-models/{composite_id}`, the asset-side view
/// of ONE composite model.
async fn describe_asset_composite(
    router: &MultiServiceRouter,
    asset_id: &str,
    composite_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset_id}/composite-models/{composite_id}"),
        None,
    )
    .await
}

#[tokio::test]
async fn an_asset_reports_and_materialises_every_composite_model_of_its_model() {
    let router = router();
    let (model_id, rows) = fixture(&router).await;
    // The model has to have settled before an asset can instantiate it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let asset_id = create_asset(&router, "Plant1", &model_id).await;

    let (status, asset) = describe_asset(&router, &asset_id, "").await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");

    // The asset's own (base) properties, which "doesn't include properties that
    // you define in composite models".
    assert_eq!(
        names_of(&asset["assetProperties"]),
        vec!["Uptime"],
        "assetProperties must carry the asset's base properties only, got {asset}"
    );

    let summaries = asset["assetCompositeModelSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let composites = asset["assetCompositeModels"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    for (row, composite_id) in &rows {
        let case = row.case;
        let expected: Vec<&str> = row.contributes.iter().map(|(name, _)| *name).collect();

        // A composite model nested inside another is reported by the composite
        // model it sits in, not at the top of the asset; a top-level one is
        // reported ONCE by the asset itself — a built-in composite model (an
        // alarm, an anomaly detector) in `assetCompositeModels`, a custom one in
        // `assetCompositeModelSummaries`.
        let summary = summaries
            .iter()
            .find(|summary| summary["id"] == json!(composite_id));
        let composite = composites
            .iter()
            .find(|composite| composite["id"] == json!(composite_id));
        let built_in = row.body["assetModelCompositeModelType"] != json!("CUSTOM");
        if row.nested {
            assert!(
                summary.is_none() && composite.is_none(),
                "case {case:?}: a nested composite model is not an immediate child of the \
                 asset, got {asset}"
            );
        } else if built_in {
            assert!(
                summary.is_none(),
                "case {case:?}: a built-in composite model is reported in \
                 assetCompositeModels, not among the custom summaries, got {asset}"
            );
            let composite = composite.unwrap_or_else(|| {
                panic!("case {case:?}: no assetCompositeModels entry in {asset}")
            });
            assert_eq!(
                names_of(&composite["properties"]),
                expected,
                "case {case:?}: the asset's instance must carry what the composite model \
                 contributes, got {composite}"
            );
        } else {
            assert!(
                composite.is_none(),
                "case {case:?}: assetCompositeModels carries the built-in composite models \
                 only, so a custom one must not be reported twice, got {asset}"
            );
            let summary = summary.unwrap_or_else(|| {
                panic!("case {case:?}: no assetCompositeModelSummaries entry in {asset}")
            });
            assert_eq!(
                summary["path"].as_array().map(Vec::len),
                Some(2),
                "case {case:?}: a top-level composite model's path is the asset root and the \
                 composite model, got {summary}"
            );
            assert_eq!(
                summary["path"][0]["id"],
                json!(asset_id),
                "case {case:?}: the path must start at the asset itself, got {summary}"
            );
        }

        // Either way the asset's own view of that composite model reports the
        // same instance, and nothing a composite model contributes leaks into
        // the asset's base property list.
        let (status, instance) =
            describe_asset_composite(&router, &asset_id, composite_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetCompositeModel got {status} {instance}"
        );
        assert_eq!(
            names_of(&instance["assetCompositeModelProperties"]),
            expected,
            "case {case:?}: the asset's view of the composite model must carry what it \
             contributes, got {instance}"
        );
        for name in &expected {
            assert!(
                !names_of(&asset["assetProperties"]).contains(name),
                "case {case:?}: {name} is a composite model's property and must not appear \
                 in assetProperties, got {asset}"
            );
        }
    }
}

#[tokio::test]
async fn a_composite_models_property_is_addressable_on_the_asset() {
    let router = router();
    let (model_id, rows) = fixture(&router).await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let asset_id = create_asset(&router, "Plant1", &model_id).await;

    for (row, composite_id) in &rows {
        let (status, instance) =
            describe_asset_composite(&router, &asset_id, composite_id).await;
        let case = row.case;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetCompositeModel got {status} {instance}"
        );
        let properties = instance["assetCompositeModelProperties"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        for (name, data_type) in &row.contributes {
            let property = properties
                .iter()
                .find(|property| property["name"] == json!(name))
                .unwrap_or_else(|| panic!("case {case:?}/{name}: not in {instance}"));
            let property_id = property["id"].as_str().unwrap_or_default();

            // The path reaches the property from the root of the asset: the
            // composite models it sits inside, outermost first, then itself.
            let path = property["path"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                path.last().map(|segment| segment["id"].clone()),
                Some(json!(property_id)),
                "case {case:?}/{name}: the path must end at the property, got {property}"
            );
            assert_eq!(
                path.get(path.len().saturating_sub(2))
                    .map(|segment| segment["id"].clone()),
                Some(json!(composite_id)),
                "case {case:?}/{name}: the path must reach the property through the \
                 composite model that holds it, got {property}"
            );

            // DescribeAssetProperty reports it under `compositeModel`, with the
            // property type its definition declares.
            let (status, described) = call(
                &router,
                "GET",
                &format!("/assets/{asset_id}/properties/{property_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{name}: DescribeAssetProperty got {status} {described}"
            );
            assert_eq!(
                described["compositeModel"]["assetProperty"]["name"],
                json!(name),
                "case {case:?}/{name}: must be described under its composite model, got \
                 {described}"
            );
            assert!(
                described["compositeModel"]["assetProperty"]["type"].is_object(),
                "case {case:?}/{name}: the described property must carry the type its \
                 definition declares, got {described}"
            );
            assert!(
                described.get("assetProperty").is_none_or(Value::is_null),
                "case {case:?}/{name}: a composite model's property is not reported as one \
                 of the asset's own, got {described}"
            );

            // And it is a stream of the asset: ingestible, then readable back.
            let value = reading(data_type);
            let (status, put) = call(
                &router,
                "POST",
                BATCH_PUT,
                Some(json!({"entries": [{
                    "entryId": format!("{name}-1"),
                    "assetId": &asset_id,
                    "propertyId": property_id,
                    // Inside the accepted ingest window — `[now - 7 days, now +
                    // 10 minutes]` — and AFTER the instant the asset was created,
                    // so this reading is the property's latest even where its
                    // definition declares a default the create already seeded.
                    "propertyValues": [{
                        "value": value,
                        "timestamp": {"timeInSeconds": now_seconds() + 60},
                    }],
                }]})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{name}: BatchPutAssetPropertyValue got {status} {put}"
            );
            assert_eq!(
                put["errorEntries"],
                json!([]),
                "case {case:?}/{name}: the entry must be ingested, got {put}"
            );

            let (status, latest) = call(
                &router,
                "GET",
                &format!("/properties/latest?assetId={asset_id}&propertyId={property_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{name}: GetAssetPropertyValue got {status} {latest}"
            );
            assert_eq!(
                latest["propertyValue"]["value"], value,
                "case {case:?}/{name}: the value read back must be the one ingested, got \
                 {latest}"
            );
        }
    }
}

#[tokio::test]
async fn exclude_properties_blanks_a_composite_models_properties_but_keeps_the_summaries() {
    let router = router();
    let (model_id, rows) = fixture(&router).await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let asset_id = create_asset(&router, "Plant1", &model_id).await;

    for (case, query, with_properties) in [
        ("the default", "", true),
        ("excludeProperties=false", "?excludeProperties=false", true),
        ("excludeProperties=true", "?excludeProperties=true", false),
    ] {
        let (status, asset) = describe_asset(&router, &asset_id, query).await;
        assert_eq!(status, 200, "case {case:?}: DescribeAsset got {status} {asset}");
        // Every immediate child is summarised bar the built-in ones, which
        // `assetCompositeModels` reports instead.
        let custom_top_level = rows
            .iter()
            .filter(|(row, _)| {
                !row.nested && row.body["assetModelCompositeModelType"] == json!("CUSTOM")
            })
            .count();
        assert_eq!(
            asset["assetCompositeModelSummaries"].as_array().map(Vec::len),
            Some(custom_top_level),
            "case {case:?}: every immediate child custom composite model is summarised, got \
             {asset}"
        );
        for composite in asset["assetCompositeModels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let empty = names_of(&composite["properties"]).is_empty();
            let expected = rows
                .iter()
                .find(|(_, id)| composite["id"] == json!(id))
                .map(|(row, _)| row.contributes.is_empty())
                .unwrap_or(true);
            assert_eq!(
                empty,
                if with_properties { expected } else { true },
                "case {case:?}: properties reported where they should not be, got {composite}"
            );
        }
        assert_eq!(
            names_of(&asset["assetProperties"]).is_empty(),
            !with_properties,
            "case {case:?}: the base property list must follow the same flag, got {asset}"
        );
    }
}

#[tokio::test]
async fn a_composite_model_added_to_or_removed_from_the_model_reaches_its_assets() {
    let router = router();
    let (status, created) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [
                {"name": "Uptime", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel(Boiler): got {status} {created}");
    let model_id = created["assetModelId"].as_str().unwrap_or_default().to_string();
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    // The asset exists BEFORE the composite model does, so what it carries can
    // only have reached it through the propagation an asset-model write owes.
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;

    let (status, before) = describe_asset(&router, &asset_id, "").await;
    assert_eq!(status, 200, "DescribeAsset(before): got {status} {before}");
    for member in ["assetCompositeModels", "assetCompositeModelSummaries"] {
        assert_eq!(
            before[member].as_array().map(Vec::len),
            Some(0),
            "an asset of a model holding no composite model reports {member} as an empty \
             list rather than omitting it, got {before}"
        );
    }

    let (status, composite) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Alarm",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Threshold", "dataType": "DOUBLE", "type": {"attribute": {"defaultValue": "90"}}},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel: got {status} {composite}"
    );
    let composite_id = composite["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let (status, after) = describe_asset(&router, &asset_id, "").await;
    assert_eq!(status, 200, "DescribeAsset(after): got {status} {after}");
    let reported: Vec<&str> = after["assetCompositeModelSummaries"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|summary| summary["id"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported,
        vec![composite_id.as_str()],
        "the composite model added to the model must reach the asset already created from \
         it, got {after}"
    );
    // The composite model is a CUSTOM one, so the asset reports it as a summary;
    // what its instance CARRIES is read through the asset-side view of it.
    let (status, instance) = describe_asset_composite(&router, &asset_id, &composite_id).await;
    assert_eq!(
        status, 200,
        "DescribeAssetCompositeModel: got {status} {instance}"
    );
    assert_eq!(
        names_of(&instance["assetCompositeModelProperties"]),
        vec!["Threshold"],
        "the propagated composite model must bring its properties with it, got {instance}"
    );
    let property_id = instance["assetCompositeModelProperties"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // An attribute's default value is the property's value on every asset of the
    // model, so the propagated property's series starts with it.
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={property_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetAssetPropertyValue: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["value"],
        json!({"doubleValue": 90.0}),
        "the propagated attribute must carry the default its definition declares, got {latest}"
    );

    // Removing the composite model from the model takes its per-asset properties
    // with it, so the property stops being addressable on the asset. Removed
    // through `UpdateAssetModel`, whose request states the whole definition, so
    // what it leaves out is what the model stops holding. The property it KEEPS
    // is resubmitted by its stored id, the way a kept property has to be.
    let kept = crate::support::named_by_stored_id(
        &router,
        &model_id,
        json!([{"name": "Uptime", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(json!({
            "assetModelName": "Boiler",
            "assetModelProperties": kept,
        })),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {updated}");
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let (status, gone) = describe_asset(&router, &asset_id, "").await;
    assert_eq!(status, 200, "DescribeAsset(gone): got {status} {gone}");
    for member in ["assetCompositeModels", "assetCompositeModelSummaries"] {
        assert_eq!(
            gone[member].as_array().map(Vec::len),
            Some(0),
            "the removed composite model must be gone from the asset, leaving {member} an \
             empty list, got {gone}"
        );
    }
    let (status, missing) = call(
        &router,
        "GET",
        &format!("/assets/{asset_id}/properties/{property_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "DescribeAssetProperty after the removal: got {status} {missing}"
    );
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "DescribeAssetProperty after the removal: unexpected code in {missing}"
    );
}
