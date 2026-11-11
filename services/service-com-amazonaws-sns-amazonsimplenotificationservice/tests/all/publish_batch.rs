// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `PublishBatch`, driven over the wire: every call below is an
//! awsQuery HTTP request dispatched through the router the emulator serves in
//! production, so nothing but persisted state carries information between calls.
//!
//! The rule under test: `PublishBatch` publishes each entry of the batch to the topic
//! its `TopicArn` names, in request order, reporting EVERY entry's own result
//! individually — a `Successful` member carrying that entry's `Id`, a freshly minted
//! `MessageId` and (for a FIFO topic only) an increasing `SequenceNumber` — and refusing
//! the request AS A WHOLE both on the batch-level conditions the model names (no entries,
//! `EmptyBatchRequestException`; more than ten,
//! `TooManyEntriesInBatchRequestException`; an `Id` outside its documented form,
//! `InvalidBatchEntryIdException`; two entries sharing an `Id`,
//! `BatchEntryIdsNotDistinctException`; a combined size over the topic's ceiling,
//! `BatchRequestTooLongException`; a topic that is not stored, `NotFoundException`) and
//! when ANY ONE entry breaks a per-message rule a single `Publish` of it would be refused
//! for (`InvalidParameterException`) — in which case no entry of the batch is published.
//!
//! The rows differ in entry count, body size and form, which members an entry carries,
//! topic type and which entry of one batch is unacceptable — what the rule generalises
//! over. The recorded `PublishBatch` call is one row of the first table.

use std::collections::{HashMap, HashSet};

use super::topics_subscriptions_publish::{xml_unescape, Answer, Sns};

/// One entry of a batch request, as a caller supplies it.
#[derive(Debug, Clone, Default)]
struct Entry {
    id: String,
    message: String,
    subject: Option<String>,
    structure: Option<String>,
    group_id: Option<String>,
    deduplication_id: Option<String>,
}

impl Entry {
    /// The plainest entry: an id and a body.
    fn new(id: &str, message: &str) -> Self {
        Self {
            id: id.to_string(),
            message: message.to_string(),
            ..Self::default()
        }
    }

    fn subject(mut self, subject: &str) -> Self {
        self.subject = Some(subject.to_string());
        self
    }

    fn structure(mut self, structure: &str) -> Self {
        self.structure = Some(structure.to_string());
        self
    }

    fn group(mut self, group_id: &str) -> Self {
        self.group_id = Some(group_id.to_string());
        self
    }

    fn deduplication(mut self, deduplication_id: &str) -> Self {
        self.deduplication_id = Some(deduplication_id.to_string());
        self
    }
}

/// The awsQuery form of a batch: one numbered `member` per entry
/// (`PublishBatchRequestEntries.member.<n>.<Member>`).
fn batch_params(topic: &str, entries: &[Entry]) -> Vec<(String, String)> {
    let mut params = vec![("TopicArn".to_string(), topic.to_string())];
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("PublishBatchRequestEntries.member.{}", index + 1);
        params.push((format!("{prefix}.Id"), entry.id.clone()));
        params.push((format!("{prefix}.Message"), entry.message.clone()));
        for (member, value) in [
            ("Subject", entry.subject.as_ref()),
            ("MessageStructure", entry.structure.as_ref()),
            ("MessageGroupId", entry.group_id.as_ref()),
            ("MessageDeduplicationId", entry.deduplication_id.as_ref()),
        ] {
            if let Some(value) = value {
                params.push((format!("{prefix}.{member}"), value.clone()));
            }
        }
    }
    params
}

/// Call `PublishBatch` with `entries` against `topic`.
async fn publish_batch(sns: &Sns, topic: &str, entries: &[Entry]) -> Answer {
    let owned = batch_params(topic, entries);
    let borrowed: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call("PublishBatch", &borrowed).await
}

