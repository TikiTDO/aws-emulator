// SPDX-License-Identifier: CC0-1.0

//! How a request refused BEFORE the handler runs is stated, asserted over tables.
//!
//! Two rules:
//!
//! 1. A modelled-constraint refusal is HTTP 400 `ValidationException` carrying the clause
//!    envelope built from the request — `N validation error(s) detected: Value '<what the
//!    request carried>' at '<member path>' failed to satisfy constraint: <requirement>` —
//!    in which a `@pattern` requirement states the expression WITHOUT the whole-value
//!    anchors the model wraps it in, and a COLLECTION member's clause quotes the
//!    collection the request carried rather than a summary of its size. The check itself
//!    is unchanged: the model's pattern is still applied as written.
//! 2. A channel reference that is not a channel reference is answered first, under the
//!    error this service's model declares for that fault, whatever else the same request
//!    broke — the service resolves every call against the channel before it reads the
//!    payload.
//!
//! The client is this crate's one test harness, reached from the suite that owns it.

use crate::put_audit_events::{channel_arn, ev, Ev, Harness, Reply};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// A JSON audit event that passes per-event validation, so nothing but the member under
/// test decides these calls.
pub const GOOD_EVENT: &str = r#"{"eventSource":"custom.app","eventName":"Login"}"#;

/// The largest batch `auditEvents` admits, from the member's own `@length`
/// (`smithy_query(query="shape", shape="AuditEvents")`: min 1, max 100).
pub const MAX_BATCH: usize = 100;
const MIN_BATCH: usize = 1;

/// The expression `AuditEvent$id` is modelled with, and the bounds on it
/// (`smithy_query(query="shape", shape="Uuid")`: `@pattern ^[-_A-Za-z0-9]+$`,
/// `@length 1..128`).
pub const ID_PATTERN: &str = "^[-_A-Za-z0-9]+$";
pub const ID_MAX_LENGTH: usize = 128;

/// The expression `PutAuditEventsRequest$externalId` is modelled with
/// (`smithy_query(query="shape", shape="ExternalId")`).
const EXTERNAL_ID_PATTERN: &str = r"^[\w+=,.@:\/-]*$";

/// `pattern` as a clause states it: the expression without the `^…$` pair the model wraps
/// a whole-value pattern in.
fn unanchored(pattern: &str) -> &str {
    pattern
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .unwrap_or(pattern)
}

/// The prose member of a refusal.
pub fn message_of(reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The envelope one clause travels in.
pub fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The requirement half a `@length` MAXIMUM states, for a member of any type.
pub fn at_most_requirement(maximum: usize) -> String {
    format!("Member must have length less than or equal to {maximum}")
}

/// The requirement half a `@length` MINIMUM states.
pub fn at_least_requirement(minimum: usize) -> String {
    format!("Member must have length greater than or equal to {minimum}")
}

/// The requirement half a `@pattern` violation states: the expression as a clause quotes
/// it, without the whole-value anchors the model wraps it in.
pub fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// ONE clause of a refusal: the value the request carried at `member_path`, and the
/// requirement it broke. THE one home for that template, so a change in how AWS words it
/// is edited once; every clause builder below and in the sibling suites is expressed over
/// it.
pub fn clause(value: &str, member_path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The clause the `auditEvents` member's own `@length` states under `requirement`: the
/// batch the request carried, quoted whole.
pub fn batch_length_clause(events: &[Ev], requirement: &str) -> String {
    clause(&rendered_batch(events), "auditEvents", requirement)
}

/// The clause one batch element's own `id` violation states: the rejected value verbatim
/// at the element's 1-BASED position under the list member.
pub fn element_id_clause(position: usize, value: &str, requirement: &str) -> String {
    clause(value, &format!("auditEvents.{position}.member.id"), requirement)
}

/// How the rendering of an `AuditEvent` reads inside a clause: the shape's name over every
/// member it declares, in declaration order, with one the request left unset stated as
/// `null` (`smithy_query(query="shape", shape="AuditEvent")`: id, eventData,
/// eventDataChecksum).
pub fn rendered_event(id: &str, event_data: &str, checksum: Option<&str>) -> String {
    format!(
        "AuditEvent(id={id}, eventData={event_data}, eventDataChecksum={})",
        checksum.unwrap_or("null")
    )
}

/// A batch of `count` events, each carrying `GOOD_EVENT`.
fn batch(count: usize) -> Vec<Ev> {
    (0..count)
        .map(|index| ev(format!("evt-{index}"), GOOD_EVENT))
        .collect()
}

/// How that batch reads inside a clause: the elements in order, comma-separated, in
/// brackets.
pub fn rendered_batch(events: &[Ev]) -> String {
    let elements: Vec<String> = events
        .iter()
        .map(|event| rendered_event(event.id(), event.data(), event.checksum()))
        .collect();
    format!("[{}]", elements.join(", "))
}

/// A refusal of the `auditEvents` member's `@length` quotes the BATCH the request carried,
/// not a summary of its size — the value such a clause rejects is the collection itself.
/// Asserted at both ends of the declared bound, and over batch sizes that differ in how
/// much text the clause has to carry.
#[tokio::test]
async fn a_batch_outside_its_declared_length_is_quoted_whole() {
    let harness = Harness::new();
    let arn = channel_arn("8c7f4e0d-93a6-4b25-8d4c-7e9f0a1b2c3d");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    // (case, the batch, the requirement the clause states). The two rows inside the bound
    // are the control: they are served, so the bound is reported only where it is broken.
    let rows: Vec<(&str, Vec<Ev>, Option<String>)> = vec![
        ("an empty batch", batch(0), Some(at_least_requirement(MIN_BATCH))),
        ("one event, the smallest batch admitted", batch(MIN_BATCH), None),
        ("a batch at the declared maximum", batch(MAX_BATCH), None),
        (
            "one event past the declared maximum",
            batch(MAX_BATCH + 1),
            Some(at_most_requirement(MAX_BATCH)),
        ),
        (
            "twenty events past the declared maximum",
            batch(MAX_BATCH + 20),
            Some(at_most_requirement(MAX_BATCH)),
        ),
    ];

    for (case, events, requirement) in &rows {
        let reply = harness.put(&arn, None, events).await;
        match requirement {
            None => assert_eq!(
                reply.status, 200,
                "case {case:?}: a batch inside the bound is served, got {} with body {}",
                reply.status, reply.body
            ),
            Some(requirement) => {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected 400 ValidationException, got {} with body {}",
                    reply.status,
                    reply.body
                );
                let expected = one_violation(&batch_length_clause(events, requirement));
                let stated = message_of(&reply);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected the clause to quote the batch the request \
                     carried ({expected:?}), got {stated:?}"
                );
            }
        }
    }
}

