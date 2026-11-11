// SPDX-License-Identifier: CC0-1.0

//! The ORDER this service lists an enumerated type's admissible values in when it refuses
//! a value outside that type.
//!
//! RULE: a request-shape refusal of a value outside an enumerated type states that type's
//! value set in the order the front door's own TABLE of the admissible values is walked —
//! the table's buckets in ascending order, each bucket being the low bits of the value's
//! own string hash ([`service_common::validation_message::in_hashed_enum_order`]) — and not
//! in the order the model declares the type's members in. The order is computed from the
//! values themselves, so it belongs to the enumerated TYPE: every member of every
//! operation targeting one enum is listed alike, at whatever depth and in whatever binding
//! the value arrived, and a type the model gains needs nothing added here.
//!
//! Nothing about the SENTENCE is this module's: the counted envelope, the clause, the
//! member path, the value the clause echoes and every other constraint class's requirement
//! come from the shared home for boundary-refusal wording
//! ([`service_common::validation_message`]) and from the framework behind it, so this
//! service cannot word a constraint differently from the services that state the same
//! sentence. Only the order inside an enum clause moves, and a refusal carrying no enum
//! clause is left to the framework entirely.

use ::service_common::validation_message::in_hashed_enum_order;
use ::smithy_server_runtime::{Refusal, Refused, Violation};

/// The error code this service answers a request-shape refusal with.
///
/// It is the code the framework's own boundary names for one (this service's modelled
/// `ValidationException`, HTTP 400), restated here because `override_refusal` replaces the
/// code and the message together: a message without its matching code is worse than
/// leaving both alone.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One violation as THIS SERVICE states it: an enum value set listed in the order the
/// table of the admissible values is walked.
///
/// Every other violation kind comes back unchanged, which is what [`stated_refusal`] reads
/// to decide whether to reword at all.
pub(crate) fn as_this_service_states_it(violation: &Violation) -> Violation {
    in_hashed_enum_order(violation)
}

/// How this service states a refusal the framework made before any handler ran, or `None`
/// when it states that refusal exactly as the framework already does.
///
/// Only the value set inside an enum clause moves; the code and the envelope are the
/// framework's, so a refusal this service does restate stays the same 400
/// `ValidationException` a client branches on.
pub(crate) fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let stated: Vec<Violation> = violations.iter().map(as_this_service_states_it).collect();
    if stated == *violations {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: ::aws_smithy_schema_server::constraints::envelope(&stated)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::smithy_server_runtime::ViolationKind;

    /// A violation of `path` carrying `value`, outside the enum whose admissible values
    /// are `declared` — the shape the framework hands this module.
    fn enum_violation(path: &str, value: &str, declared: &[&str]) -> Violation {
        Violation {
            path: path.to_string(),
            kind: ViolationKind::EnumValue(
                declared.iter().map(|value| (*value).to_string()).collect(),
            ),
            value: Some(value.to_string()),
            sensitive: false,
        }
    }

    /// The order is a property of the VALUES, so the same set is listed alike whichever
    /// order it reaches the clause in — and nothing else about the violation moves.
    #[test]
    fn one_enum_is_listed_alike_whatever_order_it_is_presented_in() {
        let declared = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"];
        let listed: Vec<String> = ::service_common::validation_message::hashed_enum_order(&declared)
            .into_iter()
            .map(str::to_string)
            .collect();
        let reversed = ["FATAL", "ERROR", "WARN", "INFO", "DEBUG", "TRACE"];
        for (case, presented) in [("as declared", declared), ("reversed", reversed)] {
            let restated = as_this_service_states_it(&enum_violation(
                "revisionConfig.serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel",
                "VERBOSE",
                &presented,
            ));
            assert_eq!(
                restated.kind,
                ViolationKind::EnumValue(listed.clone()),
                "case {case:?}: expected the value set listed {listed:?}, got {:?}",
                restated.kind
            );
            assert_eq!(
                (restated.path.as_str(), restated.value.as_deref()),
                (
                    "revisionConfig.serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel",
                    Some("VERBOSE")
                ),
                "case {case:?}: re-ordering the set changes nothing else about the violation"
            );
        }
    }

    /// A refusal that states no enum value set at all is declined whole, so the framework
    /// words it — value, clause and envelope.
    #[test]
    fn a_refusal_stating_no_enum_value_set_is_declined() {
        for (case, refusal) in [
            (
                "a length bound",
                Refusal::Invalid(vec![Violation {
                    path: "functionName".to_string(),
                    kind: ViolationKind::MaxLength(256),
                    value: Some("x".repeat(257)),
                    sensitive: false,
                }]),
            ),
            (
                "an absent required member",
                Refusal::Invalid(vec![Violation {
                    path: "functionName".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                }]),
            ),
            ("an unauthenticated caller", Refusal::Unauthenticated),
        ] {
            let stated = stated_refusal(&refusal);
            assert!(
                stated.is_none(),
                "case {case:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }
}
