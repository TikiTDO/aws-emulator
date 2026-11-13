// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ConfirmSubscription`: the call that turns a subscription
//! awaiting its subscriber's confirmation into one SNS delivers to.
//!
//! The rule under test: a subscription awaiting confirmation is confirmed ONLY by the
//! confirmation token this service minted for it, presented together with the ARN of
//! the topic that subscription belongs to — the confirmation publishes the
//! subscription's ARN and leaves it a confirmed subscription of the topic, and is
//! idempotent in that token — while a token no endpoint of the named topic was sent is
//! refused as bad input (HTTP 400 / `InvalidParameter`), a `TopicArn` the account holds
//! no topic for is the modelled lookup miss (HTTP 404 / `NotFound`), and a value that is
//! not an ARN at all is bad input too. No refused call changes any subscription.
//!
//! The rows differ along what the rule generalises over: the protocol whose subscriber
//! has to confirm and the form of its endpoint, whether `AuthenticateOnUnsubscribe` is
//! asked for, how many subscriptions of the topic are awaiting confirmation at once,
//! which topic a token is presented with, and the shape of the value presented as a
//! token or as a topic ARN.
//!
//! AWS sends the token to the ENDPOINT, in the `SubscriptionConfirmation` message it
//! posts or mails there, so no response of this service ever carries it: the token is
//! read out of the subscription record it was minted for, the way the SMS sandbox suite
//! reads the one-time password delivered to a destination number.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    CONFIRMATION_TOKEN_HEX_CHARS, INVALID_CONFIRMATION_TOKEN_REFUSAL, PENDING_CONFIRMATION_LISTED,
    TOPIC_DOES_NOT_EXIST,
};

use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// One confirmable subscription: a protocol whose subscriber has to confirm, and an
/// endpoint in the form that protocol addresses.
///
/// The protocols are the ones `SubscribeInput$Protocol` documents as addressed by a URL
/// or an email address — the subscriptions SNS cannot confirm itself — with two endpoint
/// forms each, so no assertion can pass by matching one recorded string.
const CONFIRMABLE: &[(&str, &str)] = &[
    ("http", "http://example.test/hook"),
    ("http", "http://10.0.0.1:8080/notify?x=1"),
    ("https", "https://example.test/hook"),
    ("https", "https://deep.example.test/a/b/c"),
    ("email", "ops@example.test"),
    ("email", "first.last+tag@mail.example.co.uk"),
    ("email-json", "ops@example.test"),
    ("email-json", "alerts@sub.example.test"),
];

/// Subscribe `endpoint` over `protocol`, asking for the ARN in every case, and answer
/// the subscription ARN the call reports.
async fn subscribe(sns: &Sns, case: &str, topic: &str, protocol: &str, endpoint: &str) -> String {
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
        "case {case:?}: expected Subscribe to answer 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("SubscriptionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionArn in {}", answer.body))
}

/// The confirmation token the subscription under `arn` holds, read from persisted state
/// because AWS sends it to the endpoint and no response carries it.
fn token_of(sns: &Sns, case: &str, arn: &str) -> String {
    let state = sns.state();
    // `guard` is declared after `state`, so it is dropped first.
    let guard = state.lock();
    guard
        .subscriptions
        .get(arn)
        .unwrap_or_else(|| panic!("case {case:?}: no subscription record under {arn}"))
        .confirmation_token
        .clone()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a subscription awaiting confirmation must hold a token")
        })
}

/// Whether the subscription under `arn` is still awaiting confirmation, or `None` when
/// no record is held.
fn pending(sns: &Sns, arn: &str) -> Option<bool> {
    let state = sns.state();
    let guard = state.lock();
    guard
        .subscriptions
        .get(arn)
        .map(|record| record.pending_confirmation)
}

/// How many subscriptions this account holds in all.
fn held(sns: &Sns) -> usize {
    sns.state().lock().subscriptions.len()
}

/// One `ConfirmSubscription` call with the parameters given.
async fn confirm(sns: &Sns, params: &[(&str, &str)]) -> Answer {
    sns.call("ConfirmSubscription", params).await
}

