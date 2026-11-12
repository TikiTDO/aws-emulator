// SPDX-License-Identifier: CC0-1.0

//! The members every read of an asset or an asset model DERIVES, driven end to
//! end over HTTP through the same `MultiServiceRouter` the emulator boots with.
//!
//! Three rules, each asserted over a table whose rows are what the rule
//! generalises over:
//!
//! 1. A structured `path` runs from the ROOT of the resource down to the thing it
//!    addresses: the asset (or the asset model) itself, then one hop per composite
//!    model containing it, then the property when the path names one.
//!    `AssetProperty$path` is "the structured path to the property from the root of
//!    the asset", `AssetModelProperty$path` the same from the root of the asset
//!    model, and an `AssetCompositeModelPathSegment` is "one level between a
//!    composite model and the root of the asset"
//!    (`smithy_query(query="shape",
//!    shape="AssetProperty,AssetModelProperty,AssetCompositeModelPathSegment")`).
//!    Rows: where the property sits (the resource's own definitions, a composite
//!    model's inline ones, the ones of a component model a composite model reuses,
//!    a composite model nested inside another) crossed with every read that
//!    reports it, and a rename of the root, which the path then carries.
//! 2. Every reported asset property carries its notification block — both members
//!    of `PropertyNotification` are `@required` — with the MQTT topic IoT SiteWise
//!    publishes that property's updates to and the state last written, `DISABLED`
//!    by default. Rows: a property no write has touched, one enabled, one whose
//!    write omitted the state, read back through every reader.
//! 3. The composite-model collections are ALWAYS serialized, and each immediate
//!    child is reported ONCE: `assetModelCompositeModels` / `assetCompositeModels`
//!    is "the list of built-in composite models for the asset model, such as those
//!    with those of type AWS/ALARMS" and the summaries member "the list of the
//!    immediate child custom composite model summaries"
//!    (`smithy_query(query="doc", shape="DescribeAssetModelResponse",
//!    member="assetModelCompositeModels")`). Rows: a resource with no composite
//!    model at all, one with a custom child, one with a built-in child, one with
//!    both, and one whose only child was deleted.
//!
//! The composite-model types come from the `assetModelCompositeModelType`
//! documentation ("Valid values are AWS/ALARM, CUSTOM, or AWS/L4E_ANOMALY") and
//! the statuses from `smithy_query(query="operation",
//! operation="CreateAssetModel,CreateAsset,DescribeAsset,DescribeAssetModel")`:
//! the creates answer 202 and the reads 200.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, router};
use crate::asset_model_composite_model::{create_composite, PAST_THE_OWED_TRANSITION};
use crate::asset_property_update::expected_topic;

/// The asset model the path table is built on, and the asset instantiated from it.
struct Fixture {
    model_id: String,
    model_name: &'static str,
    asset_id: String,
    asset_name: &'static str,
    /// `(id, name)` of every composite model of the model, in creation order.
    custom: (String, &'static str),
    alarm: (String, &'static str),
    reused: (String, &'static str),
    nested: (String, &'static str),
}

/// One row of the path table: a property, and the composite-model chain it sits
/// under (empty for one of the resource's own definitions).
struct PathRow {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The chain of composite models from the outermost ancestor down to the one
    /// that contributes the property, as `(id, name)`.
    chain: Vec<(String, String)>,
    /// The property's name.
    property: &'static str,
}

/// A DOUBLE measurement definition.
fn measurement(name: &str) -> Value {
    json!({"name": name, "dataType": "DOUBLE", "type": {"measurement": {}}})
}

/// Create an asset model from `body` and answer its id.
async fn create_model(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModel got {status} {created}"
    );
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
        .to_string()
}

/// Add one composite model to `model_id` and answer its id.
async fn composite(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    body: Value,
) -> String {
    let (status, created) = create_composite(router, model_id, body).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModelCompositeModel got {status} {created}"
    );
    created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no composite model id in {created}"))
        .to_string()
}

/// `GET` `uri`, asserting the read is served.
async fn read(router: &MultiServiceRouter, case: &str, uri: &str) -> Value {
    let (status, body) = call(router, "GET", uri, None).await;
    assert_eq!(status, 200, "case {case:?}: GET {uri} got {status} {body}");
    body
}

/// The entry named `name` in a property list.
fn by_name(properties: &Value, name: &str) -> Value {
    properties
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|property| property["name"] == json!(name))
        .unwrap_or_else(|| panic!("no property named {name} in {properties}"))
}

/// A reported `path` as `(id, name)` pairs, so two paths are compared as data.
fn segments(path: &Value) -> Vec<(String, String)> {
    path.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|segment| {
            (
                segment["id"].as_str().unwrap_or_default().to_string(),
                segment["name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `(id, name)` as a path hop.
fn hop(id: &str, name: &str) -> (String, String) {
    (id.to_string(), name.to_string())
}

/// The asset model, its composite models and one asset of it.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let component_id = create_model(
        router,
        "the reused component model",
        json!({
            "assetModelName": "Pump",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [measurement("Flow")],
        }),
    )
    .await;
    let model_id = create_model(
        router,
        "the host asset model",
        json!({
            "assetModelName": "Plant",
            "assetModelProperties": [measurement("Uptime")],
        }),
    )
    .await;

    let custom = composite(
        router,
        "a custom composite model with an inline definition",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Setpoints",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [measurement("Target")],
        }),
    )
    .await;
    let alarm = composite(
        router,
        "a built-in composite model",
        &model_id,
        json!({
            "assetModelCompositeModelName": "HighTemp",
            "assetModelCompositeModelType": "AWS/ALARM",
            "assetModelCompositeModelProperties": [measurement("Threshold")],
        }),
    )
    .await;
    let reused = composite(
        router,
        "a composite model reusing a component model",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &component_id,
        }),
    )
    .await;
    let nested = composite(
        router,
        "a composite model nested inside another",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Nested",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": &custom,
            "assetModelCompositeModelProperties": [measurement("Depth")],
        }),
    )
    .await;

    // The model has to have settled before an asset can instantiate it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let asset_id = create_asset(router, "Plant1", &model_id).await;

    Fixture {
        model_id,
        model_name: "Plant",
        asset_id,
        asset_name: "Plant1",
        custom: (custom, "Setpoints"),
        alarm: (alarm, "HighTemp"),
        reused: (reused, "Reused"),
        nested: (nested, "Nested"),
    }
}

