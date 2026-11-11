// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the policy-valued attributes of a subscription: how one is
//! REMOVED, and when the filtering scope beside a filter policy is reported.
//!
//! Every call goes over the wire through the router the emulator serves in production, so
//! nothing but persisted state carries information between calls.
//!
//! The rules under test:
//!
//! * an attribute whose value is a policy DOCUMENT is removed by writing an EMPTY value to
//!   it ("To delete a filter policy, set the FilterPolicy attribute to an empty value",
//!   SNS Developer Guide), so the subscription then reports what it reports for a policy it
//!   was never given — while an attribute whose value is not a document, and one whose
//!   document AWS requires a member of, still refuses one;
//! * a `FilterPolicyScope` says which part of a message "the filter is applied on"
//!   (`GetSubscriptionAttributesResponse$Attributes` doc), so it is reported only while
//!   there is a policy to apply: a scope set without a policy is held and hidden until one
//!   is set.
//!
//! The rows differ along what the rules generalise over: which policy attribute is
//! removed, which call carried the write, and whether the scope, the policy or both reach
//! the subscription first.

use super::topics_subscriptions_publish::{attribute_map, Sns};

/// A filter policy, as the `FilterPolicy` attribute carries one.
const FILTER_POLICY: &str = r#"{"kind":["alert"]}"#;

/// A dead-letter queue, as the `RedrivePolicy` attribute names one.
const REDRIVE_POLICY: &str = r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:dlq"}"#;

/// A delivery policy, in the shape a SUBSCRIPTION's `DeliveryPolicy` carries.
const DELIVERY_POLICY: &str = r#"{"healthyRetryPolicy":{"numRetries":7}}"#;

/// Subscribe an endpoint of `protocol` to `topic` with `attributes`, answering the
/// subscription's ARN.
async fn subscribe(
    sns: &Sns,
    case: &str,
    topic: &str,
    protocol: &str,
    endpoint: &str,
    attributes: &[(&str, &str)],
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), protocol.to_string()),
        ("Endpoint".to_string(), endpoint.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
    ];
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
    let answer = sns.call("Subscribe", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: Subscribe expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body))
}

/// The attributes one subscription reports.
async fn read(sns: &Sns, case: &str, arn: &str) -> std::collections::HashMap<String, String> {
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetSubscriptionAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// Write one subscription attribute, answering the call's status, code and message.
async fn set(sns: &Sns, arn: &str, name: &str, value: &str) -> (u16, String, String) {
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

/// One removal row: the case, the subscription's protocol and endpoint, the attribute, and
/// the document it holds before the removal.
type RemovalCase<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

/// Writing an EMPTY value to a policy-valued attribute removes it, whichever call carries
/// the write and whichever policy it is — while an attribute whose value is not a document
/// still refuses an empty one.
#[tokio::test]
async fn an_empty_value_removes_a_policy_attribute() {
    let sns = Sns::new();
    let cases: &[RemovalCase<'_>] = &[
        (
            "a filter policy",
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:removal-filter",
            "FilterPolicy",
            FILTER_POLICY,
        ),
        (
            "a delivery policy",
            "https",
            "https://example.test/removal",
            "DeliveryPolicy",
            DELIVERY_POLICY,
        ),
    ];

    for (index, (case, protocol, endpoint, name, document)) in cases.iter().enumerate() {
        // Written by `SetSubscriptionAttributes`, then removed by it.
        let topic = sns.topic(&format!("removal-set-{index:02}")).await;
        let arn = subscribe(&sns, case, &topic, protocol, endpoint, &[]).await;
        let (status, code, message) = set(&sns, &arn, name, document).await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing {name} expected 200, got {status} / {code} {message}"
        );
        let held = read(&sns, case, &arn).await;
        assert!(
            held.contains_key(*name),
            "case {case:?}: {name} must be reported once it is set, got {held:?}"
        );

        let (status, code, message) = set(&sns, &arn, name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing {name} expected 200, got {status} / {code} {message}"
        );
        let held = read(&sns, case, &arn).await;
        assert_eq!(
            held.get(*name),
            None,
            "case {case:?}: a removed {name} must not be reported, got {held:?}"
        );

        // And carried EMPTY by `Subscribe`, which leaves the subscription holding none of
        // it: the same rule, the other write path.
        let other = sns.topic(&format!("removal-subscribe-{index:02}")).await;
        let carried = subscribe(
            &sns,
            case,
            &other,
            protocol,
            endpoint,
            &[(name, "")],
        )
        .await;
        let held = read(&sns, case, &carried).await;
        assert_eq!(
            held.get(*name),
            None,
            "case {case:?}: an empty {name} at Subscribe must leave none held, got {held:?}"
        );
        // A re-subscribe is idempotent only in what the subscription already holds: one
        // that asks for a policy the subscription does not have is refused, and the
        // subscription is left holding none of it.
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &other),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint),
                    ("ReturnSubscriptionArn", "true"),
                    ("Attributes.entry.1.key", name),
                    ("Attributes.entry.1.value", document),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a re-subscribe carrying a policy the subscription does not hold \
             must be refused, got {} / {}",
            answer.status, answer.body
        );
        let held = read(&sns, case, &carried).await;
        assert_eq!(
            held.get(*name),
            None,
            "case {case:?}: the refused re-subscribe must leave none held, got {held:?}"
        );
        // While the IDENTICAL request — the empty value again, on a subscription that
        // holds no such policy — answers the subscription that already exists.
        let repeated = subscribe(&sns, case, &other, protocol, endpoint, &[(name, "")]).await;
        assert_eq!(
            repeated, carried,
            "case {case:?}: an identical re-subscribe must answer the same subscription"
        );
        let held = read(&sns, case, &carried).await;
        assert_eq!(
            held.get(*name),
            None,
            "case {case:?}: the subscription must still hold no {name}, got {held:?}"
        );
    }

    // An attribute whose value is NOT a policy document is not removed by an empty value:
    // the value is judged against the constraint the attribute documents, and an empty
    // one satisfies none. Neither is a policy whose document AWS requires a member of: the
    // empty value carries no such member, so it names no policy to store.
    let topic = sns.topic("removal-flag").await;
    let arn = subscribe(
        &sns,
        "a flag",
        &topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:removal-flag",
        &[("RedrivePolicy", REDRIVE_POLICY)],
    )
    .await;
    for name in ["RawMessageDelivery", "FilterPolicyScope", "RedrivePolicy"] {
        let (status, code, message) = set(&sns, &arn, name, "").await;
        assert_eq!(
            status, 400,
            "an empty {name} names no value it accepts, got {status} / {code} {message}"
        );
        assert_eq!(
            code, "InvalidParameter",
            "an empty {name} must be refused with InvalidParameter, got {code} {message}"
        );
    }
    // And the refused write changed nothing: the redrive policy the subscription held is
    // still the one it reports.
    let held = read(&sns, "a flag", &arn).await;
    assert_eq!(
        held.get("RedrivePolicy").map(String::as_str),
        Some(REDRIVE_POLICY),
        "a refused empty RedrivePolicy must leave the stored policy as it was, got {held:?}"
    );
}

