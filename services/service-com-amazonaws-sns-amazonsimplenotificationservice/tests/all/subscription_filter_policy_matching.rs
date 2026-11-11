// SPDX-License-Identifier: CC0-1.0

//! `Publish` applies a subscription's FILTER POLICY, so a subscriber receives exactly the
//! messages its policy admits — end to end over the wire, with SQS mounted on the same
//! engine so what was delivered is observed where a client observes it: on the queue.
//!
//! The RULE asserted here: "FilterPolicy – The simple JSON object that lets your
//! subscriber receive only a subset of messages, rather than receiving every message
//! published to the topic" (`SubscribeInput$Attributes` doc), matched against the part of
//! the message the subscription's scope names — "MessageAttributes (default) – The filter
//! is applied on the message attributes. MessageBody – The filter is applied on the
//! message body" (`GetSubscriptionAttributesResponse$Attributes` doc). A message satisfies
//! a policy when every property the policy names is satisfied, and a property is
//! satisfied when any of the conditions listed for it holds; a subscription holding no
//! policy receives every message. The stored scope governs the match whichever ORDER the
//! scope and the policy were written in.
//!
//! The rows differ along what the rule generalises over: the scope, the condition kind
//! (an exact value, a number, `prefix`, `suffix`, `equals-ignore-case`, `anything-but`,
//! `numeric`, `exists`, `$or`, a nested property), the type of the value matched
//! (string, number, list), and whether the message satisfies the policy at all.

