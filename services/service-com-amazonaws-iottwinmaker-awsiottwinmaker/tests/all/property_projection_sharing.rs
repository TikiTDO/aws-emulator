// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a stored property every reader of one reports through,
//! end to end over HTTP.
//!
//! RULE under test: what a read says about one property of one component — the
//! EFFECTIVE definition (the component's own declaration folded over the ones
//! the component type it instantiates and the types that type extends declare,
//! marked `isInherited` exactly when that chain declares the property), the
//! VALUE the property reads as (the most recent value written to it through
//! either plane, else the default that definition declares, else nothing), and
//! whether every value of a list- or map-typed one was returned — is one
//! projection of the stored property, so every reader reports it identically
//! whichever modelled wrapper it travels in: `GetEntity`'s `PropertyResponse`,
//! `ListProperties`' `PropertySummary` and `GetPropertyValue`'s
//! `propertyValues`.
//!
//! The rows differ along what the rule generalises over: WHERE the definition
//! comes from (the component's own declaration, the component type's, one
//! reached through `extendsFrom`, a component declaration that only REFINES an
//! inherited one, nowhere at all), WHERE the value comes from (written by the
//! control plane, written by the time-series data plane, the definition's
//! default, nowhere), the DATA TYPE it travels in (`STRING`, `DOUBLE`,
//! `BOOLEAN`, `LIST`, `MAP`), and HOW MANY values a list- or map-typed one holds
//! (one, and the most a `DataValue` can carry).

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::MAX_PROPERTY_VALUES_RETURNED;

use crate::harness::{data_writer_functions, Emulator};

/// The workspace every case below reads.
const WORKSPACE: &str = "projection";

/// The component type the inherited definitions come from. Its write connector
/// is what the time-series write below goes through.
const BASE: &str = "com.example.base";
/// A component type that EXTENDS [`BASE`], so a component of it reaches every
/// definition above beside the one this type declares itself.
const DERIVED: &str = "com.example.derived";

/// The entity carrying the component every scalar row reads.
const UNIT: &str = "unit-1";
/// The entity carrying the component the container rows read, which hold as many
/// values as a `DataValue` can.
const BULK: &str = "unit-2";
/// The component [`UNIT`] carries, of [`DERIVED`].
const SPEC: &str = "Spec";
/// The component [`BULK`] carries, of [`BASE`].
const LOAD: &str = "Load";

/// The instant the time-series write below is made for.
const T0: &str = "2026-04-01T00:00:00Z";

/// A `listValue` of `count` strings, and the `mapValue` of the same values keyed
/// by position: the containers whose size decides
/// `areAllPropertyValuesReturned`.
fn list_of(count: usize) -> Value {
    json!({
        "listValue": (0..count)
            .map(|index| json!({"stringValue": format!("v{index}")}))
            .collect::<Vec<Value>>(),
    })
}

/// See [`list_of`].
fn map_of(count: usize) -> Value {
    json!({
        "mapValue": (0..count)
            .map(|index| (format!("k{index}"), json!({"stringValue": format!("v{index}")})))
            .collect::<serde_json::Map<String, Value>>(),
    })
}

