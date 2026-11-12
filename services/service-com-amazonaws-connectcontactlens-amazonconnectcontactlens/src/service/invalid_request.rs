// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refused for a MODELLED input constraint, before any
//! handler ran: a value outside a declared `@length` or `@range`, a value outside a
//! `@pattern`, or a `@required` member the request left out.
//!
//! # The rule
//!
//! Every such refusal is the operation's own modelled `InvalidRequestException` — "The
//! request is not valid.", `@error("client")` at `@httpError(400)`, which this model binds
//! to every operation (`smithy_query(query="service_info")` lists it under
//! `common_errors`) — and never a synthetic `ValidationException`, which this model does not
//! declare at all.
//!
//! The message is a FINDING LIST rather than one sentence: the fixed text
//! `Invalid request body: [` … `]` around the findings the request's body validator
//! collected, comma-separated, in the order the members are declared in. Every sentence in
//! it, and the bracketed list itself, is the SHARED rendering that layer states for its
//! clause class ([`::service_common::validation_message`]) over the shared fixed text
//! ([`::service_common::front_door::INVALID_REQUEST_BODY`]), so nothing here words a finding
//! differently from another service handed the same violation. Two kinds of finding go in
//! the list:
//!
//! * one per violated member, worded as the validator words that clause class
//!   ([`finding_for`]) — the `@pattern` clause quotes the expression the WHOLE value has to
//!   satisfy ([`::service_common::validation::whole_value_expression`]) and the value the
//!   caller submitted;
//! * one per member the request CARRIES whose JSON Schema declares a numeric `format` the
//!   validator does not implement ([`unsupported_format`]). It is a statement about the
//!   validator rather than about the value, so it is listed whatever the value is — but it
//!   refuses nothing on its own: a request whose only finding is this one is served. This is
//!   the one sentence of the list the shared module does not hold, because nothing else in
//!   the tree has recorded it yet; it belongs beside the others once a second service does.
//!
//! # Why this cannot live in `Service::override_refusal` alone
//!
//! That seam is handed the violations and nothing else, so it can neither see which members
//! the request CARRIED (which the second kind of finding needs) nor answer in the modelled
//! shape's own body. [`modelled_refusal`] therefore states the refusal where the request is
//! still in hand — `super::front_door`, the layer every request passes — and `super`'s
//! `override_refusal` words what the framework still renders from these same helpers, so
//! the two paths cannot disagree.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema::{Schema, ShapeType};
use ::aws_smithy_schema_server::constraints::path;
use ::service_common::validation_message as message;
use ::smithy_server_runtime::{Operation, Refused, ServerHttpProtocol, Violation, ViolationKind};

use crate::ops::types::InvalidRequestException;

/// The wire error code of `com.amazonaws.connectcontactlens#InvalidRequestException` — the
/// shape name, which is what an SDK matches a restJson1 error on.
pub(super) const INVALID_REQUEST_CODE: &str = "InvalidRequestException";

/// The status that shape's `@httpError` declares
/// (`smithy_query(query="shape", shape="InvalidRequestException")`).
pub(super) const INVALID_REQUEST_STATUS: u16 = 400;

/// The greatest number of findings one refusal states. The framework's constraint walk
/// stops collecting violations at this many, so this is the same number it used rather than
/// a literal of this crate's own.
const MAX_VIOLATIONS: usize = ::service_common::validation::MAX_VIOLATIONS;

/// The response this service answers a request it refuses before `O`'s handler runs, or
/// `None` when the request reaches that handler.
///
/// `None` also when the modelled error cannot be serialized, which leaves the framework's
/// own refusal path to answer — worded by `super`'s `override_refusal` from these same
/// helpers — rather than answering a 400 with nothing in it.
pub(super) fn modelled_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    members: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> Option<RtResponse> {
    // The framework's OWN machinery — the protocol's deserializer and
    // `constraints::validate` over the built input, with each clause's value echo restored
    // to the bytes the caller sent — so this cannot find a different violation set from the
    // walk that runs behind the delegate.
    let violations = ::service_common::validation::refusing_violations::<O>(protocol, req);
    let refused = refusal_for(&violations, O::INPUT_SCHEMA, members)?;
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::SCHEMA_INVALID_REQUEST_EXCEPTION,
            INVALID_REQUEST_CODE,
            INVALID_REQUEST_STATUS,
        )
        .ok()
}

