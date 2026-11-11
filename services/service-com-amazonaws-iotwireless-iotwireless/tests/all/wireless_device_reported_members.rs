// SPDX-License-Identifier: CC0-1.0

//! What a wireless device REPORTS, and what an update to one changes, end to end over
//! HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a device's reported state is the whole stored record, so every member a reader
//!   models — the LoRaWAN `FPorts` assignments among them — reads back as stored, and
//!   `Positioning`, a status every device carries, is reported by every reader whether
//!   or not the create named one;
//! * an update mutates the stored device, so the next read of ANY member the update
//!   named reports the new value and every member it did not name keeps the old one;
//! * a device identifier that resolves to nothing is refused with the one sentence this
//!   service words that error class with, whatever identifier kind addressed it.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// The `PositioningConfigStatus` a device carries when nothing has named one, and the
/// `FPorts` assignment the service makes for a LoRaWAN device whose create named none —
/// the service constants under test, so the suite cannot drift from the implementation.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_APPLICATION_FPORT, DEFAULT_APPLICATION_TYPE, DEFAULT_POSITIONING_CLOCK_SYNC_FPORT,
    DEFAULT_POSITIONING_CONFIG_STATUS, DEFAULT_POSITIONING_GNSS_FPORT,
    DEFAULT_POSITIONING_STREAM_FPORT,
};

/// AWS's sentence for a wireless device the account does not hold — wholly fixed text,
/// spelled out here so the suite pins the wording rather than whatever the service
/// happens to emit.
const WIRELESS_DEVICE_NOT_FOUND_MESSAGE: &str = "WirelessDevice record not found";

/// The `ResourceType` AWS names a wireless device by, spelled out for the same reason.
const WIRELESS_DEVICE_RESOURCE_TYPE: &str = "WirelessDevice";

/// `PositioningConfigStatus`' modelled values, from the model-generated catalog.
const POSITIONING_CONFIG_STATUSES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::POSITIONING_CONFIG_STATUS_VALUES;

/// The identifier kinds `GetWirelessDevice` accepts, from the same generated source.
const WIRELESS_DEVICE_ID_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::WIRELESS_DEVICE_ID_TYPE_VALUES;

/// `ApplicationConfigType`'s modelled values, from the same generated source.
const APPLICATION_CONFIG_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::APPLICATION_CONFIG_TYPE_VALUES;

/// `FPort`'s published range, which every port member of `FPorts` targets.
/// Source: `smithy_query(query="closure", operation="UpdateWirelessDevice")` — shape
/// `FPort`, `range min 1 max 223`.
const FPORT_MIN: i64 = 1;
const FPORT_MAX: i64 = 223;

async fn seed_destination(router: &MultiServiceRouter, name: &str) {
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

async fn create_device(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/wireless-devices", Some(body)).await
}

/// Create a device and return its minted `Id`, panicking on anything but 201.
async fn seed_device(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = create_device(router, body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "case {case:?}: expected 201 from CreateWirelessDevice, got {status} {created}"
    );
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create must mint an Id, got {created}"))
        .to_string()
}

async fn get_device(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/wireless-devices/{id}?identifierType=WirelessDeviceId"),
        None,
    )
    .await
}

async fn update_device(router: &MultiServiceRouter, id: &str, body: Value) -> (StatusCode, Value) {
    call(
        router,
        Method::PATCH,
        &format!("/wireless-devices/{id}"),
        Some(body),
    )
    .await
}

/// The `WirelessDeviceList` entry for `id`, or a panic naming what the list held.
async fn list_entry(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, listed) = call(router, Method::GET, "/wireless-devices", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected 200 from ListWirelessDevices, got {status} {listed}"
    );
    listed["WirelessDeviceList"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["Id"] == json!(id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("the list must carry device {id}, got {listed}"))
}

