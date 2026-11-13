// SPDX-License-Identifier: CC0-1.0

//! How one stored GraphQL type is written back as JSON: the document a read of a type
//! answers with when it asks for the `JSON` format.
//!
//! The type's JSON representation is the type's own introspection document, written the
//! way this service writes one (recorded behaviour): compact — no newlines and no
//! indentation — with its members in a fixed order, and with every member a type has no
//! value for left OUT rather than written as null. A type REFERENCE carries only what it
//! says itself: the name for a named reference, and the wrapper's kind with the reference
//! it wraps for a list or non-null one, so nothing of the referenced type is resolved
//! into it.
//!
//! This is the representation of ONE type. The whole SCHEMA's introspection document
//! (`super::introspection_schema`) is the result of an introspection query over the type
//! system and is written as that system defines it, which is a different document.
//!
//! Everything here comes from the parsed definition ([`super::TypeDefinition`]) and, for
//! the object types that implement an interface, from the other types the API holds —
//! never from the request.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::schema_types::{
    EnumValueDefinition, FieldDefinition, InputValueDefinition, TypeDefinition, TypeRecord,
    TypeRef, TypeWrapper, ENUM_KIND, INPUT_OBJECT_KIND, INTERFACE_KIND, LIST_KIND, NON_NULL_KIND,
    OBJECT_KIND, UNION_KIND,
};

impl TypeDefinition {
    /// This type's definition as the service renders it in JSON, for a type held by an
    /// API that holds `api_types`.
    pub fn render_json(&self, api_types: &BTreeMap<String, TypeRecord>) -> String {
        serde_json::to_string(&self.json_document(api_types)).unwrap_or_default()
    }

    /// The document [`Self::render_json`] writes.
    fn json_document(&self, api_types: &BTreeMap<String, TypeRecord>) -> TypeDocument {
        let has_fields = self.kind == OBJECT_KIND || self.kind == INTERFACE_KIND;
        TypeDocument {
            name: self.name.clone(),
            kind: self.kind,
            description: self.description.clone(),
            fields: has_fields.then(|| self.fields.iter().map(field_document).collect()),
            input_fields: (self.kind == INPUT_OBJECT_KIND)
                .then(|| self.input_fields.iter().map(input_value_document).collect()),
            interfaces: has_fields.then(|| self.interfaces.iter().map(named_document).collect()),
            enum_values: (self.kind == ENUM_KIND)
                .then(|| self.enum_values.iter().map(enum_value_document).collect()),
            possible_types: self.possible_type_documents(api_types),
            directives: Vec::new(),
        }
    }

    /// The types this one can resolve to at runtime, as the documents they are reported
    /// as: a union's members, and the object types of the API that implement an
    /// interface. Every other kind can resolve to none, which leaves the member out.
    fn possible_type_documents(
        &self,
        api_types: &BTreeMap<String, TypeRecord>,
    ) -> Option<Vec<TypeReferenceDocument>> {
        match self.kind {
            UNION_KIND => Some(self.members.iter().map(named_document).collect()),
            INTERFACE_KIND => Some(
                api_types
                    .values()
                    .filter(|record| {
                        record.parsed.kind == OBJECT_KIND
                            && record.parsed.interfaces.contains(&self.name)
                    })
                    .map(|record| named_document(&record.parsed.name))
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// One type as this service writes it in JSON.
///
/// The members are declared in the order the document writes them, and a member the type
/// has no value for is left out of it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeDocument {
    name: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<Vec<FieldDocument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_fields: Option<Vec<InputValueDocument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interfaces: Option<Vec<TypeReferenceDocument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<EnumValueDocument>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    possible_types: Option<Vec<TypeReferenceDocument>>,
    /// The directives written on the type. A type definition this service stores carries
    /// none of its own, so the list is written empty rather than left out: the document
    /// always states which directives a type carries.
    directives: Vec<Value>,
}

/// One field of an object or interface type, as the document writes it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FieldDocument {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#type: Option<TypeReferenceDocument>,
    args: Vec<InputValueDocument>,
    directives: Vec<Value>,
    is_deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    deprecation_reason: Option<String>,
}

/// One argument of a field, or one field of an input object type: the same shape in
/// GraphQL and so in this document.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InputValueDocument {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#type: Option<TypeReferenceDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_value: Option<String>,
}

/// One value of an enum type, as the document writes it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnumValueDocument {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    directives: Vec<Value>,
    is_deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    deprecation_reason: Option<String>,
}

/// One type reference: the name a named reference states, or the kind a list or non-null
/// wrapper is together with the reference it wraps.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeReferenceDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    of_type: Option<Box<TypeReferenceDocument>>,
}

/// One field, with the type it resolves to, the arguments it takes and the deprecation it
/// carries.
fn field_document(field: &FieldDefinition) -> FieldDocument {
    FieldDocument {
        name: field.name.clone(),
        description: field.description.clone(),
        r#type: field.value_type.as_ref().map(type_reference_document),
        args: field.arguments.iter().map(input_value_document).collect(),
        directives: Vec::new(),
        is_deprecated: field.deprecation.is_deprecated,
        deprecation_reason: field.deprecation.reason.clone(),
    }
}

/// One argument or input field, with the type it takes and the default value it states.
fn input_value_document(input: &InputValueDefinition) -> InputValueDocument {
    InputValueDocument {
        name: input.name.clone(),
        description: input.description.clone(),
        r#type: input.value_type.as_ref().map(type_reference_document),
        default_value: input.default_value.clone(),
    }
}

/// One enum value, with the deprecation it carries.
fn enum_value_document(value: &EnumValueDefinition) -> EnumValueDocument {
    EnumValueDocument {
        name: value.name.clone(),
        description: value.description.clone(),
        directives: Vec::new(),
        is_deprecated: value.deprecation.is_deprecated,
        deprecation_reason: value.deprecation.reason.clone(),
    }
}

/// One type reference: a name, or a wrapper's kind around the reference it wraps.
///
/// Built from the name at the reference's root OUTWARDS over the wrappers it nests
/// (`TypeRef::flattened`), so a reference costs one step per wrapper and no stack frame
/// per wrapper however deeply the stored definition nested it.
fn type_reference_document(value_type: &TypeRef) -> TypeReferenceDocument {
    let (wrappers, name) = value_type.flattened();
    let mut document = named_document(name);
    for wrapper in wrappers.iter().rev() {
        let kind = match wrapper {
            TypeWrapper::List => LIST_KIND,
            TypeWrapper::NonNull => NON_NULL_KIND,
        };
        document = TypeReferenceDocument {
            kind: Some(kind),
            name: None,
            of_type: Some(Box::new(document)),
        };
    }
    document
}

/// A reference to the type named `name`: the name is all such a reference states, so
/// nothing of the type it names is resolved into the document.
fn named_document(name: impl AsRef<str>) -> TypeReferenceDocument {
    TypeReferenceDocument {
        kind: None,
        name: Some(name.as_ref().to_string()),
        of_type: None,
    }
}
