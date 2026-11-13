// SPDX-License-Identifier: CC0-1.0

//! WHICH properties a `GetPropertyValue` request may name, end to end over HTTP.
//!
//! RULE under test: a property a read names — in `selectedProperties`, in a
//! `tabularConditions.propertyFilters` entry or in a `tabularConditions.orderBy`
//! key — is a property of what the request addresses when a declaration reaches
//! it inside that scope (the named component type's own `extendsFrom` chain, or
//! a component in scope declaring it itself or inheriting it) OR when a
//! component in scope carries a value for it, which a property no declaration
//! reaches can still do; only a name NONE of those sources admits is refused,
//! with HTTP 400 `ValidationException`. The addressing member decides which
//! components are in scope, never which of those sources count, so the
//! component-type table and the entity-addressed read admit the same names over
//! the same workspace state — and a name the table can report a cell for is
//! never one it refuses to be asked for.
//!
//! The rows of the table below differ along what the rule generalises over: the
//! SOURCE that admits the property (the named component type's own declaration,
//! one it inherits through `extendsFrom`, one the component declared inline, one
//! the component declared inline and the data plane then wrote values to, a
//! value carried with no declaration anywhere at all, a declaration with no
//! value) and, on the other side of the rule, a name no source admits. Each row
//! is asked in every member that holds a property name, through both addressing
//! forms.

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below reads from.
const WORKSPACE: &str = "tabularsources";

/// The component type the instances' type EXTENDS: the source of an inherited
/// property declaration.
const BASE: &str = "com.example.sources.base";
/// The component type the entities' components instantiate, and the one the
/// tabular reads below address.
const SENSOR: &str = "com.example.sources.sensor";

/// The entity whose component carries every property source.
const UNIT_ONE: &str = "unit-1";
/// A second instance of [`SENSOR`], carrying only the properties its component
/// type declares — so a property only [`UNIT_ONE`]'s component declares is
/// admitted by ONE component of the scope rather than by all of them.
const UNIT_TWO: &str = "unit-2";

/// The component name both entities' components are created under.
const COMPONENT: &str = "Spec";

/// An instant one of the data-plane writes below carries, as the ISO 8601 `Time`
/// a write is stamped with.
const T0: &str = "2026-04-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-04-01T00:01:00Z";

/// The workspace, component types and entities every case below reads.
///
/// `SENSOR` declares one property and inherits another from `BASE`; `unit-1`'s
/// component adds the declarations and values neither type ever declared, which
/// is what the rule's remaining sources are.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({"propertyDefinitions": {"Inherited": {"dataType": {"type": "STRING"}}}}),
        ),
        (
            SENSOR,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {"Declared": {"dataType": {"type": "STRING"}}},
                // The write connector the data-plane write below is set THROUGH.
                "functions": data_writer_functions(),
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

    let entities: Vec<Value> = vec![
        json!({
            "entityId": UNIT_ONE,
            "entityName": UNIT_ONE,
            "components": {COMPONENT: {
                "componentTypeId": SENSOR,
                "properties": {
                    "Declared": {"value": {"stringValue": "declared-1"}},
                    "Inherited": {"value": {"stringValue": "inherited-1"}},
                    // Declared by the COMPONENT, never by either type.
                    "Local": {
                        "definition": {"dataType": {"type": "STRING"}},
                        "value": {"stringValue": "local-1"},
                    },
                    // Declared by the component as a time series, so the data
                    // plane can write values to it.
                    "Series": {
                        "definition": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    },
                    // A value with no declaration reaching it at all.
                    "Carried": {"value": {"stringValue": "carried-1"}},
                    // A declaration the component made with no value under it.
                    "Empty": {"definition": {"dataType": {"type": "STRING"}}},
                },
            }},
        }),
        json!({
            "entityId": UNIT_TWO,
            "entityName": UNIT_TWO,
            "components": {COMPONENT: {
                "componentTypeId": SENSOR,
                "properties": {
                    "Declared": {"value": {"stringValue": "declared-2"}},
                    "Inherited": {"value": {"stringValue": "inherited-2"}},
                },
            }},
        }),
    ];
    for body in entities {
        let created = emulator.create_entity(WORKSPACE, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {body:?}: expected 200, got {created:?}"
        );
    }

    // The values of the inline-declared time series property: written through
    // the data plane, so the property that admits the reads below really is one
    // a value was WRITTEN to rather than one a create declared.
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": UNIT_ONE,
                    "componentName": COMPONENT,
                    "propertyName": "Series",
                },
                "propertyValues": [
                    {"time": T0, "value": {"doubleValue": 1.5}},
                    {"time": T1, "value": {"doubleValue": 4.5}},
                ],
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues for the inline time series property: expected 200, got {written:?}"
    );
    let error_entries = written
        .member("errorEntries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        error_entries.iter().all(|entry| entry["errors"]
            .as_array()
            .is_none_or(|errors| errors.is_empty())),
        "BatchPutPropertyValues: expected no errorEntries, got {written:?}"
    );
}

