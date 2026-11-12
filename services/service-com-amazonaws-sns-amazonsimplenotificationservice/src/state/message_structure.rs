// SPDX-License-Identifier: CC0-1.0

//! THE STRUCTURE OF A PUBLISHED BODY: how a message published under a `MessageStructure`
//! is read, and which text of it one delivery protocol receives.
//!
//! "Set `MessageStructure` to `json` if you want to send a different message for each
//! protocol" (`PublishInput$MessageStructure` doc): such a body is a JSON object whose
//! top-level members are the message per transport protocol, beside the `default` member
//! every protocol no member names receives. This module holds that reading in one place —
//! the rules that judge whether a declared structure parses ([`is_json_message_document`],
//! [`default_branch_body`]) and the selection one delivery performs
//! ([`structured_message_body`]) — so a body judged as structured is the body delivered as
//! structured.

use super::*;

/// Whether a `MessageStructure: json` body is one SNS can READ: a JSON object, which is
/// the document the structure declares ("use a JSON object for the Message parameter",
/// `PublishInput$Message` doc).
///
/// The other rule the member documents — "contain at least a top-level JSON key of
/// \"default\"" (`PublishInput$MessageStructure` doc) — is judged by
/// [`default_branch_body`], because a `default` member and a `default` member that names
/// a message are the same requirement: see [`NO_DEFAULT_ENTRY_REFUSAL`].
pub(super) fn is_json_message_document(message: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(message)
        .is_ok_and(|parsed| parsed.is_object())
}

/// The body a PROTOCOL-keyed branch of a structured message names, or `None` when it
/// names none.
///
/// A protocol's branch carries the message that protocol's subscribers receive, and it is
/// read as TEXT exactly as the `default` member is ([`scalar_text`]): a string branch is the
/// body itself, and a branch that is another SCALAR is the body written out as its own JSON
/// text. Recorded evidence: the branch a number names is delivered as that number's decimal
/// text, where this emulator withheld the message for every branch that was not a string.
///
/// A branch that names NO message names no body at all, and its protocol receives what it
/// would have received had the branch not been there at all: a composite value (an object or
/// a list) is no message, and neither is `null` — "Non-string values will cause the key to be
/// ignored" (`PublishInput$Message` doc) holds for exactly those, which is what the recorded
/// fallback for an object-valued and a null-valued branch shows.
pub(super) fn protocol_branch_body(value: &serde_json::Value) -> Option<String> {
    // Every value that names no message is already `None` here: a branch does not override
    // the `null` case the way the `default` member does ([`default_body`]).
    scalar_text(value)
}

/// The text SNS writes a message value out as, or `None` when the value names no message.
///
/// A string is the body itself; any other SCALAR is the body written out as its own JSON
/// text — a number as its decimal text, a boolean as `true` or `false`. A composite value (an
/// object or a list) is no message, and `null` names none either; the two differ only in
/// which member may still carry one, so each caller decides that about `null` itself.
///
/// ONE stringification, shared by the `default` member ([`default_body`]) and a protocol's
/// branch ([`protocol_branch_body`]), so the text a body is delivered as does not depend on
/// which member named it.
fn scalar_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}

/// The body the `default` member names, or `None` when it names none.
///
/// The `default` member is not a protocol branch: it is the message every protocol no
/// branch names receives, so a document naming no default body names no message at all
/// ([`NO_DEFAULT_ENTRY_REFUSAL`]) rather than leaving one protocol without a branch. It
/// is read as TEXT ([`scalar_text`]) — a string is the body itself and any other SCALAR is
/// the body written out as SNS writes that value, which is what the recording shows being
/// served where this emulator refused a `default` that was not a string. `null` names the
/// text SNS writes it as HERE, unlike in a protocol's branch: the recorded service delivers
/// `null` as the body of a document whose `default` is it. A composite value (an object or a
/// list) is no message, so it names no body.
pub(super) fn default_body(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => Some("null".to_string()),
        other => scalar_text(other),
    }
}

/// The body the `default` member of a structured message names, or `None` when it names
/// none — which is what makes a structured publish one with no message in it.
pub(super) fn default_branch_body(message: &str) -> Option<String> {
    json_object(message)?
        .get(DEFAULT_JSON_KEY)
        .and_then(default_body)
}

