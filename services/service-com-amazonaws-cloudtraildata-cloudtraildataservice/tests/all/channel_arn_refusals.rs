// SPDX-License-Identifier: CC0-1.0

//! How `PutAuditEvents` refuses a call it cannot serve, driven over HTTP through the
//! same `MultiServiceRouter` the emulator serves with.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over — the form of the channel reference, the account it names, and
//! whether the channel it names is registered here:
//!
//! 1. A refusal states the message AWS words for its class: the sentence the model
//!    documents, with nothing interpolated where that wording leaves no slot for a
//!    caller value, and the channel the request named where it does.
//! 2. A channel reference outside the modelled `@pattern` of `ChannelArn` is refused by
//!    the SERVICE, under the error its model declares for that fault, rather than by the
//!    generic constraint walk under a code this service's model does not contain.
//! 3. A channel ARN in another account is refused by authorization, with HTTP 403 and
//!    the common `AccessDeniedException` code, before the channel is looked up — so the
//!    answer is the same whether or not such a channel is registered here.
//!
//! The client is this crate's one test harness, reached from the suite that owns it.

use crate::put_audit_events::{channel_arn, ev, Harness, Reply, REGION};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// The sentence `InvalidChannelARN` carries, as the model documents the shape
/// (`smithy_query(query="shape", shape="InvalidChannelARN")`).
const INVALID_CHANNEL_ARN: &str = "The specified channel ARN is not a valid channel ARN.";

/// The sentence `ChannelNotFound` carries: the channel the request asked about, then the
/// fact the model documents for the shape ("The channel could not be found.").
fn channel_not_found(reference: &str) -> String {
    format!("The channel {reference} could not be found.")
}

/// The sentence `DuplicatedAuditEventId` carries, from the same place.
const DUPLICATED_AUDIT_EVENT_ID: &str =
    "Two or more entries in the request have the same event ID.";

/// The error code the authorization layer names, and the status it answers with.
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

/// A JSON audit event that passes per-event validation, so nothing but the channel
/// reference decides these calls.
const GOOD_EVENT: &str = r#"{"eventSource":"custom.app","eventName":"Login"}"#;

