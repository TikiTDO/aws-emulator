// SPDX-License-Identifier: CC0-1.0

//! What a constraint refusal of a value outside an enumerated type STATES: which values
//! that type admits, in which order, and — the half this service leaves out — no submitted
//! value at all.
//!
//! Two things about such a clause are this service's rather than the framework's, and both
//! belong to the enum CLASS rather than to a member or an operation:
//!
//! * the clause names the member and the set and states NO value —
//!   `Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value
//!   set: [ … ]` — because the requirement is the set every value of that member has to
//!   come from and not the one value that was not in it, so the refusal reads the same
//!   whatever a caller sent. Every other constraint class keeps the value half the
//!   framework writes for it, which is what a `@length`, a `@range` and a `@pattern`
//!   refusal of this service state.
//! * the values are listed in the order the front door's own TABLE of the admissible values
//!   is walked — the table's buckets in ascending order, each bucket being the low bits of
//!   the value's own string hash
//!   ([`service_common::validation_message::in_hashed_enum_order`]) — and not in the order
//!   the model declares the type's members in. That order is computed from the values
//!   themselves, so it belongs to the enumerated TYPE: every member of every operation
//!   targeting one enum is listed alike, at whatever depth and in whatever binding the
//!   value arrived, and a type the model gains needs nothing added here.
//!
//! Nothing about the SENTENCE is this module's: the counted envelope, the clause, the
//! member path, the requirement each constraint class states and the mechanics of dropping
//! a clause's value half live in the shared home for boundary-refusal wording
//! ([`service_common::validation_message`]) and in the framework behind it, so this service
//! cannot word a constraint differently from the services that state the same sentence.

use ::service_common::validation_message::in_hashed_enum_order;
use ::smithy_server_runtime::{Violation, ViolationKind};

/// One violation as this service states it: an enumerated type's admissible values listed
/// in the order the table of them is walked, and every other violation unchanged.
pub(super) fn as_stated(violation: &Violation) -> Violation {
    in_hashed_enum_order(violation)
}

/// Whether the clause `violation` states names no value: a value outside an enumerated
/// type's value set, and nothing else.
pub(super) fn states_no_value(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::EnumValue(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The violation the framework hands over for `value`, outside the enumerated type
    /// whose admissible values are `declared`.
    fn enum_violation(declared: &[&str]) -> Violation {
        Violation {
            path: "status".to_string(),
            kind: ViolationKind::EnumValue(
                declared.iter().map(|value| (*value).to_string()).collect(),
            ),
            value: Some("NOT_A_MODELLED_VALUE".to_string()),
            sensitive: false,
        }
    }

    /// The values a clause lists, read back off the restated violation.
    fn listed(violation: &Violation) -> Vec<String> {
        match &violation.kind {
            ViolationKind::EnumValue(values) => values.clone(),
            other => panic!("expected an enum value set, got {other:?}"),
        }
    }

    /// A set is RE-ORDERED, never changed: every value the model declares is listed
    /// exactly once, whatever the set's size. The widest type this model declares
    /// (`IndustrySegment`, 37 values) has more values than the table has buckets, so
    /// several of its buckets hold a chain.
    #[test]
    fn every_declared_value_is_listed_exactly_once() {
        for declared in [
            crate::ops::types::INDUSTRY_SEGMENT_VALUES,
            crate::ops::types::PRIMARY_SOLUTION_TYPE_VALUES,
            crate::ops::types::INVITATION_STATUS_VALUES,
            crate::ops::types::CONNECTION_TYPE_VALUES,
            crate::ops::types::PROFILE_VISIBILITY_VALUES,
        ] {
            let stated = listed(&as_stated(&enum_violation(declared)));
            let mut sorted_stated = stated.clone();
            sorted_stated.sort_unstable();
            let mut sorted_declared: Vec<String> =
                declared.iter().map(|value| (*value).to_string()).collect();
            sorted_declared.sort_unstable();
            assert_eq!(
                sorted_stated, sorted_declared,
                "case {declared:?}: the listed set is the declared one re-ordered, got {stated:?}"
            );
        }
    }

    /// The two enumerated types this service's refusals are known to list a set for, each
    /// held to the order those refusals list it in — neither the declaration order the rows
    /// carry nor the sorted one.
    #[test]
    fn a_known_set_is_listed_in_the_order_the_refusals_list_it() {
        // (type, declared order, the order a refusal lists it in)
        let rows: &[(&str, &[&str], &[&str])] = &[
            (
                // `smithy_query(query="shape",shape="PrimarySolutionType")`.
                "PrimarySolutionType",
                &[
                    "SOFTWARE_PRODUCTS",
                    "CONSULTING_SERVICES",
                    "PROFESSIONAL_SERVICES",
                    "MANAGED_SERVICES",
                    "HARDWARE_PRODUCTS",
                    "COMMUNICATION_SERVICES",
                    "VALUE_ADDED_RESALE_AWS_SERVICES",
                    "TRAINING_SERVICES",
                ],
                &[
                    "TRAINING_SERVICES",
                    "COMMUNICATION_SERVICES",
                    "CONSULTING_SERVICES",
                    "PROFESSIONAL_SERVICES",
                    "VALUE_ADDED_RESALE_AWS_SERVICES",
                    "HARDWARE_PRODUCTS",
                    "SOFTWARE_PRODUCTS",
                    "MANAGED_SERVICES",
                ],
            ),
            (
                // `smithy_query(query="shape",shape="InvitationStatus")`.
                "InvitationStatus",
                &["PENDING", "ACCEPTED", "REJECTED", "CANCELED", "EXPIRED"],
                &["EXPIRED", "ACCEPTED", "PENDING", "CANCELED", "REJECTED"],
            ),
        ];

        for (case, declared, stated) in rows {
            let got = listed(&as_stated(&enum_violation(declared)));
            assert_eq!(
                got, *stated,
                "case {case:?}: the set is listed in the order the refusals list it in, got \
                 {got:?}"
            );
        }
    }

    /// WHICH clauses state no value: the enum class alone, so every other class keeps the
    /// framework's value half.
    #[test]
    fn only_an_enum_clause_states_no_value() {
        let stated = enum_violation(crate::ops::types::INVITATION_STATUS_VALUES);
        assert!(
            states_no_value(&stated),
            "an enum clause states no value, got {stated:?}"
        );
        for (case, kind) in [
            ("a length maximum", ViolationKind::MaxLength(64)),
            ("a length minimum", ViolationKind::MinLength(1)),
            ("a range maximum", ViolationKind::MaxValue("100".to_string())),
            (
                "a pattern",
                ViolationKind::Pattern("^[a-zA-Z0-9-]+$".to_string()),
            ),
            ("an absent required member", ViolationKind::MissingRequiredMember),
        ] {
            let other = Violation { kind, ..stated.clone() };
            assert!(
                !states_no_value(&other),
                "case {case:?}: every other class states the value it was sent, got {other:?}"
            );
        }
    }
}