/// The body a subscriber over `protocol` receives of a published message.
///
/// A message published with `MessageStructure` of `json` carries ONE body per delivery
/// protocol: "You can define other top-level keys that define the message you want to
/// send to a specific transport protocol" (`PublishInput$MessageStructure` doc), so the
/// member named for the protocol is the body that protocol's subscribers receive and the
/// `default` member is the body for every protocol no member names. A branch that names
/// no body is IGNORED (see [`protocol_branch_body`]), so its protocol falls back to
/// `default` exactly as if the branch were absent.
///
/// A message published with no `MessageStructure` is one body for every protocol, and is
/// delivered as it was published. So is one whose structure names nothing SNS reads: the
/// document is then the message rather than a set of branches.
pub fn structured_message_body(
    message: &str,
    message_structure: Option<&str>,
    protocol: &str,
) -> String {
    if !message_structure.is_some_and(is_json_message_structure) {
        return message.to_string();
    }
    let Some(members) = json_object(message) else {
        return message.to_string();
    };
    members
        .get(protocol)
        .and_then(protocol_branch_body)
        // A structured message SNS accepted has a `default` naming a body, so this is
        // the body of every protocol no branch names rather than a fallback: a message
        // whose `default` names none is refused by [`message_refusal`] before it is
        // ever published.
        .or_else(|| members.get(DEFAULT_JSON_KEY).and_then(default_body))
        .unwrap_or_else(|| message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The protocol whose branch is selected below; the rule is about the member NAMED for a
    /// protocol, so which one it is does not enter it.
    const PROTOCOL: &str = "sqs";

    /// RULE: the branch a structured body names for a protocol is delivered as that value's
    /// own JSON text, and a branch naming no message at all falls back to `default`. The rows
    /// differ along the branch value's FORM, which is what the rule generalises over.
    #[test]
    fn a_scalar_branch_is_delivered_as_its_json_text() {
        // (case, the published body, the text an `sqs` subscriber receives)
        let rows: &[(&str, &str, &str)] = &[
            ("a string branch", r#"{"default":"d","sqs":"q"}"#, "q"),
            ("an empty string branch", r#"{"default":"d","sqs":""}"#, ""),
            ("a whole-number branch", r#"{"default":"d","sqs":7}"#, "7"),
            ("a zero branch", r#"{"default":"d","sqs":0}"#, "0"),
            (
                "a negative-number branch",
                r#"{"default":"d","sqs":-12}"#,
                "-12",
            ),
            ("a fractional branch", r#"{"default":"d","sqs":1.5}"#, "1.5"),
            (
                "a nine-digit branch",
                r#"{"default":"d","sqs":733857499}"#,
                "733857499",
            ),
            ("a true branch", r#"{"default":"d","sqs":true}"#, "true"),
            ("a false branch", r#"{"default":"d","sqs":false}"#, "false"),
            // A branch that names no message is ignored, so the protocol receives the
            // `default` body.
            ("a null branch", r#"{"default":"d","sqs":null}"#, "d"),
            ("an object branch", r#"{"default":"d","sqs":{"k":1}}"#, "d"),
            ("a list branch", r#"{"default":"d","sqs":["a"]}"#, "d"),
            ("no branch for this protocol", r#"{"default":"d"}"#, "d"),
            // The `default` member is read the same way, except that it may name the text
            // `null` itself.
            ("a number as the default", r#"{"default":42}"#, "42"),
            ("null as the default", r#"{"default":null}"#, "null"),
            ("an empty string as the default", r#"{"default":""}"#, ""),
        ];
        for (case, message, expected) in rows {
            let delivered =
                structured_message_body(message, Some(JSON_MESSAGE_STRUCTURE), PROTOCOL);
            assert_eq!(
                delivered, *expected,
                "case {case:?}: expected the body {expected:?}, got {delivered:?}"
            );
        }
    }

    /// RULE: a body published under no structure SNS reads is ONE message for every
    /// protocol, so nothing is selected out of it whatever it looks like.
    #[test]
    fn an_unstructured_body_is_delivered_as_published() {
        let document = r#"{"default":"d","sqs":7}"#;
        // (case, the `MessageStructure` published under)
        let rows: &[(&str, Option<&str>)] = &[
            ("no structure at all", None),
            ("a structure SNS defines nothing for", Some("xml")),
        ];
        for (case, structure) in rows {
            let delivered = structured_message_body(document, *structure, PROTOCOL);
            assert_eq!(
                delivered, document,
                "case {case:?}: expected the body as published, got {delivered:?}"
            );
        }
    }
}
