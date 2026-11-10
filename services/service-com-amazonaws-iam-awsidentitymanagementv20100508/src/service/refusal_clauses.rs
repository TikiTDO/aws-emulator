// SPDX-License-Identifier: CC0-1.0

//! One CLAUSE of the model-level constraint walk's envelope, as IAM states it.
//!
//! The walk hands [`crate::service`] one [`Violation`] per constraint a request broke, and
//! the framework renders each as `Value '<the value>' at '<member>' failed to satisfy
//! constraint: <requirement>`. IAM states three of those halves differently, and this module
//! is where each difference is applied — per CLAUSE, before the envelope is rendered, so the
//! envelope, the member path and the requirement stay the framework's and cannot drift:
//!
//! * the VALUE half is dropped. A refusal of IAM's states the member and the requirement and
//!   nothing the request carried — for a value outside a declared bound, outside an
//!   enumerated set, outside a member's `@pattern`, and for a SCALAR member that was absent;
//! * a `@pattern` requirement quotes the expression the way AWS publishes one for a member:
//!   without the pair of anchors the model writes around a whole-value expression
//!   ([`::service_common::validation::unanchored`]). The CHECK still runs the model's pattern
//!   as written; only the rendering drops the pair. The member a clause is about is the
//!   member the model constrained, whether the value reached it at the top of an operation's
//!   input or inside one of its collections, so one key of a list is stated the same way as
//!   the input's own member;
//! * an enumerated set is stated in the order IAM publishes for that enum
//!   ([`PUBLISHED_ENUM_ORDERS`]) rather than the model's declaration order.
//!
//! The one clause that KEEPS the `null` the framework writes is an absent member whose model
//! type is a COLLECTION ([`names_a_collection`]): a list or a map has no scalar rendering, so
//! the clause states the absence itself, where a scalar member's clause states nothing at
//! all. Which members those are comes from the model, read off the generated schemas
//! ([`SHAPES_DECLARING_A_REQUIRED_COLLECTION_MEMBER`]) rather than named here.
//!
//! Every rule covers its whole CLASS rather than one member: the bound, the set, the pattern
//! and the member path all come from the model, so the same rule answers for every member of
//! every operation that carries one (`tests/all/range_refusal_wording.rs`,
//! `tests/all/enum_refusal_wording.rs`, `tests/all/validation_error_clauses.rs`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::constraints;
use aws_smithy_schema_server::{Violation, ViolationKind};

/// The admissible value sets this service states in an order of its OWN, each written in
/// the order IAM publishes for that enum.
///
/// A refusal of a value outside an enumerated set states the set, and the order it is
/// stated in belongs to the service: for some of IAM's enums it is not the order the model
/// happens to declare the values in. Where the two differ the published order is carried
/// here and replaces the renderer's; a set this table does not name keeps the model's
/// order, which is what the framework already states.
///
/// SOURCE, for adding an enum: the order AWS itself states the set in — the `Valid Values`
/// listing for that member in the IAM API Reference, or the bracketed set in IAM's own
/// refusal of a value outside it. That is AWS data about the enum, so an entry is added
/// only when such a statement is in hand and it disagrees with the model's declaration
/// order; an enum nobody has that for is simply absent from this table, which is why the
/// table is short rather than one entry per enum.
///
/// An entry is matched by its MEMBERSHIP — the set of values, in any order — so it answers
/// for the enum itself rather than for one member referencing it, and every member
/// targeting that enum, in any operation, is stated the same way.
const PUBLISHED_ENUM_ORDERS: &[&[&str]] = &[
    // The scope of a managed-policy listing (`iam#policyScopeType`, declared by the model
    // as `All, AWS, Local`).
    &["All", "Local", "AWS"],
    // The token version the account's global STS endpoint issues
    // (`iam#globalEndpointTokenVersion`, declared by the model as `v1Token, v2Token`).
    &["v2Token", "v1Token"],
];

/// The order this service states the admissible set `allowed` in, or `None` when the
/// model's own order stands.
///
/// The match is on membership and size, never on position, so a model that reordered its
/// own declaration of the same enum would still be stated in the published order.
fn published_enum_order(allowed: &[String]) -> Option<&'static [&'static str]> {
    PUBLISHED_ENUM_ORDERS.iter().copied().find(|published| {
        published.len() == allowed.len()
            && published
                .iter()
                .all(|value| allowed.iter().any(|declared| declared == value))
    })
}

