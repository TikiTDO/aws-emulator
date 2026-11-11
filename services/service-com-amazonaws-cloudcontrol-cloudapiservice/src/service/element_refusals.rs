// SPDX-License-Identifier: CC0-1.0

//! A constraint a LIST's ELEMENTS break, stated the way this service states it: at the list
//! member, quoting the list the request sent.
//!
//! ## The rule
//!
//! The framework's constraint walk reports such a violation at the ELEMENT it found it on —
//! `'<list>.<1-based position>.member'`, quoting that one element — because that is where the
//! value is. Cloud Control API reports it at the list MEMBER: the subject is the value that
//! member holds, which is the WHOLE list as the request sent it, and the requirement is the
//! element shape's own declared constraint set nested inside
//! `Member must satisfy constraint: [ … ]`. So a client that matches or reports the refusal
//! sees the member it submitted and the value it submitted for it.
//!
//! That is one rule about every list member of every operation, not about one filter, so it
//! is applied over the violations the framework's OWN walk found rather than in any handler:
//! nothing here re-checks anything, and a request the framework would accept is never refused
//! by this.
//!
//! The grouping, the nesting and the envelope are the tree's shared ones
//! (`service_common::validation_message`). What is this service's own is how it quotes a
//! `@pattern` — without the pair of anchors a model writes around a whole-value expression —
//! the ORDER its front door lists an enumerated type's value set in, and the list itself,
//! which no violation carries and only the request can answer
//! (`service_common::validation::refusing_violations_with_lists`).
//!
//! ## Where it is applied
//!
//! In [`super::StatedPerOperation`], the wrapper the operations whose input declares such a
//! list are served through, because the collecting half needs the REQUEST:
//! `Service::override_refusal` is handed the violations alone, and the value a clause states
//! at a list member is not among them. A refusal that reaches the centralized walk anyway is
//! stated through the SAME function with no list in hand, so the two paths cannot word one
//! refusal differently — that one just has only the violations to state from.

use ::std::collections::HashMap;

use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::service_common::validation::{list_of_element, unanchored};
use ::service_common::validation_message::{
    aws_validation_refusal_over_elements_stating, element_constraint_set, element_set_requirement,
    hashed_enum_order, in_hashed_enum_order, requirement,
};
use ::smithy_server_runtime::{
    Operation, Refusal, Refused, ServerHttpProtocol, Violation, ViolationKind,
};

/// The shapes whose own members carry the element constraints a grouped clause nests.
///
/// A violation path names the member the model constrained in its last segment, and a member
/// schema carries its own traits rather than the members of the shape it targets — so the
/// shapes listed here are the ones DECLARING this model's list members: both criteria of
/// `ResourceRequestStatusFilter` (`Operations` and `OperationStatuses`), which are the only
/// lists of scalar values any input of this service declares
/// (`smithy_query(query="shape",shape="ResourceRequestStatusFilter")`). An operation this
/// model grows a list member in needs the shape declaring it added here, or its clause keeps
/// the framework's own.
const SHAPES_DECLARING_A_LIST_MEMBER: &[&::aws_smithy_schema::Schema] =
    &[&crate::ops::types::ResourceRequestStatusFilter::SCHEMA];

/// The refusal this service states for `violations`.
///
/// `lists` answers the value a clause stated at a list member quotes — the list the request
/// sent, keyed by the member path the walk reports that list at — and is EMPTY on the path
/// that has no request in hand, where such a clause states no value at all rather than one
/// rebuilt from the violations: the subject is the value the member held, which only the
/// request carries.
pub(super) fn stated(violations: &[Violation], lists: &HashMap<String, String>) -> Option<Refused> {
    // Every enumerated type's value set in the order this service's front door lists one,
    // whichever clause states it.
    let restated: Vec<Violation> = violations.iter().map(in_hashed_enum_order).collect();
    aws_validation_refusal_over_elements_stating(
        &Refusal::Invalid(restated),
        // A `@pattern` is quoted as this service publishes one: without the pair of anchors
        // the model writes around a whole-value expression.
        unanchored,
        // The value a clause stated at a list member quotes: the list the request sent, in
        // the order it sent it.
        |collection| lists.get(collection).cloned(),
        // ...and the requirement it nests: the element shape's whole declared set.
        element_set,
    )
}

/// The requirement a clause stated at `collection` nests: the element shape's whole declared
/// constraint set, with an enumerated type's value set in the order this service lists it.
///
/// The set itself is the shared reading of this model's own schemas
/// ([`element_constraint_set`]) — the constraints the model declares on the element shape,
/// constraint for constraint — so it carries the constraints a value SATISFIED as well as
/// the one it broke, which no violation can. `None` where no handed-over shape declares an
/// element constraint set for that member, which leaves the clause the framework's own.
fn element_set(collection: &str) -> Option<String> {
    // The member the model constrained is the path's last segment, declared by the shape
    // that nests it.
    let member = collection.split('.').next_back()?;
    let declared = element_constraint_set(SHAPES_DECLARING_A_LIST_MEMBER, member)?;
    let requirements: Vec<String> = declared
        .into_iter()
        .map(|kind| match kind {
            ViolationKind::EnumValue(values) => requirement(ViolationKind::EnumValue(
                hashed_enum_order(&values)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            )),
            other => requirement(other),
        })
        .collect();
    Some(element_set_requirement(&requirements))
}

/// The refusal this service states for `req` under the operation `O`, and the violations it
/// is about — or `None` when the request breaks no constraint of a list's ELEMENTS, which
/// leaves the centralized walk's own path (and this service's wording of it) exactly as it
/// is.
pub(super) fn stated_from_request<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        ::service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    // Only this class needs the request. Everything else — an absent required member, a bound
    // or a pattern on a plain member — is stated identically from the violations alone, so a
    // request that breaks no element constraint is handed straight to the operation and the
    // centralized walk keeps answering it.
    if !violations
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        return None;
    }
    let refused = stated(&violations, &lists)?;
    Some((violations, refused))
}
