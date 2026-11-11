// SPDX-License-Identifier: CC0-1.0

//! How `CreateWirelessDevice` refuses an incoherent create, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * LoRaWAN key material is specification-version specific, so a create whose
//!   provisioning member (`OtaaV1_1`/`AbpV1_1` for 1.1, `OtaaV1_0_x`/`AbpV1_0_x` for
//!   1.0.x) names a version the referenced device profile's `MacVersion` family does
//!   not is refused `ValidationException` and persists nothing, while every create
//!   whose two versions agree — and every create that constrains neither side — is
//!   accepted and reads back against that profile;
//! * a refusal carries the sentence of its error class, so a duplicate `DevEui`
//!   answers one `ConflictException` message whatever the request and the store hold,
//!   and discloses nothing about the device that holds the identifier.
//!
//! The wire harness is the one the destination/device suite already has: `tests/all/*.rs`
//! are modules of ONE test target, so `router`, `call` and `error_code` are shared from
//! there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// AWS's sentence for a `DevEui` already registered in the account — fixed text whose
/// only variable part is the identifier the request itself supplied, spelled out here so
/// the suite pins the wording rather than whatever the service happens to emit.
fn duplicate_dev_eui_message(dev_eui: &str) -> String {
    format!("DevEui {dev_eui}  with same key has been claimed")
}

/// AWS's sentence for key material the referenced device profile's `MacVersion` cannot
/// provision. The only variable part is that stored `MacVersion`.
fn mac_version_mismatch_message(mac_version: &str) -> String {
    format!(
        "Validation error: the input profile version is not match macVersion defines in \
         deviceProfile: {mac_version}"
    )
}

async fn seed_destination(router: &MultiServiceRouter, name: &str) -> String {
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
    name.to_string()
}

/// Create a LoRaWAN device profile whose `LoRaWAN` member is `lorawan`, returning its
/// minted `Id`.
async fn seed_device_profile(router: &MultiServiceRouter, case: &str, lorawan: Value) -> String {
    let (status, body) = call(
        router,
        Method::POST,
        "/device-profiles",
        Some(json!({ "LoRaWAN": lorawan })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "case {case:?}: seeding the device profile expected 201, got {status} {body}"
    );
    body["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created profile must carry an Id: {body}"))
        .to_string()
}

async fn create_device(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/wireless-devices", Some(body)).await
}

/// `GetWirelessDevice` by `DevEui`, which is how a refused create is shown to have
/// persisted nothing.
async fn get_by_dev_eui(router: &MultiServiceRouter, dev_eui: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/wireless-devices/{dev_eui}?identifierType=DevEui"),
        None,
    )
    .await
}

/// One LoRaWAN 1.1 key-material member and one 1.0.x member of each provisioning kind,
/// with a body the model's shapes accept.
/// Source: `smithy_query(query="shape", shape="LoRaWANDevice")` — `OtaaV1_1`,
/// `OtaaV1_0_x`, `AbpV1_1`, `AbpV1_0_x`.
fn provisioning_member(member: &str) -> Value {
    match member {
        "OtaaV1_1" => json!({
            "AppKey": "00000000000000000000000000000011",
            "NwkKey": "00000000000000000000000000000012",
            "JoinEui": "0000000000000013",
        }),
        "OtaaV1_0_x" => json!({
            "AppKey": "00000000000000000000000000000021",
            "AppEui": "0000000000000022",
        }),
        "AbpV1_1" => json!({
            "DevAddr": "00000031",
            "SessionKeys": {
                "FNwkSIntKey": "00000000000000000000000000000032",
                "SNwkSIntKey": "00000000000000000000000000000033",
                "NwkSEncKey": "00000000000000000000000000000034",
                "AppSKey": "00000000000000000000000000000035",
            },
        }),
        "AbpV1_0_x" => json!({
            "DevAddr": "00000041",
            "SessionKeys": {
                "NwkSKey": "00000000000000000000000000000042",
                "AppSKey": "00000000000000000000000000000043",
            },
        }),
        other => panic!("no provisioning body for member {other:?}"),
    }
}

