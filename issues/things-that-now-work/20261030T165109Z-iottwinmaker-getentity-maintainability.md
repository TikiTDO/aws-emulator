Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 962bc15d984e86ce56d75221c3331a6151fbca59

# The property projection (effective definition -> isInherited -> latest value -> areAllPropertyValuesReturned) is written out twice, once in GetEntity and once in ListProperties

_Filed: 2026-10-30T16:51:09Z_
_Operation: GetEntity | Class: maintainability_

> _Note: this issue was identified against the code base at commit `962bc15d984e86ce56d75221c3331a6151fbca59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetEntity`'s `property_response` and `ListProperties`' `property_summary` compute the SAME four-step projection of one stored property, line for line, differing only in the modelled wrapper they fill (`PropertyResponse` vs `PropertySummary`, which adds `propertyName`):

1. resolve the effective definition with `Workspace::effective_property_definition`;
2. resolve the current value with `state::latest_property_value` against that definition;
3. render the definition with `state::property_definition_response`, passing `Workspace::property_definition_is_inherited`;
4. set `areAllPropertyValuesReturned` from `state::all_property_values_returned` over that value.

The two copies must agree exactly — the crate's own tests assert equality of what they produce — so any later change to one (a new `PropertyDefinitionResponse` member, a different `isInherited` reading, a different value fallback) silently diverges the other. A third, partial copy of steps 1-2 sits in `GetPropertyValue::property_value`.

The fix is one shared projection helper beside the other property helpers in `state.rs` (taking `component_type_id`, `properties`, `property_name` and handing back the rendered definition, the value and the flag), consumed by both handlers to fill their own wrapper. Every response member and every existing test stays byte-identical.

## Evidence

COPY 1 — `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_entity.rs:411-433`:
```
fn property_response(
    workspace: &crate::state::Workspace,
    component_type_id: Option<&str>,
    properties: &::std::collections::BTreeMap<String, crate::state::PropertyRecord>,
    property_name: &str,
) -> PropertyResponse {
    let definition =
        workspace.effective_property_definition(component_type_id, properties, property_name);
    let value =
        crate::state::latest_property_value(properties.get(property_name), definition.as_ref());
    PropertyResponse {
        definition: definition.as_ref().and_then(|definition| {
            crate::state::property_definition_response(
                definition,
                workspace.property_definition_is_inherited(component_type_id, property_name),
            )
        }),
        are_all_property_values_returned: Some(crate::state::all_property_values_returned(
            value.as_ref(),
        )),
        value,
    }
}
```

COPY 2 — `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_properties.rs:397-425`:
```
fn property_summary(
    workspace: &crate::state::Workspace,
    component: &AddressedComponent<'_>,
    property_name: String,
) -> PropertySummary {
    let definition = workspace.effective_property_definition(
        component.component_type_id,
        component.properties,
        &property_name,
    );
    let value = crate::state::latest_property_value(
        component.properties.get(&property_name),
        definition.as_ref(),
    );
    PropertySummary {
        definition: definition.as_ref().and_then(|definition| {
            crate::state::property_definition_response(
                definition,
                workspace
                    .property_definition_is_inherited(component.component_type_id, &property_name),
            )
        }),
        property_name,
        are_all_property_values_returned: Some(crate::state::all_property_values_returned(
            value.as_ref(),
        )),
        value,
    }
}
```

Both doc comments state that the two MUST agree — `get_entity.rs:401-403`: "Every member is read through the projection its kind of reader shares, so one property says the same thing here as it does through `ListProperties` and `GetPropertyValue`"; `list_properties.rs:388-390`: "Every member is read through the projection its kind of reader shares, so one property says the same thing here as it does through `GetEntity` and `GetPropertyValue`".

PARTIAL THIRD COPY of steps 1-2 — `src/ops/get_property_value.rs:678-692`:
```
fn property_value(
    workspace: &crate::state::Workspace,
    component: &ScopedComponent<'_>,
    property_name: &str,
) -> Option<DataValue> {
    let definition = workspace.effective_property_definition(
        component.component_type_id,
        component.properties,
        property_name,
    );
    crate::state::latest_property_value(
        component.properties.get(property_name),
        definition.as_ref(),
    )
}
```

TESTS THAT PIN THE EQUALITY (so the refactor must leave them green and unchanged):
`tests/all/component_inherited_schema.rs:455-460`:
```
            assert_eq!(
                row["definition"], component["properties"][property_name]["definition"],
                "case {:?} property {property_name:?}: expected ListProperties and GetEntity to \
                 report one definition, got {row:?}",
                case.what
            );
```
`tests/all/property_listing.rs:456-469` (ListProperties' definition vs GetComponentType's, `isInherited` aside) and `tests/all/property_listing.rs:410` (`summary["areAllPropertyValuesReturned"]`). Further coverage: `tests/all/component_refined_definition.rs`, `tests/all/reported_members.rs`, `tests/all/time_series_latest_value.rs`.

## Steps to Reproduce

1. Open `src/ops/get_entity.rs:411-433` and `src/ops/list_properties.rs:397-425` side by side: the same four calls in the same order, filling two wrappers with the same member set.
2. Change one of them only — e.g. make `property_definition_response` take an extra argument, or change the `isInherited` argument — and observe that nothing forces the other copy to follow; `cargo test -p service-com-amazonaws-iottwinmaker-awsiottwinmaker component_inherited_schema` is the only thing that would catch the drift, after the fact.
3. Extract one projection helper into `src/state.rs` beside `latest_property_value` / `property_definition_response`, call it from both handlers, and re-run the crate's tests: `tests/all/component_inherited_schema.rs`, `tests/all/property_listing.rs`, `tests/all/component_refined_definition.rs`, `tests/all/reported_members.rs` and `tests/all/time_series_latest_value.rs` must all pass unchanged.
