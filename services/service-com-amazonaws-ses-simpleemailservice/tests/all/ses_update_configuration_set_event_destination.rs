// SPDX-License-Identifier: CC0-1.0

//! `UpdateConfigurationSetEventDestination`, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them.
//!
//! What is asserted is the RULE the update follows over a SET of inputs:
//!
//! * an update REPLACES the destination its name keys with the one destination
//!   the request provides — every member of the stored destination comes from
//!   that request — for every pair of destination classes, the `Enabled` switch
//!   both ways, and event-type lists from one entry to every one the model
//!   names;
//! * the place the destination holds in the set is kept, so the order a reader
//!   reports the set's destinations in is unchanged;
//! * a name the set holds no destination of is refused as not existing, rather
//!   than recorded, and changes nothing;
//! * an update naming a configuration set the account does not hold is refused,
//!   whatever the name;
//! * a destination that describes nothing Amazon SES could publish to is refused
//!   with that destination class's own error, before the set is looked up at all,
//!   and the stored destination stands;
//! * one destination of a set is replaced without touching the set's others, or
//!   another set's.

use crate::harness::{
    account, call, create_event_destination, described_event_destinations, element, elements,
    emulator, field, topic_arn, update_event_destination, xml_text, Destination, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// Every event type `EventDestination$MatchingEventTypes` names, in the order
/// the member's documentation lists them.
const EVERY_EVENT_TYPE: [&str; 8] = [
    "send",
    "reject",
    "bounce",
    "complaint",
    "delivery",
    "open",
    "click",
    "renderingFailure",
];

/// One event destination as a request writes it: `(Enabled,
/// MatchingEventTypes, the destination it publishes to)`.
type Written = (bool, Vec<&'static str>, Destination);

/// One event destination as `DescribeConfigurationSet` reports it: `(Name,
/// Enabled, MatchingEventTypes, the destination it publishes to)`.
type Reported = (String, bool, Vec<String>, Option<Destination>);

/// A set the account holds, so a case about the DESTINATION starts from one.
async fn given_configuration_set(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case create set {name}: expected 200, got {status} with {body}"
    );
}

/// A set holding the destination `name` publishes to `destination`, so a case
/// about the UPDATE starts from a destination there is one of.
async fn given_event_destination(
    router: &MultiServiceRouter,
    set: &str,
    name: &str,
    enabled: bool,
    event_types: &[&str],
    destination: &Destination,
) {
    let (status, body) =
        create_event_destination(router, set, name, enabled, event_types, destination).await;
    assert_eq!(
        status, 200,
        "case create destination {name} on {set}: expected 200, got {status} with {body}"
    );
}

/// The destination the reported `<EventDestination>` element publishes to, read
/// back out of the XML as the class and members it names.
fn reported_destination(reported: &str) -> Option<Destination> {
    if let Some(topic) = element(reported, "TopicARN") {
        return Some(Destination::Sns(xml_text(&topic)));
    }
    if let (Some(role), Some(stream)) = (
        element(reported, "IAMRoleARN"),
        element(reported, "DeliveryStreamARN"),
    ) {
        return Some(Destination::Firehose(xml_text(&role), xml_text(&stream)));
    }
    let dimensions: Vec<(String, String, String)> = elements(reported, "DimensionName")
        .into_iter()
        .zip(elements(reported, "DimensionValueSource"))
        .zip(elements(reported, "DefaultDimensionValue"))
        .map(|((name, source), default)| (xml_text(&name), xml_text(&source), xml_text(&default)))
        .collect();
    (!dimensions.is_empty()).then_some(Destination::CloudWatch(dimensions))
}

/// The reported `<EventDestinations>` element split into one chunk per
/// destination it carries, in the order it reports them.
///
/// Split on `<Name>`, which is the first member the codec writes for an
/// `EventDestination` and the only member of that name: the shared `elements`
/// reader is flat, so it cannot be asked for a `<member>` whose own body
/// carries the `<member>` entries of `MatchingEventTypes`.
fn destination_chunks(reported: &str) -> Vec<String> {
    let opening = "<Name";
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(at) = reported[from..].find(opening) {
        let at = from + at;
        let after = at + opening.len();
        if reported[after..]
            .chars()
            .next()
            .is_some_and(|next| next == '>' || next.is_whitespace())
        {
            starts.push(at);
        }
        from = after;
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(reported.len());
            reported[*start..end].to_string()
        })
        .collect()
}

