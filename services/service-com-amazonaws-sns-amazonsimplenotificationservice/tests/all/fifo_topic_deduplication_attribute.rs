// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the deduplication setting a FIFO topic is governed by.
//!
//! The RULE asserted here: a FIFO topic reports `ContentBasedDeduplication` whether or not
//! its owner has set it, and reports the documented default until one is set — "By default,
//! ContentBasedDeduplication is set to false. If you create a FIFO topic and this attribute
//! is false, you must specify a value for the MessageDeduplicationId parameter for the
//! Publish action" (`GetTopicAttributesResponse$Attributes`, which lists the attribute among
//! the ones that "apply only to FIFO topics"). A standard topic is not governed by it, so it
//! reports none.
//!
//! The rows differ along what the rule generalises over: how the topic was made FIFO (the
//! attribute or the name suffix), whether the setting was given at create, set afterwards or
//! cleared, and which value it was set to — and the last rows assert the setting's MEANING,
//! that a publish carrying no `MessageDeduplicationId` is refused exactly while the setting
//! reads `false`.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{attribute_map, Sns};

/// The attribute under test, as the service names it.
const CONTENT_BASED_DEDUPLICATION: &str = "ContentBasedDeduplication";

/// A topic created with `attributes`, answering its ARN.
async fn given_topic(sns: &Sns, case: &str, name: &str, attributes: &[(&str, &str)]) -> String {
    let mut params: Vec<(String, String)> = vec![("Name".to_string(), name.to_string())];
    for (index, (key, value)) in attributes.iter().enumerate() {
        params.push((
            format!("Attributes.entry.{}.key", index + 1),
            (*key).to_string(),
        ));
        params.push((
            format!("Attributes.entry.{}.value", index + 1),
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
        "case {case:?}: CreateTopic expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {}", answer.body))
}

/// The attribute `name` of the topic at `arn`, as `GetTopicAttributes` reports it.
async fn reported(sns: &Sns, case: &str, arn: &str, name: &str) -> Option<String> {
    let answer = sns.call("GetTopicAttributes", &[("TopicArn", arn)]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetTopicAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer).get(name).cloned()
}

/// One row of the projection table: the case, the topic's name, the attributes it is created
/// with, and what the setting must read for it (`None` when it must report none).
type ProjectionCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

/// A FIFO topic reports the deduplication setting that governs it, whether or not its
/// owner has set one.
#[tokio::test]
async fn a_fifo_topic_reports_its_content_based_deduplication_setting() {
    let sns = Sns::new();

    // (case, the topic's name, the attributes it is created with, what the setting must read)
    let rows: &[ProjectionCase<'_>] = &[
        (
            "a FIFO topic asked for by attribute",
            "dedup-by-attribute.fifo",
            &[("FifoTopic", "true")],
            Some("false"),
        ),
        (
            "a FIFO topic asked for with the setting left unset",
            "dedup-unset.fifo",
            &[("FifoTopic", "true")],
            Some("false"),
        ),
        (
            "a FIFO topic created with the setting enabled",
            "dedup-created-true.fifo",
            &[("FifoTopic", "true"), (CONTENT_BASED_DEDUPLICATION, "true")],
            Some("true"),
        ),
        (
            "a FIFO topic created with the setting written as the default",
            "dedup-created-false.fifo",
            &[("FifoTopic", "true"), (CONTENT_BASED_DEDUPLICATION, "false")],
            Some("false"),
        ),
        (
            // "The following attributes apply only to FIFO topics" — a standard topic is not
            // governed by the setting, so it reports none.
            "a standard topic",
            "dedup-standard",
            &[],
            None,
        ),
    ];

    for (case, name, attributes, expected) in rows {
        let arn = given_topic(&sns, case, name, attributes).await;
        let actual = reported(&sns, case, &arn, CONTENT_BASED_DEDUPLICATION).await;
        assert_eq!(
            actual.as_deref(),
            *expected,
            "case {case:?}: expected ContentBasedDeduplication {expected:?}, got {actual:?}"
        );
        if expected.is_some() {
            let fifo = reported(&sns, case, &arn, "FifoTopic").await;
            assert_eq!(
                fifo.as_deref(),
                Some("true"),
                "case {case:?}: a topic reporting the FIFO setting must report that it is a \
                 FIFO topic, got {fifo:?}"
            );
        }
    }
}

/// `SetTopicAttributes` moves the read-back of the setting, and clearing it leaves the
/// topic reporting the documented default.
#[tokio::test]
async fn setting_content_based_deduplication_reads_back() {
    let sns = Sns::new();
    let arn = given_topic(
        &sns,
        "the round-trip topic",
        "dedup-roundtrip.fifo",
        &[("FifoTopic", "true")],
    )
    .await;

    // (case, the value written — `None` clears the attribute, what the read must answer)
    let rows: &[(&str, Option<&str>, &str)] = &[
        ("enabled", Some("true"), "true"),
        ("disabled again", Some("false"), "false"),
        ("enabled a second time", Some("true"), "true"),
        // A write with no value clears the attribute, so the topic reports what SNS reports
        // for a FIFO topic whose owner never set one.
        ("cleared", None, "false"),
    ];

    for (case, written, expected) in rows {
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", arn.as_str()),
            ("AttributeName", CONTENT_BASED_DEDUPLICATION),
        ];
        if let Some(value) = written {
            params.push(("AttributeValue", value));
        }
        let answer = sns.call("SetTopicAttributes", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: SetTopicAttributes expected 200, got {} / {}",
            answer.status, answer.body
        );
        let actual = reported(&sns, case, &arn, CONTENT_BASED_DEDUPLICATION).await;
        assert_eq!(
            actual.as_deref(),
            Some(*expected),
            "case {case:?}: expected ContentBasedDeduplication {expected:?}, got {actual:?}"
        );
    }
}

