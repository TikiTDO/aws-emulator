// SPDX-License-Identifier: CC0-1.0

//! What a delete of an email contact does to the channel associations that name
//! it, driven end to end over HTTP through the production router with this
//! service and the sibling User Notifications service mounted on one engine.
//!
//! The rule under test: an email-contact ARN is one of the ARNs User
//! Notifications accepts as a CHANNEL of a NotificationConfiguration — its
//! modelled `ChannelArn` admits exactly the `chatbot`, `consoleapp` and
//! `notifications-contacts` resources
//! (`smithy_query(query="shape",shape="ChannelArn")` on the notifications model)
//! — and `DeleteEmailContact` documents that "Deleting an email contact removes
//! it from all associated notification configurations". So after the delete no
//! configuration reports the contact as a channel any more, while every OTHER
//! association stands exactly where it was.
//!
//! The rows vary along what that rule generalises over: how many configurations
//! referenced the contact, where in a configuration's association order the
//! contact sat, which other channels stood beside it, how many channels the
//! configuration held, and whether the contact had been activated before it was
//! deleted.

use serde_json::{json, Value};

use crate::email_contacts::{
    activate, call_signed_for, create_ok, delete, emulator_with_notifications, error_code, get,
    issued_code, label, send_code, unknown_arn, Emulator, NOTIFICATIONS_SIGNING_NAME,
};

/// The channels that stand beside the contact and must survive its delete.
///
/// Catalog source: the `ChannelArn` pattern the notifications model declares,
/// `^arn:[a-z-]{3,10}:(chatbot|consoleapp|notifications-contacts):[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z0-9-_.@]+/[a-zA-Z0-9/_.@:-]+$`.
/// The two rows cover the other two services the alternation names — one whose
/// ARN carries a Region and a deep resource path, one whose Region segment is
/// empty — neither of which this service owns, so neither may be touched by a
/// delete here.
const OTHER_CHANNELS: &[&str] = &[
    "arn:aws:chatbot:us-west-2:000000000000:chat-configuration/slack-channel/ops-room",
    "arn:aws:consoleapp::000000000000:device/0bfe9a191c084b9fae82a53b4c1d4cd1",
];

/// One call against the sibling User Notifications service, signed for its own
/// signing name so the router resolves it to that service.
async fn notifications(
    em: &Emulator,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_signed_for(em, NOTIFICATIONS_SIGNING_NAME, method, uri, body).await
}

/// Create a NotificationConfiguration for channels to be associated with, and
/// return its ARN.
async fn create_configuration(em: &Emulator, name: &str) -> String {
    let body = json!({ "name": name, "description": "a configuration under test" });
    let (status, answer) = notifications(em, "POST", "/notification-configurations", Some(&body)).await;
    assert_eq!(
        status, 201,
        "case {name:?}: the configuration a channel is associated with must be created, got {answer:?}"
    );
    answer["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: a create answers an arn, got {answer:?}"))
        .to_string()
}

/// `AssociateChannel`: associate `channel` with `configuration`, which must
/// succeed with the modelled 201.
async fn associate(em: &Emulator, case: &str, channel: &str, configuration: &str) {
    let uri = format!("/channels/associate/{}", label(channel));
    let body = json!({ "notificationConfigurationArn": configuration });
    let (status, answer) = notifications(em, "POST", &uri, Some(&body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: associating {channel} must answer 201, got {status} / {answer:?}"
    );
}

/// `ListChannels`: every channel one configuration reports, in association order.
async fn channels_of(em: &Emulator, case: &str, configuration: &str) -> Vec<String> {
    let uri = format!("/channels?notificationConfigurationArn={configuration}");
    let (status, answer) = notifications(em, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListChannels on {configuration} must answer 200, got {status} / {answer:?}"
    );
    answer["channels"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: every listing carries the channels member, got {answer:?}")
        })
        .iter()
        .map(|channel| {
            channel
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed channel is an arn string, got {channel:?}")
                })
                .to_string()
        })
        .collect()
}

