// SPDX-License-Identifier: CC0-1.0

//! How this service states the framework's boundary refusal of a request.
//!
//! The words are NOT written here. `service_common::validation_message` is the
//! cross-service home for them — the `N validation error(s) detected:` envelope, the
//! member paths, every requirement sentence and the unanchored `@pattern` expression —
//! and this module only makes the one choice that belongs to a service: WHICH of those
//! renderings this service's recordings show.
//!
//! The dialect those recordings show, for every constrained member of every operation of
//! this service:
//!
//! * a clause states the member path and the requirement broken and NO value the request
//!   carried — whichever member and whichever constraint class. A member that was not
//!   there at all has no value to withhold and keeps the `null` the framework states an
//!   absent member with.
//! * a `@pattern` requirement quotes the expression the service advertises: the modelled
//!   pattern with the whole-value anchors the model wrapped it in dropped. The CHECK still
//!   runs the pattern exactly as the model writes it; only the quoting differs.
//!
//! Nothing here is keyed to an operation, to a member path or to a submitted value, so one
//! rule covers `vpcId`, `gatewayId`, `linkId`, `ruleId`, `resourceArn`, `description`,
//! `domainName`, the numeric bounds of a page size and every other constrained member
//! alike.

use smithy_server_runtime::{Refusal, Refused};

/// How this service states `refusal`, or `None` to keep the framework's own wording.
///
/// `None` covers both what this service does not reword (an authentication refusal is not
/// an input-constraint failure, and its code and status are not this service's to change)
/// and a refusal with nothing to state.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    service_common::validation_message::aws_validation_refusal_withholding_violations(
        refusal,
        // The value half is dropped wherever there was a value to drop: the decision is
        // about the violation, not about a set of member paths, so a constraint class this
        // service has no recording of is stated the same way as the ones it has.
        |violation| violation.value.is_some() || violation.sensitive,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithy_server_runtime::{Violation, ViolationKind};

    /// One violation of `kind` at `path`, holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// The choice this module makes, stated where the wire cases in
    /// `tests/all/validation_message_wording.rs` cannot see it: a value the request carried
    /// is withheld for EVERY constraint class, a `@pattern` is quoted without the anchors
    /// the model wraps it in, and a member the request did not carry is still the `null` it
    /// was.
    #[test]
    fn a_submitted_value_is_withheld_and_a_pattern_loses_its_anchors() {
        // (case, the violation, the clause the message must carry)
        let rows = [
            (
                "a member outside its pattern",
                violation(
                    "vpcId",
                    ViolationKind::Pattern("^vpc-[a-f0-9]{8,17}$".to_owned()),
                    Some("not-a-valid-vpc-id"),
                ),
                "Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: vpc-[a-f0-9]{8,17}",
            ),
            (
                "a value above its range maximum",
                violation("maxResults", ViolationKind::MaxValue("10".to_owned()), Some("11")),
                "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                 less than or equal to 10",
            ),
            (
                "a value below its range minimum",
                violation("maxResults", ViolationKind::MinValue("1".to_owned()), Some("0")),
                "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                 greater than or equal to 1",
            ),
            (
                "a member past its length maximum",
                violation("description", ViolationKind::MaxLength(255), Some(&"a".repeat(256))),
                "Value at 'description' failed to satisfy constraint: Member must have length \
                 less than or equal to 255",
            ),
            (
                "an absent required member",
                violation("clientToken", ViolationKind::MissingRequiredMember, None),
                "Value null at 'clientToken' failed to satisfy constraint: Member must not be \
                 null",
            ),
        ];
        for (case, offence, clause) in rows {
            let stated = refused(&Refusal::Invalid(vec![offence]))
                .unwrap_or_else(|| panic!("case {case:?}: a constraint refusal is stated"));
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
            assert_eq!(
                stated.code, "ValidationException",
                "case {case:?}: expected the modelled client error, got {:?}",
                stated.code
            );
        }
    }

    /// Nothing but a constraint refusal is reworded, and a refusal with nothing to state
    /// keeps the framework's wording.
    #[test]
    fn nothing_but_a_constraint_refusal_is_reworded() {
        for (case, refusal) in [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("no violations at all", Refusal::Invalid(Vec::new())),
        ] {
            let stated = refused(&refusal);
            assert!(
                stated.is_none(),
                "case {case:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }
}
