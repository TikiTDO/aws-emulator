// SPDX-License-Identifier: CC0-1.0

//! Whether an in-transit message may take an update, and which refusal it earns
//! when it may not — the one question `PutRawMessageContent` asks twice.
//!
//! The handler decides it in two critical sections: once before it resolves the
//! `S3Reference`, and again under the guard that installs the bytes, because the
//! store is unlocked while the MIME content is validated. Both sections answer
//! from the same rule — a message no request's region has in transit is
//! `ResourceNotFoundException` (HTTP 404) naming the id that was rejected, a
//! message that is stored but no longer eligible is `MessageFrozen` (HTTP 400),
//! and only a message in transit and eligible takes the update — so a caller
//! cannot tell which section refused it.
//!
//! The first test is a TABLE whose axis is the message's transit state crossed
//! with the form of the id and of the reference: every row drives the decision on
//! the happy path (where the accepting answer has to come out of BOTH sections for
//! the bytes to land) or on one of the two refusals. The second test drives the
//! decision in the second section specifically, by freezing the message while
//! updates are in flight, and asserts the answers are drawn from the same two
//! outcomes whichever section decided them.
//!
//! Everything goes over the wire through the `pub(crate)` harness in
//! [`crate::raw_message_content`]; state is reached only to SEED the message in
//! transit, standing in for WorkMail delivery, and to stage the S3 object in
//! Amazon S3's own store, as an S3 client's `PutObject` leaves it.

use std::sync::Arc;

use service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::state::{
    MessageKey, S3Ref,
};

use crate::raw_message_content::{
    mime, reference, versioned_reference, Emulator, Reply, StagedObject, REGION,
};

/// The longest `messageId` either operation accepts: `len 1..120` on both
/// (`smithy_query(query="operation", operation="PutRawMessageContent")`).
const MESSAGE_ID_MAX_LEN: usize = 120;

/// What the store holds under the id a request names.
#[derive(Debug, Clone, Copy)]
enum Transit {
    /// Nothing is in transit under that id in the request's region.
    Absent,
    /// A message is in transit and still eligible for an update.
    Eligible,
    /// A message is in transit but no longer eligible for an update.
    Frozen,
}

/// The answer the eligibility rule produces for a [`Transit`] state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// HTTP 404 `ResourceNotFoundException`, naming the rejected id.
    NotFound,
    /// HTTP 400 `MessageFrozen`, naming the message that may not change.
    Refused,
    /// HTTP 200, and the update's bytes are what the next read reports.
    Installed,
}

/// The transit state and the answer it earns, as the rule pairs them.
fn outcome_of(transit: Transit) -> Outcome {
    match transit {
        Transit::Absent => Outcome::NotFound,
        Transit::Eligible => Outcome::Installed,
        Transit::Frozen => Outcome::Refused,
    }
}

/// Assert `reply` is the answer `expected` names for `message_id`, including the
/// wording each refusal carries.
fn assert_answer(case: &str, reply: &Reply, expected: Outcome, message_id: &str) {
    let (status, error_type, message) = match expected {
        Outcome::NotFound => (
            404,
            "ResourceNotFoundException",
            format!("Unknown messageId: {message_id}"),
        ),
        Outcome::Refused => (
            400,
            "MessageFrozen",
            format!("The message {message_id} is not eligible for update."),
        ),
        Outcome::Installed => (200, "", String::new()),
    };

    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} ({})",
        reply.status,
        reply.summary()
    );
    assert_eq!(
        reply.error_type, error_type,
        "case {case:?}: expected error type {error_type:?}, got {:?} ({})",
        reply.error_type,
        reply.summary()
    );
    if expected != Outcome::Installed {
        assert_eq!(
            reply.error_message(),
            message,
            "case {case:?}: expected the message {message:?}, got {:?}",
            reply.error_message()
        );
    }
}