use serde_json::json;
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::filter_policy::{
    MAX_FILTER_MATCH_COMPARISONS, MAX_FILTER_POLICY_COMBINATIONS,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::state::MAX_ATTRIBUTE_BYTES_PER_RECORD;

use crate::sqs_subscription_delivery::{
    drain, element, emulator, given_queue, sns, sns_call, Emulator,
};

/// Subscribe the queue `queue_arn` to `topic` with `attributes`, and answer the
/// subscription's ARN.
async fn subscribe_with(
    emulator: &Emulator,
    topic: &str,
    queue_arn: &str,
    attributes: &[(&str, &str)],
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("TopicArn".to_string(), topic.to_string()),
        ("Protocol".to_string(), "sqs".to_string()),
        ("Endpoint".to_string(), queue_arn.to_string()),
        ("ReturnSubscriptionArn".to_string(), "true".to_string()),
        ("Attributes.entry.1.key".to_string(), "RawMessageDelivery".to_string()),
        ("Attributes.entry.1.value".to_string(), "true".to_string()),
    ];
    for (index, (name, value)) in attributes.iter().enumerate() {
        params.push((
            format!("Attributes.entry.{}.key", index + 2),
            (*name).to_string(),
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
    element(&sns(emulator, "Subscribe", &borrowed).await, "SubscriptionArn")
}

/// One body-scoped row: the case, the filter policy, the message body, and whether the
/// subscriber must receive that message.
type BodyCase<'a> = (&'a str, &'a str, &'a str, bool);

/// A policy applied on the MESSAGE BODY admits exactly the bodies it matches, and the
/// scope governs even when it was stored BEFORE the policy it scopes — the order the two
/// attributes were written in is not part of the rule.
#[tokio::test]
async fn a_body_scoped_filter_policy_admits_exactly_the_bodies_it_matches() {
    // (case, policy, body, delivered)
    let rows: &[BodyCase<'_>] = &[
        // An exact value, and a sibling property the policy does not name.
        (
            "an exact string the body carries",
            r#"{"kind":["wanted"]}"#,
            r#"{"kind":"wanted","note":"ignored"}"#,
            true,
        ),
        (
            "an exact string the body does not carry",
            r#"{"kind":["wanted"]}"#,
            r#"{"kind":"unwanted"}"#,
            false,
        ),
        (
            "one of several values listed for a property",
            r#"{"kind":["other","wanted"]}"#,
            r#"{"kind":"wanted"}"#,
            true,
        ),
        // Two properties: every property the policy names has to be satisfied.
        (
            "two properties, both satisfied",
            r#"{"kind":["wanted"],"region":["eu-west-1"]}"#,
            r#"{"kind":"wanted","region":"eu-west-1"}"#,
            true,
        ),
        (
            "two properties, one unsatisfied",
            r#"{"kind":["wanted"],"region":["eu-west-1"]}"#,
            r#"{"kind":"wanted","region":"us-east-1"}"#,
            false,
        ),
        (
            "a property the body does not carry at all",
            r#"{"missing":["wanted"]}"#,
            r#"{"kind":"wanted"}"#,
            false,
        ),
        // A number in the body is matched as a number, however it was spelled.
        (
            "a number the body carries",
            r#"{"size":[10]}"#,
            r#"{"size":10}"#,
            true,
        ),
        (
            "a number spelled with a fraction of zero",
            r#"{"size":[10]}"#,
            r#"{"size":10.0}"#,
            true,
        ),
        (
            "a number the body does not carry",
            r#"{"size":[10]}"#,
            r#"{"size":11}"#,
            false,
        ),
        // A boolean and a null are values like any other.
        (
            "a boolean property",
            r#"{"urgent":[true]}"#,
            r#"{"urgent":true}"#,
            true,
        ),
        (
            "a boolean property of the other value",
            r#"{"urgent":[true]}"#,
            r#"{"urgent":false}"#,
            false,
        ),
        // A body property holding a LIST is satisfied by any of its elements.
        (
            "one element of a list the body carries",
            r#"{"tags":["blue"]}"#,
            r#"{"tags":["red","blue"]}"#,
            true,
        ),
        (
            "no element of a list the body carries",
            r#"{"tags":["green"]}"#,
            r#"{"tags":["red","blue"]}"#,
            false,
        ),
        // The documented operators.
        (
            "a prefix that matches",
            r#"{"kind":[{"prefix":"wan"}]}"#,
            r#"{"kind":"wanted"}"#,
            true,
        ),
        (
            "a prefix that does not match",
            r#"{"kind":[{"prefix":"unw"}]}"#,
            r#"{"kind":"wanted"}"#,
            false,
        ),
        (
            "a suffix that matches",
            r#"{"kind":[{"suffix":"ted"}]}"#,
            r#"{"kind":"wanted"}"#,
            true,
        ),
        (
            "a case-insensitive equality",
            r#"{"kind":[{"equals-ignore-case":"WANTED"}]}"#,
            r#"{"kind":"wanted"}"#,
            true,
        ),
        (
            "a case-insensitive prefix",
            r#"{"kind":[{"prefix":{"equals-ignore-case":"WAN"}}]}"#,
            r#"{"kind":"wanted"}"#,
            true,
        ),
        (
            "anything but one value, on a body that carries another",
            r#"{"kind":[{"anything-but":"unwanted"}]}"#,
            r#"{"kind":"wanted"}"#,
            true,
        ),
        (
            "anything but one value, on a body that carries it",
            r#"{"kind":[{"anything-but":"wanted"}]}"#,
            r#"{"kind":"wanted"}"#,
            false,
        ),
        (
            "anything but a list of values",
            r#"{"kind":[{"anything-but":["wanted","other"]}]}"#,
            r#"{"kind":"third"}"#,
            true,
        ),
        (
            "a numeric equality",
            r#"{"size":[{"numeric":["=",10]}]}"#,
            r#"{"size":10}"#,
            true,
        ),
        (
            "a numeric range the value is inside",
            r#"{"size":[{"numeric":[">",0,"<=",10]}]}"#,
            r#"{"size":10}"#,
            true,
        ),
        (
            "a numeric range the value is outside",
            r#"{"size":[{"numeric":[">",0,"<",10]}]}"#,
            r#"{"size":10}"#,
            false,
        ),
        (
            "a property required to exist",
            r#"{"kind":[{"exists":true}]}"#,
            r#"{"kind":"anything"}"#,
            true,
        ),
        (
            "a property required to be absent",
            r#"{"kind":[{"exists":false}]}"#,
            r#"{"other":"anything"}"#,
            true,
        ),
        (
            "a property required to be absent that is present",
            r#"{"kind":[{"exists":false}]}"#,
            r#"{"kind":"anything"}"#,
            false,
        ),
        (
            "either of two policies",
            r#"{"$or":[{"kind":["wanted"]},{"size":[10]}]}"#,
            r#"{"kind":"unwanted","size":10}"#,
            true,
        ),
        (
            "neither of two policies",
            r#"{"$or":[{"kind":["wanted"]},{"size":[10]}]}"#,
            r#"{"kind":"unwanted","size":11}"#,
            false,
        ),
        // A policy applied on the body may name a property INSIDE a property.
        (
            "a nested property that matches",
            r#"{"order":{"kind":["wanted"]}}"#,
            r#"{"order":{"kind":"wanted"}}"#,
            true,
        ),
        (
            "a nested property that does not match",
            r#"{"order":{"kind":["wanted"]}}"#,
            r#"{"order":{"kind":"unwanted"}}"#,
            false,
        ),
        // A body that is not JSON holds no property for a body-scoped policy to read.
        (
            "a body that is not JSON at all",
            r#"{"kind":["wanted"]}"#,
            "wanted",
            false,
        ),
    ];

    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "body-filtered")]).await,
        "TopicArn",
    );
    // The SCOPE is written FIRST, with no policy beside it, and the policy is set
    // afterwards: the subscription holds the scope all along, so the policy set later is
    // matched against the body.
    let (filtered_url, filtered_arn) = given_queue(&emulator, "body-filtered-queue").await;
    let filtered = subscribe_with(
        &emulator,
        &topic,
        &filtered_arn,
        &[("FilterPolicyScope", "MessageBody")],
    )
    .await;
    // An unfiltered twin of the same topic: it receives every message, so an empty
    // filtered queue is the policy's doing and not a delivery that never happened.
    let (twin_url, twin_arn) = given_queue(&emulator, "body-unfiltered-queue").await;
    subscribe_with(&emulator, &topic, &twin_arn, &[]).await;

    for (case, policy, body, delivered) in rows {
        sns(
            &emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &filtered),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", policy),
            ],
        )
        .await;
        // The policy reads back under the scope stored before it: the hidden scope is in
        // force, which is what the delivery below is judged under.
        let held = sns(
            &emulator,
            "GetSubscriptionAttributes",
            &[("SubscriptionArn", &filtered)],
        )
        .await;
        assert!(
            held.contains("MessageBody"),
            "case {case:?}: the policy must read back under the stored scope, got {held}"
        );

        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", body)],
        )
        .await;

        let received = drain(&emulator, &filtered_url).await;
        let bodies: Vec<&str> = received.iter().map(|(body, _)| body.as_str()).collect();
        let expected: Vec<&str> = if *delivered { vec![body] } else { Vec::new() };
        assert_eq!(
            bodies, expected,
            "case {case:?}: expected {expected:?} on the filtered queue, got {bodies:?}"
        );
        let twin = drain(&emulator, &twin_url).await;
        let twin_bodies: Vec<&str> = twin.iter().map(|(body, _)| body.as_str()).collect();
        assert_eq!(
            twin_bodies,
            vec![*body],
            "case {case:?}: the unfiltered twin must receive every message, got \
             {twin_bodies:?}"
        );
    }
}

