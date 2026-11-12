// SPDX-License-Identifier: CC0-1.0

//! The two header criteria `PutRawMessageContent` decides about the MIME content
//! it is given: the message must carry the header fields a message is required to
//! have, and it may not change the value of a header the mail system set before
//! the message reached the flow.
//!
//! The `RawMessageContent` documentation (`smithy_query(query="doc",
//! shape="RawMessageContent")`) states both as criteria "all MIME content must
//! meet" — "The message must contain all the required header fields" and "The
//! value of immutable headers must remain unchanged" — and
//! `PutRawMessageContent` models `MessageRejected` (HTTP 400) for "an error in the
//! MIME content", so content that breaks either is refused with that error and
//! installs nothing.
//!
//! Each test is a TABLE whose axis is the header block: which required field is
//! absent, and in what way an immutable header's value differs (rewritten,
//! removed, added, dropped from a repeated trace, reordered). Every row drives the
//! same in-process router as the rest of the suite through the `pub(crate)`
//! harness in [`crate::raw_message_content`]; state is reached only to SEED the
//! message in transit, standing in for WorkMail delivery, and to stage the S3
//! object in Amazon S3's own store.

use crate::raw_message_content::{reference, Emulator, Reply, StagedObject, DATE, REGION};

/// The header fields every message must carry, lowercased. RFC 5322 §3.6 marks
/// `orig-date` (`Date`) and `from` (`From`) as the two fields a message is
/// required to have; the model's `RawMessageContent` documentation requires "all
/// the required header fields" without naming them.
const REQUIRED_HEADERS: &[&str] = &["date", "from"];

/// A message built from `headers` — (field name, value) pairs in the order the
/// message carries them — with `body` after the empty line that ends the header
/// block.
fn message(headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut content: Vec<u8> = Vec::new();
    for (name, value) in headers {
        content.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(body.as_bytes());
    content
}

/// Assert `reply` is the rejection the model declares for unusable MIME content,
/// carrying a message that says what caused it.
pub(crate) fn assert_rejected(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {} ({})",
        reply.status,
        reply.summary()
    );
    assert_eq!(
        reply.error_type, "MessageRejected",
        "case {case:?}: expected MessageRejected, got {:?} ({})",
        reply.error_type,
        reply.summary()
    );
    assert!(
        !reply.error_message().is_empty(),
        "case {case:?}: expected a message saying what caused the rejection, got {}",
        reply.summary()
    );
}

/// A header field for the required field `name`, with a value of the form that
/// field takes.
fn required_field(name: &str) -> (&'static str, &'static str) {
    match name {
        "date" => ("Date", DATE),
        "from" => ("From", "sender@example.com"),
        other => panic!(
            "the required-header catalog names {other:?}, which this test has no value for"
        ),
    }
}

/// Put `content` in transit under a fresh id, offer `replacement` at a reference,
/// call `PutRawMessageContent` and report the reply together with what
/// `GetRawMessageContent` serves afterwards.
pub(crate) async fn update(content: &[u8], replacement: &[u8]) -> (Reply, Vec<u8>) {
    let emulator = Emulator::new();
    let message_id = "in-transit-0";
    emulator.seed_message(message_id, content);
    let s3_reference = emulator.seed_object(
        &reference("updated/object"),
        StagedObject::new(replacement.to_vec()),
    );

    let reply = emulator
        .put_raw_message_content(REGION, message_id, &s3_reference)
        .await;
    let after = emulator.get_raw_message_content(REGION, message_id).await;
    assert_eq!(
        after.status, 200,
        "expected the message to still be in transit after the update, got {} ({})",
        after.status,
        after.summary()
    );
    (reply, after.payload.to_vec())
}

// ---------------------------------------------------------------------------
// "The message must contain all the required header fields."
// ---------------------------------------------------------------------------