/// A `@pattern` requirement states the expression a value must satisfy WITHOUT the
/// whole-value anchors the model wraps it in, and the clause quotes the value the request
/// carried. Spans the two pattern-constrained members a request reaches — an event id and
/// the external id — and, for the id, the `@length` bound beside the pattern.
#[tokio::test]
async fn a_constraint_clause_states_the_expression_without_its_anchors() {
    let harness = Harness::new();
    let arn = channel_arn("9d8a5f1e-a4b7-4c36-8e5d-8f0a1b2c3d4e");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    let over_long_id = "i".repeat(ID_MAX_LENGTH + 1);

    // (case, the event id, the externalId, the clause expected)
    let rows: Vec<(&str, String, Option<&str>, String)> = vec![
        (
            "an event id carrying a space",
            "event id".to_string(),
            None,
            element_id_clause(1, "event id", &pattern_requirement(ID_PATTERN)),
        ),
        (
            "an event id carrying punctuation",
            "event!".to_string(),
            None,
            element_id_clause(1, "event!", &pattern_requirement(ID_PATTERN)),
        ),
        (
            "an event id carrying a multi-byte character",
            "événement".to_string(),
            None,
            element_id_clause(1, "événement", &pattern_requirement(ID_PATTERN)),
        ),
        (
            "an event id one character past its declared maximum",
            over_long_id.clone(),
            None,
            element_id_clause(1, &over_long_id, &at_most_requirement(ID_MAX_LENGTH)),
        ),
        (
            "an external id outside its expression",
            "evt-1".to_string(),
            Some("external$id"),
            clause(
                "external$id",
                "externalId",
                &pattern_requirement(EXTERNAL_ID_PATTERN),
            ),
        ),
    ];

    for (case, id, external_id, expected) in &rows {
        let reply = harness
            .put(&arn, *external_id, &[ev(id.as_str(), GOOD_EVENT)])
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with body {}",
            reply.status,
            reply.body
        );
        let stated = message_of(&reply);
        assert_eq!(
            stated,
            one_violation(expected),
            "case {case:?}: expected {:?}, got {stated:?}",
            one_violation(expected)
        );
        // The REQUIREMENT half is what loses the anchors; a value the request carried
        // keeps whatever characters it had, a dollar sign included.
        if let Some((_, quoted)) = stated.split_once("regular expression pattern: ") {
            assert!(
                !quoted.starts_with('^') && !quoted.ends_with('$'),
                "case {case:?}: a quoted expression carries no whole-value anchor, got \
                 {quoted:?}"
            );
        }
    }

    // An event id inside both bounds is served, so the clauses above are reported only
    // where the request really broke the member.
    let reply = harness
        .put(&arn, None, &[ev("i".repeat(ID_MAX_LENGTH), GOOD_EVENT)])
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"an event id at its declared maximum\": expected HTTP 200, got {} with body \
         {}",
        reply.status, reply.body
    );
}

/// A channel reference that is not one is answered under this service's own
/// `InvalidChannelARN`, whatever else the same request broke: the channel the request
/// names is what the call is resolved against before the payload is read, so that refusal
/// is payload-independent. Spans requests whose payload is good, whose payload breaks a
/// `@pattern`, and whose payload breaks the batch bound as well.
#[tokio::test]
async fn a_channel_reference_is_answered_before_the_payloads_own_violations() {
    let harness = Harness::new();

    // (case, the payload)
    let payloads: Vec<(&str, Vec<Ev>)> = vec![
        ("a payload that breaks nothing", vec![ev("evt-1", GOOD_EVENT)]),
        (
            "a payload whose event id is outside its expression",
            vec![ev("bad id!", GOOD_EVENT)],
        ),
        (
            "a payload whose event id is past its declared maximum",
            vec![ev("i".repeat(ID_MAX_LENGTH + 1), GOOD_EVENT)],
        ),
        ("a payload past the batch maximum", batch(MAX_BATCH + 1)),
        ("an empty payload", batch(0)),
    ];

    for (case, events) in &payloads {
        let reply = harness.put("not-an-arn", None, events).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "InvalidChannelARN"),
            "case {case:?}: a value that is not a channel reference is answered first, got \
             {} with body {}",
            reply.status,
            reply.body
        );
        let stated = message_of(&reply);
        assert_eq!(
            stated, "The specified channel ARN is not a valid channel ARN.",
            "case {case:?}: expected the sentence the model documents, got {stated:?}"
        );
    }
}