/// One property, and the source that makes it a property of the seeded scope.
#[derive(Debug)]
struct Source {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    property: &'static str,
    /// The cells a component-type table of this one property reports, in row
    /// order — empty where the property reads as nothing on every instance.
    /// `unit-1` is the first row, so its own cell is this list's first entry.
    cells: Vec<Value>,
    /// `true` where a source admits the property, so every member that names it
    /// is served; `false` where no source does, so every one of them is refused.
    addressed: bool,
}

/// The `DataValue` `unit-1`'s component reads this property as, or `None` where
/// the property reads as nothing there.
fn unit_one_cell(source: &Source) -> Option<Value> {
    source.cells.first().cloned()
}

/// The rows of the ONE table a tabular answer carries.
fn table_rows(response: &Resp) -> Vec<Value> {
    response
        .member("tabularPropertyValues")
        .as_array()
        .and_then(|tables| tables.first())
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `message` a refusal carries, or the empty string when it carries none.
fn message(response: &Resp) -> String {
    response
        .member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every source a property of the seeded scope can come from, and a name none of
/// them reaches.
fn sources() -> Vec<Source> {
    vec![
        Source {
            what: "a property the addressed component type declares itself",
            property: "Declared",
            cells: vec![
                json!({"stringValue": "declared-1"}),
                json!({"stringValue": "declared-2"}),
            ],
            addressed: true,
        },
        Source {
            what: "a property the addressed component type inherits through extendsFrom",
            property: "Inherited",
            cells: vec![
                json!({"stringValue": "inherited-1"}),
                json!({"stringValue": "inherited-2"}),
            ],
            addressed: true,
        },
        Source {
            what: "a property one component in scope declared inline, which no type declares",
            property: "Local",
            cells: vec![json!({"stringValue": "local-1"})],
            addressed: true,
        },
        Source {
            what: "an inline-declared time series property the data plane wrote values to",
            property: "Series",
            cells: vec![json!({"doubleValue": 4.5})],
            addressed: true,
        },
        Source {
            what: "a property carrying a value no declaration anywhere reaches",
            property: "Carried",
            cells: vec![json!({"stringValue": "carried-1"})],
            addressed: true,
        },
        Source {
            what: "an inline-declared property that reads as nothing on every instance",
            property: "Empty",
            cells: Vec::new(),
            addressed: true,
        },
        Source {
            what: "a name no declaration reaches and no component carries",
            property: "Absent",
            cells: Vec::new(),
            addressed: false,
        },
    ]
}

/// RULE: a `componentTypeId` table admits every property the addressed scope
/// has — the component type's own declarations, the ones it inherits, the ones a
/// component in scope declared itself, and the ones a component carries a value
/// for — and reports the cells those properties read as.
#[tokio::test]
async fn a_component_type_table_admits_every_property_its_scope_has() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for source in sources() {
        let Source {
            what,
            property,
            ref cells,
            addressed,
        } = source;
        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "componentTypeId": SENSOR,
                    "selectedProperties": [property],
                    "tabularConditions": {},
                }),
            )
            .await;
        if !addressed {
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {what:?}: expected a 400 ValidationException, got {read:?}"
            );
            let reported = message(&read);
            assert!(
                reported.contains(property),
                "case {what:?}: the refusal must name {property:?}, got {reported:?}"
            );
            assert!(
                table_rows(&read).is_empty(),
                "case {what:?}: a refusal carries no rows, got {read:?}"
            );
            continue;
        }
        assert_eq!(
            read.status, 200,
            "case {what:?}: expected 200, got {read:?}"
        );
        let reported: Vec<Value> = table_rows(&read)
            .into_iter()
            .map(|row| row[property].clone())
            .collect();
        assert_eq!(
            &reported, cells,
            "case {what:?}: expected the cells the scope reads this property as, got {read:?}"
        );
    }
}

