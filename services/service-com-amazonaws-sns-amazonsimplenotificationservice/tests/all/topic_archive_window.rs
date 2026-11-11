// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the replayable window a topic with a message archive reports,
//! and for the replay a subscription of such a topic may be configured with.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.
//!
//! The rules under test:
//!
//! * a topic that holds an `ArchivePolicy` also reports `BeginningArchiveTime` — "the
//!   earliest starting point at which a message in the topic's archive can be replayed
//!   from. This point in time is based on the configured message retention period set by
//!   the topic's message archiving policy" (`GetTopicAttributesResponse$Attributes` doc) —
//!   and a topic that archives nothing reports no such point;
//! * a `ReplayPolicy` names a window of that archive, so it is refused on a subscription
//!   of a topic that holds no archive policy.
//!
//! The rows differ along what the rules generalise over: the retention period the policy
//! states (both published bounds), which call configured the archive, whether the archive
//! policy was added, kept or removed, and which call wrote the replay policy.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::REPLAY_WITHOUT_ARCHIVE_REFUSAL;

use super::topics_subscriptions_publish::{attribute_map, Sns};

/// The attribute the archive window is reported under.
const BEGINNING: &str = "BeginningArchiveTime";

/// A replay policy, as the `ReplayPolicy` attribute carries one: the window of the
/// archive to redeliver from.
const REPLAY_POLICY: &str = r#"{"PointType":"Timestamp","StartingPoint":"2026-01-01T00:00:00Z"}"#;