/// The members of one result list of the response (`Successful` or `Failed`), in
/// document order, each as its leaf elements.
///
/// [`Answer::elements`] cannot be used here: both lists carry an `Id`, so the two have
/// to be read list by list.
fn result_entries(answer: &Answer, list: &str) -> Vec<HashMap<String, String>> {
    let open = format!("<{list}>");
    let close = format!("</{list}>");
    let Some(start) = answer.body.find(&open) else {
        return Vec::new();
    };
    let rest = &answer.body[start + open.len()..];
    let Some(end) = rest.find(&close) else {
        return Vec::new();
    };
    rest[..end]
        .split("<member")
        .skip(1)
        .map(|member| {
            // Drop the rest of the opening tag: the first member of a list carries the
            // envelope's `xmlns` attribute.
            let member = member
                .split_once('>')
                .map_or("", |(_attributes, after)| after);
            let member = member.split("</member>").next().unwrap_or_default();
            let mut fields = HashMap::new();
            let mut rest = member;
            while let Some(opened) = rest.find('<') {
                rest = &rest[opened + 1..];
                let Some(name_end) = rest.find('>') else { break };
                let name = rest[..name_end].to_string();
                rest = &rest[name_end + 1..];
                let closing = format!("</{name}>");
                let Some(value_end) = rest.find(&closing) else {
                    break;
                };
                fields.insert(name, xml_unescape(&rest[..value_end]));
                rest = &rest[value_end + closing.len()..];
            }
            fields
        })
        .collect()
}

/// The `Id` of every member of one result list, in document order.
fn ids(answer: &Answer, list: &str) -> Vec<String> {
    result_entries(answer, list)
        .into_iter()
        .map(|fields| fields.get("Id").cloned().unwrap_or_default())
        .collect()
}

/// Every entry of a batch is published to each confirmed subscription of the topic and
/// reported, in request order, with an id of its own — whatever the batch's shape.
#[tokio::test]
async fn publish_batch_publishes_every_entry_to_each_confirmed_subscription() {
    let sns = Sns::new();
    let topic = sns.topic("batch-fanout-topic").await;
    let mut confirmed = Vec::new();
    for endpoint in [
        "arn:aws:sqs:us-east-1:000000000000:batch-q",
        "arn:aws:lambda:us-east-1:000000000000:function:batch-fn",
    ] {
        let protocol = if endpoint.contains(":sqs:") {
            "sqs"
        } else {
            "lambda"
        };
        confirmed.push(
            sns.call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint),
                ],
            )
            .await
            .element("SubscriptionArn")
            .expect("a subscription ARN"),
        );
    }
    // An endpoint that has to confirm first: it receives nothing.
    let pending = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "email"),
                ("Endpoint", "ops@example.test"),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("a subscription ARN");

    let ten: Vec<Entry> = (1..=10)
        .map(|n| Entry::new(&format!("e{n}"), &format!("body {n}")))
        .collect();
    let large = "x".repeat(100_000);
    let cases: Vec<(&str, Vec<Entry>)> = vec![
        ("one entry", vec![Entry::new("only", "hello")]),
        (
            "two entries, one of them with a subject",
            vec![
                Entry::new("a", "first"),
                Entry::new("b", "second").subject("hi there"),
            ],
        ),
        ("the ten entry maximum", ten),
        (
            "a json structured entry beside a plain one",
            vec![
                Entry::new("plain", "plain body"),
                Entry::new("json", r#"{"default":"d","http":"h"}"#).structure("json"),
            ],
        ),
        (
            "a group id, which a standard topic forwards rather than refuses",
            vec![Entry::new("grouped", "grouped body").group("g1")],
        ),
        (
            "a large body",
            vec![Entry::new("big", &large), Entry::new("small", "s")],
        ),
    ];

    let mut published = 0usize;
    let mut message_ids: HashSet<String> = HashSet::new();
    for (case, entries) in &cases {
        let answer = publish_batch(&sns, &topic, entries).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let successful = result_entries(&answer, "Successful");
        assert_eq!(
            successful.len(),
            entries.len(),
            "case {case:?}: every entry must be reported successful, got {}",
            answer.body
        );
        assert!(
            ids(&answer, "Failed").is_empty(),
            "case {case:?}: no entry must fail, got {}",
            answer.body
        );
        assert_eq!(
            ids(&answer, "Successful"),
            entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            "case {case:?}: the results must come back in request order, got {}",
            answer.body
        );
        for (entry, fields) in entries.iter().zip(&successful) {
            let message_id = fields.get("MessageId").cloned().unwrap_or_default();
            assert!(
                !message_id.is_empty(),
                "case {case:?}: entry {:?} must carry a MessageId, got {fields:?}",
                entry.id
            );
            assert!(
                message_ids.insert(message_id.clone()),
                "case {case:?}: entry {:?} must carry a fresh MessageId, got {message_id:?}",
                entry.id
            );
            // "This parameter applies only to FIFO (first-in-first-out) topics"
            // (`PublishBatchResultEntry$SequenceNumber` doc).
            assert_eq!(
                fields.get("SequenceNumber"),
                None,
                "case {case:?}: entry {:?} on a standard topic must carry no \
                 SequenceNumber, got {fields:?}",
                entry.id
            );
        }
        published += entries.len();
    }

    let stored = sns.state();
    let stored = stored.lock();
    for subscription in &confirmed {
        let delivered = stored
            .delivered
            .get(subscription)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            delivered.len(),
            published,
            "case {subscription:?}: expected every batched message to be delivered, got {}",
            delivered.len()
        );
        assert_eq!(
            delivered
                .iter()
                .map(|message| message.message_id.clone())
                .collect::<HashSet<_>>(),
            message_ids,
            "case {subscription:?}: the delivered message ids must be the reported ones"
        );
        assert!(
            delivered
                .iter()
                .any(|message| message.subject.as_deref() == Some("hi there")),
            "case {subscription:?}: a delivered message must keep its subject"
        );
        assert!(
            delivered
                .iter()
                .any(|message| message.message.len() == large.len()),
            "case {subscription:?}: a delivered message must keep its whole body"
        );
        assert!(
            delivered
                .iter()
                .any(|message| message.message_group_id.as_deref() == Some("g1")),
            "case {subscription:?}: a delivered message must keep its group id"
        );
    }
    assert!(
        !stored.delivered.contains_key(&pending),
        "an unconfirmed subscription must receive nothing, got {:?}",
        stored.delivered.get(&pending)
    );
}