/// `violations` as the modelled `InvalidRequestException`, or `None` when there is nothing
/// to refuse (no violation at all).
fn refusal_for(
    violations: &[Violation],
    input_schema: &Schema,
    members: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> Option<InvalidRequestException> {
    let findings = findings_for(violations, input_schema, members)?;
    Some(InvalidRequestException {
        message: Some(envelope(&findings)),
    })
}

/// How this service states a refusal the framework renders itself: the same code and the
/// same message, which is all [`Refused`] can carry.
///
/// `None` only for an empty violation set, which is not a refusal. The request is not at
/// hand here (the seam is given the violations alone), so the findings are the violated
/// members' alone — the ones that decide whether the request is refused at all.
pub(super) fn refused_for(violations: &[Violation], input_schema: &Schema) -> Option<Refused> {
    let findings = findings_for(violations, input_schema, None)?;
    Some(Refused {
        code: INVALID_REQUEST_CODE.to_string(),
        message: envelope(&findings),
    })
}

/// The whole message: the fixed text that layer puts in front of its findings, and the
/// bracketed list itself — both the shared renderings
/// ([`::service_common::front_door::INVALID_REQUEST_BODY`] and
/// [`message::bracketed_findings`]), so neither can drift per crate.
fn envelope(findings: &[String]) -> String {
    format!(
        "{}: {}",
        ::service_common::front_door::INVALID_REQUEST_BODY,
        message::bracketed_findings(findings)
    )
}

/// Every finding of one refusal, in the order the input shape declares its members, or
/// `None` when the request is not refused (no violation at all).
///
/// `members` is what the request's body carried, where that is still in hand
/// ([`super::front_door::body_members`]), which is what lets a member's
/// unsupported-`format` finding be listed for the members the request CARRIES; without it
/// the list is the violated members' findings alone.
fn findings_for(
    violations: &[Violation],
    input_schema: &Schema,
    carried: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> Option<Vec<String>> {
    if violations.is_empty() {
        return None;
    }
    let violations = &violations[..violations.len().min(MAX_VIOLATIONS)];
    let mut findings = Vec::new();
    for member in input_schema.members().iter().copied() {
        let Some(name) = member.member_name() else {
            continue;
        };
        let member_path = path::member(name);
        for violation in violations.iter().filter(|v| v.path == member_path) {
            findings.push(finding_for(violation, name));
        }
        if let (Some(format), Some(carried)) = (unsupported_format(member), carried) {
            if carried.contains_key(wire_name(member, name)) {
                findings.push(format!("format attribute \"{format}\" not supported"));
            }
        }
    }
    // A violation reported INSIDE a member (an element of a list, a map entry) is named by
    // a path no top-level member matches; it is still part of the refusal, so it is stated
    // after the members' own findings rather than dropped.
    for violation in violations {
        if !names_a_member(input_schema, &violation.path) {
            findings.push(finding_for(violation, &violation.path));
        }
    }
    (!findings.is_empty()).then_some(findings)
}

/// Whether `path` names a top-level member of `input_schema`.
fn names_a_member(input_schema: &Schema, path: &str) -> bool {
    input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == path)
    })
}

/// The name the restJson1 body carries `member` under: its `@jsonName` where the model
/// declares one, else the member name itself.
fn wire_name<'schema>(member: &'schema Schema, name: &'schema str) -> &'schema str {
    member.json_name().map_or(name, |json| json.value())
}