/// The prose member of a refusal: the modelled error shapes of this service spell it
/// `message`, and the authorization layer's `AccessDeniedException` — which is not one of
/// this service's shapes — spells it `Message`.
pub fn message_of(reply: &Reply) -> String {
    for member in ["message", "Message"] {
        if let Some(text) = reply.body.get(member).and_then(serde_json::Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

/// The denial AWS words for a call no policy allows: who the request spoke for, the
/// action it asked for, and the resource it named. The principal is the one identity this
/// emulator publishes (what `sts:GetCallerIdentity` answers with), and the resource is
/// the ARN from the request — nothing here is a literal from a recording.
fn denial(resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: cloudtrail-data:PutAuditEvents on \
         resource: {resource} because no resource-based policy allows the \
         cloudtrail-data:PutAuditEvents action",
        engine::emulator_arn("iam", "", "user/emulator")
    )
}

/// One refusal to assert.
struct Case {
    name: &'static str,
    channel: String,
    events: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
    /// What AWS words for this refusal, with whatever slots that wording has filled from
    /// THIS request.
    message: String,
    /// Whether that wording has no slot at all: the sentence the model documents for the
    /// shape, which must be stated verbatim with nothing from the request in it.
    fixed_sentence: bool,
}

/// A refusal states what AWS words for its class: the sentence the model documents, with
/// nothing from the request interpolated where that wording has no slot for one, and the
/// channel the request named where it does. The emulator never appends the offending
/// value to a sentence that has no room for it.
#[tokio::test]
async fn a_documented_refusal_states_its_sentence_and_echoes_no_request_value() {
    let harness = Harness::new();
    let registered = channel_arn("9a0b1c2d-3e4f-5061-7283-94a5b6c7d8e9");
    harness.register(Channel::from_arn(&registered, "Custom").expect("a channel ARN"));

    let unheld = channel_arn("0b1c2d3e-4f50-6172-8394-a5b6c7d8e9fa");
    let other_region = engine::emulator_arn(
        "cloudtrail",
        "eu-west-3",
        "channel/9a0b1c2d-3e4f-5061-7283-94a5b6c7d8e9",
    );

    let cases: Vec<Case> = vec![
        Case {
            name: "an arn: value that is not a channel ARN at all",
            channel: "arn:aws:cloudtrail:us-east-1:000000000000:not-a-channel-resource"
                .to_string(),
            events: vec![("evt-1", GOOD_EVENT)],
            status: 400,
            code: "InvalidChannelARN",
            message: INVALID_CHANNEL_ARN.to_string(),
            fixed_sentence: true,
        },
        Case {
            name: "another service's ARN",
            channel: "arn:aws:s3:::a-bucket-name".to_string(),
            events: vec![("evt-1", GOOD_EVENT)],
            status: 400,
            code: "InvalidChannelARN",
            message: INVALID_CHANNEL_ARN.to_string(),
            fixed_sentence: true,
        },
        Case {
            name: "a channel ARN whose channel id is empty",
            channel: engine::emulator_arn("cloudtrail", REGION, "channel/"),
            events: vec![("evt-1", GOOD_EVENT)],
            status: 400,
            code: "InvalidChannelARN",
            message: INVALID_CHANNEL_ARN.to_string(),
            fixed_sentence: true,
        },
        Case {
            name: "a value outside the member's modelled pattern",
            channel: "channel-reference-that-is-no-arn".to_string(),
            events: vec![("evt-1", GOOD_EVENT)],
            status: 400,
            code: "InvalidChannelARN",
            message: INVALID_CHANNEL_ARN.to_string(),
            fixed_sentence: true,
        },
        Case {
            name: "a well-formed channel ARN this emulator does not hold",
            channel: unheld.clone(),
            events: vec![("evt-1", GOOD_EVENT)],
            status: 400,
            code: "ChannelNotFound",
            message: channel_not_found(&unheld),
            fixed_sentence: false,
        },
        Case {
            name: "a channel ARN in a region this emulator holds no channel in",
            channel: other_region.clone(),
            events: vec![("evt-1", GOOD_EVENT)],
            status: 400,
            code: "ChannelNotFound",
            message: channel_not_found(&other_region),
            fixed_sentence: false,
        },
        Case {
            name: "two entries of a batch sharing one event id",
            channel: registered.clone(),
            events: vec![
                ("repeated-event-id", GOOD_EVENT),
                ("repeated-event-id", GOOD_EVENT),
            ],
            status: 400,
            code: "DuplicatedAuditEventId",
            message: DUPLICATED_AUDIT_EVENT_ID.to_string(),
            fixed_sentence: true,
        },
        Case {
            name: "a longer batch repeating its first event id last",
            channel: registered.clone(),
            events: vec![
                ("first-event", GOOD_EVENT),
                ("second-event", GOOD_EVENT),
                ("first-event", GOOD_EVENT),
            ],
            status: 400,
            code: "DuplicatedAuditEventId",
            message: DUPLICATED_AUDIT_EVENT_ID.to_string(),
            fixed_sentence: true,
        },
    ];

    for case in &cases {
        let events: Vec<_> = case
            .events
            .iter()
            .map(|(id, data)| ev(*id, *data))
            .collect();
        let reply = harness.put(&case.channel, None, &events).await;
        let name = case.name;
        assert_eq!(
            reply.status, case.status,
            "case {name:?}: expected HTTP {}, got {} with body {}",
            case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            case.code,
            "case {name:?}: expected error code {:?}, got body {}",
            case.code,
            reply.body
        );
        let stated = message_of(&reply);
        assert_eq!(
            stated, case.message,
            "case {name:?}: expected the message AWS words for this refusal ({:?}), got \
             {stated:?}",
            case.message
        );
        if case.fixed_sentence {
            // The point of a held-constant sentence: no value the caller sent reaches
            // it, neither the channel reference nor an event id.
            assert!(
                !stated.contains(&case.channel),
                "case {name:?}: the message must not echo the channel reference, got \
                 {stated:?}"
            );
            for (id, _) in &case.events {
                assert!(
                    !stated.contains(id),
                    "case {name:?}: the message must not echo the event id {id:?}, got \
                     {stated:?}"
                );
            }
        }
    }

    assert_eq!(
        harness.ingest_log(&registered).len(),
        0,
        "every call above was refused, so the registered channel must hold nothing: got \
         {:?}",
        harness.ingest_log(&registered)
    );
}

/// A channel reference the member's modelled `@pattern ^arn:.*$` rejects is refused by
/// the service under its own `InvalidChannelARN` — never under an error code this
/// service's model does not declare. Spans the forms such a value takes: a word, a
/// mis-cased ARN prefix, a bare channel id, a URL, an empty value and an oversized one.
#[tokio::test]
async fn a_channel_reference_outside_the_modelled_pattern_is_the_services_own_refusal() {
    let harness = Harness::new();
    let long = "c".repeat(400);

    // (case name, the channelArn value)
    let rows: Vec<(&str, String)> = vec![
        ("a bare word", "probe-value".to_string()),
        (
            "an upper-case ARN prefix",
            "ARN:aws:cloudtrail:us-east-1:000000000000:channel/abc".to_string(),
        ),
        ("the prefix without its colon", "arn".to_string()),
        (
            "a bare channel id (the ARN suffix the doc also names)",
            "1a2b3c4d-5e6f-7081-92a3-b4c5d6e7f809".to_string(),
        ),
        (
            "a URL",
            "https://cloudtrail.us-east-1.amazonaws.com/channel".to_string(),
        ),
        ("an empty value", String::new()),
        ("a value far longer than any ARN", long),
    ];

    for (case, value) in &rows {
        let reply = harness.put(value, None, &[ev("evt-1", GOOD_EVENT)]).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidChannelARN",
            "case {case:?}: a channel reference this service cannot read is its own \
             modelled refusal, got body {}",
            reply.body
        );
        let stated = message_of(&reply);
        assert_eq!(
            stated, INVALID_CHANNEL_ARN,
            "case {case:?}: expected the documented sentence {INVALID_CHANNEL_ARN:?}, \
             got {stated:?}"
        );
    }
}

/// A channel ARN in another account is refused by AUTHORIZATION — HTTP 403,
/// `AccessDeniedException`, and a `Message` naming the principal, the action and the
/// resource — taken before the channel is resolved, so the answer cannot report whether
/// that channel exists. Spans several foreign accounts and regions, a channel registered
/// here under a foreign ARN and one that is not, with the caller's own account as the
/// control.
#[tokio::test]
async fn a_channel_arn_in_another_account_is_denied_without_disclosing_existence() {
    let harness = Harness::new();

    // (case name, account, region, channel id, registered in this emulator?)
    let rows: Vec<(&str, &str, &str, &str, bool)> = vec![
        (
            "another account, nothing registered",
            "111122223333",
            REGION,
            "11111111-2222-3333-4444-555555555555",
            false,
        ),
        (
            "another account, a channel registered under that ARN",
            "111122223333",
            REGION,
            "22222222-3333-4444-5555-666666666666",
            true,
        ),
        (
            "another account in another region",
            "444455556666",
            "ap-southeast-2",
            "33333333-4444-5555-6666-777777777777",
            false,
        ),
        (
            "an account whose id differs from the caller's by one digit",
            "000000000001",
            REGION,
            "44444444-5555-6666-7777-888888888888",
            false,
        ),
    ];

    let mut redacted_bodies: Vec<(&str, String)> = Vec::new();
    for (case, account, region, channel_id, registered) in &rows {
        let arn = format!("arn:aws:cloudtrail:{region}:{account}:channel/{channel_id}");
        if *registered {
            harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));
        }
        let reply = harness.put(&arn, None, &[ev("evt-1", GOOD_EVENT)]).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            ACCESS_DENIED,
            "case {case:?}: expected {ACCESS_DENIED:?}, got {} with body {}",
            reply.status,
            reply.body
        );
        let stated = message_of(&reply);
        assert_eq!(
            stated,
            denial(&arn),
            "case {case:?}: expected the denial naming the principal, the action and the \
             resource, got {stated:?}"
        );
        assert!(
            harness.ingest_log(&arn).is_empty(),
            "case {case:?}: a denied call must persist nothing, got {:?}",
            harness.ingest_log(&arn)
        );
        // The denial is the same answer whichever channel the ARN names, so collect it
        // with the one request-derived value removed and compare the set below.
        redacted_bodies.push((case, reply.body.to_string().replace(&arn, "<resource>")));
    }

    let (first_case, first) = &redacted_bodies[0];
    for (case, body) in &redacted_bodies[1..] {
        assert_eq!(
            body, first,
            "case {case:?}: the denial must not differ from case {first_case:?} by \
             anything but the resource it names, got {body}"
        );
    }

    // The control: the SAME call in the caller's own account is served by the service,
    // which does report that it holds no such channel.
    let own = channel_arn("55555555-6666-7777-8888-999999999999");
    let reply = harness.put(&own, None, &[ev("evt-1", GOOD_EVENT)]).await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "ChannelNotFound".to_string()),
        "case \"the caller's own account\": the gate is keyed on the account the ARN \
         names, got {} with body {}",
        reply.status,
        reply.body
    );
}