/// One create: the profile's `MacVersion` (`None` when the profile declares none), the
/// provisioning members the request carries (empty when it carries none), and whether
/// every one of them agrees with the profile.
struct VersionCase {
    case: &'static str,
    mac_version: Option<&'static str>,
    provisioning: &'static [&'static str],
    accepted: bool,
}

/// The `MacVersion` values below are the LoRaWAN MAC revisions a device profile
/// declares; `1.0` and `1.1` are the two families, `1.0.2`/`1.0.3`/`1.0.4` revisions of
/// the first.
/// Source: `LoRaWANDeviceProfile.MacVersion`'s model doc ("The MAC version (such as
/// OTAA 1.1 or OTAA 1.0.3) to use with this device profile").
fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            case: "OtaaV1_1 against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_1"],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_1 against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &["OtaaV1_1"],
            accepted: false,
        },
        VersionCase {
            case: "OtaaV1_0_x against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &["OtaaV1_0_x"],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_0_x against a 1.0 profile",
            mac_version: Some("1.0"),
            provisioning: &["OtaaV1_0_x"],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_0_x against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "AbpV1_1 against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["AbpV1_1"],
            accepted: true,
        },
        VersionCase {
            case: "AbpV1_1 against a 1.0.2 profile",
            mac_version: Some("1.0.2"),
            provisioning: &["AbpV1_1"],
            accepted: false,
        },
        VersionCase {
            case: "AbpV1_0_x against a 1.0.4 profile",
            mac_version: Some("1.0.4"),
            provisioning: &["AbpV1_0_x"],
            accepted: true,
        },
        VersionCase {
            case: "AbpV1_0_x against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["AbpV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "OtaaV1_1 and AbpV1_1 together against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_1", "AbpV1_1"],
            accepted: true,
        },
        // Key material for two families cannot agree with a profile that declares one,
        // whichever of the two the profile names.
        VersionCase {
            case: "OtaaV1_1 and OtaaV1_0_x together against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_1", "OtaaV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "AbpV1_1 and AbpV1_0_x together against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &["AbpV1_1", "AbpV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "no provisioning member against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &[],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_1 against a profile that declares no MacVersion",
            mac_version: None,
            provisioning: &["OtaaV1_1"],
            accepted: true,
        },
        // `MacVersion` is `length 0..64`, so the empty string declares nothing and
        // constrains nothing.
        VersionCase {
            case: "OtaaV1_1 against a profile whose MacVersion is empty",
            mac_version: Some(""),
            provisioning: &["OtaaV1_1"],
            accepted: true,
        },
    ]
}

/// The key material a create carries must be the version the referenced device
/// profile's `MacVersion` provisions: a mismatch is a 400 that persists nothing, a match
/// is a 201 that reads back against that profile.
#[tokio::test]
async fn provisioning_version_must_agree_with_the_referenced_profiles_mac_version() {
    let router = router();
    seed_destination(&router, "VersionDest").await;

    for (index, case) in version_cases().into_iter().enumerate() {
        let VersionCase {
            case,
            mac_version,
            provisioning,
            accepted,
        } = case;

        let profile_lorawan = match mac_version {
            Some(version) => json!({ "MacVersion": version, "RegParamsRevision": "RP002-1.0.1" }),
            None => json!({ "RegParamsRevision": "RP002-1.0.1" }),
        };
        let profile_id = seed_device_profile(&router, case, profile_lorawan).await;

        let dev_eui = format!("00000000000001{index:02x}");
        let mut lorawan = json!({
            "DevEui": dev_eui,
            "DeviceProfileId": profile_id,
        });
        for member in provisioning {
            lorawan[*member] = provisioning_member(member);
        }
        let (status, response) = create_device(
            &router,
            json!({
                "Type": "LoRaWAN",
                "DestinationName": "VersionDest",
                "LoRaWAN": lorawan,
            }),
        )
        .await;

        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201, got {status} {response}"
            );
            let id = response["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got {response}"))
                .to_string();
            let (status, fetched) = call(
                &router,
                Method::GET,
                &format!("/wireless-devices/{id}?identifierType=WirelessDeviceId"),
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: the accepted device must read back, got {status} {fetched}"
            );
            assert_eq!(
                fetched.pointer("/LoRaWAN/DeviceProfileId"),
                Some(&json!(profile_id)),
                "case {case:?}: expected DeviceProfileId {profile_id:?}, got {:?}",
                fetched.pointer("/LoRaWAN/DeviceProfileId")
            );
            continue;
        }

        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        let expected = mac_version_mismatch_message(mac_version.unwrap_or_default());
        assert_eq!(
            response["Message"],
            json!(expected),
            "case {case:?}: expected Message {expected:?}, got {response}"
        );

        // A refused create persisted nothing: the rejected DevEui resolves to no device,
        // and no entry of the list carries it.
        let (status, fetched) = get_by_dev_eui(&router, &dev_eui).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {case:?}: the refused DevEui must resolve to nothing, got {status} {fetched}"
        );
        let (_, listed) = call(&router, Method::GET, "/wireless-devices", None).await;
        let carries_rejected = listed["WirelessDeviceList"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.pointer("/LoRaWAN/DevEui") == Some(&json!(dev_eui)))
            })
            .unwrap_or(false);
        assert!(
            !carries_rejected,
            "case {case:?}: the refused device must be absent from the list, got {listed}"
        );
    }
}

