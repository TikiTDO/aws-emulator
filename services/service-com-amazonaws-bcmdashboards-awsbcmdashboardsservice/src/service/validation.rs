// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The centralized walk enforces the modelled input constraints and hands [`Refusal`] to
//! [`crate::service`] STRUCTURALLY — one [`Violation`] per offending member, carrying the
//! dotted path it sits at, what it failed and what it held — because no model records the
//! sentence a service says. Two choices about that sentence are this service's, and each is
//! made for a constraint CLASS rather than for an operation or a member:
//!
//! * a `@pattern` requirement is quoted as the expression this service publishes, without
//!   the pair of whole-value anchors the model wraps it in. The CHECK is unchanged — it
//!   still runs the model's pattern as written — so only the rendering drops the pair;
//! * a numeric value outside its member's declared RANGE is stated with no value half at
//!   all: the clause names the member and the bound every value of that member has to
//!   respect, and nothing of the number this request carried. A bound is a property of the
//!   member, so the sentence reads the same whatever value tripped it, at whichever end and
//!   on whichever member. Every other class keeps the framework's value half — a string
//!   outside a `@length` or a `@pattern` is stated with the string it submitted, and a
//!   member absent from the request with the `null` it was.
//!
//! The envelope, its error count, the clause, the member paths and every constraint class's
//! requirement sentence belong to [`service_common::validation_message`] — the shared home
//! for this wording — and to the framework behind it. None of them is spelled here, so this
//! service cannot word a constraint differently from the services that state the same
//! sentence. `tests/all/range_refusal_wording.rs` pins the range clause over the wire.

use service_common::validation_message::aws_validation_refusal_withholding_violations;
use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` for anything that is not an input-constraint refusal: an authentication refusal is
/// a different failure with its own status and code, and nothing about it is this service's
/// to word. The code travels with the message and stays the modelled `ValidationException`
/// (HTTP 400) a client branches on.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    aws_validation_refusal_withholding_violations(refusal, states_no_value)
}

/// Whether the clause for `violation` states no value: a numeric value outside its member's
/// declared range, at either end of it.
fn states_no_value(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some("101".to_owned()),
            sensitive: false,
        }
    }

    /// Either end of a declared range is stated with the member and the bound and no value.
    /// The wire-level counterpart, over real operations and real members, is
    /// `tests/all/range_refusal_wording.rs`.
    #[test]
    fn a_range_clause_names_the_member_and_the_bound_only() {
        for (path, kind, expected) in [
            (
                "maxResults",
                ViolationKind::MaxValue("100".to_owned()),
                "Member must have value less than or equal to 100",
            ),
            (
                "maxResults",
                ViolationKind::MinValue("1".to_owned()),
                "Member must have value greater than or equal to 1",
            ),
            (
                "widgets.1.member.width",
                ViolationKind::MaxValue("6".to_owned()),
                "Member must have value less than or equal to 6",
            ),
            (
                "widgets.1.member.height",
                ViolationKind::MinValue("4".to_owned()),
                "Member must have value greater than or equal to 4",
            ),
        ] {
            let stated = refused(&Refusal::Invalid(vec![violation(path, kind.clone())]))
                .expect("an input-constraint refusal is this service's to state");
            let expected = format!(
                "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
                 {expected}"
            );
            assert_eq!(
                stated.message, expected,
                "case {path} {kind:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// Every other class keeps the value half the framework writes: a string states what it
    /// submitted, an absent member the `null` it was.
    #[test]
    fn another_class_still_states_its_value() {
        for (kind, expected) in [
            (
                ViolationKind::MinLength(20),
                "Value '101' at 'arn' failed to satisfy constraint: Member must have length \
                 greater than or equal to 20",
            ),
            (
                ViolationKind::MaxLength(50),
                "Value '101' at 'arn' failed to satisfy constraint: Member must have length \
                 less than or equal to 50",
            ),
            (
                ViolationKind::Pattern("^[0-9a-f]{32}$".to_owned()),
                "Value '101' at 'arn' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: [0-9a-f]{32}",
            ),
        ] {
            let stated = refused(&Refusal::Invalid(vec![violation("arn", kind.clone())]))
                .expect("an input-constraint refusal is this service's to state");
            let expected = format!("1 validation error detected: {expected}");
            assert_eq!(
                stated.message, expected,
                "case {kind:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }

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

    /// An authentication refusal is not this service's to word, and the code a constraint
    /// refusal travels with stays the modelled one.
    #[test]
    fn only_an_input_constraint_refusal_is_reworded() {
        assert_eq!(
            refused(&Refusal::Unauthenticated),
            None,
            "an authentication refusal is not this service's to word"
        );
        let chosen = refused(&Refusal::Invalid(vec![violation(
            "maxResults",
            ViolationKind::MaxValue("100".to_owned()),
        )]))
        .expect("an input-constraint refusal is reworded");
        assert_eq!(
            chosen.code, "ValidationException",
            "the code stays the modelled one, got {:?}",
            chosen.code
        );
    }
}