/// The table: one row per place a property can sit in the tree.
fn path_rows(fixture: &Fixture) -> Vec<PathRow> {
    vec![
        PathRow {
            case: "one of the resource's own definitions",
            chain: Vec::new(),
            property: "Uptime",
        },
        PathRow {
            case: "a custom composite model's inline definition",
            chain: vec![hop(&fixture.custom.0, fixture.custom.1)],
            property: "Target",
        },
        PathRow {
            case: "a built-in composite model's definition",
            chain: vec![hop(&fixture.alarm.0, fixture.alarm.1)],
            property: "Threshold",
        },
        PathRow {
            case: "a definition of the component model a composite model reuses",
            chain: vec![hop(&fixture.reused.0, fixture.reused.1)],
            property: "Flow",
        },
        PathRow {
            case: "a definition of a composite model nested inside another",
            chain: vec![
                hop(&fixture.custom.0, fixture.custom.1),
                hop(&fixture.nested.0, fixture.nested.1),
            ],
            property: "Depth",
        },
    ]
}

/// RULE: every `path` a MODEL-side read reports runs from the asset model root
/// through the composite models that contain the property down to the property —
/// and every read of one property reports the same path.
#[tokio::test]
async fn every_model_side_path_runs_from_the_asset_model_root_down() {
    let router = router();
    let fixture = fixture(&router).await;
    let model_id = &fixture.model_id;

    let described = read(&router, "DescribeAssetModel", &format!("/asset-models/{model_id}")).await;
    let listed = read(
        &router,
        "ListAssetModelProperties",
        &format!("/asset-models/{model_id}/properties?filter=ALL&maxResults=250"),
    )
    .await;

    for row in path_rows(&fixture) {
        let case = row.case;
        // Where the definition is reported depends on the chain: the model's own
        // list for a base property, the composite-model read for a contributed one.
        let reported = match row.chain.last() {
            None => by_name(&described["assetModelProperties"], row.property),
            Some((composite_id, _)) => {
                let composite_read = read(
                    &router,
                    case,
                    &format!("/asset-models/{model_id}/composite-models/{composite_id}"),
                )
                .await;
                by_name(
                    &composite_read["assetModelCompositeModelProperties"],
                    row.property,
                )
            }
        };
        let property_id = reported["id"].as_str().unwrap_or_default().to_string();
        let mut expected = vec![hop(model_id, fixture.model_name)];
        expected.extend(row.chain.iter().cloned());
        expected.push(hop(&property_id, row.property));
        assert_eq!(
            segments(&reported["path"]),
            expected,
            "case {case:?}: expected the path {expected:?}, got {reported}"
        );

        // The listing of the same definition reports the same path.
        let summary = by_name(&listed["assetModelPropertySummaries"], row.property);
        assert_eq!(
            segments(&summary["path"]),
            expected,
            "case {case:?}: the listing must report the path the describe does, got {summary}"
        );

        // So does the composite model's own path, which is the property's path
        // without its last hop.
        if let Some((composite_id, _)) = row.chain.last() {
            let composite_read = read(
                &router,
                case,
                &format!("/asset-models/{model_id}/composite-models/{composite_id}"),
            )
            .await;
            assert_eq!(
                segments(&composite_read["assetModelCompositeModelPath"]),
                expected[..expected.len() - 1].to_vec(),
                "case {case:?}: the composite model's own path must be the property's path \
                 without the property, got {composite_read}"
            );
        }
    }
}

