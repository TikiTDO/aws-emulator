// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS topic an event destination publishes to, resolved against Amazon SNS
//! before the destination is stored — end to end over the wire, with Amazon SNS mounted
//! beside this service on ONE engine exactly as the assembled router mounts them.
//!
//! THE RULE: an event destination names "the Amazon Web Services service to which Amazon SES
//! publishes the email sending events associated with a configuration set", so the topic an
//! `SNSDestination` names is Amazon SNS's resource and Amazon SNS is the one answer to
//! whether it is there. A write whose topic Amazon SNS holds nothing under is refused — HTTP
//! 400, `InvalidSNSDestination`, `"SNS topic <"` + the `TopicARN` the request carried +
//! `"> not found."` — and NOTHING is stored: the set's event-destination list is left exactly
//! as it was. The same write becomes legal the moment the topic exists.
//!
//! Both writers of a v1 event destination carry the same `EventDestination` shape and so the
//! same rule, so every row below is driven through `CreateConfigurationSetEventDestination`
//! AND through `UpdateConfigurationSetEventDestination`.
//!
//! The table spans what the rule generalises over: whether Amazon SNS holds the topic (it
//! never did, it did and the topic was deleted, it holds it), and the topic NAME the ARN
//! carries — the characters a topic name may be built from, so the refusal is shown to echo
//! whatever ARN the request named rather than one fixture's. A destination whose target no
//! other service owns (a `CloudWatchDestination`) is the negative control: there is nothing
//! to resolve, so it is stored.
//!
//! Every topic here is made and deleted through Amazon SNS's OWN operations over the
//! in-process callback, so what the pre-flight reads is what Amazon SNS really holds.

use crate::harness::{
    call, create_event_destination, described_event_destinations, element, emulator_with_sns,
    error_envelope, field, sns, topic_arn, update_event_destination, Destination,
};
use smithy_server_runtime::MultiServiceRouter;

/// The event types every destination below matches.
const EVENT_TYPES: &[&str] = &["send"];

/// Create the topic `name` through Amazon SNS, and answer the ARN Amazon SNS resolved it to.
async fn create_topic(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = sns(router)
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: creating the topic {name}: {error}"));
    created
        .topic_arn()
        .unwrap_or_else(|| panic!("case {case:?}: the created topic has an ARN"))
        .to_string()
}

/// Delete the topic `arn` through Amazon SNS.
async fn delete_topic(router: &MultiServiceRouter, case: &str, arn: &str) {
    sns(router)
        .delete_topic()
        .topic_arn(arn)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: deleting the topic {arn}: {error}"));
}

/// `CreateConfigurationSet`.
async fn given_configuration_set(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case the configuration set {name}: expected 200, got {status} with {body}"
    );
}

/// What Amazon SNS holds under the ARN a row's destination names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopicSetup {
    /// Amazon SNS holds the topic: it was created and never deleted.
    Held,
    /// Amazon SNS never held a topic of that name.
    NeverCreated,
    /// Amazon SNS held the topic and the topic was deleted before the write.
    Deleted,
}

/// One row: what Amazon SNS holds, and the topic name the ARN carries.
struct Row {
    case: &'static str,
    /// The topic name, spanning the characters a topic name may be built from: "topic names
    /// must be made up of only uppercase and lowercase ASCII letters, numbers, underscores,
    /// and hyphens" (the Amazon SNS documentation of a topic name, which is the resource
    /// field of a topic ARN).
    topic: &'static str,
    setup: TopicSetup,
}

/// The rows the table spans.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a topic Amazon SNS holds",
            topic: "events-live",
            setup: TopicSetup::Held,
        },
        Row {
            case: "a hyphenated name Amazon SNS never held",
            topic: "events-ghost-topic",
            setup: TopicSetup::NeverCreated,
        },
        Row {
            case: "an underscored, numbered name Amazon SNS never held",
            topic: "events_ghost_2",
            setup: TopicSetup::NeverCreated,
        },
        Row {
            case: "a mixed-case name Amazon SNS never held",
            topic: "EventsGhostTopic",
            setup: TopicSetup::NeverCreated,
        },
        Row {
            case: "a long name Amazon SNS never held",
            topic: "events-ghost-topic-with-a-rather-long-but-legal-name-0123456789",
            setup: TopicSetup::NeverCreated,
        },
        Row {
            case: "a topic Amazon SNS held until it was deleted",
            topic: "events-deleted",
            setup: TopicSetup::Deleted,
        },
        Row {
            case: "a topic Amazon SNS holds under an underscored name",
            topic: "events_live_2",
            setup: TopicSetup::Held,
        },
    ]
}