/// The batch-level refusals: each one is about the batch as a whole, so it answers an
/// error rather than a per-entry result, and publishes nothing at all.
#[tokio::test]
async fn publish_batch_refuses_the_batch_level_failures_the_model_names() {
    let sns = Sns::new();
    let topic = sns.topic("batch-refusal-topic").await;
    let subscription = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:refusal-q"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("a subscription ARN");

    let eleven: Vec<Entry> = (1..=11)
        .map(|n| Entry::new(&format!("e{n}"), "body"))
        .collect();
    // The topic's ceiling is the documented 256 KiB default, so two bodies of 200 KB
    // are each publishable alone and too long together.
    let half_a_batch = "y".repeat(200_000);
    let id_at_bound = "i".repeat(80);
    let id_past_bound = "i".repeat(81);
    // (case, topic, entries, status, awsQuery code)
    let cases: Vec<(&str, String, Vec<Entry>, u16, &str)> = vec![
        // The awsQuery form has no spelling for a list that is PRESENT and empty: a
        // batch with no entries carries no `PublishBatchRequestEntries.member.*`
        // parameter at all, so the boundary validator refuses the absent required
        // member before the handler's `EmptyBatchRequestException` can apply. Either
        // way the batch is refused with 400 and publishes nothing.
        ("no entries at all", topic.clone(), Vec::new(), 400, "MalformedInput"),
        (
            "one past the ten entry maximum",
            topic.clone(),
            eleven,
            400,
            "TooManyEntriesInBatchRequest",
        ),
        (
            "two entries sharing an id",
            topic.clone(),
            vec![Entry::new("same", "a"), Entry::new("same", "b")],
            400,
            "BatchEntryIdsNotDistinct",
        ),
        (
            "an id one past the 80 character bound",
            topic.clone(),
            vec![Entry::new(&id_past_bound, "a")],
            400,
            "InvalidBatchEntryId",
        ),
        (
            "an id carrying punctuation the form excludes",
            topic.clone(),
            vec![Entry::new("not!allowed", "a")],
            400,
            "InvalidBatchEntryId",
        ),
        (
            "an empty id",
            topic.clone(),
            vec![Entry::new("", "a")],
            400,
            "InvalidBatchEntryId",
        ),
        (
            "a combined size over the topic's ceiling",
            topic.clone(),
            vec![
                Entry::new("first-half", &half_a_batch),
                Entry::new("second-half", &half_a_batch),
            ],
            400,
            "BatchRequestTooLong",
        ),
        (
            "a topic that is not stored",
            "arn:aws:sns:us-east-1:000000000000:absent-topic".to_string(),
            vec![Entry::new("a", "body")],
            404,
            "NotFound",
        ),
        (
            "a TopicArn that is not an ARN",
            "batch-refusal-topic".to_string(),
            vec![Entry::new("a", "body")],
            400,
            "InvalidParameter",
        ),
    ];

    for (case, topic_arn, entries, status, code) in &cases {
        let answer = publish_batch(&sns, topic_arn, entries).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            &answer.code(),
            code,
            "case {case:?}: expected the {code} code, got {}",
            answer.body
        );
    }

    // The two id forms the rule accepts, on either published bound of its length.
    for (case, id) in [("an id of one character", "i"), ("an id at the 80 character bound", id_at_bound.as_str())] {
        let answer = publish_batch(&sns, &topic, &[Entry::new(id, "body")]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            ids(&answer, "Successful"),
            vec![id.to_string()],
            "case {case:?}: the entry must be reported under its own id, got {}",
            answer.body
        );
    }

    let stored = sns.state();
    let stored = stored.lock();
    let delivered = stored
        .delivered
        .get(&subscription)
        .map(Vec::as_slice)
        .unwrap_or_default();
    assert_eq!(
        delivered.len(),
        2,
        "only the two accepted batches must have been delivered, got {}",
        delivered.len()
    );
}

