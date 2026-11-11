// SPDX-License-Identifier: CC0-1.0

//! The VALUE half of the clause a batch member's `@length` refusal states.
//!
//! One rule: a constraint clause about a LIST-typed member quotes the value the request
//! carried — the collection itself, rendered element by element in request order, each
//! element as its shape's name over every member that shape declares with one the request
//! left unset stated as `null` — and never a summary of the collection's size, at EVERY
//! size that breaks the member's declared `@length`.
//!
//! The sibling suite `constraint_refusals` asserts that rule at the two ends of the
//! declared bound and owns the clause builders (`one_violation`, `rendered_event`,
//! `rendered_batch`) and the declared maximum, which this suite reaches rather than
//! restating. The axis here is how far PAST the bound a batch runs and which members its
//! entries carried — the parts of the rendered value that come from the request rather
//! than from the template.
//!
//! The client is this crate's one test harness, reached from the suite that owns it.

use crate::constraint_refusals::{
    at_most_requirement, batch_length_clause, message_of, one_violation, rendered_event,
    MAX_BATCH,
};
use crate::put_audit_events::{channel_arn, ev, Ev, Harness};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// The whole message a batch past the declared maximum is refused with.
fn expected_message(events: &[Ev]) -> String {
    one_violation(&batch_length_clause(events, &at_most_requirement(MAX_BATCH)))
}

/// One audit-event payload per shape a caller sends, cycled through a batch so no row of
/// the table below quotes one repeated element: a flat object, a nested one, one carrying
/// non-ASCII text, and one carrying characters the clause's own punctuation uses.
const PAYLOADS: [&str; 4] = [
    r#"{"eventSource":"custom.app","eventName":"Login"}"#,
    r#"{"eventSource":"custom.app","userIdentity":{"type":"Custom","principalId":"p-1"}}"#,
    r#"{"eventSource":"custom.app","note":"héllo wörld — ok"}"#,
    r#"{"eventSource":"custom.app","detail":"a, b=c (d) ['e']"}"#,
];

/// How an entry's `eventDataChecksum` is filled in across a batch.
#[derive(Debug, Clone, Copy)]
enum Checksums {
    /// No entry carries one: every element states `eventDataChecksum=null`.
    Omitted,
    /// Every entry carries one.
    Everywhere,
    /// Every third entry carries one, so one batch holds both renderings.
    EveryThird,
}

impl Checksums {
    fn of(self, index: usize) -> Option<String> {
        match self {
            Self::Omitted => None,
            Self::Everywhere => Some(format!("checksum-{index}")),
            Self::EveryThird => index.is_multiple_of(3).then(|| format!("checksum-{index}")),
        }
    }
}

/// A batch of `count` entries, cycling the payload shapes and filling checksums in as
/// `checksums` says.
fn batch(count: usize, checksums: Checksums) -> Vec<Ev> {
    (0..count)
        .map(|index| {
            let event = ev(
                format!("evt-{index}"),
                PAYLOADS[index % PAYLOADS.len()].to_string(),
            );
            match checksums.of(index) {
                Some(checksum) => event.with_checksum(checksum),
                None => event,
            }
        })
        .collect()
}

/// A batch past the `auditEvents` maximum is refused with the batch itself quoted in the
/// clause: every element it carried, in request order, each over the three members
/// `AuditEvent` declares — never a count of them.
///
/// Spans how far past the bound the batch ran and which members its entries carried, the
/// two things the rendered value is built from.
#[tokio::test]
async fn an_over_sized_batch_is_quoted_element_by_element() {
    let harness = Harness::new();
    let arn = channel_arn("4f1e2d3c-5b6a-4798-8a9b-0c1d2e3f4a5b");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    // (case, the batch submitted)
    let rows: Vec<(&str, Vec<Ev>)> = vec![
        (
            "one event past the maximum, no entry carrying a checksum",
            batch(MAX_BATCH + 1, Checksums::Omitted),
        ),
        (
            "one event past the maximum, every entry carrying a checksum",
            batch(MAX_BATCH + 1, Checksums::Everywhere),
        ),
        (
            "one event past the maximum, some entries carrying a checksum",
            batch(MAX_BATCH + 1, Checksums::EveryThird),
        ),
        (
            "half again the maximum, some entries carrying a checksum",
            batch(MAX_BATCH + MAX_BATCH / 2, Checksums::EveryThird),
        ),
        (
            "twice the maximum, no entry carrying a checksum",
            batch(2 * MAX_BATCH, Checksums::Omitted),
        ),
    ];

    for (case, events) in &rows {
        let reply = harness.put(&arn, None, events).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with body {}",
            reply.status,
            reply.body
        );
        let stated = message_of(&reply);
        let expected = expected_message(events);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the clause to quote the batch the request carried, got \
             {stated:?}"
        );
        // The count summary the validating walk has for a collection is never what the
        // clause states: the value half is the collection, so no row may fall back to one.
        assert!(
            !stated.contains(&format!("'{} items'", events.len())),
            "case {case:?}: the clause states a count summary instead of the batch, got \
             {stated:?}"
        );
    }
}

/// The quoted batch is the REQUEST's, element for element: the ids and payloads it carried,
/// in the order it carried them, and each entry's own checksum where it sent one. Asserted
/// element by element, so an element rendered out of order, dropped or filled in from
/// another entry is reported as the element it diverged on rather than as one long
/// mismatching message.
#[tokio::test]
async fn the_quoted_batch_states_each_submitted_entry_in_order() {
    let harness = Harness::new();
    let arn = channel_arn("5a2b3c4d-6e7f-4809-9b0c-1d2e3f4a5b6c");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    let events = batch(MAX_BATCH + 7, Checksums::EveryThird);
    let reply = harness.put(&arn, None, &events).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ValidationException"),
        "case \"a batch seven events past the maximum\": expected 400 ValidationException, \
         got {} with body {}",
        reply.status,
        reply.body
    );
    let stated = message_of(&reply);
    for (index, event) in events.iter().enumerate() {
        let element = rendered_event(event.id(), event.data(), event.checksum());
        // The element's own rendering, with the punctuation its POSITION in the batch puts
        // around it: the opening bracket before the first, a separator before every other,
        // and the closing bracket after the last.
        let positioned = match index {
            0 => format!("'[{element}, "),
            last if last + 1 == events.len() => format!(", {element}]'"),
            _ => format!(", {element}, "),
        };
        assert!(
            stated.contains(&positioned),
            "case \"element {index}\": expected the clause to carry {positioned:?}, got \
             {stated:?}"
        );
    }
}
