// SPDX-License-Identifier: CC0-1.0

//! The access record `PutRawMessageContent` leaves, on every outcome.
//!
//! RULE: this service's one MUTATING operation leaves a structured record of every
//! call it decides — who asked (the access key id the request presented), which
//! message, which S3 reference and in which region the call was made, together with
//! the outcome it answered — so a change to an in-transit message's content, and
//! every refusal of one, is reconstructable from the emulator's own records. The
//! reporting operation leaves no such record: it changes nothing.
//!
//! The rows span the outcomes the operation can reach: the accepted update, the two
//! eligibility refusals (`ResourceNotFoundException`, `MessageFrozen`), the content
//! refusal (`MessageRejected`) and the location refusal
//! (`InvalidContentLocation`) — each driven over the wire through the same harness
//! as the rest of the suite, with a capturing `tracing` subscriber scoped to the
//! calling thread. Nothing is written to disk and no global subscriber is installed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use crate::raw_message_content::{
    mime, reference, regional_reference, Emulator, StagedObject, REGION,
};

/// The access key id every request in this suite presents, which is the one the
/// harness signs with: the record has to carry the caller it was told about.
const PRESENTED_AKID: &str = "AKIDEXAMPLE";

/// One captured event: its fields rendered as strings, whatever type each was
/// recorded as.
#[derive(Clone, Debug)]
struct CapturedEvent {
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a test can read
/// the records back.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// Every captured record of `operation`, oldest first.
    fn records_for(&self, operation: &str) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| event.field("operation") == Some(operation))
            .cloned()
            .collect()
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // This suite asserts on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent { fields });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Capture this thread's `tracing` events for the life of the returned guard.
fn capture_events() -> (Capture, tracing::subscriber::DefaultGuard) {
    let capture = Capture::default();
    let guard = tracing::subscriber::set_default(capture.clone());
    (capture, guard)
}

/// What one row stages before it calls the update.
#[derive(Debug, Clone, Copy)]
enum Staged {
    /// A message in transit and a readable object: the update is accepted.
    Updatable,
    /// Nothing in transit under the id the request names.
    NoMessage,
    /// A message in transit that is no longer eligible for an update.
    Frozen,
    /// A message in transit and an object whose bytes are not usable MIME.
    UnusableContent,
    /// A message in transit and a reference naming an object S3 does not hold at
    /// all.
    NoObject,
    /// A message in transit and an object in a bucket in another Region.
    ObjectElsewhere,
}

/// Every outcome of the mutating operation leaves one record, carrying the caller,
/// the message, the reference, the region and that outcome.
#[tokio::test]
async fn every_update_outcome_leaves_one_access_record() {
    // (case, what is staged, the outcome the record must name, the HTTP status the
    // wire answer must carry)
    let rows: [(&str, Staged, &str, u16); 6] = [
        ("an accepted update", Staged::Updatable, "success", 200),
        (
            "nothing in transit under that id",
            Staged::NoMessage,
            "ResourceNotFoundException",
            404,
        ),
        (
            "a message that is no longer eligible",
            Staged::Frozen,
            "MessageFrozen",
            400,
        ),
        (
            "content that is not usable MIME",
            Staged::UnusableContent,
            "MessageRejected",
            400,
        ),
        (
            "a reference naming no object",
            Staged::NoObject,
            "InvalidContentLocation",
            400,
        ),
        (
            "an object in a bucket in another Region",
            Staged::ObjectElsewhere,
            "InvalidContentLocation",
            400,
        ),
    ];

    for (case, staged, outcome, status) in rows {
        let emulator = Emulator::new();
        let message_id = "access-record-0";
        let original = mime("original", "body");
        match staged {
            Staged::NoMessage => {}
            Staged::Frozen => emulator.with_state(|state| {
                state
                    .seed_in_transit_message(message_id, original.clone())
                    .frozen = true;
            }),
            _ => emulator.seed_message(message_id, &original),
        }
        let s3_reference = match staged {
            // Nothing is staged in S3, so the reference names no object.
            Staged::NoObject => reference("never/stored"),
            Staged::UnusableContent => emulator.seed_object(
                &reference("updated/unusable"),
                StagedObject::new(b"this is not a message at all\r\n".to_vec()),
            ),
            Staged::ObjectElsewhere => emulator.seed_object(
                &regional_reference("eu-west-1", "elsewhere/object"),
                StagedObject::new(mime("elsewhere", "body")).in_region("eu-west-1"),
            ),
            _ => emulator.seed_object(
                &reference("updated/message-mime"),
                StagedObject::new(mime("rewritten", "body")),
            ),
        };

        // The capture starts here, so only the call under test is in it: the
        // staging above reaches the stores directly and emits nothing.
        let (capture, guard) = capture_events();
        let reply = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        // And the reporting operation runs inside the capture too, to show it adds
        // no record of its own.
        let _ = emulator.get_raw_message_content(REGION, message_id).await;
        drop(guard);

        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            reply.status,
            reply.summary()
        );

        let records = capture.records_for("PutRawMessageContent");
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected exactly one access record for the update, captured \
             {records:?}"
        );
        let record = &records[0];
        assert_eq!(
            record.field("outcome"),
            Some(outcome),
            "case {case:?}: expected the record to name outcome {outcome:?}, got {record:?}"
        );
        assert_eq!(
            record.field("akid"),
            Some(PRESENTED_AKID),
            "case {case:?}: expected the record to name the presented access key id, got \
             {record:?}"
        );
        assert_eq!(
            record.field("message_id"),
            Some(message_id),
            "case {case:?}: expected the record to name the message the call addressed, got \
             {record:?}"
        );
        assert_eq!(
            record.field("region"),
            Some(REGION),
            "case {case:?}: expected the record to name the region the call was made in, got \
             {record:?}"
        );
        // The reference is on every record except the two the eligibility rule
        // decides before the reference is resolved at all.
        let expects_reference = !matches!(staged, Staged::NoMessage | Staged::Frozen);
        let reference_field = record.field("reference").unwrap_or_default();
        assert_eq!(
            !reference_field.is_empty(),
            expects_reference,
            "case {case:?}: expected the reference to be recorded={expects_reference}, got \
             {record:?}"
        );
        if expects_reference {
            assert!(
                reference_field.contains(&s3_reference.bucket)
                    && reference_field.contains(&s3_reference.key),
                "case {case:?}: expected the record's reference to name the bucket and key the \
                 request carried, got {reference_field:?}"
            );
        }
    }
}

/// A read leaves no record at all: `GetRawMessageContent` changes nothing, so there
/// is no access to attribute.
#[tokio::test]
async fn a_read_leaves_no_record_of_its_own() {
    let emulator = Emulator::new();
    let message_id = "access-record-1";
    emulator.seed_message(message_id, &mime("original", "body"));

    let (capture, guard) = capture_events();
    let reply = emulator.get_raw_message_content(REGION, message_id).await;
    let missing = emulator.get_raw_message_content(REGION, "never-there").await;
    drop(guard);

    assert_eq!(
        reply.status, 200,
        "case \"the read\": expected HTTP 200, got {} ({})",
        reply.status,
        reply.summary()
    );
    assert_eq!(
        missing.status, 404,
        "case \"the read of an unknown id\": expected HTTP 404, got {} ({})",
        missing.status,
        missing.summary()
    );
    let records = capture.records_for("GetRawMessageContent");
    assert!(
        records.is_empty(),
        "case \"the read\": expected a reporting operation to leave no access record, captured \
         {records:?}"
    );
}
