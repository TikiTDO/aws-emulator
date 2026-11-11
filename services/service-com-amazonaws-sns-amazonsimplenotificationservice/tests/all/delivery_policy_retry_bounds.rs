// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the BOUNDS of the retry policy inside a delivery policy
//! document.
//!
//! The RULE asserted here: a delivery policy is schema-validated when it is written, so
//! every member of its retry policy is held to the bound SNS publishes for that member —
//! a value outside it is refused `InvalidParameter` (HTTP 400) with a sentence naming the
//! attribute, the member by the path the document writes it at, and the bound it crossed,
//! and the value the record already held is still the one it reports. A value AT either
//! published bound is accepted and reads back.
//!
//! The rows differ along what the rule generalises over: which retry member carries the
//! value, which direction of its bound the value crosses (each published bound, and one
//! past it), which DOCUMENT carries the retry policy — a subscription's own
//! `healthyRetryPolicy` and a topic's `http.defaultHealthyRetryPolicy` — and which
//! operation carried the write (`Subscribe`, `SetSubscriptionAttributes`,
//! `SetTopicAttributes`).
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{attribute_map, Sns};

/// The attribute every document here is written under.
const ATTRIBUTE: &str = "DeliveryPolicy";

/// The member a SUBSCRIPTION's delivery policy carries its retry policy under.
const SUBSCRIPTION_PATH: &str = "healthyRetryPolicy";

/// The member a TOPIC's delivery policy carries its HTTP/S retry defaults under.
const TOPIC_PATH: &str = "http.defaultHealthyRetryPolicy";

/// One retry-policy member and its published bounds: the member as the document names
/// it, the smallest value it takes, and the largest — `None` for a member SNS publishes
/// a floor for alone.
///
/// Source: the retry-policy attributes of the SNS Developer Guide's delivery-policy
/// table ("Amazon SNS message delivery retries"): `minDelayTarget` and `maxDelayTarget`
/// are delays in seconds from 1 to 3600, `numRetries` is a count from 0 to 100, and the
/// per-phase retry counts are counts from 0 up. The rows below are therefore driven by
/// the published catalog rather than by the one member a recorded call used.
const BOUNDS: &[(&str, i64, Option<i64>)] = &[
    ("minDelayTarget", 1, Some(3600)),
    ("maxDelayTarget", 1, Some(3600)),
    ("numRetries", 0, Some(100)),
    ("numNoDelayRetries", 0, None),
    ("numMinDelayRetries", 0, None),
    ("numMaxDelayRetries", 0, None),
];

/// A subscription's delivery policy naming `member` as `value`.
fn subscription_document(member: &str, value: i64) -> String {
    format!(r#"{{"{SUBSCRIPTION_PATH}":{{"{member}":{value}}}}}"#)
}

/// A topic's delivery policy naming `member` as `value`, under the protocol group a
/// topic states its defaults in.
fn topic_document(member: &str, value: i64) -> String {
    format!(r#"{{"http":{{"defaultHealthyRetryPolicy":{{"{member}":{value}}}}}}}"#)
}

/// The sentence a value outside a bound is refused with.
fn refusal(path: &str, member: &str, relation: &str, limit: i64) -> String {
    format!("Invalid parameter: {ATTRIBUTE}: {path}.{member} must be {relation} {limit}")
}

/// Subscribe an SQS endpoint to `topic`, answering the subscription's ARN.
async fn subscribe(sns: &Sns, case: &str, topic: &str, endpoint: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: Subscribe expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body))
}

/// Write one subscription attribute, answering the status, the code and the message.
async fn set_subscription(
    sns: &Sns,
    arn: &str,
    name: &str,
    value: &str,
) -> (u16, String, String) {
    let answer = sns
        .call(
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", arn),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    (
        answer.status,
        answer.code(),
        answer.element("Message").unwrap_or_default(),
    )
}

/// Write one topic attribute, answering the status, the code and the message.
async fn set_topic(sns: &Sns, topic: &str, name: &str, value: &str) -> (u16, String, String) {
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
    (
        answer.status,
        answer.code(),
        answer.element("Message").unwrap_or_default(),
    )
}