/// RULE: every `path` an ASSET-side read reports runs from the asset root through
/// the composite models that contain the property down to the property, whichever
/// read reports it.
#[tokio::test]
async fn every_asset_side_path_runs_from_the_asset_root_down() {
    let router = router();
    let fixture = fixture(&router).await;
    let asset_id = &fixture.asset_id;

    let described = read(&router, "DescribeAsset", &format!("/assets/{asset_id}")).await;
    let listed = read(
        &router,
        "ListAssetProperties",
        &format!("/assets/{asset_id}/properties?filter=ALL&maxResults=250"),
    )
    .await;

    for row in path_rows(&fixture) {
        let case = row.case;
        let reported = match row.chain.last() {
            None => by_name(&described["assetProperties"], row.property),
            Some((composite_id, _)) => {
                let instance = read(
                    &router,
                    case,
                    &format!("/assets/{asset_id}/composite-models/{composite_id}"),
                )
                .await;
                by_name(&instance["assetCompositeModelProperties"], row.property)
            }
        };
        let property_id = reported["id"].as_str().unwrap_or_default().to_string();
        let mut expected = vec![hop(asset_id, fixture.asset_name)];
        expected.extend(row.chain.iter().cloned());
        expected.push(hop(&property_id, row.property));
        assert_eq!(
            segments(&reported["path"]),
            expected,
            "case {case:?}: expected the path {expected:?}, got {reported}"
        );

        // The listing reports one summary per materialised instance of a property,
        // addressed by id, and every one of them carries that same path.
        let summary = listed["assetPropertySummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| {
                summary["id"] == json!(&property_id)
                    && segments(&summary["path"]) == expected
            });
        assert!(
            summary.is_some(),
            "case {case:?}: the listing must report the path {expected:?} for {property_id}, \
             got {listed}"
        );

        // DescribeAssetProperty reports the same path, under `assetProperty` for a
        // base property and under `compositeModel` for a contributed one.
        let one = read(
            &router,
            case,
            &format!("/assets/{asset_id}/properties/{property_id}"),
        )
        .await;
        let property = if row.chain.is_empty() {
            one["assetProperty"].clone()
        } else {
            one["compositeModel"]["assetProperty"].clone()
        };
        assert_eq!(
            segments(&property["path"]),
            expected,
            "case {case:?}: DescribeAssetProperty must report the same path, got {one}"
        );

        // And the asset's view of the composite model carries the path without the
        // property hop.
        if let Some((composite_id, _)) = row.chain.last() {
            let instance = read(
                &router,
                case,
                &format!("/assets/{asset_id}/composite-models/{composite_id}"),
            )
            .await;
            assert_eq!(
                segments(&instance["assetCompositeModelPath"]),
                expected[..expected.len() - 1].to_vec(),
                "case {case:?}: the composite model's own path must be the property's path \
                 without the property, got {instance}"
            );
        }
    }
}

/// RULE (the same one, as the root MOVES): a path is derived from the resource as
/// it stands, so renaming the asset or the asset model changes the root hop every
/// later read reports.
#[tokio::test]
async fn a_renamed_root_is_the_root_hop_every_path_reports() {
    let router = router();
    let fixture = fixture(&router).await;
    let (model_id, asset_id) = (&fixture.model_id, &fixture.asset_id);
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // The asset model keeps its definitions — a property resubmitted to be KEPT is
    // named by its stored id — and takes a new name.
    let kept = crate::support::named_by_stored_id(&router, model_id, json!([measurement("Uptime")]))
        .await;
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(json!({"assetModelName": "Renamed Plant", "assetModelProperties": kept})),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {updated}");
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/assets/{asset_id}"),
        Some(json!({"assetName": "Renamed Plant1"})),
    )
    .await;
    assert_eq!(status, 202, "UpdateAsset: got {status} {updated}");
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // Rows: the two resources, each with the read that reports a path of its own
    // and the new name that read's root hop must carry.
    let rows = [
        (
            "the asset model",
            format!("/asset-models/{model_id}"),
            "assetModelProperties",
            model_id.as_str(),
            "Renamed Plant",
        ),
        (
            "the asset",
            format!("/assets/{asset_id}"),
            "assetProperties",
            asset_id.as_str(),
            "Renamed Plant1",
        ),
    ];
    for (case, uri, member, root_id, root_name) in rows {
        let body = read(&router, case, &uri).await;
        let property = by_name(&body[member], "Uptime");
        assert_eq!(
            segments(&property["path"]).first().cloned(),
            Some(hop(root_id, root_name)),
            "case {case:?}: the root hop must carry the name the resource now has, got \
             {property}"
        );
    }
}

