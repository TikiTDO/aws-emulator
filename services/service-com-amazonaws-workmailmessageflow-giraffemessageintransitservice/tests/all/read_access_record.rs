// SPDX-License-Identifier: CC0-1.0

//! The access record `GetRawMessageContent` leaves, on every outcome.
//!
//! RULE: reporting the content of an in-transit message is an ACCESS to that
//! message, so every call of the reporting operation leaves exactly one structured
//! record of the call it decided — who asked (the access key id the request
//! presented), which message it addressed, which region the call was made in and
//! the outcome it answered — and that record carries the SIZE of what was reported
//! and never the content itself, so access to message content is reconstructable
//! from the emulator's own records without those records becoming a second copy of
//! the messages they granted access to. A read changes nothing, so it leaves no
//! record of a change.
//!
//! The rows span what the rule generalises over: the two outcomes the operation can
//! reach (`success`, `ResourceNotFoundException`), the id's form and length (one
//! octet up to the modelled 120), the content's size (a few octets up to many
//! kilobytes), the region the call is made in, a message that is no longer eligible
//! for an update (still readable), and a message addressed through another region's
//! endpoint (not reachable at all). A second test drives the "one record per CALL"
//! half over a run of repeated reads.
//!
//! Every case goes over the wire through the same harness as the rest of the suite,
//! with a capturing `tracing` subscriber scoped to the calling thread
//! ([`crate::update_access_record::capture_events`], shared with the suite that
//! covers the mutating operation's records). Nothing is written to disk and no
//! global subscriber is installed.

use crate::raw_message_content::{mime, Emulator, REGION};
use crate::update_access_record::{capture_events, CapturedEvent, PRESENTED_AKID};

/// The `messageId` length ceiling the model states for both operations
/// (`smithy_query(query="operation", operation="GetRawMessageContent")`:
/// `messageIdType, len 1..120, pattern ^[a-z0-9\-]*$`).
const MESSAGE_ID_MAX_LEN: usize = 120;

/// What one row puts in transit before it reads.
#[derive(Debug, Clone, Copy)]
enum Staged {
    /// A message in transit in the region the read is made in.
    InTransit,
    /// A message in transit in that region that is no longer eligible for an
    /// update: only `PutRawMessageContent` refuses one, so a read still reports it.
    Frozen,
    /// Nothing in transit under the id the read names.
    Nothing,
    /// A message in transit under that id in ANOTHER region's message flow, which
    /// this region's endpoint does not reach.
    InAnotherRegion,
}

/// The body of every row's content carries this, so a record that leaked the
/// content it reported is visible as such.
const SENTINEL: &str = "sentinel-body-text";

/// Assert the shape of one access record: the caller, the message, the region, the
/// outcome, and no content.
fn assert_record(
    case: &str,
    record: &CapturedEvent,
    outcome: &str,
    addressed: &str,
    region: &str,
    content_octets: Option<usize>,
) {
    assert_eq!(
        record.field("outcome"),
        Some(outcome),
        "case {case:?}: expected the record to name outcome {outcome:?}, got {record:?}"
    );
    assert_eq!(
        record.field("akid"),
        Some(PRESENTED_AKID),
        "case {case:?}: expected the record to name the presented access key id, got {record:?}"
    );
    assert_eq!(
        record.field("message_id"),
        Some(addressed),
        "case {case:?}: expected the record to name the message the call addressed, got {record:?}"
    );
    assert_eq!(
        record.field("region"),
        Some(region),
        "case {case:?}: expected the record to name the region the call was made in, got \
         {record:?}"
    );
    // The size of what was reported is a count, not content: it is on the record
    // exactly when something was reported.
    let expected_octets = content_octets.map(|octets| octets.to_string());
    assert_eq!(
        record.field("content_octets"),
        expected_octets.as_deref(),
        "case {case:?}: expected the record to name the octets reported as {expected_octets:?}, \
         got {record:?}"
    );
    assert!(
        record.fields.values().all(|value| !value.contains(SENTINEL)),
        "case {case:?}: expected the record to carry no message content, got {record:?}"
    );
}

