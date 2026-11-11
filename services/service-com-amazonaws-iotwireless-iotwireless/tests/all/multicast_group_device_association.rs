// SPDX-License-Identifier: CC0-1.0

//! Joining wireless devices to multicast groups, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded request:
//! `AssociateWirelessDeviceWithMulticastGroup` persists the membership on the device it
//! names, so every reader derived from that membership answers from it — the group's
//! `NumberOfDevicesRequested`/`NumberOfDevicesInGroup` count exactly the devices joined
//! to it, and `ListWirelessDevices?multicastGroupId=` selects exactly those devices —
//! while a device joined to no group stays out of every count and every filtered list.
//! The cases vary the device type and which optional technology record it carries, how
//! many devices a group holds (none, one, several), the group's LoRaWAN settings, and
//! the page size the filtered list is walked at.
//!
//! The refusals belong to the same rule: both ends of the association are resolved
//! against real state (an absent group or device is a 404 whose `ResourceType`/
//! `ResourceId` say WHICH end could not be resolved), the membership is single-valued (a
//! second group for the same device is a 409 naming the device and the group it is
//! already in), and asking for a membership that already holds is the same success
//! again, counted once. A refused call persists nothing.
//!
//! The wire harness is the one the sibling destination/device suite already has:
//! `tests/all/*.rs` are modules of ONE test target, so `router`, `call` and
//! `error_code` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// AWS's sentence for a wireless device the account does not hold — wholly fixed text,
/// spelled out here so the suite pins the wording rather than whatever the service
/// happens to emit.
const WIRELESS_DEVICE_NOT_FOUND_MESSAGE: &str = "WirelessDevice record not found";

/// `SupportedRfRegion`'s modelled values — the model-generated constant, so the suite
/// and the service cannot drift apart.
const RF_REGIONS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES;

/// `DlClass`'s modelled values, from the same generated source.
const DL_CLASSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::DL_CLASS_VALUES;

/// Create one destination, so the devices below have something to register against.
pub(crate) async fn seed_destination(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": name,
            "ExpressionType": "RuleName",
            "Expression": format!("rule-for-{name}"),
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding destination {name}: expected 201, got {status} {body}"
    );
}

/// Create one multicast group and return its `Id`.
pub(crate) async fn seed_group(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, Method::POST, "/multicast-groups", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a multicast group: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding a multicast group: no Id in {created}"))
        .to_string()
}

/// Create one wireless device and return its `Id`.
pub(crate) async fn seed_device(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, Method::POST, "/wireless-devices", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding a wireless device: expected 201, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding a wireless device: no Id in {created}"))
        .to_string()
}

/// `AssociateWirelessDeviceWithMulticastGroup` over the wire.
pub(crate) async fn associate(
    router: &MultiServiceRouter,
    group_id: &str,
    device_id: &str,
) -> (StatusCode, Value) {
    call(
        router,
        Method::PUT,
        &format!("/multicast-groups/{group_id}/wireless-device"),
        Some(json!({ "WirelessDeviceId": device_id })),
    )
    .await
}

/// The membership counts `GetMulticastGroup` reports for `group_id`, as
/// `(NumberOfDevicesRequested, NumberOfDevicesInGroup)`.
pub(crate) async fn group_counts(router: &MultiServiceRouter, group_id: &str) -> (i64, i64) {
    let (status, fetched) = call(
        router,
        Method::GET,
        &format!("/multicast-groups/{group_id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reading group {group_id}: expected 200, got {status} {fetched}"
    );
    let requested = fetched["LoRaWAN"]["NumberOfDevicesRequested"]
        .as_i64()
        .unwrap_or_else(|| panic!("group {group_id}: no NumberOfDevicesRequested in {fetched}"));
    let in_group = fetched["LoRaWAN"]["NumberOfDevicesInGroup"]
        .as_i64()
        .unwrap_or_else(|| panic!("group {group_id}: no NumberOfDevicesInGroup in {fetched}"));
    (requested, in_group)
}

/// Walk every page of `ListWirelessDevices?multicastGroupId=<group_id>` at `max_results`
/// and return the `Id`s it served, sorted, so a selection is compared as a set.
pub(crate) async fn member_ids(
    router: &MultiServiceRouter,
    group_id: &str,
    max_results: i64,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut uri =
            format!("/wireless-devices?multicastGroupId={group_id}&maxResults={max_results}");
        if let Some(token) = &token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(router, Method::GET, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case multicastGroupId={group_id} maxResults={max_results}: expected 200, got \
             {status} {body}"
        );
        let page = body["WirelessDeviceList"]
            .as_array()
            .unwrap_or_else(|| panic!("WirelessDeviceList must be a list, got {body}"))
            .clone();
        if max_results > 0 {
            assert!(
                page.len() as i64 <= max_results,
                "case multicastGroupId={group_id} maxResults={max_results}: a page served {} \
                 entries",
                page.len()
            );
        }
        ids.extend(
            page.iter()
                .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string()),
        );
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    ids.sort();
    ids
}