/// Put `row`'s topic into the state the row describes, and answer the ARN its destination
/// names — the ARN Amazon SNS minted for a topic it holds, and the published form of a topic
/// ARN of this account in this Region for one it does not.
async fn given_topic(router: &MultiServiceRouter, row: &Row) -> String {
    match row.setup {
        TopicSetup::Held => create_topic(router, row.case, row.topic).await,
        TopicSetup::NeverCreated => topic_arn(row.topic),
        TopicSetup::Deleted => {
            let arn = create_topic(router, row.case, row.topic).await;
            delete_topic(router, row.case, &arn).await;
            arn
        }
    }
}

/// The refusal AWS words for a topic Amazon SNS holds nothing under.
fn not_found_message(arn: &str) -> String {
    format!("SNS topic <{arn}> not found.")
}

#[tokio::test]
async fn a_created_destination_names_a_topic_amazon_sns_holds() {
    let router = emulator_with_sns();
    for (index, row) in rows().iter().enumerate() {
        let case = format!("create with {}", row.case);
        let set = format!("create-set-{index}");
        let destination_name = "sns-destination";
        given_configuration_set(&router, &set).await;
        let arn = given_topic(&router, row).await;

        let (status, body) = create_event_destination(
            &router,
            &set,
            destination_name,
            true,
            EVENT_TYPES,
            &Destination::Sns(arn.clone()),
        )
        .await;
        let reported = described_event_destinations(&router, &set).await;

        if row.setup == TopicSetup::Held {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the destination to be accepted, got {status} with {body}"
            );
            assert_eq!(
                element(&reported, "TopicARN").as_deref(),
                Some(arn.as_str()),
                "case {case:?}: expected the stored destination to name {arn}, got {reported}"
            );
            continue;
        }

        assert_eq!(
            (status, error_envelope(&body).0),
            (400, Some("InvalidSNSDestination".to_string())),
            "case {case:?}: expected 400 InvalidSNSDestination, got {status} with {body}"
        );
        assert_eq!(
            error_envelope(&body).1,
            Some(not_found_message(&arn)),
            "case {case:?}: expected the refusal to read {:?}, got {body}",
            not_found_message(&arn)
        );
        assert!(
            reported.trim().is_empty(),
            "case {case:?}: the refused destination must not be attached, got {reported}"
        );
    }
}

#[tokio::test]
async fn an_updated_destination_is_not_repointed_at_a_topic_amazon_sns_holds_nothing_under() {
    let router = emulator_with_sns();
    for (index, row) in rows().iter().enumerate() {
        let case = format!("update with {}", row.case);
        let set = format!("update-set-{index}");
        let destination_name = "sns-destination";
        given_configuration_set(&router, &set).await;
        // The set starts out carrying a destination on a topic Amazon SNS really holds, so
        // what a refused update must leave behind is an observable last-good state.
        let live = create_topic(&router, &case, &format!("update-control-{index}")).await;
        let (status, body) = create_event_destination(
            &router,
            &set,
            destination_name,
            true,
            EVENT_TYPES,
            &Destination::Sns(live.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the set's first destination: expected 200, got {status} with {body}"
        );

        let arn = given_topic(&router, row).await;
        let (status, body) = update_event_destination(
            &router,
            &set,
            destination_name,
            false,
            EVENT_TYPES,
            &Destination::Sns(arn.clone()),
        )
        .await;
        let reported = described_event_destinations(&router, &set).await;

        if row.setup == TopicSetup::Held {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the replacement to be accepted, got {status} with {body}"
            );
            assert_eq!(
                element(&reported, "TopicARN").as_deref(),
                Some(arn.as_str()),
                "case {case:?}: expected the replacement to name {arn}, got {reported}"
            );
            continue;
        }

        assert_eq!(
            (status, error_envelope(&body).0),
            (400, Some("InvalidSNSDestination".to_string())),
            "case {case:?}: expected 400 InvalidSNSDestination, got {status} with {body}"
        );
        assert_eq!(
            error_envelope(&body).1,
            Some(not_found_message(&arn)),
            "case {case:?}: expected the refusal to read {:?}, got {body}",
            not_found_message(&arn)
        );
        assert_eq!(
            element(&reported, "TopicARN").as_deref(),
            Some(live.as_str()),
            "case {case:?}: the refused update must leave the stored destination on {live}, \
             got {reported}"
        );
        assert_eq!(
            element(&reported, "Enabled").as_deref(),
            Some("true"),
            "case {case:?}: the refused update must change nothing about the stored \
             destination, got {reported}"
        );
    }
}