/// Every member a create may supply reads back from the device's reader, and a create
/// that asked for no `FPorts` assignment and no `Positioning` status carries the one the
/// service provisions.
///
/// The rows differ along which `FPorts` assignments the create carries (no `LoRaWAN`
/// block at all, a block naming no `FPorts`, only the three top-level ports, the
/// positioning ports, one and two application entries at each published `FPort` bound),
/// along the device technology, and along whether the create named a `Positioning` status
/// — so the rows sit on both sides of the rule: an assignment the caller named is stored
/// exactly as submitted and nothing is added to it, and only a create that named none is
/// given the service's.
#[tokio::test]
async fn a_device_reports_every_member_it_was_created_with_and_the_service_assigned_rest() {
    let router = router();
    seed_destination(&router, "ReportDest").await;

    /// The `FPorts` assignment the service makes for a LoRaWAN device whose create named
    /// none, built from the service's own constants.
    fn assigned_positioning_ports() -> Value {
        json!({
            "ClockSync": DEFAULT_POSITIONING_CLOCK_SYNC_FPORT,
            "Gnss": DEFAULT_POSITIONING_GNSS_FPORT,
            "Stream": DEFAULT_POSITIONING_STREAM_FPORT,
        })
    }
    fn assigned_applications() -> Value {
        json!([{ "FPort": DEFAULT_APPLICATION_FPORT, "Type": DEFAULT_APPLICATION_TYPE }])
    }

    // Whatever the service assigns, it has to be a value the model permits: every port
    // it picks lies inside `FPort`'s published range and the application entry's type is
    // one `ApplicationConfigType` lists.
    for (member, port) in [
        ("Positioning.ClockSync", DEFAULT_POSITIONING_CLOCK_SYNC_FPORT),
        ("Positioning.Gnss", DEFAULT_POSITIONING_GNSS_FPORT),
        ("Positioning.Stream", DEFAULT_POSITIONING_STREAM_FPORT),
        ("Applications[0].FPort", DEFAULT_APPLICATION_FPORT),
    ] {
        assert!(
            (FPORT_MIN..=FPORT_MAX).contains(&i64::from(port)),
            "case {member:?}: the assigned port {port} is outside FPort's published range \
             {FPORT_MIN}..={FPORT_MAX}"
        );
    }
    assert!(
        APPLICATION_CONFIG_TYPES.contains(&DEFAULT_APPLICATION_TYPE),
        "the assigned application type {DEFAULT_APPLICATION_TYPE:?} is not in the modelled \
         catalog {APPLICATION_CONFIG_TYPES:?}"
    );

    struct Case {
        label: &'static str,
        body: Value,
        /// What `Positioning` must read back as.
        positioning: String,
        /// What the reader must carry at each of these JSON pointers, beside every
        /// member the create itself supplied.
        assigned: Vec<(&'static str, Value)>,
    }

    let cases = vec![
        Case {
            label: "LoRaWAN, no Positioning, the positioning ports and one application named",
            body: json!({
                "Type": "LoRaWAN",
                "Name": "fports-positioning",
                "DestinationName": "ReportDest",
                "LoRaWAN": {
                    "DevEui": "0000000000000f01",
                    "DeviceProfileId": "dp-report-1",
                    "ServiceProfileId": "sp-report-1",
                    "FPorts": {
                        "Positioning": { "ClockSync": 199, "Gnss": 198, "Stream": 197 },
                        "Applications": [{ "FPort": 199, "Type": "SemtechGeolocation" }],
                    },
                },
            }),
            positioning: DEFAULT_POSITIONING_CONFIG_STATUS.to_string(),
            // Every assignment was named, so nothing is assigned on top of it.
            assigned: vec![],
        },
        Case {
            label: "LoRaWAN, Positioning Enabled, only the three top-level ports named",
            body: json!({
                "Type": "LoRaWAN",
                "Name": "fports-top-level",
                "DestinationName": "ReportDest",
                "Positioning": "Enabled",
                "LoRaWAN": {
                    "DevEui": "0000000000000f02",
                    "FPorts": {
                        "Fuota": FPORT_MIN,
                        "Multicast": FPORT_MIN + 1,
                        "ClockSync": FPORT_MIN + 2,
                    },
                },
            }),
            positioning: "Enabled".to_string(),
            // The create named an assignment of its own, so it is stored as submitted:
            // the service adds neither positioning ports nor an application entry to it.
            assigned: vec![
                ("/LoRaWAN/FPorts/Positioning", Value::Null),
                ("/LoRaWAN/FPorts/Applications", Value::Null),
            ],
        },
        Case {
            label: "LoRaWAN, Positioning Disabled, two applications at both published bounds",
            body: json!({
                "Type": "LoRaWAN",
                "Name": "fports-bounds",
                "DestinationName": "ReportDest",
                "Positioning": "Disabled",
                "LoRaWAN": {
                    "DevEui": "0000000000000f03",
                    "FPorts": {
                        "Applications": [
                            {
                                "FPort": FPORT_MIN,
                                "Type": "SemtechGeolocation",
                                "DestinationName": "ReportDest",
                            },
                            { "FPort": FPORT_MAX, "Type": "SemtechGeolocation" },
                        ],
                    },
                },
            }),
            positioning: "Disabled".to_string(),
            // The create named the application entries, so the assignment is the
            // caller's whole assignment: no positioning ports are added to it.
            assigned: vec![("/LoRaWAN/FPorts/Positioning", Value::Null)],
        },
        Case {
            label: "LoRaWAN with a LoRaWAN block that names no FPorts at all",
            body: json!({
                "Type": "LoRaWAN",
                "Name": "no-fports",
                "DestinationName": "ReportDest",
                "LoRaWAN": { "DevEui": "0000000000000f04", "DeviceProfileId": "dp-report-2" },
            }),
            positioning: DEFAULT_POSITIONING_CONFIG_STATUS.to_string(),
            assigned: vec![
                ("/LoRaWAN/FPorts/Positioning", assigned_positioning_ports()),
                ("/LoRaWAN/FPorts/Applications", assigned_applications()),
            ],
        },
        Case {
            label: "LoRaWAN with no LoRaWAN block at all",
            body: json!({
                "Type": "LoRaWAN",
                "Name": "no-lorawan-block",
                "DestinationName": "ReportDest",
            }),
            positioning: DEFAULT_POSITIONING_CONFIG_STATUS.to_string(),
            // There is no radio configuration to assign ports inside, so the reader
            // reports no `LoRaWAN` block rather than one the create never sent.
            assigned: vec![("/LoRaWAN", Value::Null)],
        },
        Case {
            label: "Sidewalk, no Positioning",
            body: json!({
                "Type": "Sidewalk",
                "Name": "sidewalk-no-positioning",
                "DestinationName": "ReportDest",
                "Sidewalk": {
                    "DeviceProfileId": "dp-report-sidewalk",
                    "SidewalkManufacturingSn": "SMSN0000000000000f05",
                },
            }),
            positioning: DEFAULT_POSITIONING_CONFIG_STATUS.to_string(),
            assigned: vec![],
        },
    ];

    for case in &cases {
        assert!(
            POSITIONING_CONFIG_STATUSES.contains(&case.positioning.as_str()),
            "case {:?}: {:?} is not in the modelled catalog {POSITIONING_CONFIG_STATUSES:?}",
            case.label,
            case.positioning
        );
        let id = seed_device(&router, case.label, case.body.clone()).await;
        let (status, fetched) = get_device(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from GetWirelessDevice, got {status} {fetched}",
            case.label
        );

        // The technology block reads back member for member — `FPorts` included, with
        // the positioning ports and the application entries the create supplied.
        for member in ["LoRaWAN", "Sidewalk"] {
            if let Some(expected) = case.body.get(member) {
                assert_reads_back(case.label, &format!("/{member}"), expected, &fetched);
            }
        }
        // And the assignment the create left to the service is reported too — while a
        // pointer paired with `Null` must be ABSENT, which is how a row says the service
        // added nothing to what the create named.
        for (pointer, expected) in &case.assigned {
            if expected.is_null() {
                assert_eq!(
                    fetched.pointer(pointer),
                    None,
                    "case {:?}: {pointer} must be absent, got {:?} in {fetched}",
                    case.label,
                    fetched.pointer(pointer)
                );
                continue;
            }
            assert_eq!(
                fetched.pointer(pointer),
                Some(expected),
                "case {:?}: expected {pointer} to be {expected}, got {:?} in {fetched}",
                case.label,
                fetched.pointer(pointer)
            );
        }
        // Whatever the assignment's source, every port the device reports lies inside
        // `FPort`'s published range and every application entry names a modelled type —
        // and an entry the SERVICE assigned names no position-data destination, because
        // no rule was created for it to name.
        for pointer in [
            "/LoRaWAN/FPorts/Fuota",
            "/LoRaWAN/FPorts/Multicast",
            "/LoRaWAN/FPorts/ClockSync",
            "/LoRaWAN/FPorts/Positioning/ClockSync",
            "/LoRaWAN/FPorts/Positioning/Gnss",
            "/LoRaWAN/FPorts/Positioning/Stream",
        ] {
            if let Some(port) = fetched.pointer(pointer).and_then(Value::as_i64) {
                assert!(
                    (FPORT_MIN..=FPORT_MAX).contains(&port),
                    "case {:?}: {pointer} is {port}, outside FPort's published range \
                     {FPORT_MIN}..={FPORT_MAX}",
                    case.label
                );
            }
        }
        for (index, entry) in fetched
            .pointer("/LoRaWAN/FPorts/Applications")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let port = entry["FPort"].as_i64().unwrap_or_default();
            assert!(
                (FPORT_MIN..=FPORT_MAX).contains(&port),
                "case {:?}: Applications[{index}].FPort is {port}, outside FPort's published \
                 range {FPORT_MIN}..={FPORT_MAX}",
                case.label
            );
            let entry_type = entry["Type"].as_str().unwrap_or_default();
            assert!(
                APPLICATION_CONFIG_TYPES.contains(&entry_type),
                "case {:?}: Applications[{index}].Type is {entry_type:?}, not in the modelled \
                 catalog {APPLICATION_CONFIG_TYPES:?}",
                case.label
            );
            let requested_destination = case
                .body
                .pointer(&format!("/LoRaWAN/FPorts/Applications/{index}/DestinationName"));
            assert_eq!(
                entry.get("DestinationName"),
                requested_destination,
                "case {:?}: Applications[{index}].DestinationName must be the one the create \
                 named, got {:?}",
                case.label,
                entry.get("DestinationName")
            );
        }

        // `Positioning` is reported by BOTH readers, whether or not the create named one.
        assert_eq!(
            fetched["Positioning"],
            json!(case.positioning),
            "case {:?}: expected Positioning {:?} from GetWirelessDevice, got {fetched}",
            case.label,
            case.positioning
        );
        let entry = list_entry(&router, &id).await;
        assert_eq!(
            entry["Positioning"],
            json!(case.positioning),
            "case {:?}: expected Positioning {:?} on the list entry, got {entry}",
            case.label,
            case.positioning
        );
    }
}