/// What the setting the topic reports MEANS: a publish supplies its own
/// `MessageDeduplicationId` exactly while the setting reads `false`.
#[tokio::test]
async fn the_reported_setting_decides_whether_a_publish_must_carry_its_own_id() {
    let sns = Sns::new();
    let arn = given_topic(
        &sns,
        "the publishing topic",
        "dedup-publishing.fifo",
        &[("FifoTopic", "true")],
    )
    .await;

    // (case, the value the setting is set to before the publish — `None` leaves it at the
    //  default, whether a publish carrying no MessageDeduplicationId is accepted)
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("the documented default", None, false),
        ("content-based deduplication enabled", Some("true"), true),
        ("content-based deduplication disabled", Some("false"), false),
    ];

    for (case, setting, accepted) in rows {
        if let Some(value) = setting {
            let written = sns
                .call(
                    "SetTopicAttributes",
                    &[
                        ("TopicArn", arn.as_str()),
                        ("AttributeName", CONTENT_BASED_DEDUPLICATION),
                        ("AttributeValue", value),
                    ],
                )
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: SetTopicAttributes expected 200, got {} / {}",
                written.status, written.body
            );
        }
        let reported_setting = reported(&sns, case, &arn, CONTENT_BASED_DEDUPLICATION).await;
        assert_eq!(
            reported_setting.as_deref(),
            Some(if *accepted { "true" } else { "false" }),
            "case {case:?}: the setting the publish is judged under must be the one the topic \
             reports, got {reported_setting:?}"
        );
        let answer = sns
            .call(
                "Publish",
                &[
                    ("TopicArn", arn.as_str()),
                    ("Message", "a message with no deduplication id"),
                    ("MessageGroupId", "group-1"),
                ],
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a publish to a content-based topic needs no id of its own, got \
                 {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a publish with no id to a topic that deduplicates by id must be \
                 refused, got {} / {}",
                answer.status, answer.body
            );
        }
    }
}
