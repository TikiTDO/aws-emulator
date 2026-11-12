// SPDX-License-Identifier: CC0-1.0

//! What a stored asset-model formula reports its variables as pointing at.
//!
//! The rule these suites pin: `VariableValue.propertyId` and
//! `VariableValue.hierarchyId` each accept a NAME and an `externalId:` alias as
//! alternative INPUT spellings of an id
//! (`smithy_query(query="doc", shape="VariableValue", member="propertyId")`: "The
//! ID of the property to use as the variable. You can use the property `name` if
//! it's from the same asset model. If the property has an external ID, you can
//! specify `externalId:` followed by the external ID", and the same two spellings
//! on `hierarchyId`: "You can use the hierarchy's name instead of the hierarchy's
//! ID"), and every property and hierarchy of a written asset model carries a
//! server-assigned id — so an asset-model write RESOLVES each reference in the
//! scope it was checked in, and every read of the model reports the ID of the
//! member the reference names rather than the spelling the request used.
//!
//! Each suite is a table whose rows differ along what the rule generalises over:
//! which of the three spellings the reference uses, which member it points at
//! (one of the model's own properties, one of a child model's through a declared
//! hierarchy, a sibling inside one composite model's own list), which formula
//! member carries it (`metric` or `transform`), and which write stored it
//! (`CreateAssetModel`, `UpdateAssetModel`, `CreateAssetModelCompositeModel`,
//! `UpdateAssetModelCompositeModel`). A row whose variable carries no
//! `propertyId` at all — it addresses its property by `propertyPath` — is in the
//! table too, so resolution cannot pass by inventing an id. The NAME spelling is
//! one the model being written accepts alone ("if it's from the same asset
//! model"), so a reference through a hierarchy spells the child model's property
//! by id or alias; the refusal of a name there is pinned beside the other
//! scope-resolution refusals, in `super::asset_model_definition_validation`.
//!
//! The wire harness lives in `super::asset_lifecycle`, the composite-model
//! requests in `super::asset_model_composite_model`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, is_modelled_id, property_id, router};
use super::asset_model_composite_model::{create_composite, describe_composite};

/// The id this suite supplies for one property, so a row can spell a reference as
/// the RAW id of the property it names before anything has been created.
/// `AssetModelPropertyDefinition.id` is caller-suppliable, per
/// `smithy_query(query="shape", shape="AssetModelPropertyDefinition")`.
const PRESSURE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

/// The id this suite supplies for the one hierarchy it declares, for the same
/// reason [`PRESSURE_ID`] is supplied.
const HIERARCHY_ID: &str = "11111111-2222-4333-8444-555555555555";

/// The external id of the model's own `Power` measurement.
const POWER_EXTERNAL: &str = "ref_power";

/// The external id of the child model's `ChildPower` measurement.
const CHILD_POWER_EXTERNAL: &str = "ref_child_power";

/// The external id of the one hierarchy this suite declares.
const HIERARCHY_EXTERNAL: &str = "ref_turbines";

/// A measurement property definition, carrying the external id and the id a row
/// needs to spell a reference to it.
fn measurement(name: &str, external_id: Option<&str>, id: Option<&str>) -> Value {
    let mut property = json!({"name": name, "dataType": "DOUBLE", "type": {"measurement": {}}});
    if let Some(external_id) = external_id {
        property["externalId"] = json!(external_id);
    }
    if let Some(id) = id {
        property["id"] = json!(id);
    }
    property
}

/// A `metric` property definition over ONE variable named `v`, whose reference is
/// `value`.
fn metric(name: &str, value: Value) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"metric": {
            "expression": "avg(v)",
            "variables": [{"name": "v", "value": value}],
            "window": {"tumbling": {"interval": "5m"}},
        }},
    })
}

/// A `transform` property definition over ONE variable named `v`, whose reference
/// is `value`: the other formula member the same rule covers.
fn transform(name: &str, value: Value) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"transform": {
            "expression": "v * 2",
            "variables": [{"name": "v", "value": value}],
        }},
    })
}