/// Assert that every leaf the request supplied under `pointer` reads back from
/// `answered` unchanged, whatever the service assigned beside it.
fn assert_reads_back(case: &str, pointer: &str, expected: &Value, answered: &Value) {
    match expected {
        Value::Object(members) => {
            for (key, value) in members {
                assert_reads_back(case, &format!("{pointer}/{key}"), value, answered);
            }
        }
        _ => assert_eq!(
            answered.pointer(pointer),
            Some(expected),
            "case {case:?}: {pointer} must read back as created, got {:?} in {answered}",
            answered.pointer(pointer)
        ),
    }
}

/// An update mutates the stored device: the next read reports every member the update
/// named, and keeps every member it did not.
///
/// The rows differ along which member is updated (identity, the destination link, the
/// positioning status, the LoRaWAN block, the Sidewalk block), along how many members
/// one call carries, and along whether the member was set by the create at all.
#[tokio::test]
async fn an_update_is_observable_through_every_reader() {
    let router = router();
    seed_destination(&router, "UpdateDestA").await;
    seed_destination(&router, "UpdateDestB").await;

    /// (case, the update body, the JSON pointers the read must then carry)
    type Case = (&'static str, Value, Vec<(&'static str, Value)>);
    let cases: Vec<Case> = vec![
        (
            "a rename",
            json!({ "Name": "renamed" }),
            vec![("/Name", json!("renamed"))],
        ),
        (
            "a second rename of the same device",
            json!({ "Name": "renamed-again" }),
            vec![("/Name", json!("renamed-again"))],
        ),
        (
            "the same rename replayed",
            json!({ "Name": "renamed-again" }),
            vec![("/Name", json!("renamed-again"))],
        ),
        (
            "a description where the create set one",
            json!({ "Description": "updated description" }),
            vec![("/Description", json!("updated description"))],
        ),
        (
            "a new destination",
            json!({ "DestinationName": "UpdateDestB" }),
            vec![("/DestinationName", json!("UpdateDestB"))],
        ),
        (
            "the positioning status",
            json!({ "Positioning": "Enabled" }),
            vec![("/Positioning", json!("Enabled"))],
        ),
        (
            "the positioning status back again",
            json!({ "Positioning": "Disabled" }),
            vec![("/Positioning", json!("Disabled"))],
        ),
        (
            "the LoRaWAN profiles",
            json!({ "LoRaWAN": { "DeviceProfileId": "dp-updated", "ServiceProfileId": "sp-updated" } }),
            vec![
                ("/LoRaWAN/DeviceProfileId", json!("dp-updated")),
                ("/LoRaWAN/ServiceProfileId", json!("sp-updated")),
                // The member the update did not name keeps the created value.
                ("/LoRaWAN/DevEui", json!("00000000000000e1")),
            ],
        ),
        (
            "the LoRaWAN FPorts",
            json!({
                "LoRaWAN": {
                    "FPorts": {
                        "Positioning": { "ClockSync": 150, "Gnss": 151, "Stream": 152 },
                        "Applications": [{ "FPort": FPORT_MAX, "Type": "SemtechGeolocation" }],
                    },
                },
            }),
            vec![
                ("/LoRaWAN/FPorts/Positioning/ClockSync", json!(150)),
                ("/LoRaWAN/FPorts/Positioning/Gnss", json!(151)),
                ("/LoRaWAN/FPorts/Positioning/Stream", json!(152)),
                (
                    "/LoRaWAN/FPorts/Applications/0/FPort",
                    json!(FPORT_MAX),
                ),
                // `UpdateFPorts` models no `Fuota`, so the created assignment stands.
                ("/LoRaWAN/FPorts/Fuota", json!(10)),
            ],
        ),
        (
            "the frame counter of an ABP session",
            json!({ "LoRaWAN": { "AbpV1_0_x": { "FCntStart": 42 } } }),
            vec![("/LoRaWAN/AbpV1_0_x/FCntStart", json!(42))],
        ),
        (
            "a name, a description and a destination in one call",
            json!({
                "Name": "renamed-thrice",
                "Description": "one call",
                "DestinationName": "UpdateDestA",
            }),
            vec![
                ("/Name", json!("renamed-thrice")),
                ("/Description", json!("one call")),
                ("/DestinationName", json!("UpdateDestA")),
            ],
        ),
        (
            "an update that names nothing",
            json!({}),
            vec![("/Name", json!("renamed-thrice"))],
        ),
    ];

    let id = seed_device(
        &router,
        "the updated device",
        json!({
            "Type": "LoRaWAN",
            "Name": "original",
            "Description": "the created description",
            "DestinationName": "UpdateDestA",
            "LoRaWAN": {
                "DevEui": "00000000000000e1",
                "DeviceProfileId": "dp-created",
                "FPorts": { "Fuota": 10 },
                "AbpV1_0_x": { "DevAddr": "00000001", "FCntStart": 1 },
            },
        }),
    )
    .await;

    for (case, body, expectations) in &cases {
        let (status, answered) = update_device(&router, &id, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from UpdateWirelessDevice, got {status} {answered}"
        );

        let (status, fetched) = get_device(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from GetWirelessDevice, got {status} {fetched}"
        );
        for (pointer, expected) in expectations {
            assert_eq!(
                fetched.pointer(pointer),
                Some(expected),
                "case {case:?}: expected {pointer} to be {expected}, got {:?} in {fetched}",
                fetched.pointer(pointer)
            );
        }

        // The id never moves, and the list projects the same record the reader returns.
        let entry = list_entry(&router, &id).await;
        for member in ["Name", "DestinationName", "Positioning", "Type", "Arn"] {
            assert_eq!(
                entry.get(member),
                fetched.get(member),
                "case {case:?}: the list entry's {member} must match the reader's, got \
                 {entry} vs {fetched}"
            );
        }
    }
}

