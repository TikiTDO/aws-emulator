// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAssetCompositeModel`, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a composite model of an ASSET is the INSTANCE of a
//! composite model of that asset's model, so the read reports the definition
//! stored on the model — its name, type, description, external id, the chain of
//! composite models it sits in and the composite models nested directly inside
//! it — with one asset property materialised per property definition, inheriting
//! the definition's id, whatever the composite model's type, property count,
//! nesting depth or component-model basis. A label that resolves to no composite
//! model OF THAT ASSET is the modelled refusal, never a fabricated answer, and
//! the read changes nothing a later caller sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, is_modelled_id, router};
use super::asset_model_composite_model::{create_composite, describe_composite, plain_model};

/// `GET /assets/{asset_label}/composite-models/{composite_label}`.
async fn describe_asset_composite(
    router: &MultiServiceRouter,
    asset_label: &str,
    composite_label: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/assets/{asset_label}/composite-models/{composite_label}"),
        None,
    )
    .await
}

/// Create a composite model on `model_id` from `body` and return its id.
async fn composite_id(router: &MultiServiceRouter, model_id: &str, body: Value) -> String {
    let (status, created) = create_composite(router, model_id, body).await;
    assert_eq!(status, 202, "create composite: got {status} {created}");
    created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create composite: no id in {created}"))
        .to_string()
}

/// The ids of the properties a describe body reports under `member`, in order.
fn property_ids(body: &Value, member: &str) -> Vec<String> {
    body[member]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|property| property["id"].as_str().unwrap_or("").to_string())
        .collect()
}

/// One row of the read-back table: a composite model of the asset's model, and
/// what the asset-side read of it must report.
struct Case {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The composite model's id on the asset model.
    id: String,
    /// The label the asset-side read addresses it by — its id, or an
    /// `externalId:` label for a composite model that carries an external id.
    label: String,
    /// The name, type and description the read must report.
    name: &'static str,
    r#type: &'static str,
    description: &'static str,
    /// The external id the read must report, if the composite model has one.
    external_id: Option<&'static str>,
    /// How many asset properties the read must report.
    properties: usize,
    /// How deep in the composite-model chain this composite model sits: one per
    /// composite model from the outermost ancestor down to itself, BESIDE the
    /// asset root hop every reported path starts with.
    path_depth: usize,
    /// The ids of the composite models nested directly inside it.
    children: Vec<String>,
}