/// An entry SNS would refuse a single `Publish` for refuses the WHOLE request: the
/// per-message rules are judged for every entry before any of them is published, the call
/// answers 400 `InvalidParameter`, and not one sibling of the refused entry is published.
#[tokio::test]
async fn publish_batch_refuses_the_whole_request_for_an_entry_publish_would_refuse() {
    let sns = Sns::new();
    let topic = sns.topic("batch-mixed-topic").await;
    let subscription = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:mixed-q"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("a subscription ARN");

    let long_subject = "s".repeat(101);
    let long_ordering_id = "d".repeat(129);
    // (entry, whether the batch publishes it)
    let cases: Vec<(&str, Entry, bool)> = vec![
        ("a plain entry", Entry::new("ok-first", "first body"), true),
        ("an empty body", Entry::new("empty", ""), false),
        (
            "a json structure whose body is not JSON",
            Entry::new("structure", "not json").structure("json"),
        false),
        (
            "a json body with no default member",
            Entry::new("no-default", r#"{"http":"h"}"#).structure("json"),
            false,
        ),
        (
            "a subject one character past the bound",
            Entry::new("subject", "body").subject(&long_subject),
            false,
        ),
        (
            "a deduplication id on a standard topic",
            Entry::new("dedup", "body").deduplication("d1"),
            false,
        ),
        (
            "an ordering id one past the 128 character bound",
            Entry::new("ordering", "body").group(&long_ordering_id),
            false,
        ),
        ("another plain entry", Entry::new("ok-last", "last body"), true),
    ];

    let entries: Vec<Entry> = cases
        .iter()
        .filter(|(_, _, publishes)| *publishes)
        .map(|(_, entry, _)| entry.clone())
        .collect();
    // One batch per refused entry, each paired with the two publishable ones, so the
    // refused entry is never the whole batch: what the rows show is that a sibling AWS
    // would never have accepted the batch for is not published either.
    for (case, entry, publishes) in &cases {
        if *publishes {
            continue;
        }
        let mut batch = entries.clone();
        batch.insert(1, entry.clone());
        let answer = publish_batch(&sns, &topic, &batch).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: one unpublishable entry must refuse the whole request, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the single-publish error code, got {}",
            answer.body
        );
        assert!(
            ids(&answer, "Successful").is_empty() && ids(&answer, "Failed").is_empty(),
            "case {case:?}: a refused request reports no per-entry result, got {}",
            answer.body
        );
    }

    // The batch of the publishable entries ALONE is published, so each row above was
    // refused for the entry it added and not for the siblings it shares with this one.
    let answer = publish_batch(&sns, &topic, &entries).await;
    assert_eq!(
        answer.status, 200,
        "the publishable entries alone must be accepted, got {} / {}",
        answer.status, answer.body
    );

    let stored = sns.state();
    let stored = stored.lock();
    let delivered = stored
        .delivered
        .get(&subscription)
        .map(Vec::as_slice)
        .unwrap_or_default();
    assert_eq!(
        delivered.len(),
        entries.len(),
        "only the one accepted batch must have been delivered, got {}",
        delivered.len()
    );
    assert!(
        delivered
            .iter()
            .all(|message| ["first body", "last body"].contains(&&*message.message)),
        "a refused request must deliver nothing"
    );
}