/// RULE, the half that was diverging: the ADDRESSING member decides which
/// components are in scope, not which sources admit a property — so a selection
/// the entity-addressed read admits is admitted by the component-type table over
/// the same state, in the tabular shape and in the latest-value one.
#[tokio::test]
async fn both_addressing_forms_admit_the_same_properties() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for source in sources() {
        let Source {
            what,
            property,
            addressed,
            ..
        } = source;
        let expected_cell = unit_one_cell(&source);

        // Every form a read of one property takes over the same workspace
        // state: the component-type table, the entity-addressed table, and the
        // entity-addressed latest-value map.
        let forms: Vec<(&str, Value)> = vec![
            (
                "the componentTypeId table",
                json!({
                    "componentTypeId": SENSOR,
                    "selectedProperties": [property],
                    "tabularConditions": {},
                }),
            ),
            (
                "the entity-addressed table",
                json!({
                    "entityId": UNIT_ONE,
                    "componentName": COMPONENT,
                    "selectedProperties": [property],
                    "tabularConditions": {},
                }),
            ),
            (
                "the entity-addressed latest-value map",
                json!({
                    "entityId": UNIT_ONE,
                    "componentName": COMPONENT,
                    "selectedProperties": [property],
                }),
            ),
        ];

        for (form, body) in forms {
            let read = emulator.get_property_value(WORKSPACE, body).await;
            if !addressed {
                assert_eq!(
                    (read.status, read.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {what:?} through {form}: expected a 400 ValidationException, got {read:?}"
                );
                continue;
            }
            assert_eq!(
                read.status, 200,
                "case {what:?} through {form}: expected 200, got {read:?}"
            );
            let reported = match form {
                "the entity-addressed latest-value map" => {
                    let latest = &read.member("propertyValues")[property];
                    assert_eq!(
                        latest["propertyReference"]["propertyName"],
                        json!(property),
                        "case {what:?} through {form}: expected the reference to name the \
                         property, got {read:?}"
                    );
                    match latest["propertyValue"].is_null() {
                        true => None,
                        false => Some(latest["propertyValue"].clone()),
                    }
                }
                _ => table_rows(&read)
                    .first()
                    .map(|row| row[property].clone())
                    .filter(|cell| !cell.is_null()),
            };
            assert_eq!(
                reported, expected_cell,
                "case {what:?} through {form}: expected the cell unit-1 reads this property as, \
                 got {read:?}"
            );
        }
    }
}