#[tokio::test]
async fn an_assets_composite_model_is_the_instance_of_its_models_composite_model() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // A `COMPONENT_MODEL` with properties of its own, reused below: a
    // component-model-based composite model has no properties, and the component
    // model it names provides them.
    let (status, component) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Thermostat",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Target", "dataType": "DOUBLE", "unit": "Celsius", "type": {"attribute": {"defaultValue": "20"}}},
                {"name": "Measured", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "create component model: got {status} {component}");
    let component_model_id = component["assetModelId"]
        .as_str()
        .expect("an assetModelId")
        .to_string();

    // One composite model per thing the rule generalises over: the documented
    // type catalog, the number of property definitions, whether a description or
    // an external id is supplied, a caller-chosen id, nesting inside another
    // composite model, and a component-model-based composite model.
    let custom = composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelDescription": "a custom grouping",
            "assetModelCompositeModelExternalId": "custom_one",
            "assetModelCompositeModelProperties": [
                {"name": "Setpoint", "dataType": "DOUBLE", "unit": "Celsius", "type": {"attribute": {"defaultValue": "20"}}},
                {"name": "Cycles", "dataType": "INTEGER", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let nested = composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Nested",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": custom,
            "assetModelCompositeModelProperties": [
                {"name": "Deep", "dataType": "STRING", "type": {"attribute": {}}},
            ],
        }),
    )
    .await;
    let alarm = composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "HighTemp",
            "assetModelCompositeModelType": "AWS/ALARM",
            "assetModelCompositeModelId": "11111111-2222-4333-8444-555555555555",
            "assetModelCompositeModelProperties": [
                {"name": "State", "dataType": "STRUCT", "dataTypeSpec": "AWS/ALARM_STATE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    let anomaly = composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Anomaly",
            "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
        }),
    )
    .await;
    let reused = composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": component_model_id,
        }),
    )
    .await;

    // The asset whose composite models these are instances of.
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;

    let cases = vec![
        Case {
            case: "CUSTOM, two properties, described, external id, one child",
            id: custom.clone(),
            label: custom.clone(),
            name: "Custom1",
            r#type: "CUSTOM",
            description: "a custom grouping",
            external_id: Some("custom_one"),
            properties: 2,
            path_depth: 1,
            children: vec![nested.clone()],
        },
        Case {
            case: "the same composite model by its externalId label",
            id: custom.clone(),
            label: "externalId:custom_one".to_string(),
            name: "Custom1",
            r#type: "CUSTOM",
            description: "a custom grouping",
            external_id: Some("custom_one"),
            properties: 2,
            path_depth: 1,
            children: vec![nested.clone()],
        },
        Case {
            case: "CUSTOM nested inside another composite model",
            id: nested.clone(),
            label: nested.clone(),
            name: "Nested",
            r#type: "CUSTOM",
            description: "",
            external_id: None,
            properties: 1,
            path_depth: 2,
            children: Vec::new(),
        },
        Case {
            case: "AWS/ALARM, one struct property, caller-chosen id",
            id: alarm.clone(),
            label: alarm.clone(),
            name: "HighTemp",
            r#type: "AWS/ALARM",
            description: "",
            external_id: None,
            properties: 1,
            path_depth: 1,
            children: Vec::new(),
        },
        Case {
            case: "AWS/L4E_ANOMALY, no properties at all",
            id: anomaly.clone(),
            label: anomaly.clone(),
            name: "Anomaly",
            r#type: "AWS/L4E_ANOMALY",
            description: "",
            external_id: None,
            properties: 0,
            path_depth: 1,
            children: Vec::new(),
        },
        Case {
            case: "component-model-based: the component model provides the properties",
            id: reused.clone(),
            label: reused.clone(),
            name: "Reused",
            r#type: "CUSTOM",
            description: "",
            external_id: None,
            properties: 2,
            path_depth: 1,
            children: Vec::new(),
        },
    ];

    for case in &cases {
        let name = case.case;
        let (status, described) = describe_asset_composite(&router, &asset_id, &case.label).await;
        assert_eq!(status, 200, "case {name:?}: got {status} {described}");

        assert_eq!(
            described["assetId"].as_str(),
            Some(asset_id.as_str()),
            "case {name:?}: the read must report the asset it answered for, got {described}"
        );
        assert_eq!(
            described["assetCompositeModelId"].as_str(),
            Some(case.id.as_str()),
            "case {name:?}: expected the composite model's own id, got {described}"
        );
        assert_eq!(
            described["assetCompositeModelName"].as_str(),
            Some(case.name),
            "case {name:?}: expected name {:?}, got {described}",
            case.name
        );
        assert_eq!(
            described["assetCompositeModelType"].as_str(),
            Some(case.r#type),
            "case {name:?}: expected type {:?}, got {described}",
            case.r#type
        );
        assert_eq!(
            described["assetCompositeModelDescription"].as_str(),
            Some(case.description),
            "case {name:?}: expected description {:?}, got {described}",
            case.description
        );
        assert_eq!(
            described["assetCompositeModelExternalId"].as_str(),
            case.external_id,
            "case {name:?}: expected external id {:?}, got {described}",
            case.external_id
        );

        // The path is the ASSET itself followed by the chain of composite models
        // from the outermost ancestor down to this one, so it starts at the asset
        // root ("one level between a composite model and the root of the asset")
        // and ends at the composite model.
        let path = described["assetCompositeModelPath"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: no assetCompositeModelPath in {described}"));
        assert_eq!(
            path.len(),
            case.path_depth + 1,
            "case {name:?}: expected a {}-segment path (the asset root plus {} composite \
             hops), got {described}",
            case.path_depth + 1,
            case.path_depth
        );
        assert_eq!(
            path.first().and_then(|segment| segment["id"].as_str()),
            Some(asset_id.as_str()),
            "case {name:?}: the path must start at the asset itself, got {described}"
        );
        assert_eq!(
            path.last().and_then(|segment| segment["id"].as_str()),
            Some(case.id.as_str()),
            "case {name:?}: the path must end at the composite model itself, got {described}"
        );

        // One asset property per property definition the composite model
        // contributes, each inheriting the definition's id.
        let properties = described["assetCompositeModelProperties"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {name:?}: no assetCompositeModelProperties in {described}")
            })
            .clone();
        assert_eq!(
            properties.len(),
            case.properties,
            "case {name:?}: expected {} properties, got {described}",
            case.properties
        );
        for property in &properties {
            let id = property["id"].as_str().unwrap_or("");
            assert!(
                is_modelled_id(id),
                "case {name:?}: every asset property carries a modelled id, got {property}"
            );
            // "The structured path to the property from the root of the asset":
            // the asset, the composite models it sits inside, then the property.
            let property_path = property["path"].as_array().unwrap_or_else(|| {
                panic!("case {name:?}: property {property} reports no path in {described}")
            });
            assert_eq!(
                property_path.len(),
                case.path_depth + 2,
                "case {name:?}: expected {} path segments on {property}",
                case.path_depth + 2
            );
            assert_eq!(
                property_path.last().and_then(|segment| segment["id"].as_str()),
                Some(id),
                "case {name:?}: the property path must end at the property, got {property}"
            );
            assert_eq!(
                property_path.first().and_then(|segment| segment["id"].as_str()),
                path.first().and_then(|segment| segment["id"].as_str()),
                "case {name:?}: the property path must start where the composite model's does, \
                 got {property} against {path:?}"
            );
        }

        // The summaries are the composite models nested DIRECTLY inside this one.
        let mut summarised: Vec<String> = described["assetCompositeModelSummaries"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {name:?}: no assetCompositeModelSummaries in {described}")
            })
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or("").to_string())
            .collect();
        summarised.sort();
        let mut expected = case.children.clone();
        expected.sort();
        assert_eq!(
            summarised, expected,
            "case {name:?}: the nested composite models did not read back, got {described}"
        );

        // The asset's view and the model's view of one stored composite model
        // report the same property ids: an asset property is the instance of the
        // definition it was materialised from.
        let (status, model_side) = describe_composite(&router, &model_id, &case.label).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeAssetModelCompositeModel got {status} {model_side}"
        );
        assert_eq!(
            property_ids(&described, "assetCompositeModelProperties"),
            property_ids(&model_side, "assetModelCompositeModelProperties"),
            "case {name:?}: the asset view and the model view disagree about the property ids: \
             {described} against {model_side}"
        );

        // A read changes nothing a later caller sees.
        let (status, again) = describe_asset_composite(&router, &asset_id, &case.label).await;
        assert_eq!(status, 200, "case {name:?}: the second read got {status} {again}");
        assert_eq!(
            again, described,
            "case {name:?}: two reads of one composite model must agree"
        );
    }

    // The asset is addressable by its own `externalId:` label too, and both
    // label forms name the same instance.
    let (status, external_asset) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "Boiler2",
            "assetModelId": model_id,
            "assetExternalId": "boiler_two",
        })),
    )
    .await;
    assert_eq!(status, 202, "create Boiler2: got {status} {external_asset}");
    let (status, by_label) =
        describe_asset_composite(&router, "externalId:boiler_two", &custom).await;
    assert_eq!(status, 200, "the asset's externalId label: got {status} {by_label}");
    assert_eq!(
        by_label["assetId"].as_str(),
        external_asset["assetId"].as_str(),
        "the externalId label must name the asset that carries it, got {by_label}"
    );
}

