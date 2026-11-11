// SPDX-License-Identifier: CC0-1.0

//! What an EMPTY value written to a policy-valued attribute means, end to end over the
//! wire — so nothing but persisted state carries information between calls.
//!
//! The RULE asserted here: an attribute whose value is a policy document is removed by
//! writing an empty value to it ("To delete a filter policy, set the FilterPolicy
//! attribute to an empty value", SNS Developer Guide) — EXCEPT where the document carries
//! a member AWS requires, which an empty value carries no more than a document missing
//! that member does: such a write is refused `400 InvalidParameter` with
//! `Invalid parameter: <AttributeName>: missing required value`, naming the attribute the
//! request named, and the record keeps every attribute it held.
//!
//! The rows differ along what the rule generalises over: which policy attribute is
//! written, which record kind holds it (a topic, a subscription), which call carried the
//! write, and whether the value is empty or a document that carries none of the required
//! members.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::missing_required_value_refusal;

use super::topics_subscriptions_publish::{attribute_map, Sns};

/// A filter policy, as the `FilterPolicy` attribute carries one.
const FILTER_POLICY: &str = r#"{"kind":["alert"]}"#;

/// A dead-letter queue, as the `RedrivePolicy` attribute names one. Every subscription
/// in this suite is of a FIFO topic, and a queue takes a message only of its own kind, so
/// the dead-letter target is a FIFO queue.
const REDRIVE_POLICY: &str =
    r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq.fifo"}"#;

/// A second dead-letter queue, so a refused write can be shown to leave the FIRST policy
/// in place rather than to have stored anything of its own.
const OTHER_REDRIVE_POLICY: &str =
    r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq-two.fifo"}"#;

/// A delivery policy, in the shape a SUBSCRIPTION's `DeliveryPolicy` carries.
const DELIVERY_POLICY: &str = r#"{"healthyRetryPolicy":{"numRetries":7}}"#;

/// A replay policy, as the `ReplayPolicy` attribute carries one.
const REPLAY_POLICY: &str = r#"{"PointType":"Timestamp","StartingPoint":"2026-01-01T00:00:00Z"}"#;

/// An archive policy, as a FIFO topic's `ArchivePolicy` carries one.
const ARCHIVE_POLICY: &str = r#"{"MessageRetentionPeriod":"1"}"#;

/// Write one subscription attribute, answering the call's status, error code and message.
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

/// Write one topic attribute, answering the call's status, error code and message.
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