/// One attribute-scoped row: the case, the policy, the published attribute's type and
/// value, and whether the subscriber must receive the message.
type AttributeCase<'a> = (&'a str, &'a str, &'a str, &'a str, bool);

/// A policy applied on the MESSAGE ATTRIBUTES — the documented default scope — admits
/// exactly the messages whose attributes it matches, and reads nothing from the body.
#[tokio::test]
async fn an_attribute_scoped_filter_policy_admits_exactly_the_attributes_it_matches() {
    // (case, policy, data type, value, delivered)
    let rows: &[AttributeCase<'_>] = &[
        (
            "an exact string attribute",
            r#"{"kind":["wanted"]}"#,
            "String",
            "wanted",
            true,
        ),
        (
            "an exact string attribute of another value",
            r#"{"kind":["wanted"]}"#,
            "String",
            "unwanted",
            false,
        ),
        (
            "a number attribute matched as a number",
            r#"{"kind":[10]}"#,
            "Number",
            "10",
            true,
        ),
        (
            "a number attribute in a numeric range",
            r#"{"kind":[{"numeric":[">=",10,"<",20]}]}"#,
            "Number",
            "10",
            true,
        ),
        (
            "a number attribute outside a numeric range",
            r#"{"kind":[{"numeric":[">=",10,"<",20]}]}"#,
            "Number",
            "20",
            false,
        ),
        (
            "a prefix of a string attribute",
            r#"{"kind":[{"prefix":"wan"}]}"#,
            "String",
            "wanted",
            true,
        ),
        (
            "an element of a String.Array attribute",
            r#"{"kind":["blue"]}"#,
            "String.Array",
            r#"["red","blue"]"#,
            true,
        ),
        (
            "no element of a String.Array attribute",
            r#"{"kind":["green"]}"#,
            "String.Array",
            r#"["red","blue"]"#,
            false,
        ),
        (
            "an attribute required to exist",
            r#"{"kind":[{"exists":true}]}"#,
            "String",
            "anything",
            true,
        ),
        (
            "an attribute the policy does not name",
            r#"{"other":[{"exists":true}]}"#,
            "String",
            "anything",
            false,
        ),
        (
            "anything but one value",
            r#"{"kind":[{"anything-but":"unwanted"}]}"#,
            "String",
            "wanted",
            true,
        ),
    ];

    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "attribute-filtered")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "attribute-filtered-queue").await;
    let filtered = subscribe_with(&emulator, &topic, &filtered_arn, &[]).await;

    for (case, policy, data_type, value, delivered) in rows {
        sns(
            &emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &filtered),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", policy),
            ],
        )
        .await;
        // The body carries the OPPOSITE of what the policy asks for, so a delivery can
        // only be the attribute scope's doing: the default scope reads the attributes.
        let body = json!({"kind": "unwanted"}).to_string();
        sns(
            &emulator,
            "Publish",
            &[
                ("TopicArn", &topic),
                ("Message", &body),
                ("MessageAttributes.entry.1.Name", "kind"),
                ("MessageAttributes.entry.1.Value.DataType", data_type),
                ("MessageAttributes.entry.1.Value.StringValue", value),
            ],
        )
        .await;

        let received = drain(&emulator, &filtered_url).await;
        assert_eq!(
            received.len(),
            usize::from(*delivered),
            "case {case:?}: expected {} delivered message(s), got {received:?}",
            usize::from(*delivered)
        );
    }
}