/// RULE: the `propertyFilters` and `orderBy` keys of a tabular request are held
/// to that SAME rule, so each one names any property the addressed scope has and
/// is refused only for a name no source admits.
#[tokio::test]
async fn a_tabular_condition_names_any_property_the_scope_has() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for source in sources() {
        let Source {
            what,
            property,
            addressed,
            ..
        } = source;
        // A filter compares against a value, and a property that reads as
        // nothing still has to be NAMEABLE: a row whose cell is absent simply
        // satisfies no filter, which is an empty table and not a refusal.
        let compared = unit_one_cell(&source).unwrap_or(json!({"stringValue": "unwritten"}));

        let conditions: Vec<(&str, Value)> = vec![
            (
                "a propertyFilters entry",
                json!({"propertyFilters": [
                    {"propertyName": property, "operator": "=", "value": compared},
                ]}),
            ),
            (
                "an orderBy key",
                json!({"orderBy": [{"propertyName": property}]}),
            ),
            (
                "an orderBy key in DESCENDING order",
                json!({"orderBy": [{"propertyName": property, "order": "DESCENDING"}]}),
            ),
        ];

        for (condition, tabular_conditions) in conditions {
            let read = emulator
                .get_property_value(
                    WORKSPACE,
                    json!({
                        "componentTypeId": SENSOR,
                        // Selected on its own, so what this case is about is
                        // the condition's property name and nothing else.
                        "selectedProperties": ["Declared"],
                        "tabularConditions": tabular_conditions,
                    }),
                )
                .await;
            if !addressed {
                assert_eq!(
                    (read.status, read.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {what:?} as {condition}: expected a 400 ValidationException, got {read:?}"
                );
                let reported = message(&read);
                assert!(
                    reported.contains(property),
                    "case {what:?} as {condition}: the refusal must name {property:?}, got \
                     {reported:?}"
                );
                continue;
            }
            assert_eq!(
                read.status, 200,
                "case {what:?} as {condition}: expected 200, got {read:?}"
            );
        }
    }
}

/// RULE: an ordering key over a property only one component in scope has still
/// orders the whole scope, and a filter on one selects the rows whose cell
/// compares — the rows a condition decides come from the stored cells, not from
/// which component declared the property.
#[tokio::test]
async fn a_condition_on_an_inline_declared_property_decides_the_rows() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // A filter on the property only `unit-1`'s component declares: `unit-2`'s
    // row has no cell for it, so it compares with nothing and the table is
    // `unit-1`'s row alone.
    let filtered = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": ["Declared", "Local"],
                "tabularConditions": {"propertyFilters": [
                    {"propertyName": "Local", "operator": "=",
                     "value": {"stringValue": "local-1"}},
                ]},
            }),
        )
        .await;
    assert_eq!(
        filtered.status, 200,
        "a filter on the inline-declared property: expected 200, got {filtered:?}"
    );
    assert_eq!(
        table_rows(&filtered),
        vec![json!({"Declared": {"stringValue": "declared-1"}, "Local": {"stringValue": "local-1"}})],
        "a filter on the inline-declared property: expected the row whose cell compares, got \
         {filtered:?}"
    );

    // An ordering key over the same property: the row that has no cell for it
    // sorts after the one that has one, and the whole scope is still reported.
    let ordered = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": ["Declared"],
                "tabularConditions": {"orderBy": [{"propertyName": "Local"}]},
            }),
        )
        .await;
    assert_eq!(
        ordered.status, 200,
        "an ordering key over the inline-declared property: expected 200, got {ordered:?}"
    );
    assert_eq!(
        table_rows(&ordered),
        vec![
            json!({"Declared": {"stringValue": "declared-1"}}),
            json!({"Declared": {"stringValue": "declared-2"}}),
        ],
        "an ordering key over the inline-declared property: expected the whole scope, the row \
         carrying a cell first, got {ordered:?}"
    );

    // Reading through any of these leaves the next caller the same answer.
    let repeated = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": ["Declared"],
                "tabularConditions": {"orderBy": [{"propertyName": "Local"}]},
            }),
        )
        .await;
    assert_eq!(
        repeated.body, ordered.body,
        "two reads of unchanged state: expected the same answer, got {ordered:?} then {repeated:?}"
    );
}