/// `ListNotificationConfigurations?channelArn=…`: the configurations the sibling
/// service still reports as associated with one channel.
async fn configurations_with_channel(em: &Emulator, case: &str, channel: &str) -> Vec<String> {
    let uri = format!("/notification-configurations?channelArn={channel}");
    let (status, answer) = notifications(em, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: filtering configurations by {channel} must answer 200, got {status} / {answer:?}"
    );
    let mut arns: Vec<String> = answer["notificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the filtered listing is a list, got {answer:?}")
        })
        .iter()
        .filter_map(|item| item["arn"].as_str().map(str::to_string))
        .collect();
    arns.sort();
    arns
}

/// Activate a contact through the two operations that do it, so a row can delete
/// a contact whose status is `ACTIVE` rather than the `INACTIVE` a create leaves.
async fn activate_contact(em: &Emulator, case: &str, contact: &str) {
    let (status, answer) = send_code(em, contact).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendActivationCode must answer 200, got {status} / {answer:?}"
    );
    let code = issued_code(em, contact)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the issued code"));
    let (status, answer) = activate(em, contact, &code).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ActivateEmailContact must answer 200, got {status} / {answer:?}"
    );
}

/// RULE: deleting an email contact removes it from every notification
/// configuration it was associated with as a channel — whatever position it held
/// in the association order and whatever else was associated — and leaves every
/// other channel of those configurations, and every configuration that never
/// referenced it, exactly as they were.
#[tokio::test]
async fn deleting_a_contact_drops_it_from_every_configuration_that_listed_it_as_a_channel() {
    // Each row states the channels that stand beside the contact and the position
    // the contact takes among them, so the contact is in turn the only channel and
    // the first, middle and last of three; two rows delete an ACTIVATED contact,
    // since the documented removal is about the contact going rather than about
    // the status it went in.
    let cases: &[(&str, &[&str], usize, bool)] = &[
        ("the only channel of the configuration", &[], 0, false),
        ("the first of three channels", OTHER_CHANNELS, 0, true),
        ("between two other channels", OTHER_CHANNELS, 1, false),
        ("the last of three channels", OTHER_CHANNELS, 2, true),
    ];

    for (case, beside, position, activated) in cases {
        let em = emulator_with_notifications();
        let contact = create_ok(&em, "oncall-primary", "oncall@example.com").await;
        if *activated {
            activate_contact(&em, case, &contact).await;
        }

        // The row's association order: the contact at `position` among the
        // channels standing beside it, which for the first row is none at all.
        let others: Vec<String> = beside.iter().map(|arn| (*arn).to_string()).collect();
        let mut order = others.clone();
        order.insert(*position, contact.clone());

        // Two configurations reference the contact, so the removal is not about
        // one record; a third holds the same other channels and never referenced
        // it, so it is what proves the delete touched nothing else.
        let referencing = vec![
            create_configuration(&em, "referencing-one").await,
            create_configuration(&em, "referencing-two").await,
        ];
        let control = create_configuration(&em, "control-configuration").await;
        for configuration in &referencing {
            for channel in &order {
                associate(&em, case, channel, configuration).await;
            }
        }
        for channel in &others {
            associate(&em, case, channel, &control).await;
        }

        for configuration in &referencing {
            let listed = channels_of(&em, case, configuration).await;
            assert_eq!(
                listed, order,
                "case {case:?}: before the delete the configuration lists the contact as a \
                 channel, got {listed:?}"
            );
        }
        let selected = configurations_with_channel(&em, case, &contact).await;
        let mut expected_selected = referencing.clone();
        expected_selected.sort();
        assert_eq!(
            selected, expected_selected,
            "case {case:?}: before the delete the channel filter selects both referencing \
             configurations, got {selected:?}"
        );

        let (status, answer) = delete(&em, &contact).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEmailContact must answer 200, got {status} / {answer:?}"
        );

        for configuration in &referencing {
            let listed = channels_of(&em, case, configuration).await;
            assert_eq!(
                listed, others,
                "case {case:?}: after the delete the configuration lists every other channel \
                 and not the contact, got {listed:?}"
            );
            assert_eq!(
                listed.len(),
                order.len() - 1,
                "case {case:?}: the configuration's channel count drops by exactly one, got {}",
                listed.len()
            );
        }

        let listed = channels_of(&em, case, &control).await;
        assert_eq!(
            listed, others,
            "case {case:?}: a configuration that never referenced the contact is untouched, \
             got {listed:?}"
        );

        let selected = configurations_with_channel(&em, case, &contact).await;
        assert!(
            selected.is_empty(),
            "case {case:?}: no configuration is associated with the deleted contact any more, \
             got {selected:?}"
        );
        for channel in &others {
            let selected = configurations_with_channel(&em, case, channel).await;
            let mut expected = referencing.clone();
            expected.push(control.clone());
            expected.sort();
            assert_eq!(
                selected, expected,
                "case {case:?}: every other channel keeps every association it had, got \
                 {selected:?}"
            );
        }

        let (status, answer) = get(&em, &contact).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted contact itself is gone, got {status} / {answer:?}"
        );
    }
}