/// One group in the plan: the body it is created with, and which of the seeded devices
/// are joined to it.
struct GroupCase {
    case: &'static str,
    body: Value,
    /// Indexes into the seeded device table.
    members: &'static [usize],
}

/// The devices every case shares: both modelled `WirelessDeviceType`s, with and without
/// the optional technology record.
pub(crate) fn device_bodies(destination: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "lorawan-with-dev-eui",
            json!({
                "Type": "LoRaWAN",
                "Name": "mc-lorawan-1",
                "DestinationName": destination,
                "LoRaWAN": { "DevEui": "0000000000000a01", "DeviceProfileId": "dp-mc" },
            }),
        ),
        (
            "lorawan-bare",
            json!({ "Type": "LoRaWAN", "DestinationName": destination }),
        ),
        (
            "sidewalk",
            json!({
                "Type": "Sidewalk",
                "Name": "mc-sidewalk",
                "DestinationName": destination,
                "Sidewalk": { "DeviceProfileId": "dp-mc-sidewalk" },
            }),
        ),
        (
            "lorawan-named-only",
            json!({
                "Type": "LoRaWAN",
                "Name": "mc-lorawan-2",
                "DestinationName": destination,
                "LoRaWAN": { "DevEui": "0000000000000a02" },
            }),
        ),
    ]
}

/// A membership written by the associate call is what the group's counts and the
/// filtered list report, over a table of groups holding none, one and several devices of
/// both modelled types, at every page size.
#[tokio::test]
async fn an_associated_device_is_counted_by_its_group_and_selected_by_the_list() {
    let router = router();
    seed_destination(&router, "McAssocDest").await;

    let devices = device_bodies("McAssocDest");
    let mut device_ids = Vec::new();
    for (case, body) in &devices {
        device_ids.push((case, seed_device(&router, body.clone()).await));
    }

    // The group bodies walk the modelled `SupportedRfRegion` and `DlClass` catalogs, so
    // the membership does not depend on the group's radio settings.
    let groups = [
        GroupCase {
            case: "one-lorawan-member",
            body: json!({
                "Name": "McOneMember",
                "LoRaWAN": { "RfRegion": RF_REGIONS[0], "DlClass": DL_CLASSES[0] },
            }),
            members: &[0],
        },
        GroupCase {
            case: "two-members-mixed-types",
            body: json!({
                "Name": "McTwoMembers",
                "Description": "a bare LoRaWAN device and a Sidewalk device",
                "LoRaWAN": {
                    "RfRegion": RF_REGIONS[RF_REGIONS.len() - 1],
                    "DlClass": DL_CLASSES[DL_CLASSES.len() - 1],
                },
            }),
            members: &[1, 2],
        },
        GroupCase {
            case: "no-members",
            body: json!({ "LoRaWAN": {} }),
            members: &[],
        },
    ];

    let mut group_ids = Vec::new();
    for group in &groups {
        let group_id = seed_group(&router, group.body.clone()).await;
        // Before anything joins it, a group counts nothing and selects nothing: both
        // answers come from the devices' own records, not from the group's existence.
        assert_eq!(
            group_counts(&router, &group_id).await,
            (0, 0),
            "case {:?}: a group nothing joined must count no device",
            group.case
        );
        assert!(
            member_ids(&router, &group_id, 0).await.is_empty(),
            "case {:?}: a group nothing joined must select no device",
            group.case
        );
        group_ids.push(group_id);
    }

    for (group, group_id) in groups.iter().zip(&group_ids) {
        for member in group.members {
            let (status, body) = associate(&router, group_id, &device_ids[*member].1).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {:?} device {:?}: expected 204, got {status} {body}",
                group.case,
                device_ids[*member].0
            );
            assert!(
                body.as_object().is_none_or(|members| members.is_empty()),
                "case {:?}: a 204 carries no response members, got {body}",
                group.case
            );
        }
    }

    for (group, group_id) in groups.iter().zip(&group_ids) {
        let expected: Vec<String> = {
            let mut ids: Vec<String> = group
                .members
                .iter()
                .map(|member| device_ids[*member].1.clone())
                .collect();
            ids.sort();
            ids
        };
        let count = expected.len() as i64;
        assert_eq!(
            group_counts(&router, group_id).await,
            (count, count),
            "case {:?}: expected both counts to be {count}",
            group.case
        );
        // Page size 0 is "no caller-supplied page size"; 1 and 2 force the filter to be
        // applied before paging.
        for max_results in [0, 1, 2, 250] {
            let selected = member_ids(&router, group_id, max_results).await;
            assert_eq!(
                selected, expected,
                "case {:?} maxResults={max_results}: wrong selection, got {selected:?}",
                group.case
            );
        }
    }

    // A device joined to nothing is in no group's count and in no filtered list, while
    // the unfiltered list still reports every device exactly once.
    let (_, all) = call(&router, Method::GET, "/wireless-devices", None).await;
    let listed = all["WirelessDeviceList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut all_ids: Vec<String> = listed
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    all_ids.sort();
    assert_eq!(
        all_ids,
        {
            let mut ids: Vec<String> = device_ids.iter().map(|(_, id)| id.clone()).collect();
            ids.sort();
            ids
        },
        "the unfiltered list must still report every device, got {all_ids:?}"
    );
    let unjoined = &device_ids[3];
    for group_id in &group_ids {
        assert!(
            !member_ids(&router, group_id, 0).await.contains(&unjoined.1),
            "case {:?}: a device in no group must stay out of group {group_id}",
            unjoined.0
        );
    }
}