/// A subscription that holds NO filter policy receives every message published to its
/// topic, and one whose policy is REMOVED receives every message again: the filtering is
/// the policy's, so removing it removes the filtering.
#[tokio::test]
async fn a_subscription_with_no_filter_policy_receives_every_message() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "unfiltered")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "unfiltered-queue").await;
    // The scope is given at `Subscribe`, with no policy beside it: it is held and hidden
    // until a policy is set, and governs the policy set below.
    let subscription = subscribe_with(
        &emulator,
        &topic,
        &arn,
        &[("FilterPolicyScope", "MessageBody")],
    )
    .await;

    // (case, the policy the subscription holds, how many of the two publishes arrive)
    let rows: &[(&str, &str, usize)] = &[
        ("no policy at all", "", 2),
        ("a policy admitting one of the two", r#"{"kind":["wanted"]}"#, 1),
        ("the policy removed again", "", 2),
    ];

    for (case, policy, expected) in rows {
        sns(
            &emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", policy),
            ],
        )
        .await;
        for body in [r#"{"kind":"wanted"}"#, r#"{"kind":"unwanted"}"#] {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", body)],
            )
            .await;
        }
        let received = drain(&emulator, &url).await;
        assert_eq!(
            received.len(),
            *expected,
            "case {case:?}: expected {expected} delivered message(s), got {received:?}"
        );
    }
}