/// Every transit state, crossed with the id and reference forms the rule is
/// indifferent to: the answer depends on the message's eligibility and on nothing
/// else, and the accepting answer has to come out of both critical sections for
/// the update to land.
#[tokio::test]
async fn eligibility_decides_the_answer_whichever_critical_section_asks() {
    let cases: Vec<(&str, Transit, String, S3Ref)> = vec![
        (
            "a uuid-shaped id, in transit",
            Transit::Eligible,
            "00000000-0000-0000-0000-000000000001".to_string(),
            reference("updated/message-mime"),
        ),
        (
            "a uuid-shaped id, nothing in transit",
            Transit::Absent,
            "8e220c71-0e24-437a-98b8-f83d6c64d1c8".to_string(),
            reference("updated/message-mime"),
        ),
        (
            "a uuid-shaped id, frozen",
            Transit::Frozen,
            "0a2f71ba-6f46-4c2a-9d1a-7b4cb0f1c0d9".to_string(),
            reference("updated/message-mime"),
        ),
        (
            "the shortest permitted id, in transit",
            Transit::Eligible,
            "a".to_string(),
            reference("short/object"),
        ),
        (
            "the shortest permitted id, frozen",
            Transit::Frozen,
            "b".to_string(),
            reference("short/object"),
        ),
        (
            "the longest permitted id, nothing in transit",
            Transit::Absent,
            "z".repeat(MESSAGE_ID_MAX_LEN),
            reference("long/object"),
        ),
        (
            "the longest permitted id, in transit",
            Transit::Eligible,
            "m".repeat(MESSAGE_ID_MAX_LEN),
            reference("long/object"),
        ),
        (
            "dashes only, frozen",
            Transit::Frozen,
            "----------".to_string(),
            reference("dashes/object"),
        ),
        (
            "digits only, in transit, versioned reference",
            Transit::Eligible,
            "0123456789".to_string(),
            versioned_reference("versioned/object"),
        ),
        (
            "digits only, frozen, versioned reference",
            Transit::Frozen,
            "9876543210".to_string(),
            versioned_reference("versioned/object"),
        ),
        (
            "nothing in transit, versioned reference",
            Transit::Absent,
            "never-9".to_string(),
            versioned_reference("versioned/object"),
        ),
    ];

    for (case, transit, message_id, s3_reference) in cases {
        let emulator = Emulator::new();
        let original = mime("original", "the body the message arrived with");
        let updated = mime("rewritten", "the body the update installs");
        match transit {
            Transit::Absent => {}
            Transit::Eligible => emulator.seed_message(&message_id, &original),
            Transit::Frozen => emulator.with_state(|state| {
                state
                    .seed_in_transit_message(message_id.as_str(), original.clone())
                    .frozen = true;
            }),
        }
        let s3_reference = emulator.seed_object(&s3_reference, StagedObject::new(updated.clone()));

        let expected = outcome_of(transit);
        let reply = emulator
            .put_raw_message_content(REGION, &message_id, &s3_reference)
            .await;
        assert_answer(case, &reply, expected, &message_id);

        // What the next caller reads: the update's bytes only where the rule let
        // the update through, and the bytes the message arrived with everywhere
        // else.
        let readback = emulator.get_raw_message_content(REGION, &message_id).await;
        match expected {
            Outcome::NotFound => assert_eq!(
                readback.status, 404,
                "case {case:?}: the refused update put a message in transit: {}",
                readback.summary()
            ),
            Outcome::Refused => assert_eq!(
                readback.payload.as_ref(),
                original.as_slice(),
                "case {case:?}: the refused update changed the stored content: {}",
                readback.summary()
            ),
            Outcome::Installed => assert_eq!(
                readback.payload.as_ref(),
                updated.as_slice(),
                "case {case:?}: expected the update's bytes to be reported, got {}",
                readback.summary()
            ),
        }

        // Repeating the request asks the same question again, and answers it the
        // same way: the accepted row is still eligible, and a refused row is
        // refused identically.
        let again = emulator
            .put_raw_message_content(REGION, &message_id, &s3_reference)
            .await;
        assert_answer(case, &again, expected, &message_id);
    }
}

/// How many updates race the freeze below. Bounded: this is an integration test,
/// not a stress run.
const RACING_UPDATES: usize = 24;

/// The second critical section decides the same question as the first, so a
/// message frozen while an update is in flight is refused with `MessageFrozen`
/// rather than with some other answer — and never half-installed.
///
/// Whichever way the interleaving falls, every answer is drawn from the two the
/// rule allows for a stored message, and the content the next read reports is
/// exactly one of the two whole messages: the one the message arrived with, or the
/// one every racing update carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn freezing_a_message_mid_update_is_refused_with_the_same_answer() {
    let emulator = Arc::new(Emulator::new());
    let message_id = "racing-0";
    let original = mime("original", "the body the message arrived with");
    let updated = mime("rewritten", "the body every racing update installs");
    emulator.seed_message(message_id, &original);
    let s3_reference = emulator.seed_object(
        &reference("racing/object"),
        StagedObject::new(updated.clone()),
    );

    let mut updates = Vec::with_capacity(RACING_UPDATES);
    for _ in 0..RACING_UPDATES {
        let emulator = Arc::clone(&emulator);
        let s3_reference = s3_reference.clone();
        updates.push(tokio::spawn(async move {
            emulator
                .put_raw_message_content(REGION, message_id, &s3_reference)
                .await
        }));
    }

    // The freeze lands somewhere in the middle of that fan-out: before some
    // updates reach their first critical section and after others have passed it.
    let freezer = {
        let emulator = Arc::clone(&emulator);
        tokio::spawn(async move {
            emulator.with_state(|state| {
                let message = state
                    .messages
                    .get_mut(&MessageKey::new(REGION, message_id))
                    .expect("the message is in transit");
                // Only the eligibility flag moves: the content stays whatever the
                // updates have made it, so the read below reports their work and
                // not the freeze's.
                message.frozen = true;
            });
        })
    };

    let mut accepted = 0usize;
    for (index, update) in updates.into_iter().enumerate() {
        let reply = update.await.expect("the update task ran to completion");
        let case = format!("racing update {index}");
        let expected = if reply.status == 200 {
            accepted += 1;
            Outcome::Installed
        } else {
            Outcome::Refused
        };
        assert_answer(&case, &reply, expected, message_id);
    }
    freezer.await.expect("the freeze task ran to completion");

    // Every racing update carries the same bytes and only an accepted one writes,
    // so the content a read reports is those bytes when any update was accepted
    // and the message's original content when none was: never a mixture.
    let readback = emulator.get_raw_message_content(REGION, message_id).await;
    assert_eq!(
        readback.status, 200,
        "case \"readback\": expected HTTP 200, got {} ({})",
        readback.status,
        readback.summary()
    );
    let expected: &[u8] = if accepted > 0 { &updated } else { &original };
    assert_eq!(
        readback.payload.as_ref(),
        expected,
        "case \"readback\": {accepted} update(s) were accepted, so expected {} octet(s), got {}",
        expected.len(),
        readback.summary()
    );

    // Once the message is frozen, the rule has one answer left for it.
    let after = emulator
        .put_raw_message_content(REGION, message_id, &s3_reference)
        .await;
    assert_answer("after the freeze", &after, Outcome::Refused, message_id);
}