/// What `DescribeConfigurationSet` reports for each destination of `set`, as
/// `(Name, Enabled, MatchingEventTypes, the destination it publishes to)`.
async fn stored_event_destinations(
    router: &MultiServiceRouter,
    set: &str,
) -> Vec<Reported> {
    let reported = described_event_destinations(router, set).await;
    destination_chunks(&reported)
        .into_iter()
        .map(|destination| {
            (
                element(&destination, "Name").unwrap_or_default(),
                element(&destination, "Enabled").as_deref() == Some("true"),
                element(&destination, "MatchingEventTypes")
                    .map(|types| elements(&types, "member"))
                    .unwrap_or_default(),
                reported_destination(&destination),
            )
        })
        .collect()
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// The error code alone, for a case whose message carries the value it is about.
fn code(body: &str) -> Option<String> {
    element(body, "Code").map(|text| xml_text(&text))
}

/// The `KinesisFirehoseDestination` of this account in [`REGION`]:
/// `(IAMRoleARN, DeliveryStreamARN)`, each in its published ARN form.
fn firehose() -> Destination {
    let account = account();
    Destination::Firehose(
        format!("arn:aws:iam::{account}:role/ses-events"),
        format!("arn:aws:firehose:{REGION}:{account}:deliverystream/ses-events"),
    )
}

/// A `CloudWatchDestination` categorizing on `dimension`, with the other two
/// members in forms their own rules admit.
fn cloud_watch(dimension: &str) -> Destination {
    Destination::CloudWatch(vec![(
        dimension.to_string(),
        "messageTag".to_string(),
        "none".to_string(),
    )])
}

#[tokio::test]
async fn an_update_replaces_the_destination_its_name_keys() {
    let router = emulator();

    // One row per pair of forms the rule spans: each destination class replaced
    // by each other one and by its own class carrying other members, the
    // `Enabled` switch both ways, and event types from one to every one the
    // model names.
    let every_event_type: Vec<&str> = EVERY_EVENT_TYPE.to_vec();
    let cases: Vec<(&str, Written, Written)> = vec![
        (
            "sns-to-sns",
            (true, vec!["send"], Destination::Sns(topic_arn("first"))),
            (
                false,
                vec!["bounce", "complaint"],
                Destination::Sns(topic_arn("second")),
            ),
        ),
        (
            "sns-to-firehose",
            (false, vec!["send"], Destination::Sns(topic_arn("first"))),
            (true, every_event_type.clone(), firehose()),
        ),
        (
            "firehose-to-cloudwatch",
            (true, vec!["delivery", "open"], firehose()),
            (
                true,
                vec!["click"],
                Destination::CloudWatch(vec![
                    (
                        "campaign".to_string(),
                        "messageTag".to_string(),
                        "none".to_string(),
                    ),
                    (
                        "ses:configuration-set".to_string(),
                        "emailHeader".to_string(),
                        "ops@example.com".to_string(),
                    ),
                ]),
            ),
        ),
        (
            "cloudwatch-to-cloudwatch",
            (
                false,
                vec!["renderingFailure"],
                cloud_watch("ses:configuration-set"),
            ),
            (
                true,
                vec!["send", "reject"],
                Destination::CloudWatch(vec![(
                    "link_tag-9".to_string(),
                    "linkTag".to_string(),
                    "v1.0".to_string(),
                )]),
            ),
        ),
        (
            "cloudwatch-to-sns",
            (true, every_event_type.clone(), cloud_watch("campaign")),
            (false, vec!["reject"], Destination::Sns(topic_arn("third"))),
        ),
        (
            "firehose-to-firehose-with-a-pathed-role",
            (false, vec!["bounce"], firehose()),
            (
                true,
                vec!["bounce", "delivery"],
                Destination::Firehose(
                    format!("arn:aws:iam::{}:role/service-role/ses-events", account()),
                    format!(
                        "arn:aws:firehose:{REGION}:{}:deliverystream/other-events",
                        account()
                    ),
                ),
            ),
        ),
    ];

    for (index, (case, (was_enabled, was_types, was_destination), (enabled, types, destination))) in
        cases.iter().enumerate()
    {
        // One set per row, so each row is read back on its own. The destination
        // name is the key the update is keyed on, so it spans the name rule too:
        // short, mixed case with both separators, and the longest a name may be.
        let set = format!("set-{index}");
        let name = match index % 3 {
            0 => "dest".to_string(),
            1 => "Dest_2-x".to_string(),
            _ => "d".repeat(64),
        };
        given_configuration_set(&router, &set).await;
        given_event_destination(
            &router,
            &set,
            &name,
            *was_enabled,
            was_types,
            was_destination,
        )
        .await;

        let (status, body) =
            update_event_destination(&router, &set, &name, *enabled, types, destination).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );

        let stored = stored_event_destinations(&router, &set).await;
        let expected = vec![(
            name.clone(),
            *enabled,
            types.iter().map(|entry| (*entry).to_string()).collect(),
            Some(destination.clone()),
        )];
        assert_eq!(
            stored, expected,
            "case {case}: expected the replacement {expected:?}, got {stored:?}"
        );
    }

    // An ARN names its resource by its trimmed value, which is what the update
    // reads and asks Amazon SNS about — so that is also what the set stores and
    // a reader reports, for every ARN member of either class.
    let topic = topic_arn("events-topic");
    let Destination::Firehose(role, stream) = firehose() else {
        panic!("case padded ARNs: the Firehose fixture is a Firehose destination");
    };
    given_configuration_set(&router, "set-padded").await;
    given_event_destination(
        &router,
        "set-padded",
        "padded",
        true,
        &["send"],
        &Destination::Sns(topic.clone()),
    )
    .await;
    let (status, body) = update_event_destination(
        &router,
        "set-padded",
        "padded",
        true,
        &["send"],
        &Destination::Firehose(format!(" {role} "), format!("\t{stream}\n")),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a padded stream and role ARN: expected 200, got {status} with {body}"
    );
    let stored = stored_event_destinations(&router, "set-padded").await;
    assert_eq!(
        stored.first().map(|held| held.3.clone()),
        Some(Some(Destination::Firehose(role, stream))),
        "case padded ARNs: expected each ARN stored as the resource it names, got {stored:?}"
    );
}