/// Every outcome of a read leaves one record of the access, whatever the id, the
/// content size, the region or the state of the message it addressed.
#[tokio::test]
async fn every_read_outcome_leaves_one_access_record() {
    // (case, what is staged, the region the read is made in, the id it addresses,
    // the body size in octets, the outcome the record must name, the HTTP status the
    // wire answer must carry)
    let rows: [(&str, Staged, &str, String, usize, &str, u16); 8] = [
        (
            "a message in transit, read in its own region",
            Staged::InTransit,
            REGION,
            "access-read-0".to_string(),
            16,
            "success",
            200,
        ),
        (
            "a one-octet message id",
            Staged::InTransit,
            REGION,
            "a".to_string(),
            16,
            "success",
            200,
        ),
        (
            "a message id at the modelled length ceiling",
            Staged::InTransit,
            REGION,
            "m".repeat(MESSAGE_ID_MAX_LEN),
            16,
            "success",
            200,
        ),
        (
            "content of many kilobytes",
            Staged::InTransit,
            REGION,
            "access-read-large".to_string(),
            64 * 1024,
            "success",
            200,
        ),
        (
            "a read made in another region, of that region's own message",
            Staged::InTransit,
            "eu-west-1",
            "access-read-regional".to_string(),
            32,
            "success",
            200,
        ),
        (
            "a message no longer eligible for an update",
            Staged::Frozen,
            REGION,
            "access-read-frozen".to_string(),
            48,
            "success",
            200,
        ),
        (
            "nothing in transit under that id",
            Staged::Nothing,
            REGION,
            "access-read-missing".to_string(),
            0,
            "ResourceNotFoundException",
            404,
        ),
        (
            "a message in transit in another region only",
            Staged::InAnotherRegion,
            REGION,
            "access-read-elsewhere".to_string(),
            24,
            "ResourceNotFoundException",
            404,
        ),
    ];

    for (case, staged, region, message_id, body_octets, outcome, status) in rows {
        let emulator = Emulator::new();
        let content = mime("original", &SENTINEL.repeat(1 + body_octets / SENTINEL.len()));
        match staged {
            Staged::InTransit => emulator.with_state(|state| {
                state.seed_in_transit_message_in_region(region, &message_id, content.clone());
            }),
            Staged::Frozen => emulator.with_state(|state| {
                state
                    .seed_in_transit_message_in_region(region, &message_id, content.clone())
                    .frozen = true;
            }),
            Staged::Nothing => {}
            Staged::InAnotherRegion => emulator.with_state(|state| {
                state.seed_in_transit_message_in_region(
                    "ap-southeast-2",
                    &message_id,
                    content.clone(),
                );
            }),
        }

        // The capture starts here, so only the call under test is in it: the staging
        // above reaches the store directly and emits nothing.
        let (capture, guard) = capture_events();
        let reply = emulator.get_raw_message_content(region, &message_id).await;
        drop(guard);

        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            reply.status,
            reply.summary()
        );

        // A read decides nothing about the message, so it leaves no record of a
        // change.
        let changes = capture.records_for("PutRawMessageContent");
        assert!(
            changes.is_empty(),
            "case {case:?}: expected no record of a change, captured {changes:?}"
        );

        let records = capture.records_for("GetRawMessageContent");
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected exactly one access record for the read, captured {records:?}"
        );
        // The octets the record names are the octets the caller was handed, which is
        // what makes the record an account of this access rather than of a guess.
        let reported = (status == 200).then_some(reply.payload.len());
        assert_record(case, &records[0], outcome, &message_id, region, reported);
    }
}

/// One record per CALL: a message read again and again leaves one record of each
/// access, in the order the calls were made, and a refused read between them is
/// recorded as itself.
#[tokio::test]
async fn each_read_of_one_message_leaves_its_own_record() {
    let emulator = Emulator::new();
    let message_id = "access-read-repeated";
    let content = mime("original", SENTINEL);
    emulator.seed_message(message_id, &content);

    // (the id the call addresses, the outcome its record must name, the status)
    let calls: [(&str, &str, u16); 5] = [
        (message_id, "success", 200),
        (message_id, "success", 200),
        ("not-in-transit", "ResourceNotFoundException", 404),
        (message_id, "success", 200),
        ("also-not-in-transit", "ResourceNotFoundException", 404),
    ];

    let (capture, guard) = capture_events();
    let mut replies = Vec::new();
    for (addressed, _, _) in calls {
        replies.push(emulator.get_raw_message_content(REGION, addressed).await);
    }
    drop(guard);

    let records = capture.records_for("GetRawMessageContent");
    assert_eq!(
        records.len(),
        calls.len(),
        "case \"repeated reads\": expected one access record per call for {} calls, captured \
         {records:?}",
        calls.len()
    );
    for (index, ((addressed, outcome, status), reply)) in calls.iter().zip(&replies).enumerate() {
        let case = format!("read {index} of {addressed:?}");
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            reply.status,
            reply.summary()
        );
        let reported = (*status == 200).then_some(reply.payload.len());
        assert_record(&case, &records[index], outcome, addressed, REGION, reported);
    }

    // Reading a message many times still changed nothing: the content a later
    // caller sees is the content the first one saw.
    let last = emulator.get_raw_message_content(REGION, message_id).await;
    assert_eq!(
        last.payload.as_ref(),
        content.as_slice(),
        "case \"after the repeated reads\": expected the stored content to be reported unchanged, \
         got {}",
        last.summary()
    );
}