/// The attributes `GetSubscriptionAttributes` reports for `arn`.
async fn attributes_of(sns: &Sns, case: &str, arn: &str) -> std::collections::HashMap<String, String> {
    let answer = sns
        .call("GetSubscriptionAttributes", &[("SubscriptionArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the attribute read to answer 200, got {} / {}",
        answer.status, answer.body
    );
    attribute_map(&answer)
}

/// Presenting the token minted for a subscription confirms exactly that subscription:
/// the call answers that subscription's ARN, the record stops awaiting confirmation,
/// every report of it publishes its ARN in place of the pending sentinel, and the topic
/// counts it among its confirmed subscriptions. Presenting the same token again answers
/// the same ARN and changes nothing more.
#[tokio::test]
async fn the_token_minted_for_a_subscription_confirms_it_and_the_call_is_idempotent() {
    let sns = Sns::new();

    for (index, (protocol, endpoint)) in CONFIRMABLE.iter().enumerate() {
        // Half the rows ask for `AuthenticateOnUnsubscribe`, which is accepted and
        // withholds nothing: every request this emulator serves is signed by the one
        // identity it publishes credentials for.
        let authenticate = index % 2 == 0;
        let case = format!("{protocol} -> {endpoint} (AuthenticateOnUnsubscribe={authenticate})");
        let topic = sns.topic(&format!("confirm-{index:02}")).await;
        let arn = subscribe(&sns, &case, &topic, protocol, endpoint).await;

        let token = token_of(&sns, &case, &arn);
        assert_eq!(
            token.len(),
            CONFIRMATION_TOKEN_HEX_CHARS,
            "case {case:?}: expected a {CONFIRMATION_TOKEN_HEX_CHARS}-character token, \
             got {} characters",
            token.len()
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a hexadecimal token, got {token:?}"
        );
        assert_eq!(
            pending(&sns, &arn),
            Some(true),
            "case {case:?}: a {protocol} subscription must await confirmation"
        );

        let mut params: Vec<(&str, &str)> =
            vec![("TopicArn", &topic), ("Token", token.as_str())];
        if authenticate {
            params.push(("AuthenticateOnUnsubscribe", "true"));
        }
        let before = held(&sns);
        let answer = confirm(&sns, &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("SubscriptionArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the confirmed subscription's ARN {arn}, got {}",
            answer.body
        );
        assert_eq!(
            held(&sns),
            before,
            "case {case:?}: confirming must not add a subscription record"
        );
        assert_eq!(
            pending(&sns, &arn),
            Some(false),
            "case {case:?}: the confirmed subscription must no longer be pending"
        );

        // The confirmation is observable through every report of the subscription.
        let attributes = attributes_of(&sns, &case, &arn).await;
        assert_eq!(
            attributes.get("PendingConfirmation").map(String::as_str),
            Some("false"),
            "case {case:?}: expected PendingConfirmation=false, got {attributes:?}"
        );
        assert_eq!(
            attributes
                .get("ConfirmationWasAuthenticated")
                .map(String::as_str),
            Some("true"),
            "case {case:?}: expected ConfirmationWasAuthenticated=true, got {attributes:?}"
        );
        let listed = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
            .await;
        assert_eq!(
            listed.elements("SubscriptionArn"),
            vec![arn.clone()],
            "case {case:?}: expected the listing to publish the ARN, got {}",
            listed.body
        );
        let topic_attributes = sns
            .call("GetTopicAttributes", &[("TopicArn", &topic)])
            .await;
        let counts = attribute_map(&topic_attributes);
        assert_eq!(
            counts.get("SubscriptionsConfirmed").map(String::as_str),
            Some("1"),
            "case {case:?}: expected the topic to count one confirmed subscription, \
             got {counts:?}"
        );
        assert_eq!(
            counts.get("SubscriptionsPending").map(String::as_str),
            Some("0"),
            "case {case:?}: expected the topic to count no pending subscription, got {counts:?}"
        );

        // Idempotent in the token: the operation models no "already confirmed" error.
        let again = confirm(&sns, &params).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: a second confirmation must answer 200, got {} / {}",
            again.status, again.body
        );
        assert_eq!(
            again.element("SubscriptionArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: a second confirmation must answer the same ARN, got {}",
            again.body
        );
        assert_eq!(
            pending(&sns, &arn),
            Some(false),
            "case {case:?}: a second confirmation must leave the subscription confirmed"
        );
    }
}

/// One topic holding several subscriptions awaiting confirmation: each token confirms
/// its OWN subscription and leaves every other one pending.
#[tokio::test]
async fn each_token_confirms_only_the_subscription_it_was_minted_for() {
    let sns = Sns::new();
    let topic = sns.topic("confirm-one-of-many").await;

    let mut subscriptions: Vec<(String, String, String)> = Vec::new();
    for (protocol, endpoint) in CONFIRMABLE {
        let case = format!("{protocol} -> {endpoint}");
        let arn = subscribe(&sns, &case, &topic, protocol, endpoint).await;
        let token = token_of(&sns, &case, &arn);
        subscriptions.push((case, arn, token));
    }
    // No two subscriptions share a token, so one token can never confirm another's.
    let distinct: std::collections::HashSet<&String> =
        subscriptions.iter().map(|(_, _, token)| token).collect();
    assert_eq!(
        distinct.len(),
        subscriptions.len(),
        "expected one token per subscription, got {} distinct of {}",
        distinct.len(),
        subscriptions.len()
    );

    for (confirmed, (case, arn, token)) in subscriptions.iter().enumerate() {
        let answer = confirm(&sns, &[("TopicArn", &topic), ("Token", token)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("SubscriptionArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the ARN of the subscription this token was minted \
             for, got {}",
            answer.body
        );
        for (other_case, other_arn, _) in &subscriptions[confirmed + 1..] {
            assert_eq!(
                pending(&sns, other_arn),
                Some(true),
                "case {case:?}: confirming it must leave {other_case:?} pending"
            );
        }
        let listed = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
            .await;
        let published = listed
            .elements("SubscriptionArn")
            .into_iter()
            .filter(|reported| reported != PENDING_CONFIRMATION_LISTED)
            .count();
        assert_eq!(
            published,
            confirmed + 1,
            "case {case:?}: expected {} published ARNs in the listing, got {}",
            confirmed + 1,
            listed.body
        );
    }
}

/// A token no endpoint of the named topic was sent is refused as bad input, with the
/// service's sentence for the member, and leaves every subscription exactly as it was.
///
/// The rows are the shapes a wrong token takes: a word, whitespace, a well-formed token
/// of the right width that was never minted, the token of a subscription of ANOTHER
/// topic, the minted token with one character changed (the value is opaque, so only
/// equality against it counts), and the subscription's own ARN.
#[tokio::test]
async fn a_token_no_endpoint_of_the_topic_was_sent_is_refused() {
    let sns = Sns::new();
    let topic = sns.topic("confirm-bad-token").await;
    let other_topic = sns.topic("confirm-other-topic").await;

    let arn = subscribe(
        &sns,
        "the subscription under test",
        &topic,
        "https",
        "https://example.test/hook",
    )
    .await;
    let other_arn = subscribe(
        &sns,
        "a subscription of another topic",
        &other_topic,
        "http",
        "http://example.test/elsewhere",
    )
    .await;
    let token = token_of(&sns, "the subscription under test", &arn);
    let other_token = token_of(&sns, "a subscription of another topic", &other_arn);

    // The minted token with ONE character changed: the token is opaque, so nothing but
    // equality against the minted value may confirm anything.
    let altered: String = {
        let mut characters: Vec<char> = token.chars().collect();
        characters[0] = if characters[0] == '0' { '1' } else { '0' };
        characters.into_iter().collect()
    };
    let never_minted = "f".repeat(CONFIRMATION_TOKEN_HEX_CHARS);
    let cases: Vec<(&str, &str)> = vec![
        ("not a token at all", "bogus"),
        ("whitespace", " "),
        ("the right width, never minted", never_minted.as_str()),
        ("another topic's token", other_token.as_str()),
        ("the minted token with one character changed", altered.as_str()),
        ("the subscription's own ARN", arn.as_str()),
    ];

    for (case, presented) in &cases {
        let before = held(&sns);
        let answer = confirm(&sns, &[("TopicArn", &topic), ("Token", presented)]).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_CONFIRMATION_TOKEN_REFUSAL),
            "case {case:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
        assert_eq!(
            pending(&sns, &arn),
            Some(true),
            "case {case:?}: a refused confirmation must leave the subscription pending"
        );
        assert_eq!(
            pending(&sns, &other_arn),
            Some(true),
            "case {case:?}: a refused confirmation must leave the other topic's \
             subscription pending"
        );
        assert_eq!(
            held(&sns),
            before,
            "case {case:?}: a refused confirmation must write no subscription"
        );
    }

    // A subscription SNS confirmed itself was sent no token, so no token confirms one:
    // the queue subscription below stays confirmed and the call is refused all the same.
    let queue_topic = sns.topic("confirm-nothing-pending").await;
    let queue = subscribe(
        &sns,
        "a queue subscription",
        &queue_topic,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:q",
    )
    .await;
    assert_eq!(
        pending(&sns, &queue),
        Some(false),
        "a queue subscription must be confirmed when it is created"
    );
    for presented in [token.as_str(), never_minted.as_str()] {
        let answer = confirm(&sns, &[("TopicArn", &queue_topic), ("Token", presented)]).await;
        assert_eq!(
            answer.status, 400,
            "case {presented:?} against a topic with nothing pending: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {presented:?} against a topic with nothing pending: expected the \
             InvalidParameter code, got {}",
            answer.body
        );
    }
    assert_eq!(
        pending(&sns, &queue),
        Some(false),
        "the queue subscription must still be confirmed"
    );
}

/// A `TopicArn` the account holds no topic for is the modelled lookup miss, and a value
/// that is not an ARN at all is bad input — both judged before any token is considered,
/// and neither changing a subscription.
#[tokio::test]
async fn confirm_subscription_refuses_an_unknown_topic_and_a_value_that_is_not_an_arn() {
    let sns = Sns::new();
    let topic = sns.topic("confirm-topic-lookup").await;
    let arn = subscribe(
        &sns,
        "the subscription under test",
        &topic,
        "email",
        "ops@example.test",
    )
    .await;
    let token = token_of(&sns, "the subscription under test", &arn);

    // Well-formed topic ARNs this account holds no topic for: a name nobody created, a
    // topic deleted with its subscriptions, another region, another partition, another
    // service. An ARN naming another ACCOUNT is not one this endpoint resolves at all —
    // it is refused before the lookup, with the account-resolution refusal every
    // ARN-addressed call of this service shares (see the `arn_reference_refusals` suite).
    let deleted = sns.topic("confirm-deleted-topic").await;
    let deleted_answer = sns.call("DeleteTopic", &[("TopicArn", &deleted)]).await;
    assert_eq!(
        deleted_answer.status, 200,
        "DeleteTopic must succeed, got {} / {}",
        deleted_answer.status, deleted_answer.body
    );
    let missing = [
        ("a name nobody created", "arn:aws:sns:us-east-1:000000000000:absent-topic".to_string()),
        ("a deleted topic", deleted.clone()),
        ("another region", "arn:aws:sns:eu-west-1:000000000000:elsewhere".to_string()),
        ("another partition", "arn:aws-us-gov:sns:us-gov-west-1:000000000000:gov".to_string()),
        ("another service", "arn:aws:sqs:us-east-1:000000000000:q".to_string()),
    ];
    for (case, named) in &missing {
        let answer = confirm(&sns, &[("TopicArn", named), ("Token", &token)]).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "NotFound",
            "case {case:?}: expected the NotFound code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(TOPIC_DOES_NOT_EXIST),
            "case {case:?}: expected the fixed missing-topic sentence, got {}",
            answer.body
        );
        assert!(
            answer.element("SubscriptionArn").is_none(),
            "case {case:?}: a refused confirmation must publish no ARN, got {}",
            answer.body
        );
    }

    // Values that are not ARNs at all: too few elements, no `arn:` prefix.
    let malformed = [
        "does-not-exist",
        "arn:aws:sns",
        "arn:aws:sns:us-east-1",
        "aws:sns:us-east-1:000000000000:confirm-topic-lookup",
    ];
    for named in malformed {
        let answer = confirm(&sns, &[("TopicArn", named), ("Token", &token)]).await;
        assert_eq!(
            answer.status, 400,
            "case {named:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {named:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        let message = answer.element("Message").unwrap_or_default();
        assert!(
            message.starts_with("Invalid parameter: TopicArn Reason: "),
            "case {named:?}: expected the member's refusal sentence, got {message:?}"
        );
    }

    assert_eq!(
        pending(&sns, &arn),
        Some(true),
        "every refused confirmation must leave the subscription pending"
    );

    // And the token still works once the topic it belongs to is named.
    let served = confirm(&sns, &[("TopicArn", &topic), ("Token", &token)]).await;
    assert_eq!(
        served.status, 200,
        "the token must still confirm its own subscription, got {} / {}",
        served.status, served.body
    );
    assert_eq!(
        pending(&sns, &arn),
        Some(false),
        "the subscription must be confirmed once its own topic is named"
    );
}