/// A policy document the service cannot apply is never stored, so no subscription is left
/// filtering on one: the write is refused and the subscription keeps delivering under
/// whatever it held.
#[tokio::test]
async fn a_filter_policy_that_is_not_a_document_is_refused_rather_than_stored() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "refused-policy")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "refused-policy-queue").await;
    let subscription = subscribe_with(&emulator, &topic, &arn, &[]).await;

    for (case, document) in [
        ("a policy that is a list", r#"["wanted"]"#),
        ("a policy that is a string", r#""wanted""#),
        ("a policy that is not JSON", "kind=wanted"),
    ] {
        let (status, body) = sns_call(
            &emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &subscription),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", document),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the write to be refused, got {status} / {body}"
        );
        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", r#"{"kind":"wanted"}"#)],
        )
        .await;
        let received = drain(&emulator, &url).await;
        assert_eq!(
            received.len(),
            1,
            "case {case:?}: the refused write stored no policy, so the message is still \
             delivered, got {received:?}"
        );
    }
}

/// The policy and the message the matching reads are both caller-supplied, so neither a
/// malformed, a truncated, an oversized nor a deeply nested one may break the service:
/// an unusable document is refused when it is written, and one the matching cannot
/// satisfy withholds the message while the topic keeps serving every other subscriber.
#[tokio::test]
async fn a_malformed_truncated_or_oversized_policy_cannot_break_the_filtering() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "hostile-policy")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "hostile-policy-queue").await;
    let filtered = subscribe_with(
        &emulator,
        &topic,
        &filtered_arn,
        &[("FilterPolicyScope", "MessageBody")],
    )
    .await;
    let (twin_url, twin_arn) = given_queue(&emulator, "hostile-policy-twin").await;
    subscribe_with(&emulator, &topic, &twin_arn, &[]).await;

    // A policy nesting far deeper than any message body does, and one past the bytes one
    // subscription record may hold: both are built here rather than written out, because
    // what they test is the SIZE of the input and not its content.
    let deeply_nested = {
        let mut document = r#"["leaf"]"#.to_string();
        for depth in 0..64 {
            document = format!(r#"{{"level{depth}":{document}}}"#);
        }
        document
    };
    let oversized = format!(r#"{{"kind":["{}"]}}"#, "x".repeat(96 * 1024));

    // (case, the document, whether the write is accepted)
    let rows: &[(&str, &str, bool)] = &[
        ("a truncated document", r#"{"kind":["wanted""#, false),
        ("a document holding only an opening brace", "{", false),
        ("an empty array", "[]", false),
        ("a document past the per-record ceiling", oversized.as_str(), false),
        ("a document nesting deeper than any body", deeply_nested.as_str(), true),
    ];

    for (case, document, accepted) in rows {
        let (status, body) = sns_call(
            &emulator,
            "SetSubscriptionAttributes",
            &[
                ("SubscriptionArn", &filtered),
                ("AttributeName", "FilterPolicy"),
                ("AttributeValue", document),
            ],
        )
        .await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} from the write, got {status} / {body}"
        );

        // A body that is JSON, one that is truncated and one that is not JSON at all: the
        // publish answers for each, whatever the subscription holds.
        for message in [
            r#"{"kind":"wanted","level0":{"level1":"deep"}}"#,
            r#"{"kind":"wanted""#,
            "wanted",
        ] {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", message)],
            )
            .await;
        }
        let received = drain(&emulator, &filtered_url).await;
        if *accepted {
            assert!(
                received.is_empty(),
                "case {case:?}: no body satisfies this policy, so none is delivered, got \
                 {received:?}"
            );
        }
        // The unfiltered twin received all three either way: one subscription's policy
        // changes nothing about what the topic delivers to the others.
        let twin = drain(&emulator, &twin_url).await;
        assert_eq!(
            twin.len(),
            3,
            "case {case:?}: the unfiltered twin must receive every message, got {twin:?}"
        );
    }
}

