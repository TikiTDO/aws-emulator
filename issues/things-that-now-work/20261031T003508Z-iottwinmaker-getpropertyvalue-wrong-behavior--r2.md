Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 59fa4dd4e7f88de8e618073fff0b2e626be0bdb4

# A componentTypeId tabular query refuses any selectedProperty the component TYPE does not declare, although the rule above it (and the sibling history read) also admit a component's own declaration or a written value

_Filed: 2026-10-31T00:35:08Z_
_Operation: GetPropertyValue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `59fa4dd4e7f88de8e618073fff0b2e626be0bdb4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`addresses_property` is the single rule GetPropertyValue holds `selectedProperties`, `tabularConditions.propertyFilters` and `tabularConditions.orderBy` to. Its doc states three admissible sources for a property: the component's own definition, an inherited one, or a value having been written to it. The entity-addressed branch implements exactly that. The `componentTypeId` branch returns early and asks ONLY whether some component type on the `extendsFrom` chain declares the property — it never looks at the components in scope, so neither a definition a component declared inline nor a value `BatchPutPropertyValues` wrote to a property the type never declared can satisfy it.

Consequence: a tabular query over `componentTypeId` that selects (or filters/orders by) such a property is refused with 400 `ValidationException` "selectedProperties X is not a property of componentType Y", even though `Table::read` resolves every row's cells with `property_value(workspace, component, ...)` off that component's OWN record and would have reported a value. The sibling `GetPropertyValueHistory`, whose scope resolution is the same `componentTypeId` instance scan, accepts precisely this selection: its `selected_properties` falls back to the addressed components' effective definitions and then to `component.properties.contains_key(...)`. So one of the two readers of the same scope answers 200 with the value and the other answers 400 for the same workspace state.

## Evidence

The rule's own doc, and the branch that breaks it — `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value.rs:607`:

    /// Whether `property_name` is a property of what the request addressed: a
    /// definition for it is reachable (the component's own, or one it inherits), or
    /// a value has been written to it, which a property with no definition of its
    /// own can still carry.
    fn addresses_property(

get_property_value.rs:616 (the `componentTypeId` branch — only the type chain's declarations):

        if let Some(component_type_id) = query.component_type_id() {
            return workspace
                .component_type_chain(component_type_id)
                .into_iter()
                .any(|component_type| {
                    component_type
                        .property_definitions
                        .contains_key(property_name)
                });
        }

get_property_value.rs:626 (the entity-addressed branch, which DOES apply the documented rule):

        query.components().iter().any(|component| {
            component.properties.contains_key(property_name)
                || workspace
                    .effective_property_definition(... property_name)
                    .is_some()
        })

The module doc frames the refusal as being about the addressed COMPONENT, not the type — get_property_value.rs:21:

    //! `ResourceNotFoundException`; a component an EXISTING entity does not have, a
    //! selected property that is none of the
    //! addressed component's, ... are `ValidationException`s.

The cell would exist — get_property_value.rs:830 (`Table::read`):

            let values: ::std::collections::BTreeMap<String, DataValue> = mentioned
                .iter()
                .filter_map(|property_name| {
                    property_value(workspace, component, property_name)
                        .map(|value| (property_name.clone(), value))
                })

The sibling reader of the same scope accepts it — `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_property_value_history.rs:796`:

        for property_name in &selected {
            let definition = definition_in_scope(workspace, components, input, property_name);
            if definition.is_none()
                && !components
                    .iter()
                    .any(|component| component.properties.contains_key(property_name))
            {
                return Err(invalid(format!(
                    "selectedProperties {property_name} is not a property of what this request \
                     addresses"

and its `definition_in_scope` (get_property_value_history.rs:842) falls back to the components after the chain:

        components.iter().find_map(|component| {
            workspace.effective_property_definition(... property_name)
        })

Test coverage: no test pins this refusal. `grep -r "is not a property of componentType" services/.../tests` finds nothing; the only `not a property of` assertion in the suite is the BatchPutPropertyValues message at `tests/all/refusal_classes_and_wording.rs:234` ("Property Nowhere is not a property of component Sensor of Entity ..."). `tests/all/property_value_read.rs` and `tests/all/tabular_page_token_scope.rs` exercise type-declared properties only.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateComponentType `w`/`com.example.sensor` declaring one property `Serial` (`dataType` STRING) — i.e. a concrete, instantiable type.
3. CreateEntity in `w` with a component `Sensor` of `componentTypeId: com.example.sensor` whose `properties` declare an entity-local property `Local` with `{"definition": {"dataType": {"type": "STRING"}}, "value": {"stringValue": "x"}}` — a definition the component declares itself and the component type does not.
4. GetPropertyValue with `{"componentTypeId": "com.example.sensor", "tabularConditions": {}, "selectedProperties": ["Local"]}`: observe 400 ValidationException "selectedProperties Local is not a property of componentType com.example.sensor", although the entity's component carries both the definition and the value and `Table::read` would have produced the cell.
5. GetPropertyValueHistory with `{"componentTypeId": "com.example.sensor", "selectedProperties": ["Local"]}` (declare `Local` with `isTimeSeries: true` for the history path): observe the selection is accepted — the same scope, the opposite answer.
6. Same divergence via `tabularConditions.propertyFilters` / `orderBy` on `Local`, which `row_filters` / `order_keys` hold to the same `addresses_property`.