/// Asking for a membership that already holds is the same success again, and leaves the
/// group holding — and counting — the device exactly once.
#[tokio::test]
async fn re_associating_the_same_device_is_idempotent() {
    let router = router();
    seed_destination(&router, "McIdemDest").await;
    let group_id = seed_group(&router, json!({ "LoRaWAN": { "DlClass": DL_CLASSES[0] } })).await;

    let mut joined = 0i64;
    for (case, body) in device_bodies("McIdemDest") {
        let device_id = seed_device(&router, body).await;
        joined += 1;
        for attempt in 1..=3 {
            let (status, answered) = associate(&router, &group_id, &device_id).await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "case {case:?} attempt {attempt}: expected 204, got {status} {answered}"
            );
        }
        let members = member_ids(&router, &group_id, 0).await;
        assert_eq!(
            members.iter().filter(|id| *id == &device_id).count(),
            1,
            "case {case:?}: the group must hold the device exactly once, got {members:?}"
        );
        assert_eq!(
            group_counts(&router, &group_id).await,
            (joined, joined),
            "case {case:?}: repeated associations must not inflate the counts"
        );
    }
}

/// A device's multicast membership is single-valued: joining it to a second group is
/// refused and the membership already stored is left as it stands.
#[tokio::test]
async fn a_device_already_in_a_group_cannot_join_a_second_one() {
    let router = router();
    seed_destination(&router, "McConflictDest").await;

    let first = seed_group(
        &router,
        json!({ "Name": "McFirst", "LoRaWAN": { "RfRegion": RF_REGIONS[0] } }),
    )
    .await;
    let second = seed_group(
        &router,
        json!({ "Name": "McSecond", "LoRaWAN": { "RfRegion": RF_REGIONS[0] } }),
    )
    .await;

    let mut joined = 0i64;
    for (case, body) in device_bodies("McConflictDest") {
        let device_id = seed_device(&router, body).await;
        let (status, answered) = associate(&router, &first, &device_id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: the first association must succeed, got {status} {answered}"
        );
        joined += 1;

        let (status, answered) = associate(&router, &second, &device_id).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case:?}: a second group for the same device must be 409, got {status} \
             {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {answered}"
        );
        // The refusal says WHICH resource is in the inconsistent state (the device whose
        // membership is already taken) and names the group it is already in, both taken
        // from the request and the store rather than fixed in the body.
        assert_eq!(
            answered["ResourceType"].as_str(),
            Some("WirelessDevice"),
            "case {case:?}: expected ResourceType WirelessDevice, got {answered}"
        );
        assert_eq!(
            answered["ResourceId"].as_str(),
            Some(device_id.as_str()),
            "case {case:?}: expected ResourceId {device_id}, got {answered}"
        );
        let message = answered["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "Wireless device {device_id} is already associated with multicast group \
                 {first}"
            ),
            "case {case:?}: the whole message must be the template filled from the request \
             and the store, got {message:?}"
        );

        // The refusal changed nothing: the device is still in the first group and in no
        // other, and only the first group counts it.
        assert!(
            member_ids(&router, &first, 0).await.contains(&device_id),
            "case {case:?}: the refused call must leave the first membership in place"
        );
        assert!(
            !member_ids(&router, &second, 0).await.contains(&device_id),
            "case {case:?}: the refused call must not have joined the second group"
        );
        assert_eq!(
            group_counts(&router, &first).await,
            (joined, joined),
            "case {case:?}: the first group must still count every device it holds"
        );
        assert_eq!(
            group_counts(&router, &second).await,
            (0, 0),
            "case {case:?}: the refused group must count nothing"
        );
    }
}

