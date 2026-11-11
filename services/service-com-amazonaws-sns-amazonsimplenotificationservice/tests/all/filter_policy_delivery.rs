// SPDX-License-Identifier: CC0-1.0

//! A filter policy that matches an IP ADDRESS decides delivery by the NETWORK, not by the
//! spelling of the value.
//!
//! The RULE asserted here: "IP address matching" is one of the forms a subscription filter
//! policy matches a property with (Amazon SNS Developer Guide, "Amazon SNS subscription filter
//! policy") — a condition `{"cidr": "<network>/<prefix>"}` holds exactly when the property's
//! value names an address that lies in that network, under the general rule every condition
//! obeys: "FilterPolicy – The simple JSON object that lets your subscriber receive only a
//! subset of messages, rather than receiving every message published to the topic"
//! (`SubscribeInput$Attributes` doc), matched against the part of the message the
//! subscription's scope names.
//!
//! The rows differ along what the rule generalises over: the IP version of the address and of
//! the block, the prefix length, an address on each edge of the block and one past it, the
//! scope the policy is matched under, the exclusion of a network (`anything-but`), and the
//! values that name no address and the blocks that name no network. Every row is asserted
//! where a client observes the decision: on the subscribed queue.

use serde_json::{json, Value};

use crate::sqs_subscription_delivery::{drain, element, emulator, given_queue, sns, Emulator};

/// One row of the address-matching table: the policy, the scope it is matched under, the
/// message published, and whether the subscriber receives it.
struct Row<'a> {
    case: &'a str,
    /// The `FilterPolicyScope` the subscription is filtered under, or `None` to leave it unset
    /// — which is the documented `MessageAttributes` default.
    scope: Option<&'a str>,
    policy: Value,
    /// The value published under the property `source`, as a `String` message attribute and as
    /// the body property of the same name, so one row serves either scope.
    value: &'a str,
    /// Whether the subscribed queue must receive the published message.
    delivered: bool,
}

/// Publish a message carrying `value` as the `source` attribute and as the `source` property
/// of a JSON body, so the same row can be matched under either scope.
async fn publish(emulator: &Emulator, topic: &str, value: &str) {
    let body = json!({"source": value}).to_string();
    sns(
        emulator,
        "Publish",
        &[
            ("TopicArn", topic),
            ("Message", &body),
            ("MessageAttributes.entry.1.Name", "source"),
            ("MessageAttributes.entry.1.Value.DataType", "String"),
            ("MessageAttributes.entry.1.Value.StringValue", value),
        ],
    )
    .await;
}