/// The JSON Schema `format` attribute the request's body validator does not implement for
/// `member`, or `None` for a member whose type carries none.
///
/// A Smithy `integer` is a 32-bit signed integer, which the request's JSON Schema declares
/// as `"type": "integer", "format": "int32"` — and the validator implements no check for
/// that attribute, so it states one finding naming it. The type is read off the member's
/// own schema, so the finding follows the MODEL rather than a member name.
fn unsupported_format(member: &Schema) -> Option<&'static str> {
    match member.shape_type() {
        ShapeType::Integer => Some("int32"),
        _ => None,
    }
}

/// One violated member as the body validator states it, where `member` is the name the
/// finding refers to the member by.
///
/// Each clause class has one wording, and the rejected value is quoted where the clause
/// quotes it — the validator reports the offending input back, so the quoted value is the
/// one the caller submitted.
///
/// [`ViolationKind`] is `#[non_exhaustive]`, so a class added upstream has no wording here;
/// it takes the framework's own clause for that class INSIDE this envelope rather than
/// declining the refusal, because declining would hand the request back to the framework's
/// synthetic `ValidationException` — the one shape this service must never answer with.
fn finding_for(violation: &Violation, member: &str) -> String {
    let value = violation.value.as_deref().unwrap_or_default();
    // Every sentence below is the SHARED one that layer states for its clause class
    // ([`::service_common::validation_message`]), so this service cannot word a finding any
    // differently from another service handed the same violation.
    match &violation.kind {
        ViolationKind::MissingRequiredMember => message::missing_required_properties(&[member]),
        // The expression the WHOLE value has to satisfy, which is what this dialect quotes:
        // a model's `@pattern` is a partial match, so the ends it leaves open are spelled
        // out rather than left to the reader.
        ViolationKind::Pattern(pattern) => message::pattern_mismatch(
            &::service_common::validation::whole_value_expression(pattern),
            value,
        ),
        // The length is the one `@length` counted — characters — of the value the clause
        // quotes, which is the value the caller submitted as the collecting walk restored
        // it, so the number stated is the number that was checked.
        ViolationKind::MinLength(min) => {
            message::string_too_short(value, value.chars().count(), *min)
        }
        ViolationKind::MaxLength(max) => {
            message::string_too_long(value, value.chars().count(), *max)
        }
        ViolationKind::MinValue(min) => message::numeric_below_minimum(value, min),
        ViolationKind::MaxValue(max) => message::numeric_above_maximum(value, max),
        _ => violation.message(),
    }
}

#[cfg(test)]
mod tests {
    //! The wording of a refusal. WHICH request reaches the wire with it is pinned over the
    //! router in `tests/all/refusal_stages.rs`.

    use super::*;