/// Both ends of the association are resolved against real state: an `Id` or a
/// `WirelessDeviceId` the store does not hold is a 404 that persists nothing.
#[tokio::test]
async fn an_absent_group_or_device_is_not_found() {
    let router = router();
    seed_destination(&router, "McMissingDest").await;
    let group_id = seed_group(&router, json!({ "LoRaWAN": {} })).await;
    let device_id = seed_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "McMissingDest" }),
    )
    .await;

    // (case, the group the call names, the device it names, the end that is missing — the
    // group is resolved first, so a call naming neither reports the group).
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "absent group, real device",
            "00000000-0000-4000-8000-000000000000".to_string(),
            device_id.clone(),
            "MulticastGroup",
        ),
        (
            "group id in the wrong case",
            group_id.to_uppercase(),
            device_id.clone(),
            "MulticastGroup",
        ),
        (
            "real group, absent device",
            group_id.clone(),
            "11111111-1111-4111-8111-111111111111".to_string(),
            "WirelessDevice",
        ),
        (
            "real group, device id in the wrong case",
            group_id.clone(),
            device_id.to_uppercase(),
            "WirelessDevice",
        ),
        (
            "real group, device id that is not a uuid",
            group_id.clone(),
            "not-a-device".to_string(),
            "WirelessDevice",
        ),
        (
            "neither end exists",
            "mc-nope".to_string(),
            "wd-nope".to_string(),
            "MulticastGroup",
        ),
    ];

    for (case, group, device, missing) in cases {
        let (status, answered) = associate(&router, &group, &device).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {case:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        // The refusal says WHICH end of the association could not be resolved: its
        // `ResourceType`/`ResourceId` name the kind and the id the request supplied for
        // it, and its message is the one that end's own reader refuses with.
        assert_eq!(
            answered["ResourceType"].as_str(),
            Some(missing),
            "case {case:?}: expected ResourceType {missing}, got {answered}"
        );
        let missing_id = if missing == "MulticastGroup" {
            group.as_str()
        } else {
            device.as_str()
        };
        assert_eq!(
            answered["ResourceId"].as_str(),
            Some(missing_id),
            "case {case:?}: expected ResourceId {missing_id}, got {answered}"
        );
        // The whole message is the sentence the end that could not be resolved is
        // refused with, so a reworded prefix fails too.
        let expected_message = if missing == "MulticastGroup" {
            format!(
                "Cannot complete the step because resource is not found, with resource type \
                 MulticastGroup, resource Id {missing_id}"
            )
        } else {
            WIRELESS_DEVICE_NOT_FOUND_MESSAGE.to_string()
        };
        assert_eq!(
            answered["Message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the {missing} not-found message, got {answered}"
        );
        assert_eq!(
            group_counts(&router, &group_id).await,
            (0, 0),
            "case {case:?}: a refused association must persist nothing"
        );
        assert!(
            member_ids(&router, &group_id, 0).await.is_empty(),
            "case {case:?}: a refused association must persist nothing"
        );
    }

    // The real pair still associates after every refusal.
    let (status, answered) = associate(&router, &group_id, &device_id).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "the real pair must still associate, got {status} {answered}"
    );
    assert_eq!(
        member_ids(&router, &group_id, 0).await,
        vec![device_id],
        "the group must now hold the device"
    );
    assert_eq!(
        group_counts(&router, &group_id).await,
        (1, 1),
        "the group must now count the device"
    );
}