/// An update is refused when what it names does not resolve, and a refused update
/// changes nothing.
#[tokio::test]
async fn an_update_naming_an_unresolvable_resource_is_refused_and_changes_nothing() {
    let router = router();
    seed_destination(&router, "RefuseDest").await;
    let id = seed_device(
        &router,
        "the refused device",
        json!({
            "Type": "LoRaWAN",
            "Name": "untouched",
            "DestinationName": "RefuseDest",
            "LoRaWAN": { "DevEui": "00000000000000e2" },
        }),
    )
    .await;

    // (case, the id to address, the body, the expected status and error code)
    let cases: Vec<(&str, String, Value, StatusCode, &str)> = vec![
        (
            "an unknown device id",
            "00000000-0000-4000-8000-000000000000".to_string(),
            json!({ "Name": "nope" }),
            StatusCode::NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "an unknown destination",
            id.clone(),
            json!({ "DestinationName": "NoSuchDest" }),
            StatusCode::NOT_FOUND,
            "ResourceNotFoundException",
        ),
        (
            "a positioning status outside the modelled enum",
            id.clone(),
            json!({ "Positioning": "enabled" }),
            StatusCode::BAD_REQUEST,
            "ValidationException",
        ),
    ];

    for (case, target, body, expected_status, expected_code) in &cases {
        let (status, answered) = update_device(&router, target, body.clone()).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {answered}"
        );

        let (status, fetched) = get_device(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: the device must still read back, got {status} {fetched}"
        );
        assert_eq!(
            fetched["Name"],
            json!("untouched"),
            "case {case:?}: a refused update must change nothing, got {fetched}"
        );
        assert_eq!(
            fetched["DestinationName"],
            json!("RefuseDest"),
            "case {case:?}: a refused update must change nothing, got {fetched}"
        );
    }
}