#[tokio::test]
async fn a_label_that_names_no_composite_model_of_the_asset_is_a_modelled_refusal() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let custom = composite_id(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;

    // A second model whose composite model is NOT one of the first asset's: a
    // composite model is addressed through the asset that instantiates it.
    let other_model_id = plain_model(&router, "Pump").await;
    let other_composite = composite_id(
        &router,
        &other_model_id,
        json!({
            "assetModelCompositeModelName": "Other",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;

    let absent = "99999999-8888-4777-8666-555555555555";
    // Rows span both members, both label forms a `CustomID` takes, and a
    // composite model that exists but belongs to another asset's model. The last
    // column is the identifier the refusal must echo: the one the request
    // supplied for the resource that resolved to nothing.
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "an asset id that resolves to nothing",
            absent.to_string(),
            custom.clone(),
            absent.to_string(),
        ),
        (
            // An asset addressed by the selector failed to resolve an EXTERNAL
            // ID, which the refusal reports by the bare external id — the
            // selector is how the request named the dimension, not its value.
            "an asset externalId label that resolves to nothing",
            "externalId:no_such_asset".to_string(),
            custom.clone(),
            "no_such_asset".to_string(),
        ),
        (
            "a composite model id that resolves to nothing",
            asset_id.clone(),
            absent.to_string(),
            absent.to_string(),
        ),
        (
            "a composite model externalId label that resolves to nothing",
            asset_id.clone(),
            "externalId:no_such_composite".to_string(),
            "externalId:no_such_composite".to_string(),
        ),
        (
            "a composite model of another asset's model",
            asset_id.clone(),
            other_composite.clone(),
            other_composite.clone(),
        ),
    ];
    for (case, asset_label, composite_label, requested) in &cases {
        let (status, body) = describe_asset_composite(&router, asset_label, composite_label).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        let message = body["message"].as_str().unwrap_or("");
        assert!(
            message.contains(requested.as_str()),
            "case {case:?}: the refusal must name the identifier the request supplied \
             ({requested:?}), got {body}"
        );
    }
}
