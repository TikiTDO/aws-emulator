// SPDX-License-Identifier: CC0-1.0

//! The GraphQL types a GraphQL API holds: how one caller-supplied type definition is
//! parsed, kept and reported in either format the model names.
//!
//! A type arrives as "The type definition, in GraphQL Schema Definition Language (SDL)
//! format" (`CreateTypeRequest$definition`), so a stored type keeps the SDL text the
//! caller wrote together with the definition that text parses to. The
//! `TypeDefinitionFormat` a call names then chooses the REPRESENTATION the reported
//! `Type$definition` carries — the SDL the type was defined with, or the type's
//! introspection JSON — and that JSON is computed from the definitions the API holds,
//! never from the request.
//!
//! The parse is a real GraphQL parse (`apollo-parser`), so what the service accepts is
//! what GraphQL's SDL grammar accepts, and a definition it cannot parse is refused
//! rather than stored as text nothing understands.

use std::collections::BTreeMap;

use apollo_parser::cst::{self, CstNode};
use apollo_parser::Parser;
use serde_json::{json, Value};

use crate::ops::types::Type;

/// The `TypeDefinitionFormat` value naming a definition written in GraphQL SDL.
pub const SDL_FORMAT: &str = "SDL";

/// The `TypeDefinitionFormat` value naming a definition reported as the type's
/// introspection JSON.
pub const JSON_FORMAT: &str = "JSON";

/// The deepest nesting the GraphQL parser descends into one caller-supplied type
/// definition, and the most tokens it reads from one. `CreateTypeRequest$definition`
/// and `UpdateTypeRequest$definition` carry no modelled `@length`, so without these a
/// single request chooses how much stack and CPU one parse costs; past either bound the
/// parse fails and the request is refused.
pub const DEFINITION_RECURSION_LIMIT: usize = 64;

/// See [`DEFINITION_RECURSION_LIMIT`].
pub const DEFINITION_TOKEN_LIMIT: usize = 100_000;

/// The `__TypeKind` an SDL `type` definition has, as GraphQL's introspection names it.
pub const OBJECT_KIND: &str = "OBJECT";

/// The `__TypeKind` an SDL `interface` definition has.
pub const INTERFACE_KIND: &str = "INTERFACE";

/// The `__TypeKind` an SDL `input` definition has.
pub const INPUT_OBJECT_KIND: &str = "INPUT_OBJECT";

/// The `__TypeKind` an SDL `enum` definition has.
pub const ENUM_KIND: &str = "ENUM";

/// The `__TypeKind` an SDL `union` definition has.
pub const UNION_KIND: &str = "UNION";

/// The `__TypeKind` an SDL `scalar` definition has.
pub const SCALAR_KIND: &str = "SCALAR";

/// The `__TypeKind` a list type REFERENCE has (`[Post]`).
pub const LIST_KIND: &str = "LIST";

/// The `__TypeKind` a non-null type REFERENCE has (`Post!`).
pub const NON_NULL_KIND: &str = "NON_NULL";

/// The scalar types every AppSync schema holds without defining them: GraphQL's five
/// built-in scalars (`Int`, `Float`, `String`, `Boolean`, `ID`) and the nine AppSync
/// adds (`AWSDate`, `AWSTime`, `AWSDateTime`, `AWSTimestamp`, `AWSEmail`, `AWSJSON`,
/// `AWSURL`, `AWSPhone`, `AWSIPAddress`).
///
/// A reference to one of these names resolves to a [`SCALAR_KIND`] even though no
/// `CreateType` call defined it, which is what lets a field's type reference carry its
/// kind without the schema holding a definition for it.
pub const BUILT_IN_SCALARS: &[&str] = &[
    "Int",
    "Float",
    "String",
    "Boolean",
    "ID",
    "AWSDate",
    "AWSTime",
    "AWSDateTime",
    "AWSTimestamp",
    "AWSEmail",
    "AWSJSON",
    "AWSURL",
    "AWSPhone",
    "AWSIPAddress",
];

/// The directive a field or enum value is marked deprecated with.
const DEPRECATED_DIRECTIVE: &str = "deprecated";

/// The argument [`DEPRECATED_DIRECTIVE`] states its reason in.
const DEPRECATED_REASON_ARGUMENT: &str = "reason";

/// The reason introspection reports for a deprecation whose directive names none:
/// `@deprecated`'s `reason` argument has exactly this default in GraphQL's own
/// definition of the directive.
const DEFAULT_DEPRECATION_REASON: &str = "No longer supported";

/// One stored GraphQL type of a `GraphqlApiRecord`.
///
/// Keeps the definition as the caller wrote it AND the definition it parses to: the
/// first is what an SDL report answers with, the second is what a JSON report is
/// rendered from, so the two representations of one type cannot disagree.
#[derive(Debug, Clone)]
pub struct TypeRecord {
    /// Creation order, used to keep a listing (and its `nextToken` cursor) stable
    /// across calls.
    pub seq: u64,
    /// The type's ARN, named under its API's own path.
    pub arn: String,
    /// The SDL text the create (or the last update) carried, kept verbatim.
    pub definition: String,
    /// What [`Self::definition`] parses to.
    pub parsed: TypeDefinition,
}