/// A device identifier that resolves to nothing is refused with ONE sentence, whatever
/// identifier kind addressed it and whether the device never existed or was deleted.
#[tokio::test]
async fn an_unresolved_device_identifier_is_refused_with_one_fixed_sentence() {
    let router = router();
    seed_destination(&router, "GoneDest").await;
    let id = seed_device(
        &router,
        "the deleted device",
        json!({
            "Type": "LoRaWAN",
            "Name": "to-be-deleted",
            "DestinationName": "GoneDest",
            "LoRaWAN": { "DevEui": "00000000000000e3" },
        }),
    )
    .await;
    let (status, answered) = call(
        &router,
        Method::DELETE,
        &format!("/wireless-devices/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "deleting the device: expected 204, got {status} {answered}"
    );

    // Every modelled identifier kind, each on the deleted device's own identifiers and
    // on one no device ever carried.
    let mut cases: Vec<(String, String, String)> = Vec::new();
    for identifier_type in WIRELESS_DEVICE_ID_TYPES {
        cases.push((
            format!("{identifier_type}, the deleted device"),
            (*identifier_type).to_string(),
            id.clone(),
        ));
        cases.push((
            format!("{identifier_type}, an identifier no device ever carried"),
            (*identifier_type).to_string(),
            "0000000000000000".to_string(),
        ));
    }
    cases.push((
        "DevEui, the deleted device's own DevEui".to_string(),
        "DevEui".to_string(),
        "00000000000000e3".to_string(),
    ));

    for (case, identifier_type, identifier) in &cases {
        let (status, answered) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices/{identifier}?identifierType={identifier_type}"),
            None,
        )
        .await;
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
        assert_eq!(
            answered["Message"],
            json!(WIRELESS_DEVICE_NOT_FOUND_MESSAGE),
            "case {case:?}: expected Message {WIRELESS_DEVICE_NOT_FOUND_MESSAGE:?}, got \
             {answered}"
        );
        // The sentence is fixed whatever addressed the device, while the two optional
        // members report the identifier the request supplied and the kind it addressed.
        assert_eq!(
            answered["ResourceId"],
            json!(identifier),
            "case {case:?}: expected ResourceId {identifier:?}, got {answered}"
        );
        assert_eq!(
            answered["ResourceType"],
            json!(WIRELESS_DEVICE_RESOURCE_TYPE),
            "case {case:?}: expected ResourceType {WIRELESS_DEVICE_RESOURCE_TYPE:?}, got \
             {answered}"
        );
    }
}
