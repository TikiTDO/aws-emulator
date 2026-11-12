// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a stored property every reader of one reports through.
//!
//! One concern of [`crate::state`], kept in its own file: a reader reaches these
//! through the parent module, which re-exports them.
//!
//! A property is reported in more than one modelled wrapper — `PropertyResponse`
//! inside a `ComponentResponse` (`GetEntity`), `PropertySummary` in a page of
//! `ListProperties`, and a bare `DataValue` in `GetPropertyValue`'s
//! `propertyValues` — but what the wrappers say about one property is the same
//! thing resolved the same way, so it is resolved HERE once and each reader only
//! fills its own wrapper with it. That is what keeps one property saying the
//! same thing through every reader: a change to how a definition is resolved,
//! how `isInherited` is read, how a value falls back to its default or how
//! `areAllPropertyValuesReturned` is answered reaches every reader at once
//! instead of having to be repeated in each.

use super::*;

/// What a read reports about ONE property of one component, resolved for
/// whichever modelled wrapper the reader fills.
///
/// Carries exactly the members the wrappers share: the definition that gives the
/// property its meaning, rendered as the modelled `PropertyDefinitionResponse`,
/// the value the property reads as, and whether every value of a list- or
/// map-typed one was reported. `PropertySummary` adds the property's own name,
/// which its reader already has — it is the key it looked the property up by —
/// so the name is not part of the projection.
#[derive(Debug, Clone)]
pub struct PropertyProjection {
    /// `definition`, the rendered effective definition — `None` when no
    /// declaration reaches the property, or when the one that does carries no
    /// `dataType` ([`property_definition_response`]).
    pub definition: Option<crate::ops::types::PropertyDefinitionResponse>,
    /// `value`, what the property reads as ([`latest_property_value`]).
    pub value: Option<crate::ops::types::DataValue>,
    /// `areAllPropertyValuesReturned` over that value
    /// ([`all_property_values_returned`]).
    pub are_all_property_values_returned: bool,
}

/// The projection of ONE property of a component, for a reader that reports the
/// property's definition beside its value.
///
/// The definition is the component's own where it declared one, else the one it
/// inherits from the component type it instantiates
/// ([`Workspace::effective_property_definition`]), marked `isInherited`
/// whenever the type it instantiates or a type that type extends declares the
/// property ([`Workspace::property_definition_is_inherited`]); the value is the
/// one that definition resolves ([`latest_property_value`]); and the flag is
/// that value's ([`all_property_values_returned`]).
pub fn property_projection(
    workspace: &Workspace,
    component_type_id: Option<&str>,
    properties: &BTreeMap<String, PropertyRecord>,
    property_name: &str,
) -> PropertyProjection {
    let ResolvedProperty { definition, value } =
        resolve_property(workspace, component_type_id, properties, property_name);
    PropertyProjection {
        definition: definition.as_ref().and_then(|definition| {
            property_definition_response(
                definition,
                workspace.property_definition_is_inherited(component_type_id, property_name),
            )
        }),
        are_all_property_values_returned: all_property_values_returned(value.as_ref()),
        value,
    }
}

/// The value ONE property of a component reads as, for a reader that reports
/// the value alone.
///
/// The same value [`property_projection`] carries, resolved through the same
/// definition, so a property read on its own reads as it does beside its
/// definition.
pub fn effective_property_value(
    workspace: &Workspace,
    component_type_id: Option<&str>,
    properties: &BTreeMap<String, PropertyRecord>,
    property_name: &str,
) -> Option<crate::ops::types::DataValue> {
    resolve_property(workspace, component_type_id, properties, property_name).value
}

/// One property resolved as far as every reader needs it: the effective
/// definition as STORED (a `PropertyDefinitionRequest`, not yet rendered for the
/// wire) and the value that definition resolves.
struct ResolvedProperty {
    definition: Option<crate::ops::types::PropertyDefinitionRequest>,
    value: Option<crate::ops::types::DataValue>,
}

/// Resolve one property's effective definition and the value it reads as — the
/// part of the projection a reader of the value alone needs too, so the
/// definition a value falls back to the default of is resolved one way for all
/// of them.
fn resolve_property(
    workspace: &Workspace,
    component_type_id: Option<&str>,
    properties: &BTreeMap<String, PropertyRecord>,
    property_name: &str,
) -> ResolvedProperty {
    let definition =
        workspace.effective_property_definition(component_type_id, properties, property_name);
    let value = latest_property_value(properties.get(property_name), definition.as_ref());
    ResolvedProperty { definition, value }
}