/// A topic with one queue subscribed to it under `policy` (and `scope`, when set): the topic
/// ARN and the queue's url.
async fn given_filtered_subscription(
    emulator: &Emulator,
    name: &str,
    policy: &Value,
    scope: Option<&str>,
) -> (String, String) {
    let topic = element(
        &sns(emulator, "CreateTopic", &[("Name", name)]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(emulator, &format!("{name}-queue")).await;
    let policy = policy.to_string();
    let mut params: Vec<(&str, &str)> = vec![
        ("TopicArn", &topic),
        ("Protocol", "sqs"),
        ("Endpoint", &arn),
        ("ReturnSubscriptionArn", "true"),
        ("Attributes.entry.1.key", "FilterPolicy"),
        ("Attributes.entry.1.value", &policy),
    ];
    if let Some(scope) = scope {
        params.push(("Attributes.entry.2.key", "FilterPolicyScope"));
        params.push(("Attributes.entry.2.value", scope));
    }
    sns(emulator, "Subscribe", &params).await;
    (topic, url)
}

#[tokio::test]
async fn an_address_condition_delivers_exactly_the_addresses_inside_its_network() {
    let rows: &[Row<'_>] = &[
        Row {
            case: "an address inside the block",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0.7",
            delivered: true,
        },
        Row {
            case: "an address outside the block",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.1.7",
            delivered: false,
        },
        Row {
            case: "the first address of the block",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0.0",
            delivered: true,
        },
        Row {
            case: "the last address of the block",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0.255",
            delivered: true,
        },
        Row {
            case: "the address one past the block",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.1.0",
            delivered: false,
        },
        Row {
            case: "a single-address block matching its one address",
            scope: None,
            policy: json!({"source": [{"cidr": "192.168.4.9/32"}]}),
            value: "192.168.4.9",
            delivered: true,
        },
        Row {
            case: "a single-address block against its neighbour",
            scope: None,
            policy: json!({"source": [{"cidr": "192.168.4.9/32"}]}),
            value: "192.168.4.10",
            delivered: false,
        },
        Row {
            case: "a block covering every address",
            scope: None,
            policy: json!({"source": [{"cidr": "0.0.0.0/0"}]}),
            value: "203.0.113.5",
            delivered: true,
        },
        Row {
            case: "an IPv6 address inside an IPv6 block",
            scope: None,
            policy: json!({"source": [{"cidr": "2001:db8::/32"}]}),
            value: "2001:db8:1234::1",
            delivered: true,
        },
        Row {
            case: "an IPv6 address outside an IPv6 block",
            scope: None,
            policy: json!({"source": [{"cidr": "2001:db8::/32"}]}),
            value: "2001:dba::1",
            delivered: false,
        },
        Row {
            case: "an IPv4 address against an IPv6 block",
            scope: None,
            policy: json!({"source": [{"cidr": "2001:db8::/32"}]}),
            value: "10.0.0.7",
            delivered: false,
        },
        Row {
            case: "a value that names no address at all",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0.lan",
            delivered: false,
        },
        Row {
            // A block whose mask is wider than its family admits names no network, so a
            // policy carrying one is refused when it is written rather than stored to
            // match nothing — the write path's own rule
            // (`crate::state::filter_policy`'s `cidr` argument check), covered by
            // `validation_rules`.
            case: "a value that is not an address, under a block that is one",
            scope: None,
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0",
            delivered: false,
        },
        Row {
            case: "one block of several listed for the property",
            scope: None,
            policy: json!({"source": [{"cidr": "198.51.100.0/24"}, {"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0.7",
            delivered: true,
        },
        Row {
            case: "neither block of several listed for the property",
            scope: None,
            policy: json!({"source": [{"cidr": "198.51.100.0/24"}, {"cidr": "10.0.0.0/24"}]}),
            value: "203.0.113.5",
            delivered: false,
        },
        Row {
            case: "an address the policy excludes by its network",
            scope: None,
            policy: json!({"source": [{"anything-but": {"cidr": "10.0.0.0/24"}}]}),
            value: "10.0.0.7",
            delivered: false,
        },
        Row {
            case: "an address outside the network the policy excludes",
            scope: None,
            policy: json!({"source": [{"anything-but": {"cidr": "10.0.0.0/24"}}]}),
            value: "203.0.113.5",
            delivered: true,
        },
        Row {
            case: "an address inside the block, matched on the message body",
            scope: Some("MessageBody"),
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.0.7",
            delivered: true,
        },
        Row {
            case: "an address outside the block, matched on the message body",
            scope: Some("MessageBody"),
            policy: json!({"source": [{"cidr": "10.0.0.0/24"}]}),
            value: "10.0.1.7",
            delivered: false,
        },
    ];

    let emulator = emulator();
    for (index, row) in rows.iter().enumerate() {
        let (topic, url) = given_filtered_subscription(
            &emulator,
            &format!("addressed-{index}"),
            &row.policy,
            row.scope,
        )
        .await;
        publish(&emulator, &topic, row.value).await;
        let delivered = drain(&emulator, &url).await;
        let expected = usize::from(row.delivered);
        assert_eq!(
            delivered.len(),
            expected,
            "case {:?}: expected {expected} delivered message(s) under policy {} for value \
             {:?}, got {:?}",
            row.case,
            row.policy,
            row.value,
            delivered
        );
    }
}

#[tokio::test]
async fn an_address_condition_is_applied_to_every_entry_of_a_batch() {
    let emulator = emulator();
    let (topic, url) = given_filtered_subscription(
        &emulator,
        "addressed-batch",
        &json!({"source": [{"cidr": "10.0.0.0/24"}]}),
        None,
    )
    .await;

    // Rows differ in how many entries of the batch carry an address inside the network: each
    // entry is a message of its own, so the subscriber receives exactly those.
    let rows: &[(&str, &[&str], usize)] = &[
        ("no entry inside the network", &["10.0.1.1", "10.0.2.2"], 0),
        ("one entry of two", &["10.0.0.1", "10.0.1.1"], 1),
        ("every entry", &["10.0.0.1", "10.0.0.2", "10.0.0.3"], 3),
    ];

    for (case, addresses, expected) in rows {
        let mut params: Vec<(String, String)> = vec![("TopicArn".to_string(), topic.clone())];
        for (index, address) in addresses.iter().enumerate() {
            let prefix = format!("PublishBatchRequestEntries.member.{}", index + 1);
            params.push((format!("{prefix}.Id"), format!("e{index}")));
            params.push((format!("{prefix}.Message"), format!("from {address}")));
            params.push((
                format!("{prefix}.MessageAttributes.entry.1.Name"),
                "source".to_string(),
            ));
            params.push((
                format!("{prefix}.MessageAttributes.entry.1.Value.DataType"),
                "String".to_string(),
            ));
            params.push((
                format!("{prefix}.MessageAttributes.entry.1.Value.StringValue"),
                (*address).to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        sns(&emulator, "PublishBatch", &borrowed).await;

        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            *expected,
            "case {case:?}: expected {expected} delivered message(s), got {delivered:?}"
        );
    }
}
