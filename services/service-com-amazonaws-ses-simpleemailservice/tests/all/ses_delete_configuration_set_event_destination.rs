// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfigurationSetEventDestination`, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them.
//!
//! What is asserted is the RULE the delete follows over a SET of inputs:
//!
//! * a delete takes the destination its name keys OFF the set and leaves every
//!   other destination of that set where it was — for each destination class,
//!   each form of name the rule admits, and whichever place in the set the named
//!   destination holds;
//! * the name is freed, so a create may record a destination of that name again;
//! * a name the set holds no destination of is refused as not existing, which is
//!   also what a second delete of one destination gets, and changes nothing;
//! * a delete naming a configuration set the account does not hold is refused,
//!   whatever the name, and whatever destinations another set holds;
//! * one set's destination is taken off without touching another set's
//!   destination of the same name.

use crate::harness::{
    account, call, create_event_destination, described_event_destinations, element, elements,
    emulator, field, topic_arn, xml_text, Destination, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// `DeleteConfigurationSetEventDestination`.
async fn delete_event_destination(
    router: &MultiServiceRouter,
    configuration_set: &str,
    name: &str,
) -> (u16, String) {
    call(
        router,
        "DeleteConfigurationSetEventDestination",
        &[
            field("ConfigurationSetName", configuration_set),
            field("EventDestinationName", name),
        ],
    )
    .await
}

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
/// about the DELETE starts from a destination there is one of.
async fn given_event_destination(
    router: &MultiServiceRouter,
    set: &str,
    name: &str,
    destination: &Destination,
) {
    let (status, body) =
        create_event_destination(router, set, name, true, &["send"], destination).await;
    assert_eq!(
        status, 200,
        "case create destination {name} on {set}: expected 200, got {status} with {body}"
    );
}

/// The names `DescribeConfigurationSet` reports for `set`'s event destinations,
/// in the order it reports them.
///
/// `<Name>` is the `EventDestination` member, and the only element of that name
/// a destination carries: a `CloudWatchDestination`'s dimensions are reported as
/// `<DimensionName>`.
async fn destination_names(router: &MultiServiceRouter, set: &str) -> Vec<String> {
    let reported = described_event_destinations(router, set).await;
    elements(&reported, "Name")
        .iter()
        .map(|name| xml_text(name))
        .collect()
}

/// One row of the table the delete rule is asserted over: `(what the case is,
/// the destinations the set holds as `(Name, what it publishes to)`, the place
/// in that list the delete names)`.
type Case = (&'static str, Vec<(String, Destination)>, usize);

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// The `KinesisFirehoseDestination` of this account in [`REGION`]:
/// `(IAMRoleARN, DeliveryStreamARN)`, each in its published ARN form.
fn firehose(stream: &str) -> Destination {
    let account = account();
    Destination::Firehose(
        format!("arn:aws:iam::{account}:role/ses-events"),
        format!("arn:aws:firehose:{REGION}:{account}:deliverystream/{stream}"),
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

/// The value that identifies `destination` in a reported `<EventDestinations>`
/// element: one member no other destination of a case shares, so a report can be
/// asked whether that destination is still described there.
fn marker(destination: &Destination) -> String {
    match destination {
        Destination::Sns(topic_arn) => topic_arn.clone(),
        Destination::Firehose(_, stream_arn) => stream_arn.clone(),
        Destination::CloudWatch(dimensions) => dimensions
            .first()
            .map(|(name, _, _)| name.clone())
            .unwrap_or_default(),
        other => panic!("case {other:?}: this suite stores only describable destinations"),
    }
}

#[tokio::test]
async fn a_delete_takes_off_the_destination_its_name_keys_and_leaves_the_rest() {
    let router = emulator();

    // One row per form the rule spans: each destination class, the name forms
    // the rule admits (short, mixed case with both separators, and the longest a
    // name may be), and every place the deleted destination can hold in its set
    // — the only one, the first, the middle and the last of several.
    let cases: Vec<Case> = vec![
        (
            "the only destination of a set, an SNS one",
            vec![("dest".to_string(), Destination::Sns(topic_arn("first")))],
            0,
        ),
        (
            "the only destination of a set, a Firehose one under a mixed-case name",
            vec![("Dest_2-x".to_string(), firehose("ses-events"))],
            0,
        ),
        (
            "the only destination of a set, a CloudWatch one under the longest name",
            vec![("d".repeat(64), cloud_watch("ses:configuration-set"))],
            0,
        ),
        (
            "the first of three",
            vec![
                ("first".to_string(), Destination::Sns(topic_arn("first"))),
                ("second".to_string(), firehose("second-events")),
                ("third".to_string(), cloud_watch("campaign")),
            ],
            0,
        ),
        (
            "the middle of three",
            vec![
                ("first".to_string(), Destination::Sns(topic_arn("first"))),
                ("second".to_string(), firehose("second-events")),
                ("third".to_string(), cloud_watch("campaign")),
            ],
            1,
        ),
        (
            "the last of three",
            vec![
                ("first".to_string(), Destination::Sns(topic_arn("first"))),
                ("second".to_string(), firehose("second-events")),
                ("third".to_string(), cloud_watch("campaign")),
            ],
            2,
        ),
        (
            "one of two destinations of the same class",
            vec![
                ("Alpha-1".to_string(), Destination::Sns(topic_arn("alpha"))),
                ("beta_2".to_string(), Destination::Sns(topic_arn("beta"))),
            ],
            1,
        ),
    ];

    for (index, (case, held, at)) in cases.iter().enumerate() {
        // One set per row, so each row is read back on its own.
        let set = format!("set-{index}");
        given_configuration_set(&router, &set).await;
        for (name, destination) in held {
            given_event_destination(&router, &set, name, destination).await;
        }
        let (name, destination) = &held[*at];

        let (status, body) = delete_event_destination(&router, &set, name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );

        // Every other destination of the set is still there, in the order it was
        // reported in before, and the one named is described no more.
        let remaining: Vec<String> = held
            .iter()
            .enumerate()
            .filter(|(place, _)| place != at)
            .map(|(_, (held_name, _))| held_name.clone())
            .collect();
        let reported = described_event_destinations(&router, &set).await;
        let names: Vec<String> = elements(&reported, "Name")
            .iter()
            .map(|held_name| xml_text(held_name))
            .collect();
        assert_eq!(
            names, remaining,
            "case {case}: expected {remaining:?} to remain, got {reported}"
        );
        assert!(
            !reported.contains(&marker(destination)),
            "case {case}: expected nothing of the deleted destination, got {reported}"
        );
        for (held_name, held_destination) in held.iter().enumerate().filter_map(|(place, held)| {
            (place != *at).then_some((held.0.clone(), held.1.clone()))
        }) {
            assert!(
                reported.contains(&marker(&held_destination)),
                "case {case}: expected {held_name} to publish to {held_destination:?} still, \
                 got {reported}"
            );
        }

        // The destination is GONE rather than emptied, so deleting it a second
        // time is refused as not existing.
        let (status, body) = delete_event_destination(&router, &set, name).await;
        assert_eq!(
            status, 400,
            "case {case} deleted twice: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("EventDestinationDoesNotExist".to_string()),
                Some(format!("No EventDestination found for {set}"))
            ),
            "case {case} deleted twice: expected the not-found refusal, got {body}"
        );

        // And the name is free again: a create records a destination of it.
        let (status, body) =
            create_event_destination(&router, &set, name, true, &["bounce"], destination).await;
        assert_eq!(
            status, 200,
            "case {case} re-created: expected 200, got {status} with {body}"
        );
        let mut expected = remaining.clone();
        expected.push(name.clone());
        let names = destination_names(&router, &set).await;
        assert_eq!(
            names, expected,
            "case {case} re-created: expected {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_of_a_name_the_set_holds_no_destination_of_is_refused() {
    let router = emulator();
    given_configuration_set(&router, "set").await;
    given_event_destination(&router, "set", "held", &Destination::Sns(topic_arn("held"))).await;

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
        let (status, body) = delete_event_destination(&router, "set", &name).await;
        assert_eq!(
            status, 400,
            "case delete absent {name:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("EventDestinationDoesNotExist".to_string()),
                Some("No EventDestination found for set".to_string())
            ),
            "case delete absent {name:?}: expected the not-found refusal, got {body}"
        );
    }

    // No refused delete took anything off the set.
    let names = destination_names(&router, "set").await;
    assert_eq!(
        names,
        vec!["held".to_string()],
        "case after the refusals: expected the set to hold its destination, got {names:?}"
    );
}

#[tokio::test]
async fn a_delete_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    // A set the account DOES hold, carrying a destination of the name each
    // refused call names: the set is what the refusal is about, so holding that
    // name elsewhere changes nothing.
    given_configuration_set(&router, "set").await;
    given_event_destination(&router, "set", "dest", &Destination::Sns(topic_arn("dest"))).await;

    // The names span everything the set-naming rule admits, and none of these
    // sets was ever created.
    let cases = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for set in cases {
        let (status, body) = delete_event_destination(&router, &set, "dest").await;
        assert_eq!(
            status, 400,
            "case delete on absent {set:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{set}> does not exist."))
            ),
            "case delete on absent {set:?}: expected the not-found refusal, got {body}"
        );
    }

    let names = destination_names(&router, "set").await;
    assert_eq!(
        names,
        vec!["dest".to_string()],
        "case after the refusals: expected the held set unchanged, got {names:?}"
    );
}

#[tokio::test]
async fn a_delete_touches_no_other_sets_destination_of_that_name() {
    let router = emulator();
    let sets = ["set", "other-set"];
    let names = ["first", "second", "third"];
    for set in sets {
        given_configuration_set(&router, set).await;
        for name in names {
            given_event_destination(&router, set, name, &Destination::Sns(topic_arn(name))).await;
        }
    }

    // Each destination of one set, taken off in turn: the same name in the other
    // set stays, and so does every destination of the set that is not named.
    for (index, name) in names.iter().enumerate() {
        let (status, body) = delete_event_destination(&router, "set", name).await;
        assert_eq!(
            status, 200,
            "case delete {name}: expected 200, got {status} with {body}"
        );

        let expected: Vec<String> = names[index + 1..].iter().map(|&held| held.into()).collect();
        let held = destination_names(&router, "set").await;
        assert_eq!(
            held, expected,
            "case delete {name}: expected {expected:?}, got {held:?}"
        );
        let untouched = destination_names(&router, "other-set").await;
        assert_eq!(
            untouched,
            names.map(String::from).to_vec(),
            "case delete {name}: expected the other set unchanged, got {untouched:?}"
        );
    }
}