/// An archive policy stating `days` of retention. The member is
/// `MessageRetentionPeriod`, "the configured message retention period set by the topic's
/// message archiving policy" (`GetTopicAttributesResponse$Attributes` doc), stated in
/// days.
fn archive_policy(days: &str) -> String {
    format!(r#"{{"MessageRetentionPeriod":"{days}"}}"#)
}

/// `GetTopicAttributes` for one topic, as a map.
async fn attributes(
    sns: &Sns,
    topic: &str,
) -> std::collections::HashMap<String, String> {
    let answer = sns
        .call("GetTopicAttributes", &[("TopicArn", topic)])
        .await;
    assert_eq!(
        answer.status, 200,
        "GetTopicAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// Write one topic attribute, answering the call's status and message.
async fn set_topic_attribute(
    sns: &Sns,
    topic: &str,
    name: &str,
    value: &str,
) -> (u16, String) {
    let answer = sns
        .call(
            "SetTopicAttributes",
            &[
                ("TopicArn", topic),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    (answer.status, answer.body)
}

/// The instant `value` names, as seconds since the epoch, when it is the RFC 3339 instant
/// SNS reports a timestamp attribute as.
fn epoch_seconds(case: &str, value: &str) -> f64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        value,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {BEGINNING} must be an RFC 3339 instant, got {value:?} ({error})")
    });
    parsed.as_secs_f64()
}

/// Now, as seconds since the epoch.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A topic that archives its messages reports where its replayable window starts; one
/// that archives nothing reports no such point, and a read reports the same document
/// twice.
#[tokio::test]
async fn an_archiving_topic_reports_where_its_replayable_window_starts() {
    let sns = Sns::new();
    // The retention period in days, on each published bound of the member's range (1 to
    // 365 days, SNS Developer Guide "Message archiving and replay") and inside it.
    let cases: &[(&str, &str)] = &[
        ("one day, the shortest retention", "1"),
        ("thirty days", "30"),
        ("three hundred and sixty five days, the longest retention", "365"),
    ];

    for (index, (case, days)) in cases.iter().enumerate() {
        let name = format!("archive-{index:02}.fifo");
        let before = now_seconds();
        let answer = sns
            .call(
                "CreateTopic",
                &[
                    ("Name", &name),
                    ("Attributes.entry.1.key", "FifoTopic"),
                    ("Attributes.entry.1.value", "true"),
                    ("Attributes.entry.2.key", "ArchivePolicy"),
                    ("Attributes.entry.2.value", &archive_policy(days)),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: CreateTopic expected 200, got {} / {}",
            answer.status, answer.body
        );
        let topic = answer
            .element("TopicArn")
            .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {}", answer.body));

        let held = attributes(&sns, &topic).await;
        let reported = held
            .get(BEGINNING)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {BEGINNING} in {held:?}"));
        // Nothing older than the moment archiving came into force is in the archive, so
        // for a topic that has just been created that moment IS where the window starts,
        // whatever the retention period.
        let window = epoch_seconds(case, &reported);
        assert!(
            window >= before - 1.0 && window <= now_seconds() + 1.0,
            "case {case:?}: the window must start when archiving came into force, got \
             {reported:?} outside [{before}, {}]",
            now_seconds()
        );
        // A read reports the same document twice: it is reporting, not configuring.
        let again = attributes(&sns, &topic).await;
        assert_eq!(
            again.get(BEGINNING),
            held.get(BEGINNING),
            "case {case:?}: two reads must report one window, got {again:?} after {held:?}"
        );
    }

    // A topic with no archive policy reports no window; one given a policy afterwards
    // reports one; one whose policy is removed reports none again.
    let topic = sns.topic("archive-lifecycle.fifo").await;
    let held = attributes(&sns, &topic).await;
    assert_eq!(
        held.get(BEGINNING),
        None,
        "a topic that archives nothing has no replayable window, got {held:?}"
    );

    let (status, body) = set_topic_attribute(&sns, &topic, "ArchivePolicy", &archive_policy("7")).await;
    assert_eq!(
        status, 200,
        "SetTopicAttributes expected 200, got {status} / {body}"
    );
    let held = attributes(&sns, &topic).await;
    assert!(
        held.contains_key(BEGINNING),
        "a topic given an archive policy reports where its window starts, got {held:?}"
    );

    // An empty value removes the policy, and with it the window.
    let (status, body) = set_topic_attribute(&sns, &topic, "ArchivePolicy", "").await;
    assert_eq!(
        status, 200,
        "removing the archive policy expected 200, got {status} / {body}"
    );
    let held = attributes(&sns, &topic).await;
    assert_eq!(
        held.get("ArchivePolicy"),
        None,
        "a removed archive policy must not be reported, got {held:?}"
    );
    assert_eq!(
        held.get(BEGINNING),
        None,
        "a topic that archives nothing again has no window, got {held:?}"
    );
}

/// A `ReplayPolicy` names a window of the TOPIC's message archive, so it is accepted on a
/// subscription of an archiving topic and refused — with SNS's own sentence — on a
/// subscription of a topic that archives nothing, by whichever call wrote it.
#[tokio::test]
async fn a_replay_policy_needs_a_topic_that_archives_its_messages() {
    let sns = Sns::new();
    let archiving = {
        let answer = sns
            .call(
                "CreateTopic",
                &[
                    ("Name", "replay-archiving.fifo"),
                    ("Attributes.entry.1.key", "FifoTopic"),
                    ("Attributes.entry.1.value", "true"),
                    ("Attributes.entry.2.key", "ArchivePolicy"),
                    ("Attributes.entry.2.value", &archive_policy("1")),
                ],
            )
            .await;
        answer
            .element("TopicArn")
            .unwrap_or_else(|| panic!("no TopicArn in {}", answer.body))
    };
    let plain = sns.topic("replay-plain.fifo").await;

    // (case, the topic, whether the write is accepted)
    let cases: &[(&str, &str, bool)] = &[
        ("a topic with an archive policy", &archiving, true),
        ("a topic with none", &plain, false),
    ];

    for (index, (case, topic, accepted)) in cases.iter().enumerate() {
        let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:replay-queue-{index:02}");
        // Written by `Subscribe`…
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", &endpoint),
                    ("Attributes.entry.1.key", "ReplayPolicy"),
                    ("Attributes.entry.1.value", REPLAY_POLICY),
                ],
            )
            .await;
        let expected_status = if *accepted { 200 } else { 400 };
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: Subscribe expected {expected_status}, got {} / {}",
            answer.status, answer.body
        );
        if !*accepted {
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").as_deref(),
                Some(REPLAY_WITHOUT_ARCHIVE_REFUSAL),
                "case {case:?}: unexpected sentence in {}",
                answer.body
            );
        }

        // …and by `SetSubscriptionAttributes`, on a subscription of the same topic that
        // carried no policy: the same rule, the same sentence.
        let bare = format!("arn:aws:sqs:us-east-1:000000000000:replay-bare-{index:02}");
        let subscribed = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", &bare),
                ],
            )
            .await;
        let arn = subscribed
            .element("SubscriptionArn")
            .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", subscribed.body));
        let written = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "ReplayPolicy"),
                    ("AttributeValue", REPLAY_POLICY),
                ],
            )
            .await;
        assert_eq!(
            written.status, expected_status,
            "case {case:?}: SetSubscriptionAttributes expected {expected_status}, got {} / {}",
            written.status, written.body
        );
        if !*accepted {
            assert_eq!(
                written.element("Message").as_deref(),
                Some(REPLAY_WITHOUT_ARCHIVE_REFUSAL),
                "case {case:?}: unexpected sentence in {}",
                written.body
            );
        }

        // Clearing the attribute is no configuration of a replay, so it is accepted
        // whatever the topic archives.
        let cleared = sns
            .call(
                "SetSubscriptionAttributes",
                &[
                    ("SubscriptionArn", &arn),
                    ("AttributeName", "ReplayPolicy"),
                    ("AttributeValue", ""),
                ],
            )
            .await;
        assert_eq!(
            cleared.status, 200,
            "case {case:?}: removing a replay policy expected 200, got {} / {}",
            cleared.status, cleared.body
        );
    }
}
