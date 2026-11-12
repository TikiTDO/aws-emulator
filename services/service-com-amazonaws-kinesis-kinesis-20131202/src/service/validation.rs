// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how the constraint violations the
//! framework's own boundary walk found become the `message` a 400 `ValidationException`
//! carries.
//!
//! [`crate::service`] owns the SEAM — which requests are read for violations, and how those
//! violations are collected with the values the caller submitted — and this module owns the
//! sentence, so the clause rules live in ONE place instead of in each operation that trips
//! them. What is ENFORCED is always the model's: every violation handed to [`refusal`] comes
//! from the framework's own walk, and nothing here accepts or refuses anything.
//!
//! The rules, each holding for every member of every operation rather than for the members
//! one report happened to cite:
//!
//! 1. the envelope is AWS's `<N> validation error(s) detected: ` over one clause per
//!    violation, and a clause states the value, the member's dotted path and the requirement
//!    it failed in the framework's own words — both come from the shared renderer
//!    (`service_common::validation_message::aws_validation_refusal_withholding_with`), so no
//!    requirement and no member path is spelled in this crate;
//! 2. the value a clause quotes is the one the caller SUBMITTED, in full: the message is the
//!    only place the service reports the offending input back, and a caller comparing the
//!    echo against what it sent has to see its own bytes. The framework's own walk stores a
//!    shortened rendering of a long value, so [`crate::service`] collects the violations
//!    with the submitted values restored before handing them here;
//! 3. a reported `@pattern` is the expression without the `^`/`$` anchors a model writes to
//!    demand a whole-string match (`service_common::validation::unanchored`), and for the
//!    resource-ARN expression it is the one the service's own front door publishes
//!    ([`STATED_EXPRESSIONS`]);
//! 4. an enumerated type's value set is listed in LEXICOGRAPHIC order
//!    (`service_common::validation_message::in_sorted_enum_order`), whatever order the model
//!    declares its members in;
//! 5. one member's clause states NO value at all ([`WITHHELD_VALUE_MEMBERS`]).
//!
//! The status (400) and the error code are what the framework already answers a constraint
//! failure with on this `awsJson1_1` service and are restated by that shared renderer, not
//! changed.

use ::aws_smithy_schema_server::constraints::Violation;
use ::smithy_server_runtime::{Refusal, Refused};

/// This service's wording for the violations the framework found, or `None` when there is
/// nothing to word (no violations) and for a refusal that is not a constraint failure (an
/// unauthenticated caller keeps the framework's 403 and its wording), which keeps the
/// framework's own.
pub(super) fn refusal(violations: &[Violation]) -> Option<Refused> {
    // The value set of an enumerated type is re-ordered BEFORE the clauses are rendered, so
    // the clause itself — the member path, the echoed value and the requirement's wording —
    // still comes from the one place that writes it.
    let stated: Vec<Violation> = violations
        .iter()
        .map(::service_common::validation_message::in_sorted_enum_order)
        .collect();
    ::service_common::validation_message::aws_validation_refusal_withholding_with(
        &Refusal::Invalid(stated),
        stated_expression,
        withholds_value,
    )
}

/// The members whose clause states NO value: the member path is named, the requirement is
/// named, and neither the submitted bytes nor the `null` an absent member is stated with
/// appears.
///
/// A channel's name is the one such member of this service: a refusal of a `ChannelName`
/// outside its expression names the member and the expression alone. Every other member —
/// the resource ARN a tagging call carries, a partition key, a stream mode — is quoted back
/// with the value that was submitted.
///
/// The usual source for this set is the model, which marks a member whose value is not to be
/// disclosed `@sensitive` — `service_common::sensitive_members::WithheldMembers::from_model`
/// answers that for a service, and is what to reach for the moment this model marks one. It
/// cannot answer this row: this model marks no member `@sensitive`, so the set is this
/// service's recorded behaviour and lives here. The path rule below is the shared one's, so
/// the two cannot disagree about which clause a member's name decides.
const WITHHELD_VALUE_MEMBERS: &[&str] = &["channelName"];

