// SPDX-License-Identifier: CC0-1.0

//! The two MODEL-SIDE views of one stored composite model, driven end to end over
//! HTTP through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule under test: the property definitions a composite model contributes are
//! read from ONE place — its own inline definitions, or, when it reuses a
//! `COMPONENT_MODEL`, that model's stored properties — so the list
//! `DescribeAssetModel` reports for a BUILT-IN composite model under
//! `assetModelCompositeModels` and the list `DescribeAssetModelCompositeModel`
//! reports for the same composite model are
//! the same definitions, member for member and in the same order, however the
//! composite model came by them and whenever its source last changed.
//!
//! Asserted over a TABLE whose rows are what the rule generalises over: where the
//! definitions come from (inline, a reused component model, a reused component model
//! named by its `externalId:` label), how many there are (none, one, several) and of
//! which data types — crossed with a second phase in which the reused component
//! models' definitions CHANGE after the composite models were created, so a view
//! that copied them at create time disagrees with one that reads them.
//!
//! The `type` / `dataType` values written out here come from
//! `smithy_query(query="shape", shape="PropertyType,PropertyDataType")`, the
//! composite-model types from `CreateAssetModelCompositeModel`'s
//! `assetModelCompositeModelType` documentation, and the statuses from
//! `smithy_query(query="operation",
//! operation="CreateAssetModel,CreateAssetModelCompositeModel,DescribeAssetModel")`
//! on this crate's `smithy-model/`: the creates answer 202 and the reads 200.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, router};
use crate::asset_model_composite_model::{
    create_composite, describe_composite, PAST_THE_OWED_TRANSITION,
};

/// The `externalId` the aliased component model carries, so one row names its
/// reused model by the `externalId:<id>` label rather than by its UUID.
const ALIASED_COMPONENT_EXTERNAL_ID: &str = "views_aliased_component";

/// A DOUBLE measurement definition.
fn measurement(name: &str, unit: &str) -> Value {
    json!({"name": name, "dataType": "DOUBLE", "unit": unit, "type": {"measurement": {}}})
}

/// A STRING attribute definition.
fn attribute(name: &str, default_value: &str) -> Value {
    json!({
        "name": name,
        "dataType": "STRING",
        "type": {"attribute": {"defaultValue": default_value}},
    })
}