#[tokio::test]
async fn the_same_write_is_served_once_amazon_sns_holds_the_topic() {
    // The restore leg: the refusal is about the topic, not about the request, so creating
    // the topic the ARN names and re-issuing the IDENTICAL call is served and the
    // destination is reported with that same ARN.
    let router = emulator_with_sns();
    let case = "a write re-issued once the topic exists";
    let set = "restore-set";
    let destination_name = "sns-destination";
    given_configuration_set(&router, set).await;
    let arn = topic_arn("events-restored");

    let (status, body) = create_event_destination(
        &router,
        set,
        destination_name,
        true,
        EVENT_TYPES,
        &Destination::Sns(arn.clone()),
    )
    .await;
    assert_eq!(
        (status, error_envelope(&body).0, error_envelope(&body).1),
        (
            400,
            Some("InvalidSNSDestination".to_string()),
            Some(not_found_message(&arn))
        ),
        "case {case:?}: expected the first call to be refused, got {status} with {body}"
    );

    let minted = create_topic(&router, case, "events-restored").await;
    assert_eq!(
        minted, arn,
        "case {case:?}: Amazon SNS must mint the ARN the refused call named, got {minted}"
    );

    let (status, body) = create_event_destination(
        &router,
        set,
        destination_name,
        true,
        EVENT_TYPES,
        &Destination::Sns(arn.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the re-issued call to be served, got {status} with {body}"
    );
    let reported = described_event_destinations(&router, set).await;
    assert_eq!(
        element(&reported, "TopicARN").as_deref(),
        Some(arn.as_str()),
        "case {case:?}: expected the stored destination to name {arn}, got {reported}"
    );
}

#[tokio::test]
async fn what_the_request_identifies_is_resolved_before_the_destination_s_target() {
    // The resource a request IDENTIFIES is resolved first: a set the account does not hold,
    // and a destination name the set does not carry, are reported as not being there even
    // when the destination the request describes also names a topic Amazon SNS holds
    // nothing under — so the order does not depend on which fault a caller fixes first.
    let router = emulator_with_sns();
    let ghost = topic_arn("events-order-ghost");
    let held = create_topic(&router, "the ordering fixtures", "events-order-held").await;
    given_configuration_set(&router, "order-set").await;
    let (status, body) = create_event_destination(
        &router,
        "order-set",
        "sns-destination",
        true,
        EVENT_TYPES,
        &Destination::Sns(held.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the ordering fixtures: expected 200, got {status} with {body}"
    );

    // (what the row is, the call, the set it names, the destination name it names, the
    //  code expected, the message expected)
    let rows: Vec<(&str, &str, &str, &str, &str, String)> = vec![
        (
            "a create naming a set nothing created, and a topic Amazon SNS holds nothing \
             under",
            "CreateConfigurationSetEventDestination",
            "order-absent-set",
            "sns-destination",
            "ConfigurationSetDoesNotExist",
            "Configuration set <order-absent-set> does not exist.".to_string(),
        ),
        (
            "an update naming a set nothing created, and a topic Amazon SNS holds nothing \
             under",
            "UpdateConfigurationSetEventDestination",
            "order-absent-set",
            "sns-destination",
            "ConfigurationSetDoesNotExist",
            "Configuration set <order-absent-set> does not exist.".to_string(),
        ),
        (
            "an update naming a destination the set does not carry, and a topic Amazon SNS \
             holds nothing under",
            "UpdateConfigurationSetEventDestination",
            "order-set",
            "absent-destination",
            "EventDestinationDoesNotExist",
            // The sentence this service answers for a destination a set does not carry
            // (`ses_missing_event_destination_wording.rs` pins the wording itself).
            "No EventDestination found for order-set".to_string(),
        ),
    ];

    for (case, action, set, destination_name, code, message) in rows {
        let (status, body) = crate::harness::write_event_destination(
            &router,
            action,
            set,
            destination_name,
            true,
            EVENT_TYPES,
            &Destination::Sns(ghost.clone()),
        )
        .await;
        assert_eq!(
            (status, error_envelope(&body).0, error_envelope(&body).1),
            (400, Some(code.to_string()), Some(message.clone())),
            "case {case:?}: expected 400 {code} reading {message:?}, got {status} with {body}"
        );
    }

    // The set that was there still carries exactly what it carried.
    let reported = described_event_destinations(&router, "order-set").await;
    assert_eq!(
        element(&reported, "TopicARN").as_deref(),
        Some(held.as_str()),
        "case the ordering rows: the stored destination must stand, got {reported}"
    );
}

#[tokio::test]
async fn a_destination_no_other_service_owns_a_target_of_is_stored() {
    // The negative control: a `CloudWatchDestination` names no resource of another service,
    // so there is nothing to resolve and the write is served with Amazon SNS mounted beside
    // this service exactly as the rows above are refused against it.
    let router = emulator_with_sns();
    let case = "a CloudWatch destination";
    let set = "cloudwatch-set";
    given_configuration_set(&router, set).await;
    let (status, body) = create_event_destination(
        &router,
        set,
        "cloudwatch-destination",
        true,
        EVENT_TYPES,
        &Destination::CloudWatch(vec![(
            "campaign".to_string(),
            "messageTag".to_string(),
            "none".to_string(),
        )]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {body}"
    );
    let reported = described_event_destinations(&router, set).await;
    assert_eq!(
        element(&reported, "DimensionName").as_deref(),
        Some("campaign"),
        "case {case:?}: expected the stored dimension, got {reported}"
    );
}