/// Whether the clause for the member at `path` states no value ([`WITHHELD_VALUE_MEMBERS`]).
///
/// ANY segment decides, not only the last: what sits under such a member is part of that
/// member's value. The validating walk spells a path in lowerCamel, with a `<1-based
/// index>.member` step from a collection into one of its elements, so splitting on `.`
/// reaches every segment of it.
fn withholds_value(path: &str) -> bool {
    path.split('.')
        .any(|segment| WITHHELD_VALUE_MEMBERS.contains(&segment))
}

/// The expressions this service's front door publishes for a value that has to match one,
/// keyed by the expression the model declares (anchors already dropped, see
/// [`stated_expression`]).
///
/// A row exists only where the two differ. The resource-ARN expression is the one such: the
/// classes of Kinesis resource a tagging call may name are a stream, a channel and a queue,
/// while this model's copy of the expression lists the first two — so a client told the
/// expression would read a valid resource class as invalid. Only the expression REPORTED
/// comes from here; which values are ACCEPTED is the model's `@pattern`, which is what the
/// framework's walk checks.
const STATED_EXPRESSIONS: &[(&str, &str)] = &[(
    r"arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+",
    r"arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel|queue)/\S+",
)];

/// The expression a clause quotes for the modelled `pattern`: without the `^`/`$` pair a
/// model writes around a whole-value match, and as the front door publishes it where the two
/// differ ([`STATED_EXPRESSIONS`]).
fn stated_expression(pattern: &str) -> String {
    let unanchored = ::service_common::validation::unanchored(pattern);
    STATED_EXPRESSIONS
        .iter()
        .find(|(modelled, _)| *modelled == unanchored)
        .map_or(unanchored, |(_, stated)| (*stated).to_string())
}

#[cfg(test)]
mod tests {
    //! The clause rules this module owns, each over a SET of inputs rather than the one
    //! recorded case. The same rules are pinned on the wire, through real requests, in
    //! `tests/all/constraint_refusal_wording.rs`.