/// The attributes a subscription reports.
async fn subscription_attributes(
    sns: &Sns,
    arn: &str,
) -> std::collections::HashMap<String, String> {
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "GetSubscriptionAttributes {arn}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// The attributes a topic reports.
async fn topic_attributes(sns: &Sns, topic: &str) -> std::collections::HashMap<String, String> {
    let answer = sns.call("GetTopicAttributes", &[("TopicArn", topic)]).await;
    assert_eq!(
        answer.status, 200,
        "GetTopicAttributes {topic}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// The `numRetries` a document reports, or `None` when it names none.
fn num_retries(document: &str) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(document)
        .ok()?
        .pointer(&format!("/{SUBSCRIPTION_PATH}/numRetries"))
        .and_then(serde_json::Value::as_i64)
}

/// Every published bound of a SUBSCRIPTION's retry policy is accepted at the bound and
/// refused one past it, by both operations that write the attribute, and a refusal leaves
/// the stored policy as it was.
#[tokio::test]
async fn a_subscriptions_retry_policy_is_held_to_its_published_bounds() {
    let sns = Sns::new();
    // A well-formed policy the subscription already holds, so every refusal below is
    // checked against a stored document it must not replace.
    let accepted_retries = 3;
    let held = subscription_document("numRetries", accepted_retries);

    for (index, (member, minimum, maximum)) in BOUNDS.iter().enumerate() {
        let topic = sns.topic(&format!("retry-bound-{index:02}")).await;
        let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:retry-bound-{index:02}");
        let case = format!("{member} on a subscription");
        let arn = subscribe(&sns, &case, &topic, &endpoint).await;
        let (status, code, message) = set_subscription(&sns, &arn, ATTRIBUTE, &held).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control policy expected 200, got {status} / {code} {message}"
        );

        // At each published bound the value is accepted and reads back.
        let mut at_bounds = vec![*minimum];
        at_bounds.extend(*maximum);
        for value in at_bounds {
            let case = format!("{member} at {value}");
            let document = subscription_document(member, value);
            let (status, code, message) =
                set_subscription(&sns, &arn, ATTRIBUTE, &document).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a value at the bound expected 200, got {status} / {code} \
                 {message}"
            );
            let stored = subscription_attributes(&sns, &arn)
                .await
                .get(ATTRIBUTE)
                .cloned()
                .unwrap_or_default();
            let reported = serde_json::from_str::<serde_json::Value>(&stored)
                .unwrap_or(serde_json::Value::Null)
                .pointer(&format!("/{SUBSCRIPTION_PATH}/{member}"))
                .and_then(serde_json::Value::as_i64);
            assert_eq!(
                reported,
                Some(value),
                "case {case:?}: the accepted value must read back, got {stored}"
            );
        }

        // Put the control policy back, so each refusal below is measured against it.
        let (status, code, message) = set_subscription(&sns, &arn, ATTRIBUTE, &held).await;
        assert_eq!(
            status, 200,
            "case {case:?}: restoring the control policy expected 200, got {status} / {code} \
             {message}"
        );

        // And one past each published bound it is refused, with the bound in the
        // sentence and nothing written.
        let mut past: Vec<(i64, String)> = vec![(
            minimum - 1,
            refusal(SUBSCRIPTION_PATH, member, "greater than or equal to", *minimum),
        )];
        if let Some(maximum) = maximum {
            past.push((
                maximum + 1,
                refusal(SUBSCRIPTION_PATH, member, "less than or equal to", *maximum),
            ));
        }
        for (value, expected) in past {
            let case = format!("{member} at {value}");
            let document = subscription_document(member, value);
            let (status, code, message) =
                set_subscription(&sns, &arn, ATTRIBUTE, &document).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a value past the bound expected 400, got {status} / {code} \
                 {message}"
            );
            assert_eq!(
                code, "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {code} {message}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            let stored = subscription_attributes(&sns, &arn)
                .await
                .get(ATTRIBUTE)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                num_retries(&stored),
                Some(accepted_retries),
                "case {case:?}: the refused write must leave the stored policy as it was, got \
                 {stored}"
            );

            // `Subscribe` holds the same attribute to the same bound, on a subscription
            // of its own so the write is the one being judged.
            let other = sns.topic(&format!("retry-bound-sub-{index:02}")).await;
            let answer = sns
                .call(
                    "Subscribe",
                    &[
                        ("TopicArn", &other),
                        ("Protocol", "sqs"),
                        ("Endpoint", &endpoint),
                        ("ReturnSubscriptionArn", "true"),
                        ("Attributes.entry.1.key", ATTRIBUTE),
                        ("Attributes.entry.1.value", &document),
                    ],
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: Subscribe must refuse the same document, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.element("Message").unwrap_or_default(),
                expected,
                "case {case:?}: Subscribe must state the same sentence, got {}",
                answer.body
            );
            let listed = sns
                .call("ListSubscriptionsByTopic", &[("TopicArn", &other)])
                .await;
            assert!(
                listed.elements("SubscriptionArn").is_empty(),
                "case {case:?}: the refused Subscribe must create no subscription, got {}",
                listed.body
            );
        }
    }
}

