// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the DEDUPLICATION of a publish to a FIFO topic, driven over the
//! wire: every call below is an awsQuery HTTP request dispatched through the router the
//! emulator serves in production, so nothing but persisted state carries information
//! between calls.
//!
//! The rule under test: a message published to a FIFO topic carries a deduplication token
//! — the `MessageDeduplicationId` the caller supplied, or the SHA-256 of the body when the
//! topic enables `ContentBasedDeduplication` — and "if a message with a particular
//! MessageDeduplicationId is sent successfully, subsequent messages within the
//! deduplication scope and interval, with the same MessageDeduplicationId, are accepted
//! successfully but aren't delivered" (`PublishInput$MessageDeduplicationId` doc): the
//! duplicate is answered with the `MessageId` and `SequenceNumber` of the copy that WAS
//! delivered, and nothing is published for it. The scope is the whole topic, or one
//! message group when the topic's `FifoThroughputScope` says so.
//!
//! The rows differ along what the rule generalises over: how the token arises (supplied or
//! derived), the deduplication scope the topic sets, whether two copies share a message
//! group, which operation published each copy (`Publish`, `PublishBatch`) and whether the
//! copies are in one request or in two.

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use super::topics_subscriptions_publish::Sns;

/// One publish of one message: its body, its message group, and the deduplication token
/// the caller supplied (`None` leaves the topic to derive one).
type Message<'a> = (&'a str, &'a str, Option<&'a str>);

/// One row of the deduplication table: the case, the attributes its FIFO topic is created
/// with, the two publishes, and whether the second is collapsed onto the first.
type DedupCase<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    Message<'a>,
    Message<'a>,
    bool,
);

/// The deduplication token a content-based FIFO topic derives for `body`: "Amazon SNS
/// uses a SHA-256 hash to generate the MessageDeduplicationId using the body of the
/// message (but not the attributes of the message)"
/// (`GetTopicAttributesResponse$Attributes` doc).
fn derived_token(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}