/// `POST /asset-models` with `body`, answering the id of the model it created.
async fn create_model(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel: no assetModelId in {created}"))
        .to_string()
}

/// `GET /asset-models/{model_id}`, asserting it answered.
async fn describe_model(router: &MultiServiceRouter, model_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel {model_id}: got {status} {body}"
    );
    body
}

/// The property definitions a model reports.
async fn model_properties(router: &MultiServiceRouter, model_id: &str) -> Value {
    describe_model(router, model_id).await["assetModelProperties"].clone()
}

/// The ONE variable reference the property `name` of `properties` carries, as a
/// read reports it: `(propertyId, hierarchyId)`, each `None` when the member is
/// absent from the answer.
fn reference_of(properties: &Value, name: &str) -> (Option<String>, Option<String>) {
    let property = properties
        .as_array()
        .unwrap_or_else(|| panic!("expected a property list, got {properties}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("no property named {name} in {properties}"));
    let formula = &property["type"];
    let variables = if formula["metric"].is_object() {
        &formula["metric"]["variables"]
    } else {
        &formula["transform"]["variables"]
    };
    let value = &variables[0]["value"];
    (
        value["propertyId"].as_str().map(str::to_string),
        value["hierarchyId"].as_str().map(str::to_string),
    )
}

/// The name of the formula property one row of the create table contributes.
fn row_property(at: usize) -> String {
    format!("Ref{at}")
}

/// What a row's reference must read back as, once the write has minted the ids —
/// written down before the ids exist, resolved against the read back.
enum Expect {
    /// The id the model's own property of this name reports.
    Own(&'static str),
    /// The id the child model's own `ChildPower` reports, reached through the one
    /// declared hierarchy — so `hierarchyId` must read back as that hierarchy's
    /// id too.
    Child,
    /// No `propertyId` at all: the row addressed its property by path, so there
    /// is nothing to resolve and nothing may be invented.
    Nothing,
}

#[tokio::test]
async fn every_spelling_of_a_variable_reference_reads_back_as_the_resolved_id() {
    let router = router();

    // The child model one group of rows reaches through the declared hierarchy.
    let child = create_model(
        &router,
        json!({
            "assetModelName": "RefChild",
            "assetModelProperties": [measurement("ChildPower", Some(CHILD_POWER_EXTERNAL), None)],
        }),
    )
    .await;
    let child_power = property_id(&model_properties(&router, &child).await, "ChildPower");
    assert!(
        is_modelled_id(&child_power),
        "the child model's property must carry a minted id, got {child_power:?}"
    );

    // Rows differ along the spelling the reference uses and the scope it resolves
    // in; the formula member alternates with the row, so both `metric` and
    // `transform` carry every spelling group.
    let rows: Vec<(&str, Value, Expect)> = vec![
        (
            "a same-model property spelled by its name",
            json!({"propertyId": "Power"}),
            Expect::Own("Power"),
        ),
        (
            "a same-model property spelled by its externalId alias",
            json!({"propertyId": format!("externalId:{POWER_EXTERNAL}")}),
            Expect::Own("Power"),
        ),
        (
            "a same-model property spelled by its own id",
            json!({"propertyId": PRESSURE_ID}),
            Expect::Own("Pressure"),
        ),
        (
            "a child-model property by its own id, its hierarchy spelled by name",
            json!({"propertyId": child_power.clone(), "hierarchyId": "Turbines"}),
            Expect::Child,
        ),
        (
            "a child-model property and its hierarchy spelled by externalId alias",
            json!({
                "propertyId": format!("externalId:{CHILD_POWER_EXTERNAL}"),
                "hierarchyId": format!("externalId:{HIERARCHY_EXTERNAL}"),
            }),
            Expect::Child,
        ),
        (
            "a child-model property and its hierarchy spelled by their own ids",
            json!({"propertyId": child_power.clone(), "hierarchyId": HIERARCHY_ID}),
            Expect::Child,
        ),
        (
            "no propertyId at all, the property addressed by its path",
            json!({"propertyPath": [{"name": "Power"}]}),
            Expect::Nothing,
        ),
    ];

    let mut properties = vec![
        measurement("Power", Some(POWER_EXTERNAL), None),
        measurement("Pressure", None, Some(PRESSURE_ID)),
    ];
    for (at, (_, value, _)) in rows.iter().enumerate() {
        let name = row_property(at);
        properties.push(if at % 2 == 0 {
            metric(&name, value.clone())
        } else {
            transform(&name, value.clone())
        });
    }

    let plant = create_model(
        &router,
        json!({
            "assetModelName": "RefPlant",
            "assetModelProperties": properties,
            "assetModelHierarchies": [{
                "name": "Turbines",
                "id": HIERARCHY_ID,
                "externalId": HIERARCHY_EXTERNAL,
                "childAssetModelId": &child,
            }],
        }),
    )
    .await;

    let described = model_properties(&router, &plant).await;
    for (at, (case, _, expect)) in rows.iter().enumerate() {
        let name = row_property(at);
        let (property, hierarchy) = reference_of(&described, &name);
        let (expected_property, expected_hierarchy) = match expect {
            Expect::Own(target) => (Some(property_id(&described, target)), None),
            Expect::Child => (Some(child_power.clone()), Some(HIERARCHY_ID.to_string())),
            Expect::Nothing => (None, None),
        };
        assert_eq!(
            property, expected_property,
            "case {case:?}: expected propertyId {expected_property:?}, got {property:?}"
        );
        assert_eq!(
            hierarchy, expected_hierarchy,
            "case {case:?}: expected hierarchyId {expected_hierarchy:?}, got {hierarchy:?}"
        );
        if let Some(resolved) = property.as_deref() {
            assert!(
                is_modelled_id(resolved),
                "case {case:?}: a stored reference must be an id, not the spelling the \
                 request used, got {resolved:?}"
            );
        }
    }

    // The projection `ListAssetModelProperties` reports is the same stored
    // definition, so it reports the same resolved reference.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/asset-models/{plant}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListAssetModelProperties: got {status} {listed}"
    );
    let summaries = listed["assetModelPropertySummaries"].clone();
    let power = property_id(&described, "Power");
    let (summarised, _) = reference_of(&summaries, &row_property(0));
    assert_eq!(
        summarised,
        Some(power.clone()),
        "the summary of {} must report the resolved reference {power:?}, got {summarised:?}",
        row_property(0)
    );
}

/// What a row of the hierarchy-spelling table expects.
enum Spelled {
    /// The reference is accepted and stored as the child property's id.
    Resolved,
    /// The label is not an identifier at all, so it is an id invalid for that
    /// variable: the refusal names the label and the formula member carrying it.
    Invalid(&'static str),
    /// The label IS an identifier, but the child model holds no such property.
    Absent(&'static str),
}

#[tokio::test]
async fn a_reference_through_a_hierarchy_must_be_spelled_as_an_identifier() {
    let router = router();

    let child = create_model(
        &router,
        json!({
            "assetModelName": "SpelledChild",
            "assetModelProperties": [measurement("ChildPower", Some(CHILD_POWER_EXTERNAL), None)],
        }),
    )
    .await;
    let child_power = property_id(&model_properties(&router, &child).await, "ChildPower");

    // Rows differ along the spelling the reference uses and the formula member
    // carrying it, since the refusal names that member. The NAME spellings are
    // the ones the child model's scope does not accept — the child's own property
    // name and the writing model's, which is not in that scope at all — and the
    // accepted rows sit beside them, so the table cannot pass by refusing
    // everything.
    let rows: Vec<(&str, bool, Value, Spelled)> = vec![
        (
            "the child property's own name, on a metric",
            true,
            json!({"propertyId": "ChildPower", "hierarchyId": "Turbines"}),
            Spelled::Invalid("Property Id ChildPower invalid for variables in Metric property."),
        ),
        (
            "the writing model's own property name, on a transform",
            false,
            json!({"propertyId": "Power", "hierarchyId": "Turbines"}),
            Spelled::Invalid("Property Id Power invalid for variables in Transform property."),
        ),
        (
            "the child property's externalId alias, on a metric",
            true,
            json!({
                "propertyId": format!("externalId:{CHILD_POWER_EXTERNAL}"),
                "hierarchyId": "Turbines",
            }),
            Spelled::Resolved,
        ),
        (
            "the child property's own id, on a transform",
            false,
            json!({"propertyId": child_power.clone(), "hierarchyId": "Turbines"}),
            Spelled::Resolved,
        ),
        (
            "an id-shaped label the child model does not hold, on a metric",
            true,
            json!({
                "propertyId": "11111111-2222-3333-4444-555555555555",
                "hierarchyId": "Turbines",
            }),
            Spelled::Absent("Property 11111111-2222-3333-4444-555555555555 does not exist."),
        ),
        (
            // The one value the modelled `ID` pattern's negative lookahead
            // refuses, so it is no identifier spelling either.
            "the all-zero id the ID pattern refuses, on a metric",
            true,
            json!({
                "propertyId": "00000000-0000-0000-0000-000000000000",
                "hierarchyId": "Turbines",
            }),
            Spelled::Invalid(
                "Property Id 00000000-0000-0000-0000-000000000000 invalid for variables in \
                 Metric property.",
            ),
        ),
    ];

    for (at, (case, is_metric, value, expected)) in rows.into_iter().enumerate() {
        let name = format!("SpelledPlant-{at}");
        let formula = if is_metric {
            metric("Average", value)
        } else {
            transform("Average", value)
        };
        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                "assetModelName": &name,
                "assetModelProperties": [
                    measurement("Power", None, None),
                    formula,
                ],
                "assetModelHierarchies": [{"name": "Turbines", "childAssetModelId": &child}],
            })),
        )
        .await;
        match expected {
            Spelled::Resolved => {
                assert_eq!(
                    status, 202,
                    "case {case:?}: an identifier spelling must be accepted, got {status} {body}"
                );
                let stored = model_properties(
                    &router,
                    body["assetModelId"].as_str().unwrap_or_default(),
                )
                .await;
                let (property, _) = reference_of(&stored, "Average");
                assert_eq!(
                    property,
                    Some(child_power.clone()),
                    "case {case:?}: expected propertyId {child_power:?}, got {property:?}"
                );
            }
            Spelled::Invalid(message) | Spelled::Absent(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(message),
                    "case {case:?}: expected {message:?}, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_resolves_the_references_of_the_definition_it_stores() {
    let router = router();

    let model = create_model(
        &router,
        json!({
            "assetModelName": "RefUpdated",
            "assetModelProperties": [
                measurement("Power", Some(POWER_EXTERNAL), None),
                metric("Average", json!({"propertyId": "Power"})),
            ],
        }),
    )
    .await;
    let before = model_properties(&router, &model).await;
    let power = property_id(&before, "Power");
    let average = property_id(&before, "Average");

    // The update replaces the definition in full: the kept properties carry the
    // ids they were read under, and each formula spells its reference as a name
    // or an alias again — the spellings a client re-sending the definition it
    // read is free to use.
    let (status, answered) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model}"),
        Some(json!({
            "assetModelName": "RefUpdated",
            "assetModelProperties": [
                {
                    "id": &power,
                    "name": "Power",
                    "externalId": POWER_EXTERNAL,
                    "dataType": "DOUBLE",
                    "type": {"measurement": {}},
                },
                {
                    "id": &average,
                    "name": "Average",
                    "dataType": "DOUBLE",
                    "type": {"metric": {
                        "expression": "avg(v)",
                        "variables": [{"name": "v", "value": {"propertyId": "Power"}}],
                        "window": {"tumbling": {"interval": "5m"}},
                    }},
                },
                transform(
                    "Doubled",
                    json!({"propertyId": format!("externalId:{POWER_EXTERNAL}")}),
                ),
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {answered}");

    let after = model_properties(&router, &model).await;
    assert_eq!(
        property_id(&after, "Power"),
        power,
        "a kept property keeps its id, so the reference to it keeps its value: {after}"
    );
    for (case, name) in [
        ("the kept metric, re-sent spelled by name", "Average"),
        ("the added transform, spelled by externalId alias", "Doubled"),
    ] {
        let (property, hierarchy) = reference_of(&after, name);
        assert_eq!(
            property,
            Some(power.clone()),
            "case {case:?}: expected propertyId {power:?}, got {property:?}"
        );
        assert_eq!(
            hierarchy, None,
            "case {case:?}: a same-model reference carries no hierarchyId, got {hierarchy:?}"
        );
    }
}

#[tokio::test]
async fn a_composite_model_write_resolves_the_references_it_stores() {
    let router = router();

    let model = create_model(
        &router,
        json!({
            "assetModelName": "RefPack",
            "assetModelProperties": [measurement("Power", Some(POWER_EXTERNAL), None)],
        }),
    )
    .await;
    let power = property_id(&model_properties(&router, &model).await, "Power");

    // A composite model's own list is held to the same rule, and resolves in the
    // same scope: its own entries, plus everything the asset model holding it
    // carries.
    let (status, created) = create_composite(
        &router,
        &model,
        json!({
            "assetModelCompositeModelName": "Pack",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                measurement("CompPower", None, None),
                metric("CompAverage", json!({"propertyId": "CompPower"})),
                transform("CompDoubled", json!({"propertyId": "Power"})),
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel: got {status} {created}"
    );
    let composite = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelId in {created}"))
        .to_string();

    let (status, described) = describe_composite(&router, &model, &composite).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModelCompositeModel: got {status} {described}"
    );
    let nested = described["assetModelCompositeModelProperties"].clone();
    let comp_power = property_id(&nested, "CompPower");
    let comp_average = property_id(&nested, "CompAverage");
    for (case, name, expected) in [
        (
            "a sibling of the composite model's own list, spelled by name",
            "CompAverage",
            comp_power.clone(),
        ),
        (
            "a property of the asset model holding it, spelled by name",
            "CompDoubled",
            power.clone(),
        ),
    ] {
        let (property, _) = reference_of(&nested, name);
        assert_eq!(
            property,
            Some(expected.clone()),
            "case {case:?}: expected propertyId {expected:?}, got {property:?}"
        );
    }

    // The update of one composite model stores what it is handed under the same
    // rule, so a definition re-sent with name spellings resolves again.
    let (status, answered) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model}/composite-models/{composite}"),
        Some(json!({
            "assetModelCompositeModelName": "Pack",
            "assetModelCompositeModelProperties": [
                {
                    "id": &comp_power,
                    "name": "CompPower",
                    "dataType": "DOUBLE",
                    "type": {"measurement": {}},
                },
                {
                    "id": &comp_average,
                    "name": "CompAverage",
                    "dataType": "DOUBLE",
                    "type": {"metric": {
                        "expression": "avg(v)",
                        "variables": [{"name": "v", "value": {"propertyId": "CompPower"}}],
                        "window": {"tumbling": {"interval": "5m"}},
                    }},
                },
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "UpdateAssetModelCompositeModel: got {status} {answered}"
    );
    let (status, described) = describe_composite(&router, &model, &composite).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModelCompositeModel after the update: got {status} {described}"
    );
    let nested = described["assetModelCompositeModelProperties"].clone();
    let (property, _) = reference_of(&nested, "CompAverage");
    assert_eq!(
        property,
        Some(comp_power.clone()),
        "the updated composite model must report the resolved reference {comp_power:?}, \
         got {property:?}"
    );
}