#[tokio::test]
async fn an_update_keeps_the_place_the_destination_holds_and_touches_no_other() {
    let router = emulator();
    given_configuration_set(&router, "set").await;
    given_configuration_set(&router, "other-set").await;
    let names = ["first", "second", "third"];
    for name in names {
        given_event_destination(
            &router,
            "set",
            name,
            true,
            &["send"],
            &Destination::Sns(topic_arn(name)),
        )
        .await;
        given_event_destination(
            &router,
            "other-set",
            name,
            true,
            &["send"],
            &Destination::Sns(topic_arn(name)),
        )
        .await;
    }

    // Each destination of the set, replaced in turn: the one named is the only
    // one that changes, it keeps the place it held, and the same name in another
    // set is untouched.
    for (index, name) in names.iter().enumerate() {
        let (status, body) = update_event_destination(
            &router,
            "set",
            name,
            false,
            &["bounce"],
            &cloud_watch("campaign"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case update {name}: expected 200, got {status} with {body}"
        );

        let stored = stored_event_destinations(&router, "set").await;
        assert_eq!(
            stored.iter().map(|held| held.0.clone()).collect::<Vec<_>>(),
            names.map(String::from).to_vec(),
            "case update {name}: the publish order is unchanged, got {stored:?}"
        );
        for (position, other) in names.iter().enumerate() {
            let replaced = position <= index;
            let expected = (
                (*other).to_string(),
                !replaced,
                vec![if replaced { "bounce" } else { "send" }.to_string()],
                Some(if replaced {
                    cloud_watch("campaign")
                } else {
                    Destination::Sns(topic_arn(other))
                }),
            );
            assert_eq!(
                stored[position], expected,
                "case update {name}: expected {other} to read back as {expected:?}, \
                 got {:?}",
                stored[position]
            );
        }
    }

    let untouched = stored_event_destinations(&router, "other-set").await;
    let expected: Vec<_> = names
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                true,
                vec!["send".to_string()],
                Some(Destination::Sns(topic_arn(name))),
            )
        })
        .collect();
    assert_eq!(
        untouched, expected,
        "case another set: expected {expected:?}, got {untouched:?}"
    );
}

