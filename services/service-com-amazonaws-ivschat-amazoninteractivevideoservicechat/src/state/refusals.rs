// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request it refuses because one member's VALUE is wrong, and
//! the modelled `ValidationException` that wording travels in.
//!
//! Every sentence here is built from the request — the operation it named, the member it
//! sent, the bound or pattern that member declares and the value it carried — by ONE
//! construction: the operation, a dot, the dotted member path, then the requirement. The
//! `@range` and `@pattern` forms are the recorded ones; the remaining constraint classes
//! are stated by the same construction, as the sibling Amazon IVS endpoints were recorded
//! stating them (`<Operation>.<member> is required to have a maximum length of 128, found
//! 129`), because one validation layer words them all.
//!
//! The modelled shape is what carries them: `ValidationException` requires `message` and
//! `reason` and carries a `fieldList` (`smithy_query(query="shape",
//! shape="com.amazonaws.ivschat#ValidationException")`), so a refusal of a member states
//! the sentence as its `message`, the reason as the modelled token for a field that failed
//! validation, and one `fieldList` entry per member — each naming that member under the
//! SAME path the sentence names it by, which is what a client binding a form error to a
//! field reads.
//!
//! Nothing modelled is restated: a bound, a pattern or a value set a clause prints comes
//! off the [`Violation`] the framework's own constraint walk produced.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};

use crate::ops::types::{ValidationException, ValidationExceptionField};

/// The modelled `ValidationExceptionReason` a refusal of one member's value carries: a
/// field failed validation, rather than the enum's `UNKNOWN_OPERATION` or `OTHER`.
///
/// `VALIDATION_EXCEPTION_REASON_VALUES` is the generated catalog of that enum, and the
/// test below pins this token as one of its values, so the constant cannot drift from the
/// model.
pub const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The path a refusal names a member by: the operation, then the dotted path to the member
/// within that operation's input.
///
/// One spelling for the sentence and for the `fieldList` entry, so the two cannot name one
/// member two ways.
pub fn member_path(operation: &str, member: &str) -> String {
    format!("{operation}.{member}")
}

/// A numeric bound or submitted value as this service's validation layer renders it: the
/// decimal form, so an integral value carries a `.0` (`50` reads as `50.0`).
///
/// The bound arrives as the text its constraint trait displays with and the submitted value
/// as the text the request carried, so a value that already shows a fraction is left
/// exactly as it is.
pub fn as_decimal(value: &str) -> String {
    if value.contains('.') {
        value.to_string()
    } else {
        format!("{value}.0")
    }
}

/// The length the request actually sent, for a `@length` violation.
///
/// A [`Violation`] carries WHAT was sent, not its size, so the size is read off that
/// rendering: the framework renders a collection as `"<n> items"` and a string as the
/// string itself. A string longer than the framework's echo cap arrives elided, which is
/// the one case this count is short — it affects the number in the sentence and nothing
/// about the refusal.
fn found_length(violation: &Violation) -> usize {
    let Some(value) = violation.value.as_deref() else {
        return 0;
    };
    value
        .strip_suffix(" items")
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| value.chars().count())
}

/// One violation as this service states it: the operation, the dotted member path and the
/// requirement the request violated.
pub fn violation_clause(operation: &str, violation: &Violation) -> String {
    let path = member_path(operation, &violation.path);
    let found = violation.value.as_deref().unwrap_or_default();
    match &violation.kind {
        ViolationKind::MinValue(minimum) => format!(
            "{path} is required to have a minimum value of {}, found {}",
            as_decimal(minimum),
            as_decimal(found),
        ),
        ViolationKind::MaxValue(maximum) => format!(
            "{path} is required to have a maximum value of {}, found {}",
            as_decimal(maximum),
            as_decimal(found),
        ),
        ViolationKind::Pattern(pattern) => {
            format!("{path} does not match the required pattern {pattern}")
        }
        ViolationKind::MinLength(minimum) => format!(
            "{path} is required to have a minimum length of {minimum}, found {}",
            found_length(violation),
        ),
        ViolationKind::MaxLength(maximum) => format!(
            "{path} is required to have a maximum length of {maximum}, found {}",
            found_length(violation),
        ),
        ViolationKind::MissingRequiredMember => format!("{path} is required"),
        ViolationKind::EnumValue(allowed) => {
            format!("{path} is required to be one of [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => format!("{path} is required to have unique values"),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class the framework gains
        // later keeps the framework's OWN clause for it, rather than a sentence this module
        // would have to invent for a requirement it cannot state.
        _ => violation.message(),
    }
}

/// The modelled `ValidationException` for a request member this service refuses, with the
/// member named in `fieldList` the way a field-level refusal reports it.
///
/// `message` is the sentence the caller has to fix the request from, and the one
/// `fieldList` entry carries it again under the member path it is about — which is how a
/// client binds the refusal to the field it names.
pub fn field_validation_failed(
    operation: &str,
    member: &str,
    message: impl Into<String>,
) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member_path(operation, member),
            message,
        }]),
    }
}