/// The workspace, component types and entities every case below reads.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    "Tag": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset-tag"},
                    },
                    "Threshold": {
                        "dataType": {"type": "DOUBLE"},
                        "defaultValue": {"doubleValue": 1.5},
                    },
                    // No default: the property reads as nothing at all.
                    "Enabled": {"dataType": {"type": "BOOLEAN"}},
                    // A container data type takes a `nestedType`.
                    "Readings": {"dataType": {"type": "LIST", "nestedType": {"type": "STRING"}}},
                    "Labels": {"dataType": {"type": "MAP", "nestedType": {"type": "STRING"}}},
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                },
                // The connector the time-series write below is made through.
                "functions": data_writer_functions(),
            }),
        ),
        (
            DERIVED,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {
                    "Site": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "plant-a"},
                    },
                },
            }),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let entities: Vec<(&str, Value)> = vec![
        (
            UNIT,
            json!({
                "entityId": UNIT,
                "entityName": UNIT,
                "components": {SPEC: {
                    "componentTypeId": DERIVED,
                    "properties": {
                        // A control-plane value over an inherited default.
                        "Tag": {"value": {"stringValue": "alpha"}},
                        // A declaration that only REFINES the inherited
                        // definition: it restates no `dataType` and no default.
                        "Threshold": {"definition": {"displayName": "Trip point"}},
                        "Readings": {"value": list_of(1)},
                        "Labels": {"value": map_of(1)},
                        // The component's OWN property: nothing on the chain
                        // declares it.
                        "Local": {
                            "definition": {"dataType": {"type": "STRING"}},
                            "value": {"stringValue": "only here"},
                        },
                        // A value no declaration anywhere gives a definition.
                        "Loose": {"value": {"booleanValue": true}},
                    },
                }},
            }),
        ),
        (
            BULK,
            json!({
                "entityId": BULK,
                "entityName": BULK,
                "components": {LOAD: {
                    "componentTypeId": BASE,
                    "properties": {
                        "Readings": {"value": list_of(MAX_PROPERTY_VALUES_RETURNED)},
                        "Labels": {"value": map_of(MAX_PROPERTY_VALUES_RETURNED)},
                    },
                }},
            }),
        ),
    ];
    for (entity_id, body) in entities {
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }

    // The one value written through the time-series data plane, so a row reads a
    // value neither the control plane nor a default put there.
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": UNIT,
                    "componentName": SPEC,
                    "propertyName": "Temp",
                },
                "propertyValues": [{"time": T0, "value": {"doubleValue": 42.25}}],
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues: expected 200, got {written:?}"
    );
    assert!(
        written
            .member("errorEntries")
            .as_array()
            .is_none_or(|entries| entries.iter().all(|entry| entry["errors"]
                .as_array()
                .is_none_or(|errors| errors.is_empty()))),
        "BatchPutPropertyValues: expected no errorEntries, got {written:?}"
    );
}

/// One row of the projection: the property to read, and what the projection has
/// to say about it.
#[derive(Debug)]
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    entity_id: &'static str,
    component_name: &'static str,
    property: &'static str,
    /// `dataType.type` of the reported definition, or `None` where no
    /// declaration reaches the property, so no definition is reported at all.
    data_type: Option<&'static str>,
    /// `isInherited` of the reported definition — `None` where there is none.
    is_inherited: Option<bool>,
    /// `displayName` of the reported definition, which a refinement keeps from
    /// the declaration that restated it.
    display_name: Option<&'static str>,
    /// The `DataValue` the property reads as, or `None` where it reads as
    /// nothing.
    value: Option<Value>,
}