/// Create a FIFO topic named `name` with `attributes` beside its FIFO flag, answering its
/// ARN.
async fn fifo_topic(sns: &Sns, name: &str, attributes: &[(&str, &str)]) -> String {
    let fifo_name = format!("{name}.fifo");
    let mut params: Vec<(String, String)> = vec![
        ("Name".to_string(), fifo_name),
        ("Attributes.entry.1.key".to_string(), "FifoTopic".to_string()),
        ("Attributes.entry.1.value".to_string(), "true".to_string()),
    ];
    for (index, (key, value)) in attributes.iter().enumerate() {
        params.push((
            format!("Attributes.entry.{}.key", index + 2),
            (*key).to_string(),
        ));
        params.push((
            format!("Attributes.entry.{}.value", index + 2),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = sns.call("CreateTopic", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "CreateTopic {name}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("TopicArn")
        .unwrap_or_else(|| panic!("CreateTopic {name}: no TopicArn in {}", answer.body))
}

/// Subscribe a queue endpoint to `topic`, answering the subscription's ARN: the delivery
/// log of this subscription is where a message that was DELIVERED is observed.
async fn subscribe(sns: &Sns, topic: &str, queue: &str) -> String {
    let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:{queue}");
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", &endpoint),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "Subscribe {queue}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("Subscribe {queue}: no SubscriptionArn in {}", answer.body))
}

/// The `(MessageId, SequenceNumber)` one accepted publish answered with.
async fn publish(sns: &Sns, case: &str, topic: &str, message: Message<'_>) -> (String, String) {
    let (body, group, token) = message;
    let mut params: Vec<(&str, &str)> = vec![
        ("TopicArn", topic),
        ("Message", body),
        ("MessageGroupId", group),
    ];
    if let Some(token) = token {
        params.push(("MessageDeduplicationId", token));
    }
    let answer = sns.call("Publish", &params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: Publish expected 200, got {} / {}",
        answer.status, answer.body
    );
    (
        answer
            .element("MessageId")
            .unwrap_or_else(|| panic!("case {case:?}: no MessageId in {}", answer.body)),
        answer
            .element("SequenceNumber")
            .unwrap_or_else(|| panic!("case {case:?}: no SequenceNumber in {}", answer.body)),
    )
}

/// How many messages the delivery log holds for one subscription.
fn delivered(sns: &Sns, subscription: &str) -> usize {
    let stored = sns.state();
    let stored = stored.lock();
    stored
        .delivered
        .get(subscription)
        .map(Vec::len)
        .unwrap_or_default()
}

/// A second copy of a message already accepted inside the deduplication scope is accepted
/// too, answers with the FIRST copy's `MessageId` and `SequenceNumber`, and is not
/// delivered; a message that is NOT a duplicate inside that scope is published on its own.
#[tokio::test]
async fn a_duplicate_fifo_publish_answers_the_first_copy_and_delivers_nothing() {
    let sns = Sns::new();
    let hashed_body = derived_token("a body published earlier");
    // The attribute names and values come from the `CreateTopicInput$Attributes` catalog:
    // `ContentBasedDeduplication` ("Enables content-based deduplication for FIFO topics")
    // and `FifoThroughputScope` ("Topic … the default value" / "MessageGroup – The scope
    // of deduplication is within each individual message group").
    let cases: &[DedupCase<'_>] = &[
        (
            "the same supplied token in one message group",
            &[],
            ("first", "g1", Some("d1")),
            ("second", "g1", Some("d1")),
            true,
        ),
        (
            "the same supplied token in another message group, under the default scope",
            &[],
            ("first", "g1", Some("d1")),
            ("second", "g2", Some("d1")),
            true,
        ),
        (
            "the same supplied token in another message group, scoped to the group",
            &[("FifoThroughputScope", "MessageGroup")],
            ("first", "g1", Some("d1")),
            ("second", "g2", Some("d1")),
            false,
        ),
        (
            "the same supplied token in one message group, scoped to the group",
            &[("FifoThroughputScope", "MessageGroup")],
            ("first", "g1", Some("d1")),
            ("second", "g1", Some("d1")),
            true,
        ),
        (
            "the same supplied token under the topic scope stated explicitly",
            &[("FifoThroughputScope", "Topic")],
            ("first", "g1", Some("d1")),
            ("second", "g2", Some("d1")),
            true,
        ),
        (
            "two different supplied tokens",
            &[],
            ("one body", "g1", Some("d1")),
            ("one body", "g1", Some("d2")),
            false,
        ),
        (
            "the same body on a content based topic",
            &[("ContentBasedDeduplication", "true")],
            ("the same body", "g1", None),
            ("the same body", "g1", None),
            true,
        ),
        (
            "the same body in another group on a content based topic",
            &[("ContentBasedDeduplication", "true")],
            ("the same body", "g1", None),
            ("the same body", "g2", None),
            true,
        ),
        (
            "two different bodies on a content based topic",
            &[("ContentBasedDeduplication", "true")],
            ("one body", "g1", None),
            ("another body", "g1", None),
            false,
        ),
        // "If you send one message with ContentBasedDeduplication enabled, and then
        // another message with a MessageDeduplicationId that is the same as the one
        // generated for the first MessageDeduplicationId, the two messages are treated as
        // duplicates" (`PublishInput$MessageDeduplicationId` doc).
        (
            "a supplied token equal to the token derived for an earlier body",
            &[("ContentBasedDeduplication", "true")],
            ("a body published earlier", "g1", None),
            ("a different body entirely", "g1", Some(&hashed_body)),
            true,
        ),
        (
            "a supplied token overriding the derived one",
            &[("ContentBasedDeduplication", "true")],
            ("the same body", "g1", None),
            ("the same body", "g1", Some("explicit")),
            false,
        ),
    ];

    for (index, (case, attributes, first, second, collapsed)) in cases.iter().enumerate() {
        let topic = fifo_topic(&sns, &format!("dedup-{index:02}"), attributes).await;
        let subscription = subscribe(&sns, &topic, &format!("dedup-queue-{index:02}")).await;

        let (first_id, first_sequence) = publish(&sns, case, &topic, *first).await;
        let (second_id, second_sequence) = publish(&sns, case, &topic, *second).await;

        if *collapsed {
            assert_eq!(
                second_id, first_id,
                "case {case:?}: a duplicate must answer with the first copy's MessageId, \
                 got {second_id:?} after {first_id:?}"
            );
            assert_eq!(
                second_sequence, first_sequence,
                "case {case:?}: a duplicate must answer with the first copy's \
                 SequenceNumber, got {second_sequence:?} after {first_sequence:?}"
            );
            assert_eq!(
                delivered(&sns, &subscription),
                1,
                "case {case:?}: a duplicate must not be delivered"
            );
        } else {
            assert_ne!(
                second_id, first_id,
                "case {case:?}: a message that is no duplicate must carry a fresh \
                 MessageId, got {second_id:?} twice"
            );
            assert_ne!(
                second_sequence, first_sequence,
                "case {case:?}: a message that is no duplicate must be sequenced on its \
                 own, got {second_sequence:?} twice"
            );
            assert_eq!(
                delivered(&sns, &subscription),
                2,
                "case {case:?}: both messages must be delivered"
            );
        }
    }
}

/// The deduplication scope is ONE topic: the same token on another topic names another
/// message. A STANDARD topic deduplicates nothing, so two identical publishes to one are
/// two messages.
#[tokio::test]
async fn deduplication_is_scoped_to_the_topic_and_applies_to_fifo_topics_alone() {
    let sns = Sns::new();
    let here = fifo_topic(&sns, "dedup-here", &[]).await;
    let there = fifo_topic(&sns, "dedup-there", &[]).await;
    let message: Message<'_> = ("one body", "g1", Some("shared-token"));

    let (first, _) = publish(&sns, "the first topic", &here, message).await;
    let (other, _) = publish(&sns, "another topic", &there, message).await;
    assert_ne!(
        other, first,
        "the same token on another topic names another message, got {other:?} twice"
    );
    let (again, _) = publish(&sns, "the first topic again", &here, message).await;
    assert_eq!(
        again, first,
        "the token is still held by the topic that accepted it, got {again:?} after \
         {first:?}"
    );

    // A standard topic takes no deduplication token at all, and sequences nothing: two
    // identical publishes are two messages.
    let standard = sns.topic("dedup-standard").await;
    let subscription = subscribe(&sns, &standard, "dedup-standard-queue").await;
    let mut ids: HashSet<String> = HashSet::new();
    for attempt in 0..2 {
        let answer = sns
            .call(
                "Publish",
                &[("TopicArn", &standard), ("Message", "one body")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "the standard publish {attempt} expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("SequenceNumber"),
            None,
            "a standard topic sequences nothing, got {}",
            answer.body
        );
        ids.insert(answer.element("MessageId").unwrap_or_default());
    }
    assert_eq!(
        ids.len(),
        2,
        "a standard topic must mint a MessageId per publish, got {ids:?}"
    );
    assert_eq!(
        delivered(&sns, &subscription),
        2,
        "a standard topic must deliver every publish"
    );
}

/// `PublishBatch` deduplicates against the same records: "messages are deduplicated
/// within the batch and across batches for five minutes" (the `PublishBatch` doc), and
/// against what a single `Publish` accepted — so a duplicate entry is reported with the
/// message that WAS delivered, whichever operation delivered it.
#[tokio::test]
async fn a_batched_publish_deduplicates_within_the_batch_and_against_publish() {
    let sns = Sns::new();
    let topic = fifo_topic(&sns, "dedup-batch", &[]).await;
    let subscription = subscribe(&sns, &topic, "dedup-batch-queue").await;

    // Two entries of ONE batch carrying one token: both are reported, on one message.
    let answer = sns
        .call(
            "PublishBatch",
            &[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "e1"),
                ("PublishBatchRequestEntries.member.1.Message", "first"),
                ("PublishBatchRequestEntries.member.1.MessageGroupId", "g1"),
                (
                    "PublishBatchRequestEntries.member.1.MessageDeduplicationId",
                    "batched",
                ),
                ("PublishBatchRequestEntries.member.2.Id", "e2"),
                ("PublishBatchRequestEntries.member.2.Message", "second"),
                ("PublishBatchRequestEntries.member.2.MessageGroupId", "g1"),
                (
                    "PublishBatchRequestEntries.member.2.MessageDeduplicationId",
                    "batched",
                ),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "PublishBatch expected 200, got {} / {}",
        answer.status, answer.body
    );
    let ids = answer.elements("MessageId");
    assert_eq!(
        ids.len(),
        2,
        "every entry must be reported, got {}",
        answer.body
    );
    assert_eq!(
        ids[1], ids[0],
        "a duplicate entry must report the message the batch published, got {ids:?}"
    );
    let sequences = answer.elements("SequenceNumber");
    assert_eq!(
        sequences[1], sequences[0],
        "a duplicate entry must report that message's SequenceNumber, got {sequences:?}"
    );
    assert_eq!(
        delivered(&sns, &subscription),
        1,
        "only the message the batch published may be delivered"
    );

    // The same token in a LATER batch, and in a single `Publish`: one message still.
    let again = sns
        .call(
            "PublishBatch",
            &[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "e3"),
                ("PublishBatchRequestEntries.member.1.Message", "third"),
                ("PublishBatchRequestEntries.member.1.MessageGroupId", "g1"),
                (
                    "PublishBatchRequestEntries.member.1.MessageDeduplicationId",
                    "batched",
                ),
            ],
        )
        .await;
    assert_eq!(
        again.element("MessageId").as_deref(),
        Some(ids[0].as_str()),
        "a later batch carrying the same token must report the same message, got {}",
        again.body
    );
    let (published, _) = publish(&sns, "a single publish of a batched token", &topic, ("fourth", "g1", Some("batched"))).await;
    assert_eq!(
        published, ids[0],
        "a Publish carrying a token a batch accepted must report that message, got \
         {published:?} after {:?}",
        ids[0]
    );
    assert_eq!(
        delivered(&sns, &subscription),
        1,
        "no duplicate may be delivered, whichever operation carried it"
    );

    // A token the topic has not seen is a new message, in a batch as in a publish.
    let fresh = sns
        .call(
            "PublishBatch",
            &[
                ("TopicArn", &topic),
                ("PublishBatchRequestEntries.member.1.Id", "e4"),
                ("PublishBatchRequestEntries.member.1.Message", "fifth"),
                ("PublishBatchRequestEntries.member.1.MessageGroupId", "g1"),
                (
                    "PublishBatchRequestEntries.member.1.MessageDeduplicationId",
                    "unseen",
                ),
            ],
        )
        .await;
    assert_ne!(
        fresh.element("MessageId").as_deref(),
        Some(ids[0].as_str()),
        "an unseen token must be a message of its own, got {}",
        fresh.body
    );
    assert_eq!(
        delivered(&sns, &subscription),
        2,
        "the message of an unseen token must be delivered"
    );
}