/// Every input shape of this model that declares a `@required` member of a COLLECTION type.
///
/// Those are the only members an absent-member clause can name as a collection: a clause
/// about absence is raised for a `@required` member alone, so an optional list or map never
/// reaches [`names_a_collection`]. The entries are SHAPES, not members: which of their
/// members are collections is read off the model by [`collection_member_names`], so nothing
/// here states a member's type and the set cannot drift from the schema codegen emits.
///
/// SOURCE, for adding a shape: an operation input (or a structure one nests) whose model
/// declares a `@required` list or map member — `smithy_query(query="operation",
/// operation="<the operation>")` reports the member's `type` beside its `required`. An
/// operation this model grows therefore needs a line here; one whose required members are
/// all scalars needs none.
const SHAPES_DECLARING_A_REQUIRED_COLLECTION_MEMBER: &[&Schema] = &[
    // `Tags` (`tagListType`), required on every tagging write.
    &crate::ops::types::TagInstanceProfileInput::SCHEMA,
    &crate::ops::types::TagMFADeviceInput::SCHEMA,
    &crate::ops::types::TagOpenIDConnectProviderInput::SCHEMA,
    &crate::ops::types::TagPolicyInput::SCHEMA,
    &crate::ops::types::TagRoleInput::SCHEMA,
    &crate::ops::types::TagSAMLProviderInput::SCHEMA,
    &crate::ops::types::TagServerCertificateInput::SCHEMA,
    &crate::ops::types::TagUserInput::SCHEMA,
    // `TagKeys` (`tagKeyListType`), required on every untagging write.
    &crate::ops::types::UntagInstanceProfileInput::SCHEMA,
    &crate::ops::types::UntagMFADeviceInput::SCHEMA,
    &crate::ops::types::UntagOpenIDConnectProviderInput::SCHEMA,
    &crate::ops::types::UntagPolicyInput::SCHEMA,
    &crate::ops::types::UntagRoleInput::SCHEMA,
    &crate::ops::types::UntagSAMLProviderInput::SCHEMA,
    &crate::ops::types::UntagServerCertificateInput::SCHEMA,
    &crate::ops::types::UntagUserInput::SCHEMA,
    // `ThumbprintList` (`thumbprintListType`).
    &crate::ops::types::UpdateOpenIDConnectProviderThumbprintInput::SCHEMA,
    // `ActionNames` (`ActionNameListType`) on both simulations, and `PolicyInputList`
    // (`SimulationPolicyListType`) on the custom one.
    &crate::ops::types::SimulateCustomPolicyInput::SCHEMA,
    &crate::ops::types::SimulatePrincipalPolicyInput::SCHEMA,
    // `PolicyInputList` again, on the context-key read of a custom policy.
    &crate::ops::types::GetContextKeysForCustomPolicyInput::SCHEMA,
    // `ServiceNamespaces` (`serviceNamespaceListType`).
    &crate::ops::types::ListPoliciesGrantingServiceAccessInput::SCHEMA,
    // `Properties` (`accountPropertiesMapType`), the one required MAP member.
    &crate::ops::types::PutAccountPropertiesInput::SCHEMA,
    // `Values` (`replacementValueListType`), required on the entry an `AcquireRole`
    // request nests under its `ReplacementValues` map.
    &crate::ops::types::ReplacementValueEntry::SCHEMA,
];

/// The names a clause spells the COLLECTION members of
/// [`SHAPES_DECLARING_A_REQUIRED_COLLECTION_MEMBER`] with, built off the model rather than
/// named by hand.
///
/// Built once: the schemas are compile-time constants, so the set is a property of the
/// model and not of a request.
fn collection_member_names() -> &'static BTreeSet<String> {
    static NAMES: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
        let mut names = BTreeSet::new();
        for shape in SHAPES_DECLARING_A_REQUIRED_COLLECTION_MEMBER {
            for member in shape.members() {
                let Some(name) = member.member_name() else {
                    continue;
                };
                if matches!(member.shape_type(), ShapeType::List | ShapeType::Map) {
                    // The casing of a validation path segment belongs to the path, not to
                    // the model, so it is spelled by the framework module that owns it.
                    names.insert(constraints::path::member(name));
                }
            }
        }
        names
    });
    &NAMES
}