/// RULE: the removal is scoped to the contact that was deleted — deleting one
/// email contact leaves every other contact's association standing, in the order
/// the associations were made, and leaves those contacts readable.
#[tokio::test]
async fn deleting_one_contact_leaves_every_other_contacts_association_standing() {
    // Three contacts associated with the same two configurations; each row
    // deletes a different one of them, so no position in the association order is
    // special.
    let contacts = ["first", "second", "third"];

    for deleted_index in 0..contacts.len() {
        let case = format!("deleting the contact at position {deleted_index}");
        let em = emulator_with_notifications();

        let mut arns: Vec<String> = Vec::new();
        for name in contacts {
            arns.push(create_ok(&em, name, &format!("{name}@example.com")).await);
        }
        let configurations = vec![
            create_configuration(&em, "shared-one").await,
            create_configuration(&em, "shared-two").await,
        ];
        for configuration in &configurations {
            for arn in &arns {
                associate(&em, &case, arn, configuration).await;
            }
        }

        let (status, answer) = delete(&em, &arns[deleted_index]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEmailContact must answer 200, got {status} / {answer:?}"
        );

        let survivors: Vec<String> = arns
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != deleted_index)
            .map(|(_, arn)| arn.clone())
            .collect();
        for configuration in &configurations {
            let listed = channels_of(&em, &case, configuration).await;
            assert_eq!(
                listed, survivors,
                "case {case:?}: only the deleted contact's association goes, and the others keep \
                 their order, got {listed:?}"
            );
        }
        for arn in &survivors {
            let (status, answer) = get(&em, arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a contact that was not deleted is still readable, got {status} / \
                 {answer:?}"
            );
        }
    }
}

/// RULE: only an association that exists is removed, so a delete with nothing to
/// propagate — a contact no configuration referenced, or an ARN this service
/// holds no contact for, which is the modelled 404 — leaves every configuration's
/// channels exactly as they were.
#[tokio::test]
async fn a_delete_with_nothing_to_propagate_leaves_every_configuration_untouched() {
    let em = emulator_with_notifications();
    let case = "a delete with nothing to propagate";

    let associated = create_ok(&em, "associated", "associated@example.com").await;
    let unassociated = create_ok(&em, "unassociated", "unassociated@example.com").await;
    let configuration = create_configuration(&em, "untouched-configuration").await;
    let mut order: Vec<String> = OTHER_CHANNELS.iter().map(|arn| (*arn).to_string()).collect();
    order.push(associated.clone());
    for channel in &order {
        associate(&em, case, channel, &configuration).await;
    }

    // A contact this emulator holds but no configuration ever referenced.
    let (status, answer) = delete(&em, &unassociated).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting an unassociated contact answers 200, got {status} / {answer:?}"
    );
    let listed = channels_of(&em, case, &configuration).await;
    assert_eq!(
        listed, order,
        "case {case:?}: deleting an unassociated contact changes no association, got {listed:?}"
    );

    // An ARN the model admits that names no contact: the modelled 404, and no
    // association may move on the way to it.
    let unknown = unknown_arn('q');
    let (status, answer) = delete(&em, &unknown).await;
    assert_eq!(
        status, 404,
        "case {case:?}: deleting an unknown contact is the modelled 404, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case {case:?}: that refusal is a ResourceNotFoundException, got {answer:?}"
    );
    let listed = channels_of(&em, case, &configuration).await;
    assert_eq!(
        listed, order,
        "case {case:?}: a refused delete changes no association, got {listed:?}"
    );
    assert!(
        listed.contains(&associated),
        "case {case:?}: the association naming a contact this emulator still holds survives \
         both calls, got {listed:?}"
    );
}