impl TypeRecord {
    /// This record as the modelled `Type` shape, with its definition in `format`.
    ///
    /// The single projection every operation that reports a type answers with, so a
    /// create and the reads after it cannot disagree about one type. `api_types` is the
    /// set of types the type's own API holds, which the JSON rendering resolves type
    /// references against.
    pub fn to_type(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> Type {
        Type {
            name: Some(self.parsed.name.clone()),
            description: self.parsed.description.clone(),
            arn: Some(self.arn.clone()),
            definition: Some(self.definition_in(format, api_types)),
            format: Some(format.to_string()),
        }
    }

    /// This type's definition in `format`: the stored SDL, or the type's introspection
    /// JSON rendered from the stored definitions.
    fn definition_in(&self, format: &str, api_types: &BTreeMap<String, TypeRecord>) -> String {
        if format == JSON_FORMAT {
            serde_json::to_string_pretty(&self.introspection(api_types)).unwrap_or_default()
        } else {
            self.definition.clone()
        }
    }

    /// This type as a GraphQL introspection `__Type`: the JSON representation of a
    /// GraphQL type, as the introspection system defines it.
    ///
    /// Reached by the whole SCHEMA's introspection too (`super::introspection_schema`),
    /// so one type reported on its own and the same type reported as part of the schema
    /// cannot disagree.
    ///
    /// Every member is taken from the stored definition, and every type REFERENCE is
    /// resolved against the types the API holds plus the schema's built-in scalars
    /// ([`BUILT_IN_SCALARS`]) — so an interface reports the object types that implement
    /// it, and a reference to a type nothing has defined carries its name with no kind
    /// rather than a kind the schema does not support.
    pub(crate) fn introspection(&self, api_types: &BTreeMap<String, TypeRecord>) -> Value {
        let parsed = &self.parsed;
        let has_fields = parsed.kind == OBJECT_KIND || parsed.kind == INTERFACE_KIND;
        json!({
            "kind": parsed.kind,
            "name": parsed.name,
            "description": parsed.description,
            "fields": has_fields.then(|| {
                parsed
                    .fields
                    .iter()
                    .map(|field| field_json(field, api_types))
                    .collect::<Vec<Value>>()
            }),
            "inputFields": (parsed.kind == INPUT_OBJECT_KIND).then(|| {
                parsed
                    .input_fields
                    .iter()
                    .map(|input| input_value_json(input, api_types))
                    .collect::<Vec<Value>>()
            }),
            "interfaces": has_fields.then(|| {
                parsed
                    .interfaces
                    .iter()
                    .map(|name| named_type_json(name, api_types))
                    .collect::<Vec<Value>>()
            }),
            "enumValues": (parsed.kind == ENUM_KIND).then(|| {
                parsed
                    .enum_values
                    .iter()
                    .map(enum_value_json)
                    .collect::<Vec<Value>>()
            }),
            "possibleTypes": self.possible_types(api_types),
        })
    }

    /// The types this one can resolve to at runtime: a union's members, and the object
    /// types that implement an interface. Every other kind has none, which
    /// introspection reports as null rather than as an empty list.
    fn possible_types(&self, api_types: &BTreeMap<String, TypeRecord>) -> Value {
        match self.parsed.kind {
            UNION_KIND => Value::Array(
                self.parsed
                    .members
                    .iter()
                    .map(|name| named_type_json(name, api_types))
                    .collect(),
            ),
            INTERFACE_KIND => Value::Array(
                api_types
                    .values()
                    .filter(|record| {
                        record.parsed.kind == OBJECT_KIND
                            && record.parsed.interfaces.contains(&self.parsed.name)
                    })
                    .map(|record| named_type_json(&record.parsed.name, api_types))
                    .collect(),
            ),
            _ => Value::Null,
        }
    }
}

/// One parsed GraphQL type definition: the type's kind, its name and the members that
/// kind carries.
///
/// Only the members the kind can hold are filled, so a projection reads them by kind
/// rather than guessing from what is empty.
#[derive(Debug, Clone, Default)]
pub struct TypeDefinition {
    /// The `__TypeKind` the SDL keyword declares ([`OBJECT_KIND`] and the rest).
    pub kind: &'static str,
    pub name: String,
    /// The definition's description, i.e. the SDL description string written in front
    /// of it, which is what a GraphQL type's description is.
    pub description: Option<String>,
    /// The interfaces an object or interface type implements.
    pub interfaces: Vec<String>,
    /// The fields an object or interface type declares.
    pub fields: Vec<FieldDefinition>,
    /// The input fields an input object type declares.
    pub input_fields: Vec<InputValueDefinition>,
    /// The values an enum type declares.
    pub enum_values: Vec<EnumValueDefinition>,
    /// The member types a union type names.
    pub members: Vec<String>,
}

/// One field of a parsed object or interface type.
#[derive(Debug, Clone)]
pub struct FieldDefinition {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<InputValueDefinition>,
    /// The type the field resolves to, or `None` for a field whose definition names
    /// none.
    pub value_type: Option<TypeRef>,
    pub deprecation: Deprecation,
}

/// One argument of a field, or one field of an input object type: the two are the same
/// shape in GraphQL (`__InputValue`) and in SDL.
#[derive(Debug, Clone)]
pub struct InputValueDefinition {
    pub name: String,
    pub description: Option<String>,
    pub value_type: Option<TypeRef>,
    /// The default value as GraphQL writes it, which is the form introspection reports
    /// it in ("a GraphQL-formatted string representing the default value").
    pub default_value: Option<String>,
}

/// One value of a parsed enum type.
#[derive(Debug, Clone)]
pub struct EnumValueDefinition {
    pub name: String,
    pub description: Option<String>,
    pub deprecation: Deprecation,
}

/// Whether a field or enum value is deprecated, and why.
#[derive(Debug, Clone, Default)]
pub struct Deprecation {
    pub is_deprecated: bool,
    pub reason: Option<String>,
}

/// A reference to a type from a field or an argument: a name, a list of them, or a
/// non-null one.
#[derive(Debug, Clone)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

/// Why a caller-supplied type definition is not one type definition this service can
/// store.
#[derive(Debug, Clone)]
pub enum TypeDefinitionError {
    /// The definition is not well-formed GraphQL, carrying the parser's own account of
    /// the first thing wrong with it.
    Unparseable(String),
    /// The definition parses but declares no type (an empty document, or one that only
    /// declares something a type cannot be, such as an operation or a directive).
    NoTypeDeclared,
    /// The definition declares more than one type, and `CreateType` "Creates a Type
    /// object" — one — so there is no single type for the call to answer with.
    MoreThanOneTypeDeclared,
}

impl TypeDefinitionError {
    /// The text a `BadRequestException` (HTTP 400) carries for this refusal: what about
    /// the definition the caller has to fix.
    ///
    /// The requirement the refusal answers is the status and the error shape; the
    /// wording follows the service's other malformed-request sentences, with the
    /// parser's account of a definition it could not read carried through so the caller
    /// can fix it from the message alone.
    pub fn message(&self) -> String {
        match self {
            Self::Unparseable(detail) => format!("Invalid type definition: {detail}"),
            Self::NoTypeDeclared => {
                "Invalid type definition: it must declare one GraphQL type.".to_string()
            }
            Self::MoreThanOneTypeDeclared => {
                "Invalid type definition: it must declare one GraphQL type, not several."
                    .to_string()
            }
        }
    }
}

/// Parse one caller-supplied type definition, written in GraphQL SDL.
///
/// A type definition declares exactly one type — `CreateType` "Creates a Type object"
/// and addresses it by that type's name afterwards — so a document that declares none,
/// or more than one, is refused rather than stored.
///
/// The name a definition declares is a GraphQL name (`[_A-Za-z][_0-9A-Za-z]*`), which
/// is the alphabet the modelled `ResourceName` admits, so a parsed name can always be
/// addressed back by `GetType`, `UpdateType` and `DeleteType`.
///
/// The parse is bounded by [`DEFINITION_RECURSION_LIMIT`] and
/// [`DEFINITION_TOKEN_LIMIT`], so neither the depth nor the amount of work one
/// caller-supplied definition costs comes from the request.
pub fn parse_type_definition(definition: &str) -> Result<TypeDefinition, TypeDefinitionError> {
    let tree = Parser::new(definition)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    if let Some(error) = tree.errors().next() {
        return Err(TypeDefinitionError::Unparseable(error.message().to_string()));
    }

    let mut declared: Option<TypeDefinition> = None;
    for node in tree.document().definitions() {
        // Everything else a GraphQL document can declare — an operation, a fragment, a
        // directive, a schema, a type EXTENSION — declares no type, so it leaves nothing
        // for this call to create.
        let Some(parsed) = declared_type(&node) else {
            return Err(TypeDefinitionError::NoTypeDeclared);
        };
        if declared.replace(parsed?).is_some() {
            return Err(TypeDefinitionError::MoreThanOneTypeDeclared);
        }
    }

    declared.ok_or(TypeDefinitionError::NoTypeDeclared)
}

/// The type `node` declares, or `None` for a definition that declares no type at all.
///
/// The one reading of what each SDL type keyword declares, so a type parsed out of a
/// single definition and one parsed out of a whole schema document cannot differ.
fn declared_type(
    node: &cst::Definition,
) -> Option<Result<TypeDefinition, TypeDefinitionError>> {
    let parsed = match node {
        cst::Definition::ObjectTypeDefinition(object) => (|| {
            Ok(TypeDefinition {
                kind: OBJECT_KIND,
                name: name_text(object.name())?,
                description: description_text(object.description()),
                interfaces: implemented_interfaces(object.implements_interfaces()),
                fields: field_definitions(object.fields_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InterfaceTypeDefinition(interface) => (|| {
            Ok(TypeDefinition {
                kind: INTERFACE_KIND,
                name: name_text(interface.name())?,
                description: description_text(interface.description()),
                interfaces: implemented_interfaces(interface.implements_interfaces()),
                fields: field_definitions(interface.fields_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InputObjectTypeDefinition(input) => (|| {
            Ok(TypeDefinition {
                kind: INPUT_OBJECT_KIND,
                name: name_text(input.name())?,
                description: description_text(input.description()),
                input_fields: input_field_definitions(input.input_fields_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::EnumTypeDefinition(enumeration) => (|| {
            Ok(TypeDefinition {
                kind: ENUM_KIND,
                name: name_text(enumeration.name())?,
                description: description_text(enumeration.description()),
                enum_values: enum_value_definitions(enumeration.enum_values_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::UnionTypeDefinition(union) => (|| {
            Ok(TypeDefinition {
                kind: UNION_KIND,
                name: name_text(union.name())?,
                description: description_text(union.description()),
                members: union_members(union.union_member_types()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::ScalarTypeDefinition(scalar) => (|| {
            Ok(TypeDefinition {
                kind: SCALAR_KIND,
                name: name_text(scalar.name())?,
                description: description_text(scalar.description()),
                ..TypeDefinition::default()
            })
        })(),
        _ => return None,
    };
    Some(parsed)
}

/// The type `node` EXTENDS, as the members the extension contributes to it, or `None`
/// for a definition that extends no type.
///
/// An extension carries no description of its own — GraphQL's SDL grammar gives a
/// description only to a definition — so what it contributes is members alone.
fn extended_type(node: &cst::Definition) -> Option<Result<TypeDefinition, TypeDefinitionError>> {
    let parsed = match node {
        cst::Definition::ObjectTypeExtension(object) => (|| {
            Ok(TypeDefinition {
                kind: OBJECT_KIND,
                name: name_text(object.name())?,
                interfaces: implemented_interfaces(object.implements_interfaces()),
                fields: field_definitions(object.fields_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InterfaceTypeExtension(interface) => (|| {
            Ok(TypeDefinition {
                kind: INTERFACE_KIND,
                name: name_text(interface.name())?,
                interfaces: implemented_interfaces(interface.implements_interfaces()),
                fields: field_definitions(interface.fields_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::InputObjectTypeExtension(input) => (|| {
            Ok(TypeDefinition {
                kind: INPUT_OBJECT_KIND,
                name: name_text(input.name())?,
                input_fields: input_field_definitions(input.input_fields_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::EnumTypeExtension(enumeration) => (|| {
            Ok(TypeDefinition {
                kind: ENUM_KIND,
                name: name_text(enumeration.name())?,
                enum_values: enum_value_definitions(enumeration.enum_values_definition()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::UnionTypeExtension(union) => (|| {
            Ok(TypeDefinition {
                kind: UNION_KIND,
                name: name_text(union.name())?,
                members: union_members(union.union_member_types()),
                ..TypeDefinition::default()
            })
        })(),
        cst::Definition::ScalarTypeExtension(scalar) => (|| {
            Ok(TypeDefinition {
                kind: SCALAR_KIND,
                name: name_text(scalar.name())?,
                ..TypeDefinition::default()
            })
        })(),
        _ => return None,
    };
    Some(parsed)
}

/// The input fields a `{ … }` input field set declares.
fn input_field_definitions(
    fields: Option<cst::InputFieldsDefinition>,
) -> Vec<InputValueDefinition> {
    fields
        .map(|fields| {
            fields
                .input_value_definitions()
                .map(input_value_definition)
                .collect()
        })
        .unwrap_or_default()
}

/// The values an `{ … }` enum value set declares.
fn enum_value_definitions(
    values: Option<cst::EnumValuesDefinition>,
) -> Vec<EnumValueDefinition> {
    values
        .map(|values| {
            values
                .enum_value_definitions()
                .map(enum_value_definition)
                .collect()
        })
        .unwrap_or_default()
}

/// The member types a `= A | B` union member list names.
fn union_members(members: Option<cst::UnionMemberTypes>) -> Vec<String> {
    members
        .map(|members| named_type_names(members.named_types()))
        .unwrap_or_default()
}

/// The text of a declared name, or [`TypeDefinitionError::NoTypeDeclared`] for a
/// definition that names nothing.
fn name_text(name: Option<cst::Name>) -> Result<String, TypeDefinitionError> {
    name.map(|name| name.text().to_string())
        .filter(|name| !name.is_empty())
        .ok_or(TypeDefinitionError::NoTypeDeclared)
}

/// The text of an SDL description, which is a string value written in front of the
/// definition it describes.
fn description_text(description: Option<cst::Description>) -> Option<String> {
    description
        .and_then(|description| description.string_value())
        .map(String::from)
}

/// The names an `implements` clause lists.
fn implemented_interfaces(interfaces: Option<cst::ImplementsInterfaces>) -> Vec<String> {
    interfaces
        .map(|interfaces| named_type_names(interfaces.named_types()))
        .unwrap_or_default()
}

/// The names of a list of named type references.
fn named_type_names(types: impl Iterator<Item = cst::NamedType>) -> Vec<String> {
    types
        .filter_map(|named| named.name().map(|name| name.text().to_string()))
        .collect()
}

/// The fields a `{ … }` field set declares, in the order the definition writes them —
/// the order introspection reports them in.
fn field_definitions(fields: Option<cst::FieldsDefinition>) -> Vec<FieldDefinition> {
    fields
        .map(|fields| {
            fields
                .field_definitions()
                .map(|field| FieldDefinition {
                    name: field
                        .name()
                        .map(|name| name.text().to_string())
                        .unwrap_or_default(),
                    description: description_text(field.description()),
                    arguments: field
                        .arguments_definition()
                        .map(|arguments| {
                            arguments
                                .input_value_definitions()
                                .map(input_value_definition)
                                .collect()
                        })
                        .unwrap_or_default(),
                    value_type: type_reference(field.ty()),
                    deprecation: deprecation(field.directives()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One argument or input field as its parsed form.
pub(crate) fn input_value_definition(input: cst::InputValueDefinition) -> InputValueDefinition {
    InputValueDefinition {
        name: input
            .name()
            .map(|name| name.text().to_string())
            .unwrap_or_default(),
        description: description_text(input.description()),
        value_type: type_reference(input.ty()),
        default_value: input
            .default_value()
            .and_then(|default| default.value())
            .map(|value| value.syntax().text().to_string().trim().to_string()),
    }
}

/// One enum value as its parsed form.
fn enum_value_definition(value: cst::EnumValueDefinition) -> EnumValueDefinition {
    EnumValueDefinition {
        name: value
            .enum_value()
            .and_then(|value| value.name())
            .map(|name| name.text().to_string())
            .unwrap_or_default(),
        description: description_text(value.description()),
        deprecation: deprecation(value.directives()),
    }
}

/// The type a field or argument names, as a reference tree.
fn type_reference(value_type: Option<cst::Type>) -> Option<TypeRef> {
    match value_type? {
        cst::Type::NamedType(named) => Some(TypeRef::Named(
            named.name().map(|name| name.text().to_string())?,
        )),
        cst::Type::ListType(list) => Some(TypeRef::List(Box::new(type_reference(list.ty())?))),
        cst::Type::NonNullType(non_null) => {
            let inner = match (non_null.named_type(), non_null.list_type()) {
                (Some(named), _) => {
                    TypeRef::Named(named.name().map(|name| name.text().to_string())?)
                }
                (_, Some(list)) => TypeRef::List(Box::new(type_reference(list.ty())?)),
                (None, None) => return None,
            };
            Some(TypeRef::NonNull(Box::new(inner)))
        }
    }
}

/// Whether the directives a field or enum value carries deprecate it, and the reason
/// they state.
fn deprecation(directives: Option<cst::Directives>) -> Deprecation {
    let Some(directives) = directives else {
        return Deprecation::default();
    };
    for directive in directives.directives() {
        if directive
            .name()
            .map(|name| name.text().to_string())
            .is_none_or(|name| name != DEPRECATED_DIRECTIVE)
        {
            continue;
        }
        let reason = directive
            .arguments()
            .and_then(|arguments| {
                arguments
                    .arguments()
                    .filter(|argument| {
                        argument
                            .name()
                            .is_some_and(|name| name.text() == DEPRECATED_REASON_ARGUMENT)
                    })
                    .find_map(|argument| match argument.value() {
                        Some(cst::Value::StringValue(reason)) => Some(String::from(reason)),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| DEFAULT_DEPRECATION_REASON.to_string());
        return Deprecation {
            is_deprecated: true,
            reason: Some(reason),
        };
    }
    Deprecation::default()
}

/// One field as introspection's `__Field`.
fn field_json(field: &FieldDefinition, api_types: &BTreeMap<String, TypeRecord>) -> Value {
    json!({
        "name": field.name,
        "description": field.description,
        "args": field
            .arguments
            .iter()
            .map(|argument| input_value_json(argument, api_types))
            .collect::<Vec<Value>>(),
        "type": type_reference_json(field.value_type.as_ref(), api_types),
        "isDeprecated": field.deprecation.is_deprecated,
        "deprecationReason": field.deprecation.reason,
    })
}

/// One argument or input field as introspection's `__InputValue`.
pub(crate) fn input_value_json(
    input: &InputValueDefinition,
    api_types: &BTreeMap<String, TypeRecord>,
) -> Value {
    json!({
        "name": input.name,
        "description": input.description,
        "type": type_reference_json(input.value_type.as_ref(), api_types),
        "defaultValue": input.default_value,
    })
}

/// One enum value as introspection's `__EnumValue`.
fn enum_value_json(value: &EnumValueDefinition) -> Value {
    json!({
        "name": value.name,
        "description": value.description,
        "isDeprecated": value.deprecation.is_deprecated,
        "deprecationReason": value.deprecation.reason,
    })
}

/// A type reference as introspection's wrapping `__Type`: a list and a non-null
/// reference carry their inner reference in `ofType` and no name of their own.
fn type_reference_json(
    value_type: Option<&TypeRef>,
    api_types: &BTreeMap<String, TypeRecord>,
) -> Value {
    match value_type {
        None => Value::Null,
        Some(TypeRef::Named(name)) => named_type_json(name, api_types),
        Some(TypeRef::List(inner)) => json!({
            "kind": LIST_KIND,
            "name": Value::Null,
            "ofType": type_reference_json(Some(inner.as_ref()), api_types),
        }),
        Some(TypeRef::NonNull(inner)) => json!({
            "kind": NON_NULL_KIND,
            "name": Value::Null,
            "ofType": type_reference_json(Some(inner.as_ref()), api_types),
        }),
    }
}

/// A named type reference as introspection's `__Type`, with the kind the API's stored
/// types give that name.
fn named_type_json(name: &str, api_types: &BTreeMap<String, TypeRecord>) -> Value {
    json!({
        "kind": kind_of(name, api_types),
        "name": name,
        "ofType": Value::Null,
    })
}

/// The kind the type `name` has in this API: the kind of its stored definition, or
/// [`SCALAR_KIND`] for a scalar every schema holds ([`BUILT_IN_SCALARS`]). `None` when
/// nothing defines it, which is reported as a reference with no kind rather than as a
/// guess.
fn kind_of(name: &str, api_types: &BTreeMap<String, TypeRecord>) -> Option<&'static str> {
    if BUILT_IN_SCALARS.contains(&name) {
        return Some(SCALAR_KIND);
    }
    api_types.get(name).map(|record| record.parsed.kind)
}

/// The AppSync service quota for types on ONE GraphQL API.
///
/// It bounds the per-API collection however a type got into it — created one at a time
/// by `CreateType` or installed by a whole schema document — so neither path can grow
/// the store without limit.
pub const TYPES_PER_API: usize = 2_500;

/// The resource portion of a type's ARN: a type is a child of its API, so it is named
/// under the API's own `apis/{apiId}` path, as `GetType`'s URI
/// (`/v1/apis/{apiId}/types/{typeName}`) addresses it.
pub fn type_resource(api_id: &str, type_name: &str) -> String {
    format!("apis/{api_id}/types/{type_name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a type the named API does not
/// hold.
///
/// The service words this one as a FIXED sentence with no variable part — it names the
/// kind of resource that was not found and nothing of the request — so there is nothing
/// to interpolate and this is a constant rather than a template.
///
/// Shared by every operation that addresses a type by name, so they cannot disagree
/// about how a missing type is reported. The wording is the one a recorded `DeleteType`
/// refusal carries; should a read ever be recorded answering a different sentence, the
/// readers would need their own text rather than this one.
pub const TYPE_NOT_FOUND_MESSAGE: &str = "Type not found.";

/// The text a `BadRequestException` (HTTP 400) carries for a create whose type name is
/// already taken within the API: the name, which is the member the caller has to
/// change.
///
/// Worded after the service's other name-taken sentence
/// (`crate::state::data_source_exists_message`).
pub fn type_exists_message(type_name: &str) -> String {
    format!("Type with name {type_name} already exists.")
}

/// One type a whole schema document declares: the SDL text the document writes it in
/// and the definition that text parses to.
///
/// The same pair a single `CreateType` stores ([`TypeRecord`]), so a type that arrived
/// inside a schema document and one created on its own are stored and reported
/// identically.
#[derive(Debug, Clone)]
pub struct DeclaredType {
    /// The document's own text for this type, the definition and every extension of it
    /// the document writes, in document order.
    pub definition: String,
    /// What [`Self::definition`] parses to.
    pub parsed: TypeDefinition,
}

/// Why a caller-supplied schema document is not a GraphQL schema this service can
/// create.
///
/// Each ground is a rule of GraphQL's own type system, checked against the parsed
/// document rather than against the request's text.
#[derive(Debug, Clone)]
pub enum SchemaDocumentError {
    /// The document is not well-formed GraphQL: reading it as GraphQL stops at this
    /// line and column — the line counted from one and the column from zero, the way
    /// the service's own diagnostic states a position.
    Unparseable {
        line: usize,
        column: usize,
    },
    /// The document declares no type at all, so it defines no schema.
    NoTypeDeclared,
    /// The document declares a type that carries no name.
    TypeWithoutName,
    /// The document declares one type name twice: a GraphQL type system defines each
    /// name once, so the second declaration has no type of its own to be.
    TypeDeclaredTwice(String),
    /// The document extends a type it never declares, which leaves the extension's
    /// members on no type.
    ExtendedTypeNotDeclared(String),
    /// Types of the document reference names nothing declares and that are no scalar
    /// every schema holds, so those references resolve to no type — every one of them,
    /// in the order the document writes them, because the schema has as many problems as
    /// it has unresolved references.
    UndefinedTypesReferenced(Vec<UndefinedTypeReference>),
    /// The document declares more types than an API can hold ([`TYPES_PER_API`]), so
    /// the type system it defines does not fit on one API.
    MoreTypesThanTheQuota(usize),
}

/// One reference of a schema document that resolves to no type.
#[derive(Debug, Clone)]
pub struct UndefinedTypeReference {
    /// The type whose definition carries the reference.
    pub type_name: String,
    /// The name it references.
    pub referenced: String,
}

/// The text a document that is not well-formed GraphQL is reported with: the parser's
/// own error list, naming the position in the document where reading it stops.
///
/// Recorded behaviour supplies this sentence — an API whose schema was never created is
/// answered with the parse of its EMPTY document — and everything but the line and the
/// column is fixed text. The line is counted from one and the column from zero, which is
/// where that recorded diagnostic puts an empty document's origin ([`DOCUMENT_ORIGIN`]).
pub fn invalid_syntax_detail(line: usize, column: usize) -> String {
    format!(
        "errors=[InvalidSyntaxError{{ message=Invalid Syntax ,locations=\
         [SourceLocation{{line={line}, column={column}}}]}}]"
    )
}

/// The text a document whose TYPE SYSTEM does not hold together is reported with: how
/// many problems the schema has, and each problem on its own line.
///
/// Recorded behaviour supplies the envelope and the problem sentence for a field whose
/// type the schema does not define ([`SchemaDocumentError::detail`]).
pub fn schema_problems_detail(problems: &[String]) -> String {
    format!(
        "Found {} problem(s) with the schema:\n{}",
        problems.len(),
        problems.join("\n")
    )
}

/// The line and column `index` falls at in `document`: the line counted from one and the
/// column from zero, the way the service's own diagnostic states a position
/// ([`DOCUMENT_ORIGIN`]).
fn source_location(document: &str, index: usize) -> (usize, usize) {
    let Some(read) = document.get(..index) else {
        return DOCUMENT_ORIGIN;
    };
    let line = read.matches('\n').count() + 1;
    let column = read
        .rsplit_once('\n')
        .map_or(read, |(_, last)| last)
        .chars()
        .count();
    (line, column)
}

/// A GraphQL document's own origin, as the position the service's diagnostic states:
/// line one, column zero. Reading an EMPTY document — the document an API that holds no
/// schema has — stops there, which is the position recorded behaviour reports for it.
pub const DOCUMENT_ORIGIN: (usize, usize) = (1, 0);

impl SchemaDocumentError {
    /// The detail a failed schema creation reports: what about the document has to be
    /// fixed.
    ///
    /// This is the `details` of the schema status, "Detailed information about the
    /// status of the schema creation operation", and it is what the schema itself is
    /// reported as invalid with when a later call asks for it
    /// (`GraphqlApiRecord::introspection_schema`), so the two cannot disagree about one
    /// document. A document that does not PARSE is reported as the parser's error list
    /// ([`invalid_syntax_detail`]); one that parses but defines no type system that
    /// holds together is reported as that system's problems
    /// ([`schema_problems_detail`]).
    pub fn detail(&self) -> String {
        match self {
            Self::Unparseable { line, column } => invalid_syntax_detail(*line, *column),
            _ => schema_problems_detail(&self.problems()),
        }
    }

    /// The problems the type system this document defines has, one sentence each.
    ///
    /// The sentence for a reference the schema does not define is the service's own
    /// (recorded behaviour); the remaining grounds — none of which a recording states —
    /// name the document and what about it has to be fixed.
    fn problems(&self) -> Vec<String> {
        match self {
            // Reading the document failed before any type system was read out of it, so
            // it has no type-system problem to report; the parse is reported instead
            // (see `detail`).
            Self::Unparseable { line, column } => vec![invalid_syntax_detail(*line, *column)],
            Self::NoTypeDeclared => {
                vec!["The schema document declares no GraphQL type.".to_string()]
            }
            Self::TypeWithoutName => {
                vec!["The schema document declares a type without a name.".to_string()]
            }
            Self::TypeDeclaredTwice(name) => vec![format!(
                "The schema document declares the type {name} more than once."
            )],
            Self::ExtendedTypeNotDeclared(name) => vec![format!(
                "The schema document extends the type {name}, which it does not declare."
            )],
            Self::UndefinedTypesReferenced(references) => references
                .iter()
                .map(|reference| {
                    format!(
                        "The field type '{}' is not present when resolving type '{}'.",
                        reference.referenced, reference.type_name
                    )
                })
                .collect(),
            Self::MoreTypesThanTheQuota(declared) => vec![format!(
                "The schema document declares {declared} types, more than the \
                 {TYPES_PER_API} an API can hold."
            )],
        }
    }
}

/// Parse one caller-supplied schema document, written in GraphQL schema language, into
/// the types it declares — in document order.
///
/// A schema document declares a whole type system rather than the one type
/// [`parse_type_definition`] reads, so every type definition it writes is kept, a type
/// extension contributes its members to the type of that name, and the definitions a
/// type system carries that are no type at all (a `schema` definition naming the root
/// operation types, a directive definition) declare no type and are left to the
/// document.
///
/// The type system the document defines has to hold together, so the parse refuses a
/// document that declares nothing, declares one name twice, extends a type it does not
/// declare, or references a type neither it nor GraphQL defines
/// ([`BUILT_IN_SCALARS`]) — each the rule GraphQL's own type system states, and each
/// reported as the detail of a FAILED creation rather than stored as a schema nothing
/// resolves.
///
/// The parse is bounded by [`DEFINITION_RECURSION_LIMIT`] and
/// [`DEFINITION_TOKEN_LIMIT`], the same bounds a single definition is read under, so
/// neither the depth nor the amount of work one caller-supplied document costs comes
/// from the request.
pub fn parse_schema_document(definition: &str) -> Result<Vec<DeclaredType>, SchemaDocumentError> {
    let tree = Parser::new(definition)
        .recursion_limit(DEFINITION_RECURSION_LIMIT)
        .token_limit(DEFINITION_TOKEN_LIMIT)
        .parse();
    if let Some(error) = tree.errors().next() {
        let (line, column) = source_location(definition, error.index());
        return Err(SchemaDocumentError::Unparseable { line, column });
    }

    // Declaration order is the document's, which is the order introspection reports a
    // schema's types in, so it is kept beside the by-name index the checks below need.
    let mut order: Vec<String> = Vec::new();
    let mut declared: BTreeMap<String, DeclaredType> = BTreeMap::new();
    let mut extensions: Vec<(TypeDefinition, String)> = Vec::new();

    for node in tree.document().definitions() {
        let text = node.syntax().text().to_string().trim().to_string();
        if let Some(parsed) = declared_type(&node) {
            let parsed = parsed.map_err(|_| SchemaDocumentError::TypeWithoutName)?;
            let name = parsed.name.clone();
            if declared.contains_key(&name) {
                return Err(SchemaDocumentError::TypeDeclaredTwice(name));
            }
            order.push(name.clone());
            declared.insert(
                name,
                DeclaredType {
                    definition: text,
                    parsed,
                },
            );
            continue;
        }
        if let Some(parsed) = extended_type(&node) {
            let parsed = parsed.map_err(|_| SchemaDocumentError::TypeWithoutName)?;
            extensions.push((parsed, text));
        }
    }

    // An extension contributes to the type of its name, so it is applied after the
    // whole document has been read: GraphQL does not require a type to be declared
    // before the extension that extends it.
    for (extension, text) in extensions {
        let Some(target) = declared.get_mut(&extension.name) else {
            return Err(SchemaDocumentError::ExtendedTypeNotDeclared(extension.name));
        };
        target.parsed.extend_with(extension);
        target.definition.push_str("\n\n");
        target.definition.push_str(&text);
    }

    if order.is_empty() {
        return Err(SchemaDocumentError::NoTypeDeclared);
    }

    // The type system a document installs is bounded by the same per-API quota a type
    // created on its own counts against, so one document cannot grow the store past it.
    if order.len() > TYPES_PER_API {
        return Err(SchemaDocumentError::MoreTypesThanTheQuota(order.len()));
    }

    // Every type reference of the finished type system has to resolve, so a document
    // that names a type it does not define is refused rather than stored as a schema
    // whose fields resolve to nothing. EVERY unresolved reference is collected: the
    // schema has as many problems as it has of them, and the detail states each.
    let mut undefined: Vec<UndefinedTypeReference> = Vec::new();
    for name in &order {
        let Some(entry) = declared.get(name) else {
            continue;
        };
        for referenced in entry.parsed.referenced_types() {
            if BUILT_IN_SCALARS.contains(&referenced.as_str()) || declared.contains_key(&referenced)
            {
                continue;
            }
            undefined.push(UndefinedTypeReference {
                type_name: name.clone(),
                referenced,
            });
        }
    }
    if !undefined.is_empty() {
        return Err(SchemaDocumentError::UndefinedTypesReferenced(undefined));
    }

    Ok(order
        .iter()
        .filter_map(|name| declared.remove(name))
        .collect())
}

impl TypeDefinition {
    /// Contribute everything the extension `extension` declares to this type.
    ///
    /// A type extension "can be used to represent a type which has been extended from
    /// some original type": the members it writes are ADDED to the ones the definition
    /// already declares, in the order the document writes them, and nothing of the
    /// definition is replaced — an extension carries no description and no kind of its
    /// own to replace them with.
    fn extend_with(&mut self, extension: Self) {
        self.interfaces.extend(extension.interfaces);
        self.fields.extend(extension.fields);
        self.input_fields.extend(extension.input_fields);
        self.enum_values.extend(extension.enum_values);
        self.members.extend(extension.members);
    }

    /// Every type name this definition references: the interfaces it implements, the
    /// types a union holds, and the type of each field, field argument and input field.
    ///
    /// A list or non-null reference is a wrapper around a named one (`[Post!]!`), so
    /// what is reported is the NAME at its root — the only part that has to resolve to a
    /// declared type.
    pub(crate) fn referenced_types(&self) -> Vec<String> {
        let mut referenced: Vec<String> = Vec::new();
        referenced.extend(self.interfaces.iter().cloned());
        referenced.extend(self.members.iter().cloned());
        for field in &self.fields {
            referenced.extend(field.value_type.as_ref().map(TypeRef::root_name));
            for argument in &field.arguments {
                referenced.extend(argument.value_type.as_ref().map(TypeRef::root_name));
            }
        }
        for input_field in &self.input_fields {
            referenced.extend(input_field.value_type.as_ref().map(TypeRef::root_name));
        }
        referenced
    }
}

impl TypeRef {
    /// The name at the root of this reference, unwrapping every list and non-null
    /// around it.
    fn root_name(&self) -> String {
        match self {
            Self::Named(name) => name.clone(),
            Self::List(inner) | Self::NonNull(inner) => inner.root_name(),
        }
    }
}