/// RULE: every asset property reports its notification block — the MQTT topic its
/// value updates publish to and the state last written, `DISABLED` until one is —
/// through every read that reports the property.
#[tokio::test]
async fn every_reported_asset_property_carries_its_notification_block() {
    let router = router();
    let fixture = fixture(&router).await;
    let (model_id, asset_id) = (&fixture.model_id, &fixture.asset_id);

    let described = read(&router, "DescribeAsset", &format!("/assets/{asset_id}")).await;
    let uptime = by_name(&described["assetProperties"], "Uptime")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let instance = read(
        &router,
        "DescribeAssetCompositeModel",
        &format!("/assets/{asset_id}/composite-models/{}", fixture.custom.0),
    )
    .await;
    let target = by_name(&instance["assetCompositeModelProperties"], "Target")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: the state the property ends up in and how it got there — never
    // written, written ENABLED, written with the state omitted (the documented
    // default), and a composite model's property, which is written the same way.
    let rows: Vec<(&str, String, Option<Value>, &str)> = vec![
        ("a property no write has touched", uptime.clone(), None, "DISABLED"),
        (
            "a property enabled by a write",
            uptime.clone(),
            Some(json!({"propertyNotificationState": "ENABLED"})),
            "ENABLED",
        ),
        (
            "a write that omitted the state",
            uptime.clone(),
            Some(json!({})),
            "DISABLED",
        ),
        (
            "a composite model's property, enabled by a write",
            target.clone(),
            Some(json!({"propertyNotificationState": "ENABLED"})),
            "ENABLED",
        ),
    ];

    for (case, property_id, write, state) in rows {
        if let Some(body) = write {
            let (status, answer) = call(
                &router,
                "PUT",
                &format!("/assets/{asset_id}/properties/{property_id}"),
                Some(body),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateAssetProperty got {status} {answer}"
            );
        }
        let topic = expected_topic(model_id, asset_id, &property_id);

        // Every reader of that property: the asset, the asset's view of the
        // composite model that holds it, the listing, and the property's own read.
        let described = read(&router, case, &format!("/assets/{asset_id}")).await;
        let listed = read(
            &router,
            case,
            &format!("/assets/{asset_id}/properties?filter=ALL&maxResults=250"),
        )
        .await;
        let one = read(
            &router,
            case,
            &format!("/assets/{asset_id}/properties/{property_id}"),
        )
        .await;
        let instance = read(
            &router,
            case,
            &format!("/assets/{asset_id}/composite-models/{}", fixture.custom.0),
        )
        .await;
        let mut reported: Vec<(&str, Value)> = vec![
            ("ListAssetProperties", {
                listed["assetPropertySummaries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|summary| summary["id"] == json!(&property_id))
                    .unwrap_or_else(|| panic!("case {case:?}: not listed in {listed}"))
            }),
            ("DescribeAssetProperty", {
                let property = &one["assetProperty"];
                if property.is_null() {
                    one["compositeModel"]["assetProperty"].clone()
                } else {
                    property.clone()
                }
            }),
        ];
        if property_id == uptime {
            reported.push((
                "DescribeAsset",
                by_name(&described["assetProperties"], "Uptime"),
            ));
        } else {
            reported.push((
                "DescribeAssetCompositeModel",
                by_name(&instance["assetCompositeModelProperties"], "Target"),
            ));
        }

        for (reader, property) in reported {
            assert_eq!(
                property["notification"]["state"].as_str(),
                Some(state),
                "case {case:?}/{reader}: expected notification state {state:?}, got {property}"
            );
            assert_eq!(
                property["notification"]["topic"].as_str(),
                Some(topic.as_str()),
                "case {case:?}/{reader}: expected the topic {topic:?}, got {property}"
            );
        }
    }
}

/// One row of the collection table: the immediate children a fresh asset model
/// is given, and what then happens to the last of them.
struct CollectionRow {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// `(name, assetModelCompositeModelType)` of each composite model created,
    /// in order.
    children: Vec<(&'static str, &'static str)>,
    /// Whether the LAST child is nested inside the one created before it, and so
    /// is an immediate child of no asset model.
    nest_last: bool,
    /// Whether the last top-level child is deleted again once created.
    delete_last: bool,
}

/// RULE: both composite-model collections are always serialized, and each
/// immediate child is reported ONCE — a built-in composite model in
/// `assetModelCompositeModels` / `assetCompositeModels`, a custom one among the
/// summaries.
#[tokio::test]
async fn the_composite_model_collections_are_serialized_and_partitioned_by_kind() {
    let router = router();

    // Rows: how many immediate children of which kind the model holds, including
    // none at all, a nested child (which neither member names) and a child
    // deleted again.
    let cases = vec![
        CollectionRow {
            case: "no composite model at all",
            children: Vec::new(),
            nest_last: false,
            delete_last: false,
        },
        CollectionRow {
            case: "one custom child",
            children: vec![("Setpoints", "CUSTOM")],
            nest_last: false,
            delete_last: false,
        },
        CollectionRow {
            case: "one built-in child",
            children: vec![("HighTemp", "AWS/ALARM")],
            nest_last: false,
            delete_last: false,
        },
        CollectionRow {
            case: "one child of each kind",
            children: vec![("Setpoints", "CUSTOM"), ("Anomaly", "AWS/L4E_ANOMALY")],
            nest_last: false,
            delete_last: false,
        },
        CollectionRow {
            case: "a child nested inside another",
            children: vec![("Setpoints", "CUSTOM"), ("Nested", "CUSTOM")],
            nest_last: true,
            delete_last: false,
        },
        CollectionRow {
            case: "the only child deleted again",
            children: vec![("Setpoints", "CUSTOM")],
            nest_last: false,
            delete_last: true,
        },
    ];

    for row in cases {
        let CollectionRow {
            case,
            children,
            nest_last,
            delete_last,
        } = row;
        let model_id = create_model(
            &router,
            case,
            json!({
                "assetModelName": format!("Model for {case}"),
                "assetModelProperties": [measurement("Uptime")],
            }),
        )
        .await;
        let mut created: Vec<(String, &str, &str)> = Vec::new();
        for (at, (name, composite_type)) in children.iter().enumerate() {
            let mut body = json!({
                "assetModelCompositeModelName": name,
                "assetModelCompositeModelType": composite_type,
            });
            let nested = nest_last && at + 1 == children.len();
            if nested {
                body["parentAssetModelCompositeModelId"] = json!(created[at - 1].0);
            }
            let id = composite(&router, case, &model_id, body).await;
            if !nested {
                created.push((id, name, composite_type));
            }
        }
        if delete_last {
            let (id, _, _) = created.pop().expect("a composite model to delete");
            let (status, body) = call(
                &router,
                "DELETE",
                &format!("/asset-models/{model_id}/composite-models/{id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteAssetModelCompositeModel got {status} {body}"
            );
        }
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let asset_id = create_asset(&router, &format!("Asset for {case}"), &model_id).await;

        let expected = |built_in: bool| -> Vec<String> {
            created
                .iter()
                .filter(|(_, _, composite_type)| (*composite_type != "CUSTOM") == built_in)
                .map(|(id, _, _)| id.clone())
                .collect()
        };
        let ids = |body: &Value, member: &str| -> Vec<String> {
            body[member]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {member} must be serialized, got {body}")
                })
                .iter()
                .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
                .collect()
        };

        let model = read(&router, case, &format!("/asset-models/{model_id}")).await;
        assert_eq!(
            ids(&model, "assetModelCompositeModels"),
            expected(true),
            "case {case:?}: assetModelCompositeModels must report the built-in immediate \
             children, got {model}"
        );
        assert_eq!(
            ids(&model, "assetModelCompositeModelSummaries"),
            expected(false),
            "case {case:?}: assetModelCompositeModelSummaries must report the custom \
             immediate children, got {model}"
        );

        // The asset instantiated from it reports the same partition of the same
        // composite models.
        let asset = read(&router, case, &format!("/assets/{asset_id}")).await;
        assert_eq!(
            ids(&asset, "assetCompositeModels"),
            expected(true),
            "case {case:?}: assetCompositeModels must report the built-in immediate \
             children, got {asset}"
        );
        assert_eq!(
            ids(&asset, "assetCompositeModelSummaries"),
            expected(false),
            "case {case:?}: assetCompositeModelSummaries must report the custom immediate \
             children, got {asset}"
        );
    }
}