/// Content whose header block is missing a required field is refused, and the
/// message in transit keeps its content. The axis is WHICH required field is
/// absent and how: each field missing on its own, both missing, and a field that
/// appears only below the empty line that ends the header block.
#[tokio::test]
async fn content_missing_a_required_header_field_is_rejected() {
    // One row per required field missing on its own, so no row passes because
    // another field happened to be absent too. Built from the catalog above rather
    // than listed, so a field added to it is driven here as well.
    let mut refused: Vec<(String, Vec<u8>, Vec<&str>)> = Vec::new();
    for missing in REQUIRED_HEADERS {
        let present: Vec<(&str, &str)> = REQUIRED_HEADERS
            .iter()
            .copied()
            .filter(|name| name != missing)
            .map(required_field)
            .collect();
        refused.push((
            format!("only the {missing} field is missing"),
            message(&present, "body\r\n"),
            vec![*missing],
        ));
    }

    refused.extend([
        (
            "a header block with neither required field".to_string(),
            message(&[("X-Pad", "x")], "body\r\n"),
            REQUIRED_HEADERS.to_vec(),
        ),
        (
            "the shortest header-only message there is".to_string(),
            b"a:\r\n".to_vec(),
            REQUIRED_HEADERS.to_vec(),
        ),
        (
            "the required fields appear in the body, below the header block"
                .to_string(),
            message(
                &[("X-Pad", "x")],
                &format!("Date: {DATE}\r\nFrom: sender@example.com\r\n"),
            ),
            REQUIRED_HEADERS.to_vec(),
        ),
        (
            "a field whose name merely starts with a required one".to_string(),
            message(
                &[("Date-Received", DATE), ("From-Address", "a@example.com")],
                "body\r\n",
            ),
            REQUIRED_HEADERS.to_vec(),
        ),
    ]);

    for (case, content, missing) in refused {
        let original = message(
            &[("Date", DATE), ("From", "sender@example.com"), ("Subject", "original")],
            "body\r\n",
        );
        let (reply, after) = update(&original, &content).await;
        assert_rejected(&case, &reply);
        let reported = reply.error_message().to_ascii_lowercase();
        for name in &missing {
            assert!(
                reported.contains(*name),
                "case {case:?}: expected the rejection to name the missing {name} field, got {:?}",
                reply.error_message()
            );
        }
        assert_eq!(
            after, original,
            "case {case:?}: the rejected update changed the content in transit to {:?}",
            String::from_utf8_lossy(&after[..after.len().min(120)])
        );
    }

    // The same message with both required fields present is accepted, in every
    // form the fields may take: the rows above are refused for the absent field
    // and not for anything else about them.
    let accepted: Vec<(&str, Vec<u8>)> = vec![
        (
            "both required fields, nothing else",
            message(&[("Date", DATE), ("From", "sender@example.com")], ""),
        ),
        (
            "the required fields in lower case",
            message(&[("date", DATE), ("from", "sender@example.com")], "body\r\n"),
        ),
        (
            "the required fields in upper case",
            message(&[("DATE", DATE), ("FROM", "sender@example.com")], "body\r\n"),
        ),
        (
            "the required fields last in the block",
            message(
                &[
                    ("X-Pad", "x"),
                    ("Subject", "rewritten"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "body\r\n",
            ),
        ),
        ("a folded Date value", {
            let mut content = format!("Date: {DATE}\r\n").into_bytes();
            content.extend_from_slice(b" (the fold continues the same field)\r\n");
            content.extend_from_slice(b"From: sender@example.com\r\n\r\nbody\r\n");
            content
        }),
        (
            "a required field with no value at all",
            message(&[("Date", ""), ("From", "")], "body\r\n"),
        ),
    ];

    for (case, content) in accepted {
        let original = message(
            &[("Date", DATE), ("From", "sender@example.com"), ("Subject", "original")],
            "body\r\n",
        );
        let (reply, after) = update(&original, &content).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            after, content,
            "case {case:?}: expected the accepted content in transit, got {:?}",
            String::from_utf8_lossy(&after[..after.len().min(120)])
        );
    }
}

// ---------------------------------------------------------------------------
// "The value of immutable headers must remain unchanged."
// ---------------------------------------------------------------------------

/// The header block of a message already in transit: it carries both required
/// fields, the identifier the mail system gave the message and the trace of the
/// two relays that have already handled it.
fn in_transit_message() -> Vec<u8> {
    message(
        &[
            ("Received", "by relay-two with SMTP"),
            ("Received", "by relay-one with SMTP"),
            ("Message-ID", "<00000001@example.com>"),
            ("Date", DATE),
            ("From", "sender@example.com"),
            ("Subject", "original"),
        ],
        "the original body\r\n",
    )
}

/// An update that changes the value of an immutable header is refused and installs
/// nothing. The axis is HOW the value differs: rewritten, removed, added where
/// there was none, one of a repeated trace dropped, the trace reordered, one
/// appended.
#[tokio::test]
async fn an_update_that_changes_an_immutable_header_is_rejected() {
    let refused: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        (
            "the Message-ID rewritten",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("Received", "by relay-one with SMTP"),
                    ("Message-ID", "<99999999@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                    ("Subject", "original"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "the Message-ID dropped",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("Received", "by relay-one with SMTP"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "a Message-ID added to a message that had none",
            message(
                &[("Date", DATE), ("From", "sender@example.com")],
                "body\r\n",
            ),
            message(
                &[
                    ("Message-ID", "<invented@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "body\r\n",
            ),
        ),
        (
            "one Received line rewritten",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("Received", "by somewhere-else with SMTP"),
                    ("Message-ID", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "one Received line dropped",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("Message-ID", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "the Received trace reordered",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-one with SMTP"),
                    ("Received", "by relay-two with SMTP"),
                    ("Message-ID", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "a Received line appended to the trace",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("Received", "by relay-one with SMTP"),
                    ("Received", "by a relay that never saw it"),
                    ("Message-ID", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "every immutable header stripped at once",
            in_transit_message(),
            message(
                &[("Date", DATE), ("From", "sender@example.com")],
                "the original body\r\n",
            ),
        ),
    ];

    for (case, original, content) in refused {
        let (reply, after) = update(&original, &content).await;
        assert_rejected(case, &reply);
        assert_eq!(
            after, original,
            "case {case:?}: the rejected update changed the content in transit to {:?}",
            String::from_utf8_lossy(&after[..after.len().min(120)])
        );
    }
}

/// An update that carries every immutable header exactly as the message in transit
/// has it is accepted, however much else it rewrites. The axis is what the update
/// changes AROUND the immutable headers: the subject, the body, added headers,
/// the order of the block, the case of the field names.
#[tokio::test]
async fn an_update_that_preserves_the_immutable_headers_is_accepted() {
    let accepted: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        (
            "the subject and the body rewritten",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("Received", "by relay-one with SMTP"),
                    ("Message-ID", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "rewritten@example.com"),
                    ("Subject", "rewritten by the message flow"),
                ],
                "a body the rule says nothing about\r\n",
            ),
        ),
        (
            "headers added around the immutable ones",
            in_transit_message(),
            message(
                &[
                    ("Received", "by relay-two with SMTP"),
                    ("X-Scanned", "yes"),
                    ("Received", "by relay-one with SMTP"),
                    ("Message-ID", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                    ("Reply-To", "replies@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "the immutable headers moved to the end of the block",
            in_transit_message(),
            message(
                &[
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                    ("Subject", "rewritten"),
                    ("Received", "by relay-two with SMTP"),
                    ("Received", "by relay-one with SMTP"),
                    ("Message-ID", "<00000001@example.com>"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "the immutable field names in a different case",
            in_transit_message(),
            message(
                &[
                    ("RECEIVED", "by relay-two with SMTP"),
                    ("received", "by relay-one with SMTP"),
                    ("message-id", "<00000001@example.com>"),
                    ("Date", DATE),
                    ("From", "sender@example.com"),
                ],
                "the original body\r\n",
            ),
        ),
        (
            "a message with no immutable header on either side",
            message(
                &[("Date", DATE), ("From", "sender@example.com")],
                "body\r\n",
            ),
            message(
                &[
                    ("Date", DATE),
                    ("From", "someone-else@example.com"),
                    ("Subject", "rewritten"),
                ],
                "a different body\r\n",
            ),
        ),
    ];

    for (case, original, content) in accepted {
        let (reply, after) = update(&original, &content).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.summary()
        );
        assert_eq!(
            after, content,
            "case {case:?}: expected the accepted content in transit, got {:?}",
            String::from_utf8_lossy(&after[..after.len().min(120)])
        );
    }
}