    /// A violation of `kind` on `path`, echoing `value` as the caller submitted it.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// The operation's input schema, which is where the member order and the member types
    /// the findings are read off come from.
    fn input_schema() -> &'static Schema {
        <crate::ops::list_realtime_contact_analysis_segments::ListRealtimeContactAnalysisSegments<
            ::engine::InMemoryEngine,
        > as Operation>::INPUT_SCHEMA
    }

    /// RULE: every clause class is stated as its own finding inside the fixed
    /// `Invalid request body: [...]` text, with the value the caller submitted quoted where
    /// that class quotes it.
    #[test]
    fn every_clause_class_is_stated_as_a_finding() {
        // (case, violation, the finding expected inside the brackets)
        let rows: [(&str, Violation, &str); 6] = [
            (
                "a pattern violation",
                violation("contactId", ViolationKind::Pattern("\\S".to_string()), Some("   ")),
                r#"ECMA 262 regex ".*\S.*" does not match input string "   ""#,
            ),
            (
                "an absent required member",
                violation("contactId", ViolationKind::MissingRequiredMember, None),
                r#"object has missing required properties (["ContactId"])"#,
            ),
            (
                "a value below a length minimum",
                violation("contactId", ViolationKind::MinLength(1), Some("")),
                r#"string "" is too short (length: 0, required minimum: 1)"#,
            ),
            (
                "a value above a length maximum",
                violation("contactId", ViolationKind::MaxLength(256), Some("abc")),
                r#"string "abc" is too long (length: 3, maximum allowed: 256)"#,
            ),
            (
                "a number below a range minimum",
                violation(
                    "maxResults",
                    ViolationKind::MinValue("1".to_string()),
                    Some("0"),
                ),
                "numeric instance is lower than the required minimum (minimum: 1, found: 0)",
            ),
            (
                "a number above a range maximum",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("100".to_string()),
                    Some("101"),
                ),
                "numeric instance is greater than the required maximum (maximum: 100, found: 101)",
            ),
        ];

        for (case, violated, expected) in rows {
            let refused = refused_for(&[violated], input_schema())
                .unwrap_or_else(|| panic!("case {case:?}: must be refused"));
            assert_eq!(
                refused.code, INVALID_REQUEST_CODE,
                "case {case:?}: expected {INVALID_REQUEST_CODE:?}, got {:?}",
                refused.code
            );
            assert_eq!(
                refused.message,
                format!("Invalid request body: [{expected}]"),
                "case {case:?}: expected the finding {expected:?}, got {:?}",
                refused.message
            );
        }
    }

    /// RULE: the findings are listed in the order the input shape declares its members, and
    /// several findings are comma-separated inside the one bracketed list.
    #[test]
    fn several_findings_are_listed_in_member_order() {
        let refused = refused_for(
            &[
                violation(
                    "maxResults",
                    ViolationKind::MinValue("1".to_string()),
                    Some("0"),
                ),
                violation(
                    "contactId",
                    ViolationKind::Pattern("\\S".to_string()),
                    Some(" "),
                ),
            ],
            input_schema(),
        )
        .expect("two violations are a refusal");
        assert_eq!(
            refused.message,
            concat!(
                r#"Invalid request body: [ECMA 262 regex ".*\S.*" does not match input string "#,
                r#"" ", numeric instance is lower than the required minimum (minimum: 1, "#,
                "found: 0)]"
            ),
            "expected ContactId's finding before MaxResults's, got {:?}",
            refused.message
        );
    }

    /// RULE: a clause class this module has no wording of its own for is still stated as a
    /// finding of the MODELLED refusal — the framework's own clause inside this envelope —
    /// so the synthetic `ValidationException` can never reach the wire for this service.
    #[test]
    fn an_unworded_clause_class_is_still_the_modelled_refusal() {
        let unworded = violation(
            "contactId",
            ViolationKind::EnumValue(vec!["TRANSCRIPT".to_string()]),
            Some("nope"),
        );
        let refused = refused_for(std::slice::from_ref(&unworded), input_schema())
            .expect("an unworded class is still a refusal");
        assert_eq!(
            refused.code, INVALID_REQUEST_CODE,
            "case \"an unworded clause class\": expected {INVALID_REQUEST_CODE:?}, got {:?}",
            refused.code
        );
        assert_eq!(
            refused.message,
            format!("Invalid request body: [{}]", unworded.message()),
            "case \"an unworded clause class\": expected the framework's clause inside the \
             envelope, got {:?}",
            refused.message
        );
    }

    /// RULE: a request the walk found nothing wrong with is not refused, whatever members it
    /// carries.
    #[test]
    fn no_violation_is_no_refusal() {
        assert!(
            refused_for(&[], input_schema()).is_none(),
            "an empty violation set must not be worded as a refusal"
        );
        assert!(
            findings_for(&[], input_schema(), None).is_none(),
            "an empty violation set must state no findings"
        );
    }

    /// RULE: the unsupported-`format` finding is read off the member's modelled TYPE — an
    /// `integer` member carries one, a string member none.
    #[test]
    fn only_an_integer_member_carries_an_unsupported_format() {
        for member in input_schema().members().iter().copied() {
            let name = member.member_name().unwrap_or_default();
            let expected = match member.shape_type() {
                ShapeType::Integer => Some("int32"),
                _ => None,
            };
            let found = unsupported_format(member);
            assert_eq!(
                found, expected,
                "case {name:?}: expected {expected:?}, got {found:?}"
            );
        }
    }
}