/// The modelled `ValidationException` for `violations` of `operation`, or `None` when there
/// is nothing to refuse.
///
/// The recorded refusal carries ONE violation and states its sentence alone — no count
/// prefix, unlike the framework's own envelope — so one violation is its clause; several
/// are joined in the order the walk collected them, and each gets its own `fieldList`
/// entry so a client reads every member it must fix.
pub fn stated_validation_exception(
    operation: &str,
    violations: &[Violation],
) -> Option<ValidationException> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(|violation| violation_clause(operation, violation))
        .collect();
    Some(ValidationException {
        message: clauses.join("; "),
        reason: FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(
            violations
                .iter()
                .zip(clauses)
                .map(|(violation, clause)| ValidationExceptionField {
                    name: member_path(operation, &violation.path),
                    message: clause,
                })
                .collect(),
        ),
    })
}

/// The violation for a `@required` member the request did not send.
///
/// Absence is found while READING the request, so it arrives as a codec error rather than
/// from the constraint walk; this is the same [`Violation`] the framework's own
/// `refusal_of` builds for it, so one refusal is not stated two ways.
pub fn missing_member_violation(member_name: String) -> Violation {
    Violation {
        path: member_name,
        kind: ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    }
}

/// Refuse `value` when it sits outside `range`, the way this service's validation layer
/// states a range violation, or pass it through.
///
/// The one checker for the range-constrained members whose modelled constraint the HANDLER
/// owns (`skip_validation` in `src/ops/types/`), so a violation found there is stated
/// exactly as [`violation_clause`] states one the framework's walk found. A value ON
/// either bound is accepted.
pub fn check_range(
    operation: &str,
    member: &str,
    value: i32,
    range: super::ModelledRange,
) -> Result<(), ValidationException> {
    let kind = if value < range.min {
        Some(ViolationKind::MinValue(range.min.to_string()))
    } else if value > range.max {
        Some(ViolationKind::MaxValue(range.max.to_string()))
    } else {
        None
    };
    match kind {
        None => Ok(()),
        Some(kind) => {
            let violation = Violation {
                path: member.to_string(),
                kind,
                value: Some(value.to_string()),
                sensitive: false,
            };
            Err(field_validation_failed(
                operation,
                member,
                violation_clause(operation, &violation),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;

    /// The reason token this module carries is one the modelled enum declares.
    #[test]
    fn the_field_validation_reason_is_a_modelled_enum_value() {
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(&FIELD_VALIDATION_FAILED),
            "expected {FIELD_VALIDATION_FAILED:?} among the modelled reasons, got {:?}",
            VALIDATION_EXCEPTION_REASON_VALUES
        );
    }

    /// Every constraint class is stated by the one construction: the operation-qualified
    /// member path, then the requirement.
    #[test]
    fn each_constraint_class_is_stated_from_the_operation_and_the_member() {
        let rows: Vec<(&str, ViolationKind, Option<&str>, &str)> = vec![
            (
                "a maximum value",
                ViolationKind::MaxValue("50".to_string()),
                Some("100"),
                "ListRooms.maxResults is required to have a maximum value of 50.0, found 100.0",
            ),
            (
                "a minimum value",
                ViolationKind::MinValue("1".to_string()),
                Some("0"),
                "ListRooms.maxResults is required to have a minimum value of 1.0, found 0.0",
            ),
            (
                "a pattern",
                ViolationKind::Pattern("^[a-zA-Z0-9-_]*$".to_string()),
                Some("bad name"),
                "ListRooms.maxResults does not match the required pattern ^[a-zA-Z0-9-_]*$",
            ),
            (
                "a maximum length",
                ViolationKind::MaxLength(3),
                Some("abcd"),
                "ListRooms.maxResults is required to have a maximum length of 3, found 4",
            ),
            (
                "a minimum length of a collection",
                ViolationKind::MinLength(1),
                Some("0 items"),
                "ListRooms.maxResults is required to have a minimum length of 1, found 0",
            ),
            (
                "an absent required member",
                ViolationKind::MissingRequiredMember,
                None,
                "ListRooms.maxResults is required",
            ),
        ];
        for (case, kind, value, expected) in rows {
            let violation = Violation {
                path: "maxResults".to_string(),
                kind,
                value: value.map(str::to_string),
                sensitive: false,
            };
            let actual = violation_clause("ListRooms", &violation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A range refusal the handler owns is worded, reasoned and field-listed exactly as one
    /// the framework's walk finds.
    #[test]
    fn a_handler_range_refusal_names_the_operation_qualified_member() {
        let refused = check_range("ListRooms", "maxResults", 100, super::super::MAX_ROOM_RESULTS_RANGE)
            .expect_err("100 is outside the modelled range");
        assert_eq!(
            refused.message,
            "ListRooms.maxResults is required to have a maximum value of 50.0, found 100.0",
            "got {:?}",
            refused.message
        );
        assert_eq!(
            refused.reason, FIELD_VALIDATION_FAILED,
            "got {:?}",
            refused.reason
        );
        assert_eq!(
            refused
                .field_list
                .as_ref()
                .and_then(|fields| fields.first())
                .map(|field| field.name.as_str()),
            Some("ListRooms.maxResults"),
            "got {:?}",
            refused.field_list
        );
    }

    /// A value ON either modelled bound is accepted.
    #[test]
    fn a_value_on_a_bound_is_accepted() {
        for value in [
            super::super::MAX_ROOM_RESULTS_RANGE.min,
            super::super::MAX_ROOM_RESULTS_RANGE.max,
        ] {
            assert!(
                check_range("ListRooms", "maxResults", value, super::super::MAX_ROOM_RESULTS_RANGE)
                    .is_ok(),
                "case {value}: a value on a modelled bound must be accepted"
            );
        }
    }
}