    use super::*;
    use ::aws_smithy_schema_server::constraints::ViolationKind;
    use ::smithy_server_runtime::Service;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json"
    );

    /// One violation of `kind` at `path`, carrying `value` as its echo.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// The message [`refusal`] states for one violation.
    fn stated(violation: Violation) -> String {
        refusal(&[violation])
            .expect("a constraint refusal is stated")
            .message
    }

    /// A `@pattern` is quoted without the whole-value anchors a model writes the total match
    /// with — over this service's own expressions, including one the issue did not cite.
    #[test]
    fn a_pattern_is_quoted_without_its_whole_value_anchors() {
        // (case, modelled pattern, quoted expression). Source: `smithy_query(query="shape",
        // shape="com.amazonaws.kinesis#<Shape>")` for each.
        let rows: [(&str, &str, &str); 4] = [
            ("ChannelName", r"^[a-zA-Z0-9_.-]+$", r"[a-zA-Z0-9_.-]+"),
            (
                "StreamId",
                r"^[a-z0-9]{20}-[a-z0-9]{3}$",
                r"[a-z0-9]{20}-[a-z0-9]{3}",
            ),
            (
                "StreamARN",
                r"^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$",
                r"arn:aws.*:kinesis:.*:\d{12}:stream/\S+",
            ),
            // An expression with no anchors at all is quoted as written.
            ("an unanchored expression", r"[a-z]+", r"[a-z]+"),
        ];

        for (case, modelled, quoted) in rows {
            assert_eq!(
                stated_expression(modelled),
                quoted,
                "case {case:?}: expected the expression quoted as {quoted:?}"
            );
        }
    }

    /// The resource-ARN expression is reported with every resource class the front door
    /// admits, and the row is keyed by an expression this model really declares.
    #[test]
    fn the_stated_resource_arn_expression_names_every_resource_class() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        for (modelled, stated) in STATED_EXPRESSIONS {
            assert!(
                model.contains(&modelled.replace('\\', "\\\\")),
                "case {modelled:?}: a stated expression is keyed by one the model declares"
            );
            assert_eq!(
                stated_expression(&format!("^{modelled}$")),
                *stated,
                "case {modelled:?}: expected it reported as {stated:?}"
            );
        }
        assert_eq!(
            stated_expression(r"^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$"),
            r"arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel|queue)/\S+",
            "the resource-ARN expression names the stream, channel and queue classes"
        );
    }

    /// An enumerated type's value set is listed in lexicographic order, whatever order the
    /// model declares it in.
    #[test]
    fn an_enum_value_set_is_listed_in_lexicographic_order() {
        // (case, the set as the model declares it, the set as a clause lists it). Source:
        // `smithy_query(query="shape", shape="com.amazonaws.kinesis#<Enum>")`.
        let rows: [(&str, &[&str], &str); 3] = [
            ("StreamMode", &["PROVISIONED", "ON_DEMAND"], "[ON_DEMAND, PROVISIONED]"),
            (
                "EncryptionType",
                &["NONE", "KMS"],
                "[KMS, NONE]",
            ),
            (
                "ShardIteratorType",
                &[
                    "AT_SEQUENCE_NUMBER",
                    "AFTER_SEQUENCE_NUMBER",
                    "TRIM_HORIZON",
                    "LATEST",
                    "AT_TIMESTAMP",
                ],
                "[AFTER_SEQUENCE_NUMBER, AT_SEQUENCE_NUMBER, AT_TIMESTAMP, LATEST, TRIM_HORIZON]",
            ),
        ];

        for (case, declared, listed) in rows {
            let declared: Vec<String> = declared.iter().map(|value| (*value).to_string()).collect();
            let message = stated(violation(
                "member",
                ViolationKind::EnumValue(declared),
                Some("NOPE"),
            ));
            let expected = format!(
                "1 validation error detected: Value 'NOPE' at 'member' failed to satisfy \
                 constraint: Member must satisfy enum value set: {listed}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the value set listed as {listed:?}"
            );
        }
    }

    /// The value half of a clause: quoted for every member but the channel's name, which
    /// states the member and the requirement alone — whatever the constraint class, and
    /// whatever the request carried.
    #[test]
    fn the_value_half_is_quoted_except_for_the_withheld_member() {
        // (case, member path, the value the walk recorded, the clause's subject half)
        let rows: [(&str, &str, Option<&str>, &str); 5] = [
            ("a stream name", "streamName", Some("bad name"), "Value 'bad name' at 'streamName'"),
            (
                "a partition key",
                "records.1.member.partitionKey",
                Some("kkk"),
                "Value 'kkk' at 'records.1.member.partitionKey'",
            ),
            (
                "an absent member",
                "resourceARN",
                None,
                "Value null at 'resourceARN'",
            ),
            (
                "the channel name, present",
                "channelName",
                Some("bad channel name!!"),
                "Value at 'channelName'",
            ),
            (
                "the channel name, absent",
                "channelName",
                None,
                "Value at 'channelName'",
            ),
        ];

        for (case, path, value, subject) in rows {
            let message = stated(violation(path, ViolationKind::MaxLength(8), value));
            let expected = format!(
                "1 validation error detected: {subject} failed to satisfy constraint: Member \
                 must have length less than or equal to 8"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the clause to open with {subject:?}"
            );
        }
    }

    /// A refusal with nothing to word, and one that is not a constraint failure at all, keep
    /// the framework's own wording.
    #[test]
    fn a_refusal_this_service_does_not_word_keeps_the_frameworks_wording() {
        assert!(
            refusal(&[]).is_none(),
            "case no violations: expected the framework's own wording"
        );
        assert!(
            super::super::EmulatedService::new(std::sync::Arc::new(
                ::engine::InMemoryEngine::new()
            ))
            .override_refusal(&Refusal::Unauthenticated)
            .is_none(),
            "case an unauthenticated caller: expected the framework's own 403 wording"
        );
    }
}