/// Whether `path` names a member the model types as a list or a map.
///
/// The LAST path segment decides, as it does everywhere else this crate reads a member out
/// of a clause's path ([`crate::state::member_format_refusal`]): a member reached inside a
/// nested structure, a list element or a map entry is the same member as one reached at the
/// top of an operation's input, and is typed the same way by the model.
fn names_a_collection(path: &str) -> bool {
    path.split('.')
        .next_back()
        .is_some_and(|member| collection_member_names().contains(member))
}

/// The framework's own clause for `violation` with its VALUE half dropped — `Value at
/// '<member>' failed to satisfy constraint: <requirement>` — whichever half the framework
/// wrote: the value a present member submitted, or the `null` an absent one is stated with.
///
/// Built by REMOVING that half from the framework's clause rather than by spelling a clause
/// here, so the requirement, the member path and the punctuation around them stay
/// single-homed in the framework. `None` when the framework's clause does not open with the
/// half this would remove, which is how a clause that already states no value is left alone.
fn without_the_value(violation: &Violation) -> Option<String> {
    let value_half = match violation.value.as_deref() {
        Some(stored) => format!("Value '{stored}' "),
        // How the framework writes a member that was not there at all.
        None => "Value null ".to_string(),
    };
    let stated = violation.message();
    let requirement_half = stated.strip_prefix(&value_half)?;
    Some(format!("Value {requirement_half}"))
}

/// The clause for a member that was ABSENT: the requirement alone for a scalar member, and
/// the framework's own `null` for a member the model types as a collection (see the module
/// docs).
fn absent_member_clause(violation: &Violation) -> Option<String> {
    if names_a_collection(&violation.path) {
        return None;
    }
    without_the_value(violation)
}

/// The clause for a value outside its member's `@pattern`: the member and the expression,
/// with the expression quoted as AWS publishes it and nothing of the value.
///
/// The restatement is about the MEMBER the model constrained, which a validation path names
/// in its last segment — `tagKeys.1.member` is one key of a list, constrained by the key
/// shape's own `@pattern` — so a value reached inside a collection is stated exactly as one
/// supplied for a member of the input itself (`tests/all/validation_error_clauses.rs` pins
/// the input's own members, `tests/all/server_certificate_untag_input_bounds.rs` one key of a
/// list).
fn pattern_clause(violation: &Violation, pattern: &str) -> Option<String> {
    let stated = without_the_value(violation)?;
    // The value half is already gone, so the only occurrence left is the requirement's own
    // quoting of the expression.
    Some(stated.replace(pattern, &::service_common::validation::unanchored(pattern)))
}

/// The clause for a value outside an enumerated set: the member and the set, in the order
/// IAM publishes it, and nothing of the value.
fn enum_clause(violation: &Violation, allowed: &[String]) -> Option<String> {
    let stated = without_the_value(violation)?;
    let Some(published) = published_enum_order(allowed) else {
        return Some(stated);
    };
    Some(stated.replace(
        &format!("[{}]", allowed.join(", ")),
        &format!("[{}]", published.join(", ")),
    ))
}

/// How IAM states the clause for `violation`.
///
/// Each class is restated by the rule the module docs state for it; a class none of them
/// covers — a repeated element of a `@uniqueItems` collection — keeps the framework's own
/// clause, and so does any clause the restating could not read the value half out of, so a
/// renderer that worded that half differently loses the restatement rather than the clause.
pub(super) fn stated_clause(violation: &Violation) -> String {
    let stated = match &violation.kind {
        ViolationKind::MissingRequiredMember => absent_member_clause(violation),
        ViolationKind::Pattern(pattern) => pattern_clause(violation, pattern),
        ViolationKind::EnumValue(allowed) => enum_clause(violation, allowed),
        // A value outside a BOUND its member declares: a number outside its `@range`, or a
        // string or a collection outside its `@length`.
        ViolationKind::MinValue(_)
        | ViolationKind::MaxValue(_)
        | ViolationKind::MinLength(_)
        | ViolationKind::MaxLength(_) => without_the_value(violation),
        _ => None,
    };
    stated.unwrap_or_else(|| violation.message())
}
