// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The centralized walk enforces the modelled input constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending member, carrying the dotted
//! path it sits at, what it failed and what it held — because no model records the sentence
//! a service says. This module is the two choices this service makes about that sentence,
//! and it makes each of them for a constraint CLASS rather than for an operation or a
//! member:
//!
//! * a clause names the offending MEMBER PATH and the REQUIREMENT that member violated, and
//!   NOTHING of the value the caller sent: the requirement is what every value of that
//!   member has to satisfy rather than the one value that did not, so the refusal reads the
//!   same whatever a caller submitted. A member ABSENT from the request is the one
//!   exception, stated as the `null` it was — a fact of the request's shape and not a
//!   rendering of a value;
//! * an enumerated type's value SET is listed in the order this front door walks its own
//!   table of the admissible values — a table keyed on each value's own text, read bucket by
//!   bucket ([`in_hashed_enum_order`]) — and not in the order the model declares them. That
//!   order is derived from the values themselves, so it belongs to the enumerated TYPE: one
//!   type is listed alike wherever it is carried, at whatever depth and in whatever
//!   operation, and a type the model gains needs nothing added here.
//!
//! The envelope, the clause, the member paths, the `@pattern` rendering and every
//! constraint class's requirement belong to [`service_common::validation_message`] — the
//! shared home for this wording — and to the framework behind it. None of them is spelled
//! here, so this service cannot word a constraint differently from the services that state
//! the same sentence.

use service_common::validation_message::{
    aws_validation_refusal_withholding_violations, in_hashed_enum_order,
};
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// `None` for anything that is not an input-constraint refusal: an authentication refusal is
/// a different failure with its own code and status, and nothing about it is this service's
/// to word. The code travels with the message and stays the modelled `ValidationException`
/// (HTTP 400) a client branches on.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // An enumerated type's admissible values are listed in the order the table of them is
    // walked rather than in the order the schema presented them, so each violation is
    // re-ordered BEFORE the envelope words it: only the values move, and every other
    // violation class comes back untouched. Doing it on the one refusal this service states
    // is what makes the order hold for every enum of this model.
    let stated = Refusal::Invalid(violations.iter().map(in_hashed_enum_order).collect());
    aws_validation_refusal_withholding_violations(&stated, states_no_value)
}

/// Whether the clause for `violation` states no value: every class but a member ABSENT from
/// the request, which is stated as the `null` it was rather than with no value at all.
fn states_no_value(violation: &Violation) -> bool {
    !matches!(violation.kind, ViolationKind::MissingRequiredMember)
}

#[cfg(test)]
mod tests {
    use super::*;
    use service_common::validation_message::hashed_enum_order;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some("submitted".to_owned()),
            sensitive: false,
        }
    }

    /// Every constraint class of a PRESENT member states its requirement and no value. The
    /// wire-level counterpart, over real operations, is
    /// `tests/all/validation_message_wording.rs`.
    #[test]
    fn a_clause_names_the_member_and_the_requirement_only() {
        for (kind, expected) in [
            (
                ViolationKind::MinLength(3),
                "Member must have length greater than or equal to 3",
            ),
            (
                ViolationKind::MaxLength(63),
                "Member must have length less than or equal to 63",
            ),
            (
                ViolationKind::MinValue("1".to_owned()),
                "Member must have value greater than or equal to 1",
            ),
            (
                ViolationKind::MaxValue("100".to_owned()),
                "Member must have value less than or equal to 100",
            ),
            (
                ViolationKind::Pattern("^[a-z0-9]+$".to_owned()),
                "Member must satisfy regular expression pattern: [a-z0-9]+",
            ),
            (ViolationKind::UniqueItems, "Member must have unique values"),
        ] {
            let stated = refused(&Refusal::Invalid(vec![violation("name", kind.clone())]))
                .expect("an input-constraint refusal is this service's to state");
            let expected = format!(
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 {expected}"
            );
            assert_eq!(
                stated.message, expected,
                "case {kind:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// A member absent from the request is the one clause that keeps a value half, the
    /// `null` the request's shape states.
    #[test]
    fn an_absent_member_is_stated_as_null() {
        let absent = Violation {
            path: "name".to_owned(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let stated = refused(&Refusal::Invalid(vec![absent]))
            .expect("an absent required member is a constraint refusal");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value null at 'name' failed to satisfy constraint: \
             Member must not be null",
            "an absent member states the null it was, got {:?}",
            stated.message
        );
    }

    /// An enumerated type's value set is listed in the order the table of them is walked,
    /// and no submitted value reaches the clause. The order is derived from the values
    /// themselves, so a type no two of whose values share a bucket is listed the same way
    /// whichever order the schema presented them in.
    #[test]
    fn an_enum_value_set_is_listed_in_the_order_the_table_is_walked() {
        // The model declares `IdentityProvider` [IAM, IDC]
        // (`smithy_query(query="shape", shape="IdentityProvider")`); the two sit in buckets
        // of their own, so the listing cannot depend on the presentation.
        let declared = ["IAM", "IDC"];
        let listed = format!("[{}]", hashed_enum_order(&declared).join(", "));
        let reversed: Vec<&str> = declared.iter().rev().copied().collect();
        for (label, presented) in [
            ("as the model declares it", declared.to_vec()),
            ("reversed", reversed),
        ] {
            let refusal = Refusal::Invalid(vec![violation(
                "identityProviders.1.member",
                ViolationKind::EnumValue(
                    presented.iter().map(|value| (*value).to_owned()).collect(),
                ),
            )]);
            let stated = refused(&refusal).expect("an enum value set is a constraint refusal");
            let expected = format!(
                "1 validation error detected: Value at 'identityProviders.1.member' failed to \
                 satisfy constraint: Member must satisfy enum value set: {listed}"
            );
            assert_eq!(
                stated.message, expected,
                "case {label:?}: expected {expected:?}, got {:?}",
                stated.message
            );
            assert!(
                !stated.message.contains("submitted"),
                "case {label:?}: expected no submitted value in the clause, got {:?}",
                stated.message
            );
        }
    }

    /// An authentication refusal keeps the framework's wording.
    #[test]
    fn only_an_input_constraint_refusal_is_reworded() {
        assert_eq!(
            refused(&Refusal::Unauthenticated),
            None,
            "an authentication refusal is not this service's to word"
        );
        let chosen = refused(&Refusal::Invalid(vec![violation(
            "name",
            ViolationKind::MinLength(3),
        )]))
        .expect("an input-constraint refusal is reworded");
        assert_eq!(
            chosen.code, "ValidationException",
            "the code stays the modelled one, got {:?}",
            chosen.code
        );
    }
}