/// A FIFO topic needs each entry's ordering parameters, and sequences the entries it
/// accepts in request order — "multiple messages within a single batch are published in
/// the order they are sent" (the `PublishBatch` doc).
#[tokio::test]
async fn publish_batch_to_a_fifo_topic_sequences_its_entries_in_request_order() {
    let sns = Sns::new();
    let plain_fifo = sns.topic("batch-ordered.fifo").await;
    let content_fifo = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "batch-hashed.fifo"),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
                ("Attributes.entry.2.key", "ContentBasedDeduplication"),
                ("Attributes.entry.2.value", "true"),
            ],
        )
        .await
        .element("TopicArn")
        .expect("the FIFO topic ARN");
    // Both topics keep a confirmed subscription, so what each accepted entry stored is
    // readable afterwards.
    for (topic, queue) in [
        (&plain_fifo, "arn:aws:sqs:us-east-1:000000000000:ordered-q"),
        (&content_fifo, "arn:aws:sqs:us-east-1:000000000000:hashed-q"),
    ] {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", queue),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "Subscribe {queue}: expected 200, got {} / {}",
            answer.status, answer.body
        );
    }

    // (case, topic, entries, the ids the batch publishes — none when the whole request
    // is refused for an entry of it)
    type FifoCase<'a> = (&'a str, &'a str, Vec<Entry>, Option<Vec<&'a str>>);
    let cases: Vec<FifoCase<'_>> = vec![
        (
            "explicit ordering ids",
            &plain_fifo,
            vec![
                Entry::new("one", "m1").group("g1").deduplication("d1"),
                Entry::new("two", "m2").group("g1").deduplication("d2"),
                Entry::new("three", "m3").group("g2").deduplication("d3"),
            ],
            Some(vec!["one", "two", "three"]),
        ),
        (
            "an entry with no group id refuses the whole request",
            &plain_fifo,
            vec![
                Entry::new("grouped", "m1").group("g1").deduplication("d4"),
                Entry::new("ungrouped", "m2").deduplication("d5"),
            ],
            None,
        ),
        (
            "an entry with no deduplication id refuses the request on a topic that \
             derives none",
            &plain_fifo,
            vec![
                Entry::new("explicit", "m1").group("g1").deduplication("d6"),
                Entry::new("derived", "m2").group("g1"),
            ],
            None,
        ),
        (
            "a content based topic derives the deduplication ids",
            &content_fifo,
            vec![
                Entry::new("hashed-one", "m1").group("g1"),
                Entry::new("hashed-two", "m2").group("g1"),
            ],
            Some(vec!["hashed-one", "hashed-two"]),
        ),
        (
            "a content based topic still needs each entry's group id",
            &content_fifo,
            vec![
                Entry::new("hashed-three", "m3").group("g1"),
                Entry::new("ungrouped", "m4"),
            ],
            None,
        ),
    ];

    let mut sequence_numbers: Vec<String> = Vec::new();
    for (case, topic, entries, expected_ok) in &cases {
        let answer = publish_batch(&sns, topic, entries).await;
        let Some(expected_ok) = expected_ok else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected the whole request to be refused, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                ids(&answer, "Successful").is_empty(),
                "case {case:?}: a refused request publishes no entry, got {}",
                answer.body
            );
            continue;
        };
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            ids(&answer, "Successful"),
            expected_ok
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: unexpected successful entries, got {}",
            answer.body
        );
        assert!(
            ids(&answer, "Failed").is_empty(),
            "case {case:?}: no entry of an accepted request fails, got {}",
            answer.body
        );
        for fields in result_entries(&answer, "Successful") {
            let sequence = fields.get("SequenceNumber").cloned().unwrap_or_default();
            assert!(
                !sequence.is_empty(),
                "case {case:?}: a FIFO entry must carry a SequenceNumber, got {fields:?}"
            );
            sequence_numbers.push(sequence);
        }
    }

    let mut sorted = sequence_numbers.clone();
    sorted.sort();
    assert_eq!(
        sorted, sequence_numbers,
        "sequence numbers must increase with the order the entries were sent in, got \
         {sequence_numbers:?}"
    );
    let unique: HashSet<&String> = sequence_numbers.iter().collect();
    assert_eq!(
        unique.len(),
        sequence_numbers.len(),
        "sequence numbers must be unique, got {sequence_numbers:?}"
    );

    // The deduplication ids a content-based FIFO topic derived are the SHA-256 of each
    // body, not of anything the request named.
    let stored = sns.state();
    let stored = stored.lock();
    let derived: HashSet<Option<String>> = stored
        .delivered
        .values()
        .flat_map(|held| held.iter())
        .filter(|message| message.topic_arn == content_fifo)
        .map(|message| message.message_deduplication_id.clone())
        .collect();
    assert!(
        !derived.is_empty()
            && derived
                .iter()
                .all(|id| id.as_ref().is_some_and(|id| id.len() == 64)),
        "a derived deduplication id must be a SHA-256 digest, got {derived:?}"
    );
}