#[tokio::test]
async fn an_update_of_a_name_the_set_holds_no_destination_of_is_refused() {
    let router = emulator();
    given_configuration_set(&router, "set").await;
    given_event_destination(
        &router,
        "set",
        "held",
        true,
        &["send"],
        &Destination::Sns(topic_arn("events-topic")),
    )
    .await;

    // The names span everything the name rule admits, and the set holds a
    // destination of none of them — including one differing from the name it
    // does hold only in letter case, which is a different key.
    let cases = [
        "absent".to_string(),
        "Held".to_string(),
        "Absent_Dest-2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for name in cases {
        let (status, body) = update_event_destination(
            &router,
            "set",
            &name,
            false,
            &["bounce"],
            &cloud_watch("campaign"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case update absent {name:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("EventDestinationDoesNotExist".to_string()),
                Some("No EventDestination found for set".to_string())
            ),
            "case update absent {name:?}: expected the not-found refusal, got {body}"
        );
    }

    // No refused update recorded a destination, and the one the set holds stands.
    let stored = stored_event_destinations(&router, "set").await;
    let expected = vec![(
        "held".to_string(),
        true,
        vec!["send".to_string()],
        Some(Destination::Sns(topic_arn("events-topic"))),
    )];
    assert_eq!(
        stored, expected,
        "case after the refusals: expected {expected:?}, got {stored:?}"
    );
}

#[tokio::test]
async fn an_update_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    // The names span everything the set-naming rule admits, and none of these
    // sets was ever created.
    let cases = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for set in cases {
        let (status, body) = update_event_destination(
            &router,
            &set,
            "dest",
            true,
            &["send"],
            &Destination::Sns(topic_arn("events-topic")),
        )
        .await;
        assert_eq!(
            status, 400,
            "case update on absent {set:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{set}> does not exist."))
            ),
            "case update on absent {set:?}: expected the not-found refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn an_update_to_a_destination_ses_could_not_publish_to_is_refused() {
    let router = emulator();
    given_configuration_set(&router, "set").await;
    let stored_topic = topic_arn("events-topic");
    given_event_destination(
        &router,
        "set",
        "dest",
        true,
        &["send"],
        &Destination::Sns(stored_topic.clone()),
    )
    .await;
    let account = account();
    let other_account = "210987654321";
    let stream = format!("arn:aws:firehose:{REGION}:{account}:deliverystream/ses-events");
    let role = format!("arn:aws:iam::{account}:role/ses-events");

    // (what is wrong with it, the destination, the error code it is refused with)
    let cases: Vec<(&str, Destination, &str)> = vec![
        (
            "a topic of another account",
            Destination::Sns(format!("arn:aws:sns:{REGION}:{other_account}:events-topic")),
            "InvalidSNSDestination",
        ),
        (
            "a topic of another Region",
            Destination::Sns(format!("arn:aws:sns:eu-west-1:{account}:events-topic")),
            "InvalidSNSDestination",
        ),
        (
            "a topic name outside the form Amazon SNS mints",
            Destination::Sns(format!("arn:aws:sns:{REGION}:{account}:events topic")),
            "InvalidSNSDestination",
        ),
        (
            "a delivery stream of another account",
            Destination::Firehose(
                role.clone(),
                format!("arn:aws:firehose:{REGION}:{other_account}:deliverystream/ses-events"),
            ),
            "InvalidFirehoseDestination",
        ),
        (
            "an IAM user rather than a role",
            Destination::Firehose(format!("arn:aws:iam::{account}:user/ops"), stream.clone()),
            "InvalidFirehoseDestination",
        ),
        (
            "a dimension name carrying a character its rule excludes",
            cloud_watch("camp aign"),
            "InvalidCloudWatchDestination",
        ),
        (
            "a dimension name past its documented length",
            cloud_watch(&"a".repeat(257)),
            "InvalidCloudWatchDestination",
        ),
        // On the awsQuery wire an empty dimension list carries no field at all,
        // so such a request provides no destination — which the
        // one-and-only-one rule is what refuses.
        (
            "no destination at all",
            Destination::None,
            "InvalidParameterValue",
        ),
        (
            "more than one destination",
            Destination::SnsAndCloudWatch(stored_topic.clone()),
            "InvalidParameterValue",
        ),
    ];

    for (case, destination, expected) in cases {
        let (status, body) =
            update_event_destination(&router, "set", "dest", true, &["send"], &destination).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            code(&body).as_deref(),
            Some(expected),
            "case {case}: expected the {expected} refusal, got {body}"
        );
        // The refusal is read off the request, so it is the same for a set, and
        // a destination name, the account does not hold — the destination is
        // refused before either is looked up at all.
        let (status, absent_body) =
            update_event_destination(&router, "absent-set", "absent", true, &["send"], &destination)
                .await;
        assert_eq!(
            status, 400,
            "case {case} on an absent set: expected 400, got {status} with {absent_body}"
        );
        assert_eq!(
            code(&absent_body).as_deref(),
            Some(expected),
            "case {case} on an absent set: expected the {expected} refusal, got {absent_body}"
        );
    }

    // Nothing any of those refusals describes replaced the stored destination,
    // and a destination that IS one Amazon SES could publish to still replaces
    // it afterwards.
    let stored = stored_event_destinations(&router, "set").await;
    let expected = vec![(
        "dest".to_string(),
        true,
        vec!["send".to_string()],
        Some(Destination::Sns(stored_topic.clone())),
    )];
    assert_eq!(
        stored, expected,
        "case after the refusals: expected {expected:?}, got {stored:?}"
    );
    let (status, body) =
        update_event_destination(&router, "set", "dest", false, &["open"], &firehose()).await;
    assert_eq!(
        status, 200,
        "case update after the refusals: expected 200, got {status} with {body}"
    );
    let stored = stored_event_destinations(&router, "set").await;
    let expected = vec![(
        "dest".to_string(),
        false,
        vec!["open".to_string()],
        Some(firehose()),
    )];
    assert_eq!(
        stored, expected,
        "case update after the refusals: expected {expected:?}, got {stored:?}"
    );
}