/// The attributes one subscription reports.
async fn subscription_attributes(
    sns: &Sns,
    arn: &str,
) -> std::collections::HashMap<String, String> {
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "GetSubscriptionAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// The attributes one topic reports.
async fn topic_attributes(sns: &Sns, topic: &str) -> std::collections::HashMap<String, String> {
    let answer = sns.call("GetTopicAttributes", &[("TopicArn", topic)]).await;
    assert_eq!(
        answer.status, 200,
        "GetTopicAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// A FIFO topic that archives its messages, so a `ReplayPolicy` has a window to name.
async fn archiving_topic(sns: &Sns, name: &str) -> String {
    let answer = sns
        .call(
            "CreateTopic",
            &[
                ("Name", &format!("{name}.fifo")),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
                ("Attributes.entry.2.key", "ArchivePolicy"),
                ("Attributes.entry.2.value", ARCHIVE_POLICY),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreateTopic {name}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("TopicArn")
        .unwrap_or_else(|| panic!("CreateTopic {name}: no TopicArn in {}", answer.body))
}

/// Subscribe an endpoint of `protocol` to `topic`, answering the subscription's ARN.
async fn subscribe(sns: &Sns, topic: &str, protocol: &str, endpoint: &str) -> String {
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", topic),
                ("Protocol", protocol),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "Subscribe expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("no SubscriptionArn in {}", answer.body))
}

/// One subscription row: the case, the protocol and endpoint the subscription is created
/// for, the attribute, the document it holds first, and whether an empty value REMOVES it.
type SubscriptionCase<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, bool);

/// An empty value removes the policy a subscription attribute holds — except where the
/// document AWS requires a member of is the attribute's, which is refused with the
/// sentence naming that attribute while the record keeps everything it held.
#[tokio::test]
async fn an_empty_policy_value_removes_the_policy_unless_its_document_requires_a_member() {
    let sns = Sns::new();
    let rows: &[SubscriptionCase<'_>] = &[
        (
            "a filter policy",
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:empty-filter",
            "FilterPolicy",
            FILTER_POLICY,
            true,
        ),
        (
            "a delivery policy",
            "https",
            "https://example.test/empty-delivery",
            "DeliveryPolicy",
            DELIVERY_POLICY,
            true,
        ),
        (
            "a replay policy",
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:empty-replay",
            "ReplayPolicy",
            REPLAY_POLICY,
            true,
        ),
        (
            "a redrive policy, whose document requires its dead-letter target",
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:empty-redrive",
            "RedrivePolicy",
            REDRIVE_POLICY,
            false,
        ),
    ];

    for (index, (case, protocol, endpoint, name, document, removes)) in rows.iter().enumerate() {
        // Every subscription is of an ARCHIVING FIFO topic, so a replay policy has a
        // window to name whichever row is being written.
        let topic = archiving_topic(&sns, &format!("empty-value-{index:02}")).await;
        let arn = subscribe(&sns, &topic, protocol, endpoint).await;
        // A sibling attribute whose value is not a policy document at all: the refused
        // write below must leave it exactly as it is.
        let (status, code, message) = set_subscription(&sns, &arn, "RawMessageDelivery", "true")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing RawMessageDelivery expected 200, got {status} / {code} \
             {message}"
        );
        let (status, code, message) = set_subscription(&sns, &arn, name, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing {name} expected 200, got {status} / {code} {message}"
        );

        let (status, code, message) = set_subscription(&sns, &arn, name, "").await;
        let held = subscription_attributes(&sns, &arn).await;
        if *removes {
            assert_eq!(
                status, 200,
                "case {case:?}: an empty {name} removes it, got {status} / {code} {message}"
            );
            assert_eq!(
                held.get(*name),
                None,
                "case {case:?}: a removed {name} must not be reported, got {held:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: an empty {name} names no document, so the write is refused, \
                 got {status} / {code} {message}"
            );
            assert_eq!(
                code, "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {code} {message}"
            );
            assert_eq!(
                message,
                missing_required_value_refusal(name),
                "case {case:?}: unexpected sentence, got {message:?}"
            );
            assert_eq!(
                held.get(*name).map(String::as_str),
                Some(*document),
                "case {case:?}: the refused write must leave {name} as it was, got {held:?}"
            );
        }
        // And the sibling attribute is untouched either way: a write of one attribute
        // changes no other.
        assert_eq!(
            held.get("RawMessageDelivery").map(String::as_str),
            Some("true"),
            "case {case:?}: the write must leave RawMessageDelivery as it was, got {held:?}"
        );
    }
}

/// The same rule holds for the document a redrive policy carries rather than for the
/// emptiness of the string: a document that carries none of the members the attribute
/// requires is refused with the same sentence, whichever call carried the write.
#[tokio::test]
async fn a_policy_document_missing_its_required_member_is_refused_by_every_write_path() {
    let sns = Sns::new();
    let topic = archiving_topic(&sns, "missing-member").await;
    let endpoint = "arn:aws:sqs:us-east-1:000000000000:missing-member";
    let arn = subscribe(&sns, &topic, "sqs", endpoint).await;
    let (status, code, message) = set_subscription(&sns, &arn, "RedrivePolicy", REDRIVE_POLICY)
        .await;
    assert_eq!(
        status, 200,
        "writing the first redrive policy expected 200, got {status} / {code} {message}"
    );

    // (case, the value written)
    let refused: &[(&str, &str)] = &[
        ("an empty value", ""),
        ("an empty document", "{}"),
        ("a document naming another member", r#"{"maxReceiveCount":"5"}"#),
        ("a value that is not a document at all", "dlq"),
        ("a document whose target is not a string", r#"{"deadLetterTargetArn":7}"#),
    ];

    for (case, value) in refused {
        let (status, code, message) = set_subscription(&sns, &arn, "RedrivePolicy", value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused, got {status} / {code} {message}"
        );
        assert_eq!(
            message,
            missing_required_value_refusal("RedrivePolicy"),
            "case {case:?}: unexpected sentence, got {message:?}"
        );
        let held = subscription_attributes(&sns, &arn).await;
        assert_eq!(
            held.get("RedrivePolicy").map(String::as_str),
            Some(REDRIVE_POLICY),
            "case {case:?}: the refused write must leave the stored policy as it was, got \
             {held:?}"
        );
    }

    // `Subscribe` holds a write to the same rule, so an endpoint subscribed with such a
    // document is refused rather than subscribed without the policy…
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:subscribe-empty"),
                ("Attributes.entry.1.key", "RedrivePolicy"),
                ("Attributes.entry.1.value", ""),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "Subscribe with an empty RedrivePolicy must be refused, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(missing_required_value_refusal("RedrivePolicy").as_str()),
        "Subscribe must state the same sentence, got {}",
        answer.body
    );

    // …and a re-subscribe of an endpoint that already holds one leaves that policy as it
    // is, because the write is refused before anything is applied.
    let answer = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
                ("ReturnSubscriptionArn", "true"),
                ("Attributes.entry.1.key", "RedrivePolicy"),
                ("Attributes.entry.1.value", ""),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "re-subscribing with an empty RedrivePolicy must be refused, got {} / {}",
        answer.status, answer.body
    );
    let held = subscription_attributes(&sns, &arn).await;
    assert_eq!(
        held.get("RedrivePolicy").map(String::as_str),
        Some(REDRIVE_POLICY),
        "the refused re-subscribe must leave the stored policy as it was, got {held:?}"
    );

    // A document that DOES carry the required member replaces the stored one, so the
    // refusals above are the document's doing and not a read-only attribute.
    let (status, code, message) =
        set_subscription(&sns, &arn, "RedrivePolicy", OTHER_REDRIVE_POLICY).await;
    assert_eq!(
        status, 200,
        "replacing the redrive policy expected 200, got {status} / {code} {message}"
    );
    let held = subscription_attributes(&sns, &arn).await;
    assert_eq!(
        held.get("RedrivePolicy").map(String::as_str),
        Some(OTHER_REDRIVE_POLICY),
        "a write that names the required member replaces the policy, got {held:?}"
    );
}

/// A TOPIC's policy attributes follow the same rule: none of them documents a required
/// member, so an empty value removes each of them.
#[tokio::test]
async fn an_empty_value_removes_a_topic_policy_attribute() {
    let sns = Sns::new();
    let topic = archiving_topic(&sns, "topic-empty-value").await;

    // (case, the attribute, the document it holds first)
    let rows: &[(&str, &str, &str)] = &[
        ("an archive policy", "ArchivePolicy", ARCHIVE_POLICY),
        (
            "a delivery policy",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":3}}}"#,
        ),
    ];

    for (case, name, document) in rows {
        let (status, code, message) = set_topic(&sns, &topic, name, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing {name} expected 200, got {status} / {code} {message}"
        );
        assert!(
            topic_attributes(&sns, &topic).await.contains_key(*name),
            "case {case:?}: {name} must be reported once it is set"
        );

        let (status, code, message) = set_topic(&sns, &topic, name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: an empty {name} removes it, got {status} / {code} {message}"
        );
        let held = topic_attributes(&sns, &topic).await;
        assert_eq!(
            held.get(*name),
            None,
            "case {case:?}: a removed {name} must not be reported, got {held:?}"
        );
    }
}