/// Every row of the rule.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "aControlPlaneValueOverAnInheritedDefault",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Tag",
            data_type: Some("STRING"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(json!({"stringValue": "alpha"})),
        },
        Case {
            what: "aRefinedInheritedDefinitionKeepsTheInheritedTypeAndDefault",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Threshold",
            data_type: Some("DOUBLE"),
            is_inherited: Some(true),
            display_name: Some("Trip point"),
            value: Some(json!({"doubleValue": 1.5})),
        },
        Case {
            what: "aDefinitionDeclaredByTheExtendingTypeItself",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Site",
            data_type: Some("STRING"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(json!({"stringValue": "plant-a"})),
        },
        Case {
            what: "anInheritedDefinitionWithNoDefaultReadsAsNothing",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Enabled",
            data_type: Some("BOOLEAN"),
            is_inherited: Some(true),
            display_name: None,
            value: None,
        },
        Case {
            what: "theComponentsOwnDefinitionIsNotInherited",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Local",
            data_type: Some("STRING"),
            is_inherited: Some(false),
            display_name: None,
            value: Some(json!({"stringValue": "only here"})),
        },
        Case {
            what: "aValueNoDeclarationDefinesIsReportedWithoutADefinition",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Loose",
            data_type: None,
            is_inherited: None,
            display_name: None,
            value: Some(json!({"booleanValue": true})),
        },
        Case {
            what: "aValueWrittenThroughTheTimeSeriesDataPlane",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Temp",
            data_type: Some("DOUBLE"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(json!({"doubleValue": 42.25})),
        },
        Case {
            what: "aListValueHoldingOneValue",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Readings",
            data_type: Some("LIST"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(list_of(1)),
        },
        Case {
            what: "aMapValueHoldingOneValue",
            entity_id: UNIT,
            component_name: SPEC,
            property: "Labels",
            data_type: Some("MAP"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(map_of(1)),
        },
        Case {
            what: "aListValueHoldingAsManyValuesAsOneCanHold",
            entity_id: BULK,
            component_name: LOAD,
            property: "Readings",
            data_type: Some("LIST"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(list_of(MAX_PROPERTY_VALUES_RETURNED)),
        },
        Case {
            what: "aMapValueHoldingAsManyValuesAsOneCanHold",
            entity_id: BULK,
            component_name: LOAD,
            property: "Labels",
            data_type: Some("MAP"),
            is_inherited: Some(true),
            display_name: None,
            value: Some(map_of(MAX_PROPERTY_VALUES_RETURNED)),
        },
    ]
}

/// The `PropertyResponse` `GetEntity` reports for one property.
async fn through_get_entity(emulator: &Emulator, case: &Case) -> Value {
    let read = emulator.get_entity(WORKSPACE, case.entity_id).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected 200 from GetEntity, got {read:?}",
        case.what
    );
    read.member("components")[case.component_name]["properties"][case.property].clone()
}

/// The `PropertySummary` `ListProperties` reports for one property.
async fn through_list_properties(emulator: &Emulator, case: &Case) -> Value {
    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": case.entity_id, "componentName": case.component_name}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {:?}: expected 200 from ListProperties, got {listed:?}",
        case.what
    );
    listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["propertyName"] == json!(case.property))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a summary for {:?}, got {listed:?}",
                case.what, case.property
            )
        })
}

/// The `DataValue` `GetPropertyValue` reports for one property.
async fn through_get_property_value(emulator: &Emulator, case: &Case) -> Value {
    let read = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "entityId": case.entity_id,
                "componentName": case.component_name,
                "selectedProperties": [case.property],
            }),
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected 200 from GetPropertyValue, got {read:?}",
        case.what
    );
    read.member("propertyValues")[case.property]["propertyValue"].clone()
}

