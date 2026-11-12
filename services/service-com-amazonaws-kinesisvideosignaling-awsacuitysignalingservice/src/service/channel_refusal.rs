// SPDX-License-Identifier: CC0-1.0

//! How this data plane WORDS a `ChannelARN` it cannot serve, for every route that takes
//! one.
//!
//! Both operations take the same `ChannelARN` member, and a value of it fails in the same
//! two ways on both: it is not a channel ARN at all, or it names a channel that does not
//! exist. The service answers each with one sentence of its own, so the two routes — and
//! the request boundary that refuses a modelled constraint before a handler runs — cannot
//! word one failure three ways:
//!
//! * a value outside the modelled `ChannelARN` constraints, or one that is not a Kinesis
//!   Video signaling-channel ARN, is [`INVALID_CHANNEL_ARN`]: fixed text, with nothing of
//!   the request in it. The framework's own constraint report would echo the caller's value
//!   and the model's regular expression back, which is neither what the service says nor
//!   something a client can match on, so the boundary refusal for that member is reworded
//!   here too ([`blames_the_channel_arn`]).
//! * a well-formed ARN for a channel nobody created, or one that has been deleted, is
//!   [`channel_does_not_exist`]: the CHANNEL NAME out of the ARN, and the service's fixed
//!   remainder — never the whole ARN.

use ::smithy_server_runtime::{Violation, ViolationKind};

/// The error code this service names for every bad input, on both routes: the modelled
/// `InvalidArgumentException` — "The value for this input parameter is invalid".
pub const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// What the service answers a `ChannelARN` that is not one: entirely fixed text, with no
/// request value interpolated.
pub const INVALID_CHANNEL_ARN: &str = "Invalid Channel ARN !";

/// What it answers a well-formed `ChannelARN` for a channel that does not exist: the
/// channel NAME the ARN carries, and the fixed remainder.
#[must_use]
pub fn channel_does_not_exist(channel_name: &str) -> String {
    format!("{channel_name} channel does not exist !")
}

/// The input member whose constraint violations this service words itself: the one both
/// operations take their channel ARN in, as the restJson1 boundary names it.
const CHANNEL_ARN_MEMBER: &str = "channelARN";

/// Whether `violations` blame the `ChannelARN` member's own constraints — its `@length` or
/// its `@pattern` — which is the refusal [`INVALID_CHANNEL_ARN`] words.
///
/// A violation of ANOTHER member keeps the framework's constraint report: what the service
/// says about a channel ARN is not what it says about every value, and only this member's
/// sentence is known. A violation whose kind the framework may add to later
/// ([`ViolationKind`] is `#[non_exhaustive]`) counts only when it is one of the kinds a
/// value of this member can break.
#[must_use]
pub fn blames_the_channel_arn(violations: &[Violation]) -> bool {
    violations.iter().any(|violation| {
        violation.path.eq_ignore_ascii_case(CHANNEL_ARN_MEMBER)
            && matches!(
                violation.kind,
                ViolationKind::Pattern(_)
                    | ViolationKind::MinLength(_)
                    | ViolationKind::MaxLength(_)
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: Some("a value".to_string()),
            sensitive: false,
        }
    }

    #[test]
    fn only_the_channel_arns_own_constraints_are_worded_by_this_service() {
        // The rows span the kinds a `ChannelARN` value breaks, the members a request
        // carries beside it, and the kinds that are not this member's.
        let cases: Vec<(&str, Vec<Violation>, bool)> = vec![
            (
                "the modelled ChannelARN pattern",
                vec![violation(
                    "channelARN",
                    ViolationKind::Pattern(
                        "^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$"
                            .to_string(),
                    ),
                )],
                true,
            ),
            (
                "a ChannelARN past the modelled length",
                vec![violation("channelARN", ViolationKind::MaxLength(1024))],
                true,
            ),
            (
                "an empty ChannelARN",
                vec![violation("channelARN", ViolationKind::MinLength(1))],
                true,
            ),
            (
                "a ChannelARN violation beside another member's",
                vec![
                    violation("clientId", ViolationKind::Pattern("^[a-zA-Z0-9_.-]+$".to_string())),
                    violation("channelARN", ViolationKind::Pattern("^arn:aws:.+$".to_string())),
                ],
                true,
            ),
            (
                "another member alone",
                vec![violation(
                    "messagePayload",
                    ViolationKind::Pattern("^[a-zA-Z0-9+/=]+$".to_string()),
                )],
                false,
            ),
            (
                "a missing ChannelARN, which is not a value of it at all",
                vec![violation("channelARN", ViolationKind::MissingRequiredMember)],
                false,
            ),
            ("no violations", Vec::new(), false),
        ];
        for (case, violations, expected) in cases {
            let blamed = blames_the_channel_arn(&violations);
            assert_eq!(
                blamed, expected,
                "case {case:?}: expected {expected}, got {blamed}"
            );
        }
    }

    #[test]
    fn a_missing_channel_is_named_by_its_channel_name_alone() {
        // The rows span the channel names an ARN carries: the plain form, one with the
        // characters the modelled name pattern admits, and a single character.
        let cases: &[&str] = &["demo-channel", "alexa.channel_2", "a"];
        for name in cases {
            let message = channel_does_not_exist(name);
            assert_eq!(
                message,
                format!("{name} channel does not exist !"),
                "case {name:?}: expected the service's sentence, got {message:?}"
            );
            assert!(
                !message.contains("arn:"),
                "case {name:?}: the message must not carry the ARN, got {message:?}"
            );
        }
    }
}