/// One row of the table: a composite model of the fixture's asset model, with the
/// property names BOTH views of it must report.
struct Row {
    /// What the row varies, named in every assertion it drives.
    case: &'static str,
    /// The stored composite model's id.
    id: String,
    /// The names both views must report, in order.
    expected: Vec<String>,
    /// Whether the asset model holds this composite model DIRECTLY, and so reports
    /// it in one of its two composite-model members.
    top_level: bool,
    /// Whether its type is a BUILT-IN one, which decides WHICH of those two
    /// members reports it.
    built_in: bool,
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

/// A `COMPONENT_MODEL` carrying `properties`, with an `externalId` when one is given.
async fn component_model(
    router: &MultiServiceRouter,
    name: &str,
    external_id: Option<&str>,
    properties: Value,
) -> String {
    let mut body = json!({
        "assetModelName": name,
        "assetModelType": "COMPONENT_MODEL",
        "assetModelProperties": properties,
    });
    if let Some(external_id) = external_id {
        body["assetModelExternalId"] = json!(external_id);
    }
    create_model(router, name, body).await
}

/// Add one composite model to `model_id` and answer its id.
async fn composite(router: &MultiServiceRouter, case: &str, model_id: &str, body: Value) -> String {
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

/// `PUT /asset-models/{component_id}` rewriting a component model's definitions.
async fn rewrite_component_model(
    router: &MultiServiceRouter,
    name: &str,
    component_id: &str,
    properties: Value,
) {
    // A property this component model already holds is resubmitted by its stored
    // id, so the rewrite KEEPS it instead of asking for the replacement
    // `UpdateAssetModel` refuses; see `crate::support::named_by_stored_id`.
    let properties = crate::support::named_by_stored_id(router, component_id, properties).await;
    let (status, body) = call(
        router,
        "PUT",
        &format!("/asset-models/{component_id}"),
        Some(json!({
            "assetModelName": name,
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "UpdateAssetModel({name}): got {status} {body}"
    );
}

/// The names of a reported definition list, in the order it reports them. A list
/// a view does not carry at all is no definitions, which is what an empty one is.
fn names(properties: &Value) -> Vec<String> {
    properties
        .as_array()
        .map(|properties| {
            properties
                .iter()
                .map(|property| property["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A reported definition list as entries, so two views can be compared member for
/// member. Absent is the same answer as empty.
fn definitions(properties: &Value) -> Vec<Value> {
    properties.as_array().cloned().unwrap_or_default()
}

/// The ids a `DescribeAssetModel` body reports under `member`, in the order the
/// body lists them.
fn reported_ids(described: &Value, member: &str) -> Vec<String> {
    described[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Names the fixture's component models carry, so one update can be written
/// against them from either phase of a test.
struct Components {
    /// The component model reused by the one-property row.
    single: String,
    /// The component model reused by the several-properties row.
    several: String,
    /// The component model with no properties of its own.
    bare: String,
    /// The component model reused through its `externalId:` label.
    aliased: String,
}

/// An asset model holding one composite model per row of the table, together with
/// the component models those rows reuse.
async fn fixture(router: &MultiServiceRouter) -> (String, Vec<Row>, Components) {
    let single = component_model(router, "Pump", None, json!([measurement("Flow", "m3/s")])).await;
    let several = component_model(
        router,
        "Motor",
        None,
        json!([
            measurement("Speed", "RPM"),
            attribute("Serial", "unset"),
            measurement("Temperature", "Celsius"),
        ]),
    )
    .await;
    let bare = component_model(router, "BareComponent", None, json!([])).await;
    let aliased = component_model(
        router,
        "Gearbox",
        Some(ALIASED_COMPONENT_EXTERNAL_ID),
        json!([measurement("Torque", "Nm")]),
    )
    .await;

    let model_id = create_model(
        router,
        "fixture",
        json!({
            "assetModelName": "Plant",
            "assetModelProperties": [measurement("Uptime", "s")],
        }),
    )
    .await;

    let inline_one = composite(
        router,
        "inline definitions, one of them",
        &model_id,
        json!({
            "assetModelCompositeModelName": "InlineOne",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [attribute("Setpoint", "20")],
        }),
    )
    .await;
    let inline_several = composite(
        router,
        "inline definitions, several of them",
        &model_id,
        json!({
            "assetModelCompositeModelName": "InlineSeveral",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                measurement("Low", "bar"),
                measurement("High", "bar"),
                attribute("Label", "none"),
            ],
        }),
    )
    .await;
    let reused_one = composite(
        router,
        "a reused component model with one definition",
        &model_id,
        json!({
            "assetModelCompositeModelName": "ReusedOne",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &single,
        }),
    )
    .await;
    let reused_several = composite(
        router,
        "a reused component model with several definitions",
        &model_id,
        json!({
            "assetModelCompositeModelName": "ReusedSeveral",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &several,
        }),
    )
    .await;
    let reused_bare = composite(
        router,
        "a reused component model with no definitions",
        &model_id,
        json!({
            "assetModelCompositeModelName": "ReusedBare",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &bare,
        }),
    )
    .await;
    let reused_by_label = composite(
        router,
        "a component model reused by its externalId label",
        &model_id,
        json!({
            "assetModelCompositeModelName": "ReusedByLabel",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": format!("externalId:{ALIASED_COMPONENT_EXTERNAL_ID}"),
        }),
    )
    .await;
    let without_definitions = composite(
        router,
        "no definitions at all",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Bare",
            "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
        }),
    )
    .await;
    // Nested inside another composite model, so the asset model does not hold it
    // directly: the two views still have to agree about what it contributes.
    let nested = composite(
        router,
        "a reused component model nested inside another composite model",
        &model_id,
        json!({
            "assetModelCompositeModelName": "NestedReused",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": &inline_one,
            "composedAssetModelId": &several,
        }),
    )
    .await;

    let rows = vec![
        Row {
            case: "inline definitions, one of them",
            id: inline_one,
            expected: vec!["Setpoint".to_string()],
            top_level: true,
            built_in: false,
        },
        Row {
            case: "inline definitions, several of them",
            id: inline_several,
            expected: vec!["Low".to_string(), "High".to_string(), "Label".to_string()],
            top_level: true,
            built_in: false,
        },
        Row {
            case: "a reused component model with one definition",
            id: reused_one,
            expected: vec!["Flow".to_string()],
            top_level: true,
            built_in: false,
        },
        Row {
            case: "a reused component model with several definitions",
            id: reused_several,
            expected: vec![
                "Speed".to_string(),
                "Serial".to_string(),
                "Temperature".to_string(),
            ],
            top_level: true,
            built_in: false,
        },
        Row {
            case: "a reused component model with no definitions",
            id: reused_bare,
            expected: Vec::new(),
            top_level: true,
            built_in: false,
        },
        Row {
            case: "a component model reused by its externalId label",
            id: reused_by_label,
            expected: vec!["Torque".to_string()],
            top_level: true,
            built_in: false,
        },
        Row {
            case: "no definitions at all",
            id: without_definitions,
            expected: Vec::new(),
            top_level: true,
            built_in: true,
        },
        Row {
            case: "a reused component model nested inside another composite model",
            id: nested,
            expected: vec![
                "Speed".to_string(),
                "Serial".to_string(),
                "Temperature".to_string(),
            ],
            top_level: false,
            built_in: false,
        },
    ];
    (
        model_id,
        rows,
        Components {
            single,
            several,
            bare,
            aliased,
        },
    )
}

/// Assert, for every row, that the two model-side views of its composite model
/// report the same definitions, and that those are the ones the row expects.
async fn assert_views_agree(
    router: &MultiServiceRouter,
    model_id: &str,
    rows: &[Row],
    phase: &str,
) {
    let (status, described) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "phase {phase:?}: DescribeAssetModel got {status} {described}"
    );

    for row in rows {
        let case = row.case;
        let (status, composite_view) = describe_composite(router, model_id, &row.id).await;
        assert_eq!(
            status, 200,
            "phase {phase:?}, case {case:?}: DescribeAssetModelCompositeModel got {status} \
             {composite_view}"
        );
        let from_composite_read = &composite_view["assetModelCompositeModelProperties"];
        assert_eq!(
            names(from_composite_read),
            row.expected,
            "phase {phase:?}, case {case:?}: the composite-model read must report the \
             definitions the composite model contributes, got {composite_view}"
        );

        // `assetModelCompositeModels` is "the list of built-in composite models
        // for the asset model", so a CUSTOM composite model is not reported there
        // at all: its own view is the one above, and `DescribeAssetModel` names it
        // among the custom summaries.
        let entry = described["assetModelCompositeModels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|composite| composite["id"] == json!(row.id));
        let Some(entry) = entry else {
            assert!(
                !(row.top_level && row.built_in),
                "phase {phase:?}, case {case:?}: a built-in composite model the asset model \
                 holds directly must be reported in assetModelCompositeModels, got {described}"
            );
            continue;
        };
        assert!(
            row.top_level && row.built_in,
            "phase {phase:?}, case {case:?}: only a built-in composite model the asset model \
             holds directly belongs there, got {entry}"
        );
        assert_eq!(
            definitions(&entry["properties"]),
            definitions(from_composite_read),
            "phase {phase:?}, case {case:?}: the two views of one composite model must \
             report the same definitions ({entry} vs {composite_view})"
        );
    }
}

/// RULE: both model-side views of a composite model — the entry
/// `DescribeAssetModel` reports for it and `DescribeAssetModelCompositeModel`'s own
/// answer — report the definitions it contributes, whether those are its inline
/// ones or the properties of the component model it reuses.
#[tokio::test]
async fn the_two_model_side_views_of_a_composite_model_report_the_same_definitions() {
    let router = router();
    let (model_id, rows, _components) = fixture(&router).await;
    assert_views_agree(&router, &model_id, &rows, "as created").await;
}

/// RULE (the same one, from the other side): the asset model reports each
/// composite model it holds DIRECTLY exactly ONCE — a built-in one in
/// `assetModelCompositeModels`, a custom one in
/// `assetModelCompositeModelSummaries` — and neither member names a nested one.
#[tokio::test]
async fn both_composite_model_members_report_the_immediate_children() {
    let router = router();
    let (model_id, rows, _components) = fixture(&router).await;

    let (status, described) =
        call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");

    for (member, built_in) in [
        ("assetModelCompositeModels", true),
        ("assetModelCompositeModelSummaries", false),
    ] {
        let expected: Vec<String> = rows
            .iter()
            .filter(|row| row.top_level && row.built_in == built_in)
            .map(|row| row.id.clone())
            .collect();
        assert_eq!(
            reported_ids(&described, member),
            expected,
            "case {member:?}: the member must name every composite model of its kind the \
             asset model holds directly, and only those, got {described}"
        );
    }
}

/// RULE: what a component-model-based composite model contributes is read from the
/// `COMPONENT_MODEL`'s stored record rather than copied when the composite model was
/// created, so a change to that component model shows in BOTH views at once.
#[tokio::test]
async fn the_views_agree_after_a_reused_component_models_definitions_change() {
    let router = router();
    let (model_id, mut rows, components) = fixture(&router).await;
    assert_views_agree(&router, &model_id, &rows, "as created").await;

    // A definition added to one component model, one removed from another, a
    // component model that had none given one, and one left exactly as it was.
    rewrite_component_model(
        &router,
        "Pump",
        &components.single,
        json!([measurement("Flow", "m3/s"), measurement("Pressure", "bar")]),
    )
    .await;
    rewrite_component_model(
        &router,
        "Motor",
        &components.several,
        json!([measurement("Speed", "RPM"), attribute("Serial", "unset")]),
    )
    .await;
    rewrite_component_model(
        &router,
        "BareComponent",
        &components.bare,
        json!([measurement("Vibration", "mm/s")]),
    )
    .await;
    rewrite_component_model(
        &router,
        "Gearbox",
        &components.aliased,
        json!([measurement("Torque", "Nm")]),
    )
    .await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    for row in &mut rows {
        row.expected = match row.case {
            "a reused component model with one definition" => {
                vec!["Flow".to_string(), "Pressure".to_string()]
            }
            "a reused component model with several definitions"
            | "a reused component model nested inside another composite model" => {
                vec!["Speed".to_string(), "Serial".to_string()]
            }
            "a reused component model with no definitions" => vec!["Vibration".to_string()],
            _ => row.expected.clone(),
        };
    }
    assert_views_agree(&router, &model_id, &rows, "after the component models changed").await;
}