/// RULE: the projection a reader reports for one property is the effective
/// definition, the value the property reads as and
/// `areAllPropertyValuesReturned` over that value — whatever the reader.
#[tokio::test]
async fn one_property_reads_as_one_projection_through_every_reader() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let property = through_get_entity(&emulator, &case).await;
        let summary = through_list_properties(&emulator, &case).await;
        let value = through_get_property_value(&emulator, &case).await;

        // The projection, as GetEntity reports it.
        let definition = &property["definition"];
        assert_eq!(
            definition["dataType"]["type"].as_str(),
            case.data_type,
            "case {:?}: expected dataType {:?}, got {property:?}",
            case.what,
            case.data_type
        );
        assert_eq!(
            definition["isInherited"].as_bool(),
            case.is_inherited,
            "case {:?}: expected isInherited {:?}, got {property:?}",
            case.what,
            case.is_inherited
        );
        assert_eq!(
            definition["displayName"].as_str(),
            case.display_name,
            "case {:?}: expected displayName {:?}, got {property:?}",
            case.what,
            case.display_name
        );
        assert_eq!(
            property.get("value").cloned(),
            case.value,
            "case {:?}: expected the value the property reads as to be {:?}, got {property:?}",
            case.what,
            case.value
        );
        // The ceiling a read reports up to is exactly the top of the modelled
        // `length` of a `DataValue`'s `listValue` and `mapValue`, so no value
        // this service can be asked to store exceeds it and every row — the two
        // that hold as many values as one can included — reports them all.
        assert_eq!(
            property["areAllPropertyValuesReturned"].as_bool(),
            Some(true),
            "case {:?}: expected every value of the property to be returned, got {property:?}",
            case.what
        );

        // The SAME projection, as the other two readers report it: the summary
        // adds only the property's own name.
        assert_eq!(
            summary["definition"], property["definition"],
            "case {:?}: expected ListProperties and GetEntity to report one definition, got \
             {summary:?} against {property:?}",
            case.what
        );
        assert_eq!(
            summary["value"], property["value"],
            "case {:?}: expected ListProperties and GetEntity to report one value, got {summary:?} \
             against {property:?}",
            case.what
        );
        assert_eq!(
            summary["areAllPropertyValuesReturned"], property["areAllPropertyValuesReturned"],
            "case {:?}: expected ListProperties and GetEntity to answer \
             areAllPropertyValuesReturned alike, got {summary:?} against {property:?}",
            case.what
        );
        assert_eq!(
            value,
            case.value.clone().unwrap_or(Value::Null),
            "case {:?}: expected GetPropertyValue to report the value the property reads as, got \
             {value:?}",
            case.what
        );
        assert_eq!(
            value, property["value"],
            "case {:?}: expected GetPropertyValue and GetEntity to report one value, got {value:?} \
             against {property:?}",
            case.what
        );
    }
}

/// RULE (the projection is computed, not remembered): a control-plane write
/// moves what the projection says about the property, so all three readers
/// answer with the NEW value and with the definition the patch left in place.
#[tokio::test]
async fn a_write_moves_every_readers_projection_together() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The rows differ along the data type and the container size the write
    // leaves behind, and on whether the property had a value before it.
    let writes: Vec<(&str, &str, Value)> = vec![
        ("Tag", "stringValue", json!({"stringValue": "omega"})),
        ("Enabled", "booleanValue", json!({"booleanValue": false})),
        ("Threshold", "doubleValue", json!({"doubleValue": -17.5})),
        ("Labels", "mapValue", map_of(2)),
        (
            "Readings",
            "listValue",
            list_of(MAX_PROPERTY_VALUES_RETURNED),
        ),
    ];
    for (property, member, written) in writes {
        let patched = emulator
            .update_entity(
                WORKSPACE,
                UNIT,
                json!({"componentUpdates": {SPEC: {"propertyUpdates": {property: {
                    "updateType": "UPDATE",
                    "value": written.clone(),
                }}}}}),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {property:?}: expected 200 from UpdateEntity, got {patched:?}"
        );

        let case = Case {
            what: "afterAControlPlaneWrite",
            entity_id: UNIT,
            component_name: SPEC,
            property,
            data_type: None,
            is_inherited: None,
            display_name: None,
            value: None,
        };
        let reported = through_get_entity(&emulator, &case).await;
        let summary = through_list_properties(&emulator, &case).await;
        let value = through_get_property_value(&emulator, &case).await;

        assert_eq!(
            reported["value"][member],
            written[member],
            "case {property:?}: expected GetEntity to report the value just written, got \
             {reported:?}"
        );
        assert_eq!(
            summary["value"], reported["value"],
            "case {property:?}: expected ListProperties to move with GetEntity, got {summary:?} \
             against {reported:?}"
        );
        assert_eq!(
            value, reported["value"],
            "case {property:?}: expected GetPropertyValue to move with GetEntity, got {value:?} \
             against {reported:?}"
        );
        // The patch named no definition, so the one the projection resolves is
        // still the one the component had.
        assert_eq!(
            summary["definition"], reported["definition"],
            "case {property:?}: expected one definition after the write, got {summary:?} against \
             {reported:?}"
        );
    }
}