/// A duplicate `DevEui` is one conflict class, so its refusal carries one sentence
/// whatever the rest of the request holds — naming the identifier the request supplied
/// and nothing about the device that already holds it.
#[tokio::test]
async fn a_duplicate_dev_eui_conflict_carries_one_fixed_sentence_for_every_input() {
    let router = router();
    seed_destination(&router, "ConflictDestA").await;
    seed_destination(&router, "ConflictDestB").await;

    // The rows differ along the identifier's case and along what the SECOND create
    // carries beside it: a conflict class has one sentence across all of them.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "lowercase hex, an otherwise identical retry",
            "00000000000000a1",
            json!({}),
        ),
        (
            "uppercase hex, a different name and destination",
            "ABCDEF0123456789",
            json!({ "Name": "second", "DestinationName": "ConflictDestB" }),
        ),
        (
            "mixed-case hex, a described second create",
            "aBcDeF0123456789",
            json!({ "Description": "a second claimant" }),
        ),
        ("all-f hex, a tagged second create", "ffffffffffffffff", {
            json!({ "Tags": [{ "Key": "owner", "Value": "second" }] })
        }),
    ];

    for (case, dev_eui, extra) in cases {
        let (status, first) = create_device(
            &router,
            json!({
                "Type": "LoRaWAN",
                "Name": "first",
                "DestinationName": "ConflictDestA",
                "LoRaWAN": { "DevEui": dev_eui },
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: the first create must be 201, got {status} {first}"
        );
        let owner = first["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got {first}"))
            .to_string();

        let mut body = json!({
            "Type": "LoRaWAN",
            "DestinationName": "ConflictDestA",
            "LoRaWAN": { "DevEui": dev_eui },
        });
        for (key, value) in extra.as_object().expect("the extra members are an object") {
            body[key.as_str()] = value.clone();
        }
        let (status, response) = create_device(&router, body).await;

        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case:?}: expected 409, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response}"
        );
        let expected_message = duplicate_dev_eui_message(dev_eui);
        assert_eq!(
            response["Message"],
            json!(expected_message),
            "case {case:?}: expected Message {expected_message:?}, got {response}"
        );
        let message = response["Message"].as_str().unwrap_or_default().to_string();
        assert!(
            !message.contains(&owner),
            "case {case:?}: the refusal must not name the identifier's owner, got {message:?}"
        );
        assert!(
            !response.to_string().contains(&owner),
            "case {case:?}: the refusal must disclose nothing about the owning device, got \
             {response}"
        );

        // The identifier still resolves to the device that claimed it first.
        let (status, resolved) = get_by_dev_eui(&router, dev_eui).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: the identifier must still resolve, got {status} {resolved}"
        );
        assert_eq!(
            resolved["Id"],
            json!(owner),
            "case {case:?}: expected the first device {owner:?}, got {:?}",
            resolved["Id"]
        );
    }
}