/// The matching one request may spend is BOUNDED, and a subscription the budget runs out
/// on is withheld from rather than handed a message its filter was not applied to: a
/// policy and a message built to multiply the work out cannot stop the topic serving its
/// other subscribers.
#[tokio::test]
async fn the_matching_one_request_spends_is_bounded() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "budgeted")]).await,
        "TopicArn",
    );
    let (filtered_url, filtered_arn) = given_queue(&emulator, "budgeted-queue").await;
    let filtered = subscribe_with(&emulator, &topic, &filtered_arn, &[]).await;
    let (twin_url, twin_arn) = given_queue(&emulator, "budgeted-twin").await;
    subscribe_with(&emulator, &topic, &twin_arn, &[]).await;

    // Far more condition-versus-value comparisons than the request's matching budget
    // allows: the policy lists as many conditions as SNS's own filter-policy constraint
    // admits and the published attribute holds hundreds of values, so the product is past
    // the budget. The one condition the message WOULD satisfy is the LAST, and the
    // value satisfying it is the FIRST of the attribute's values — so a matcher that ran
    // out of budget withholds, while one that applied the whole policy would deliver.
    let wanted = "wanted";
    let mut conditions: Vec<String> = (0..MAX_FILTER_POLICY_COMBINATIONS - 1)
        .map(|index| format!(r#""v-{index}""#))
        .collect();
    conditions.push(format!(r#""{wanted}""#));
    let exhausting = format!(r#"{{"kind":[{}]}}"#, conditions.join(","));
    assert!(
        exhausting.len() < MAX_ATTRIBUTE_BYTES_PER_RECORD,
        "the exhausting policy must be a document the subscription may hold, got {} bytes",
        exhausting.len()
    );
    let (status, body) = sns_call(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &filtered),
            ("AttributeName", "FilterPolicy"),
            ("AttributeValue", &exhausting),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the policy is a document the attribute accepts, got {status} / {body}"
    );

    // The attribute's first value is the one the policy's last condition names, so the
    // only thing standing between this message and the queue is the budget.
    let mut values: Vec<String> = vec![format!(r#""{wanted}""#)];
    values.extend((0..512).map(|index| format!(r#""other-{index}""#)));
    let array = format!("[{}]", values.join(","));
    // The product the matcher would have to walk to reach the last condition — the
    // conditions listed times the values one property contributes — is past the budget,
    // which is what this case rests on.
    const { assert!(MAX_FILTER_POLICY_COMBINATIONS * 256 > MAX_FILTER_MATCH_COMPARISONS) };
    sns(
        &emulator,
        "Publish",
        &[
            ("TopicArn", &topic),
            ("Message", r#"{"kind":"wanted"}"#),
            ("MessageAttributes.entry.1.Name", "kind"),
            ("MessageAttributes.entry.1.Value.DataType", "String.Array"),
            ("MessageAttributes.entry.1.Value.StringValue", &array),
        ],
    )
    .await;

    let received = drain(&emulator, &filtered_url).await;
    assert!(
        received.is_empty(),
        "a filter the request could not finish applying must withhold the message rather \
         than deliver it, got {received:?}"
    );
    let twin = drain(&emulator, &twin_url).await;
    assert_eq!(
        twin.len(),
        1,
        "the topic must keep serving its unfiltered subscriber, got {twin:?}"
    );
}