/// A filtering scope is reported only beside the policy it scopes: one set without a
/// policy is HELD and hidden, appears when a policy is set, and is hidden again when the
/// policy is removed.
#[tokio::test]
async fn a_filtering_scope_is_reported_only_beside_the_policy_it_scopes() {
    let sns = Sns::new();
    // (case, the scope the subscription is given, the scope it reports once a policy is
    // set)
    let cases: &[(&str, &str, &str)] = &[
        ("the message body scope", "MessageBody", "MessageBody"),
        (
            "the message attribute scope stated explicitly",
            "MessageAttributes",
            "MessageAttributes",
        ),
        // A subscription given no scope is filtered under the documented default:
        // "MessageAttributes (default) – The filter is applied on the message attributes"
        // (`GetSubscriptionAttributesResponse$Attributes` doc).
        ("no scope at all", "", "MessageAttributes"),
    ];

    for (index, (case, scope, reported)) in cases.iter().enumerate() {
        let topic = sns.topic(&format!("hidden-scope-{index:02}")).await;
        let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:hidden-scope-{index:02}");
        let given: Vec<(&str, &str)> = if scope.is_empty() {
            Vec::new()
        } else {
            vec![("FilterPolicyScope", *scope)]
        };
        let arn = subscribe(&sns, case, &topic, "sqs", &endpoint, &given).await;

        let early = read(&sns, case, &arn).await;
        assert_eq!(
            early.get("FilterPolicyScope"),
            None,
            "case {case:?}: a scope with no policy to apply must not be reported, got \
             {early:?}"
        );
        assert_eq!(
            early.get("FilterPolicy"),
            None,
            "case {case:?}: no policy was set, so none may be reported, got {early:?}"
        );

        // Setting a policy reveals the scope the subscription has been holding all along.
        let (status, code, message) = set(&sns, &arn, "FilterPolicy", FILTER_POLICY).await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing the policy expected 200, got {status} / {code} {message}"
        );
        let held = read(&sns, case, &arn).await;
        assert_eq!(
            held.get("FilterPolicyScope").map(String::as_str),
            Some(*reported),
            "case {case:?}: the policy must be applied under the scope the subscription \
             holds, got {held:?}"
        );

        // Removing the policy hides the scope again, and the subscription reports exactly
        // what it reported before either was set.
        let (status, code, message) = set(&sns, &arn, "FilterPolicy", "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing the policy expected 200, got {status} / {code} {message}"
        );
        let after = read(&sns, case, &arn).await;
        assert_eq!(
            after, early,
            "case {case:?}: removing the policy must leave the subscription reporting what \
             it reported before, got {after:?}"
        );
    }
}