/// A TOPIC's delivery policy carries the same retry policy one level down, so the same
/// bounds hold for it — and the refusal names the member by the path a topic's document
/// writes it at.
#[tokio::test]
async fn a_topics_retry_policy_is_held_to_the_same_bounds() {
    let sns = Sns::new();
    let held = topic_document("numRetries", 3);

    for (index, (member, minimum, maximum)) in BOUNDS.iter().enumerate() {
        let topic = sns.topic(&format!("topic-retry-bound-{index:02}")).await;
        let case = format!("{member} on a topic");
        let (status, code, message) = set_topic(&sns, &topic, ATTRIBUTE, &held).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control policy expected 200, got {status} / {code} {message}"
        );

        let mut at_bounds = vec![*minimum];
        at_bounds.extend(*maximum);
        for value in at_bounds {
            let case = format!("{member} at {value} on a topic");
            let document = topic_document(member, value);
            let (status, code, message) = set_topic(&sns, &topic, ATTRIBUTE, &document).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a value at the bound expected 200, got {status} / {code} \
                 {message}"
            );
            let reported = topic_attributes(&sns, &topic)
                .await
                .get("EffectiveDeliveryPolicy")
                .and_then(|policy| serde_json::from_str::<serde_json::Value>(policy).ok())
                .and_then(|policy| {
                    policy
                        .pointer(&format!("/http/defaultHealthyRetryPolicy/{member}"))
                        .and_then(serde_json::Value::as_i64)
                });
            assert_eq!(
                reported,
                Some(value),
                "case {case:?}: the accepted value must reach the effective policy, got \
                 {reported:?}"
            );
        }

        let (status, code, message) = set_topic(&sns, &topic, ATTRIBUTE, &held).await;
        assert_eq!(
            status, 200,
            "case {case:?}: restoring the control policy expected 200, got {status} / {code} \
             {message}"
        );

        let mut past: Vec<(i64, String)> = vec![(
            minimum - 1,
            refusal(TOPIC_PATH, member, "greater than or equal to", *minimum),
        )];
        if let Some(maximum) = maximum {
            past.push((
                maximum + 1,
                refusal(TOPIC_PATH, member, "less than or equal to", *maximum),
            ));
        }
        for (value, expected) in past {
            let case = format!("{member} at {value} on a topic");
            let document = topic_document(member, value);
            let (status, code, message) = set_topic(&sns, &topic, ATTRIBUTE, &document).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a value past the bound expected 400, got {status} / {code} \
                 {message}"
            );
            assert_eq!(
                code, "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {code} {message}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            let stored = topic_attributes(&sns, &topic)
                .await
                .get(ATTRIBUTE)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                stored, held,
                "case {case:?}: the refused write must leave the stored policy as it was, got \
                 {stored}"
            );

            // `CreateTopic` carries the same document to the same bounds, so the policy
            // cannot be smuggled past them by the call that mints the topic. The create
            // is refused, and the topic it names is not there to read.
            let fresh = format!("created-retry-bound-{index:02}");
            let created = sns
                .call(
                    "CreateTopic",
                    &[
                        ("Name", &fresh),
                        ("Attributes.entry.1.key", ATTRIBUTE),
                        ("Attributes.entry.1.value", &document),
                    ],
                )
                .await;
            assert_eq!(
                created.status, 400,
                "case {case:?}: CreateTopic must refuse the same document, got {} / {}",
                created.status, created.body
            );
            assert_eq!(
                created.element("Message").unwrap_or_default(),
                expected,
                "case {case:?}: CreateTopic must state the same sentence, got {}",
                created.body
            );
            let listed = sns.call("ListTopics", &[]).await;
            assert!(
                !listed
                    .elements("TopicArn")
                    .iter()
                    .any(|arn| arn.ends_with(&format!(":{fresh}"))),
                "case {case:?}: the refused CreateTopic must create no topic, got {}",
                listed.body
            );
        }
    }
}

/// A member with no bound of its own, and a document that names none of the bounded
/// members, are both accepted: the rule bounds the members SNS publishes bounds for and
/// invents none.
#[tokio::test]
async fn a_member_with_no_published_bound_is_accepted() {
    let sns = Sns::new();
    let topic = sns.topic("retry-unbounded").await;
    let endpoint = "arn:aws:sqs:us-east-1:000000000000:retry-unbounded";
    let arn = subscribe(&sns, "an unbounded member", &topic, endpoint).await;

    // (case, the document written)
    let accepted: &[(&str, String)] = &[
        (
            "a per-phase retry count inside a retry budget it names beside it",
            format!(
                r#"{{"{SUBSCRIPTION_PATH}":{{"numRetries":20,"numNoDelayRetries":5}}}}"#
            ),
        ),
        (
            "a backoff function, which carries no numeric bound",
            format!(r#"{{"{SUBSCRIPTION_PATH}":{{"backoffFunction":"exponential"}}}}"#),
        ),
        (
            "a retry policy naming none of the bounded members",
            format!(r#"{{"{SUBSCRIPTION_PATH}":{{}},"guaranteed":true}}"#),
        ),
        (
            "a document whose only member is one this emulator gives no meaning to",
            format!(r#"{{"{SUBSCRIPTION_PATH}":{{"numRetries":4,"unknownMember":"kept"}}}}"#),
        ),
    ];

    for (case, document) in accepted {
        let (status, code, message) = set_subscription(&sns, &arn, ATTRIBUTE, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {code} {message}"
        );
    }
}
