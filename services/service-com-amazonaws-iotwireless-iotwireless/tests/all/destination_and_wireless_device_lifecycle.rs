// SPDX-License-Identifier: CC0-1.0

//! The destination + wireless-device lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * a create persists the record and a later read returns exactly what was stored;
//! * a resource key (`Destination.Name`, `LoRaWAN.DevEui`) identifies at most one
//!   resource, so a second claimant is refused rather than overwriting the first;
//! * a reference to another resource (`CreateWirelessDevice.DestinationName`) is
//!   resolved against real state;
//! * a paginated list enumerates every stored record exactly once, in one stable
//!   order, across any page size;
//! * a delete is observable by the next caller, through every reader.

use std::sync::Arc;

use bytes::Bytes;
use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The store ceilings the service enforces, so this suite asserts the bound the
/// implementation declares rather than a number copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_POSITIONING_CONFIG_STATUS, DEFAULT_REGION, MAX_DESTINATIONS, MAX_WIRELESS_DEVICES,
};

/// The region the test's SigV4 credential scope names; every minted ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";

/// The account segment every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// `ExpressionType`'s modelled values — the model-generated constant the handlers
/// validate against, so the suite and the service cannot drift apart.
const EXPRESSION_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::EXPRESSION_TYPE_VALUES;

/// `WirelessDeviceType`'s modelled values, from the same generated source.
const WIRELESS_DEVICE_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::WIRELESS_DEVICE_TYPE_VALUES;

/// The identifier kinds `GetWirelessDevice` accepts, from the same generated source.
const WIRELESS_DEVICE_ID_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::WIRELESS_DEVICE_ID_TYPE_VALUES;

/// `MaxResults`' published range on `ListWirelessDevices`.
/// Source: `smithy_query(query="operation", operation="ListWirelessDevices")`.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 250;

pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iotwireless_iotwireless::EmulatedService::new(engine),
    );
    router
}

/// A request that routes to this service. `scope_region` is the region its SigV4
/// credential scope names; `None` leaves the request unsigned, so it routes by its
/// host-style endpoint and a minted ARN falls back to [`DEFAULT_REGION`].
pub(crate) fn request_signed_for(
    scope_region: Option<&str>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> http::Request<Bytes> {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
        .unwrap_or_default();
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!(
                "iotwireless.{}.amazonaws.com",
                scope_region.unwrap_or(DEFAULT_REGION)
            ),
        )
        .header("content-type", "application/json");
    if let Some(region) = scope_region {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/iotwireless/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        );
    }
    builder.body(payload).expect("build request")
}

/// A request signed for [`REGION`], which is what every case but the region table uses.
fn request(method: Method, uri: &str, body: Option<Value>) -> http::Request<Bytes> {
    request_signed_for(Some(REGION), method, uri, body)
}

/// Dispatch one call and return its status plus its parsed JSON body (`Value::Null`
/// for an empty body, which is what a 204 carries).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), or `"<no __type>"`.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("<no __type>")
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and
/// `DEFAULT_REGION` when the request carries no parseable scope — for both resources
/// this slice mints.
#[tokio::test]
async fn a_minted_arn_carries_the_region_the_request_names() {
    // (case, credential-scope region, the region the ARN must carry)
    let cases = [
        ("signed for us-west-2", Some("us-west-2"), "us-west-2"),
        ("signed for eu-central-1", Some("eu-central-1"), "eu-central-1"),
        ("signed for ap-southeast-2", Some("ap-southeast-2"), "ap-southeast-2"),
        ("unsigned, routed by endpoint", None, DEFAULT_REGION),
    ];

    for (case, scope_region, expected_region) in cases {
        let router = router();

        let response = router
            .dispatch(request_signed_for(
                scope_region,
                Method::POST,
                "/destinations",
                Some(destination_body("RegionDest", "RuleName")),
            ))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "case {case}: expected 201 from CreateDestination"
        );
        let created: Value =
            serde_json::from_slice(&response.into_body()).expect("a JSON response");
        assert_eq!(
            created["Arn"],
            json!(format!(
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:Destination/RegionDest"
            )),
            "case {case}: expected the destination ARN in {expected_region}, got {:?}",
            created["Arn"]
        );

        let response = router
            .dispatch(request_signed_for(
                scope_region,
                Method::POST,
                "/wireless-devices",
                Some(json!({ "Type": "LoRaWAN", "DestinationName": "RegionDest" })),
            ))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "case {case}: expected 201 from CreateWirelessDevice"
        );
        let device: Value =
            serde_json::from_slice(&response.into_body()).expect("a JSON response");
        let id = device["Id"].as_str().expect("an Id");
        assert_eq!(
            device["Arn"],
            json!(format!(
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:WirelessDevice/{id}"
            )),
            "case {case}: expected the device ARN in {expected_region}, got {:?}",
            device["Arn"]
        );
    }
}

async fn create_destination(
    router: &MultiServiceRouter,
    body: Value,
) -> (StatusCode, Value) {
    call(router, Method::POST, "/destinations", Some(body)).await
}

async fn create_device(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/wireless-devices", Some(body)).await
}

/// A minimal valid destination body under `name`.
fn destination_body(name: &str, expression_type: &str) -> Value {
    json!({
        "Name": name,
        "ExpressionType": expression_type,
        "Expression": format!("rule-for-{name}"),
        "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
    })
}

/// Create a destination and return its name, panicking on anything but 201.
async fn seed_destination(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = create_destination(router, destination_body(name, "RuleName")).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding destination {name}: expected 201, got {status} {body}"
    );
    name.to_string()
}

// ---------------------------------------------------------------------------
// Destinations
// ---------------------------------------------------------------------------

/// A create persists the record and `GetDestination` returns exactly what was stored,
/// across the whole `ExpressionType` catalog and both presence states of the optional
/// `Description` — including `MqttTopic`, which the issue's reproduction never used.
#[tokio::test]
async fn a_destination_round_trips_through_get_for_every_expression_type() {
    let router = router();

    struct Case {
        name: &'static str,
        expression_type: &'static str,
        expression: &'static str,
        description: Option<&'static str>,
        role_arn: &'static str,
    }

    let cases = [
        Case {
            name: "RuleNameNoDescription",
            expression_type: "RuleName",
            expression: "MyRule",
            description: None,
            role_arn: "arn:aws:iam::000000000000:role/iotwireless",
        },
        Case {
            name: "RuleName_With-Description",
            expression_type: "RuleName",
            expression: "another/rule",
            description: Some("a rule-backed destination"),
            role_arn: "arn:aws:iam::000000000000:role/iotwireless-other",
        },
        Case {
            name: "MqttTopicDestination",
            expression_type: "MqttTopic",
            expression: "device/+/uplink",
            description: Some(""),
            role_arn: "arn:aws:iam::000000000000:role/iotwireless-mqtt",
        },
    ];

    for case in &cases {
        assert!(
            EXPRESSION_TYPES.contains(&case.expression_type),
            "case {}: ExpressionType {:?} is not in the modelled catalog {EXPRESSION_TYPES:?}",
            case.name,
            case.expression_type
        );

        let mut body = json!({
            "Name": case.name,
            "ExpressionType": case.expression_type,
            "Expression": case.expression,
            "RoleArn": case.role_arn,
        });
        if let Some(description) = case.description {
            body["Description"] = json!(description);
        }

        let (status, created) = create_destination(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {}: expected 201, got {status} {created}",
            case.name
        );

        let expected_arn =
            format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:Destination/{}", case.name);
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {}: expected Arn {expected_arn}, got {:?}",
            case.name,
            created["Arn"]
        );
        assert_eq!(
            created["Name"],
            json!(case.name),
            "case {}: expected Name {:?}, got {:?}",
            case.name,
            case.name,
            created["Name"]
        );

        let (status, fetched) =
            call(&router, Method::GET, &format!("/destinations/{}", case.name), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {}: expected 200 from GetDestination, got {status} {fetched}",
            case.name
        );
        assert_eq!(
            fetched["Arn"],
            json!(expected_arn),
            "case {}: GetDestination Arn must match the created ARN, got {:?}",
            case.name,
            fetched["Arn"]
        );
        assert_eq!(
            fetched["Expression"],
            json!(case.expression),
            "case {}: expected Expression {:?}, got {:?}",
            case.name,
            case.expression,
            fetched["Expression"]
        );
        assert_eq!(
            fetched["ExpressionType"],
            json!(case.expression_type),
            "case {}: expected ExpressionType {:?}, got {:?}",
            case.name,
            case.expression_type,
            fetched["ExpressionType"]
        );
        assert_eq!(
            fetched["RoleArn"],
            json!(case.role_arn),
            "case {}: expected RoleArn {:?}, got {:?}",
            case.name,
            case.role_arn,
            fetched["RoleArn"]
        );
        assert_eq!(
            fetched.get("Description").cloned(),
            case.description.map(|d| json!(d)),
            "case {}: expected Description {:?}, got {:?}",
            case.name,
            case.description,
            fetched.get("Description")
        );
    }
}

/// A destination name identifies at most one destination: a second create under a name
/// already in the store is a 409, and it must NOT overwrite the stored record.
#[tokio::test]
async fn a_second_destination_under_one_name_conflicts_and_changes_nothing() {
    let router = router();

    for expression_type in EXPRESSION_TYPES {
        let name = format!("Dup{expression_type}");
        let (status, _) =
            create_destination(&router, destination_body(&name, expression_type)).await;
        assert_eq!(status, StatusCode::CREATED, "case {name}: first create");

        let mut retry = destination_body(&name, expression_type);
        retry["Expression"] = json!("a-different-rule");
        let (status, body) = create_destination(&router, retry).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {name}: a duplicate name must be 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {name}: expected ConflictException, got {body}"
        );

        let (_, fetched) =
            call(&router, Method::GET, &format!("/destinations/{name}"), None).await;
        assert_eq!(
            fetched["Expression"],
            json!(format!("rule-for-{name}")),
            "case {name}: the refused create must not have overwritten Expression, got {:?}",
            fetched["Expression"]
        );
    }
}

/// `GetDestination` and `DeleteDestination` answer 404 for a name the store does not
/// hold — whatever the name looks like.
#[tokio::test]
async fn an_unknown_destination_name_is_not_found_by_get_and_delete() {
    let router = router();
    seed_destination(&router, "Present").await;

    for name in ["Absent", "present", "P", "a-very_long-9999-name"] {
        for method in [Method::GET, Method::DELETE] {
            let (status, body) =
                call(&router, method.clone(), &format!("/destinations/{name}"), None).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "case {method} {name}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {method} {name}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// Deleting a destination is observable by the next caller: 204, then 404 from Get.
#[tokio::test]
async fn deleting_a_destination_is_observable_by_the_next_get() {
    let router = router();
    for expression_type in EXPRESSION_TYPES {
        let name = format!("Transient{expression_type}");
        create_destination(&router, destination_body(&name, expression_type)).await;

        let (status, body) =
            call(&router, Method::DELETE, &format!("/destinations/{name}"), None).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {name}: expected 204 from DeleteDestination, got {status} {body}"
        );

        let (status, body) =
            call(&router, Method::GET, &format!("/destinations/{name}"), None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {name}: a deleted destination must be 404, got {status} {body}"
        );
    }
}

/// `ExpressionType` outside the modelled enum is refused with 400, not stored.
#[tokio::test]
async fn an_unmodelled_expression_type_is_refused() {
    let router = router();
    for bad in ["rulename", "Rule Name", "SqsQueue", ""] {
        let mut body = destination_body("BadExpressionType", "RuleName");
        body["ExpressionType"] = json!(bad);
        let (status, response) = create_destination(&router, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {bad:?}: expected 400, got {status} {response}"
        );

        let (status, _) =
            call(&router, Method::GET, "/destinations/BadExpressionType", None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {bad:?}: a refused create must store nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Wireless devices
// ---------------------------------------------------------------------------

/// A device create persists the whole record and every supported identifier resolves
/// back to it: `WirelessDeviceId` for both device types, `DevEui` for a LoRaWAN
/// device, `SidewalkManufacturingSn` for a Sidewalk one.
#[tokio::test]
async fn a_wireless_device_round_trips_through_get_for_every_device_type() {
    let router = router();
    seed_destination(&router, "LoRaDest").await;
    seed_destination(&router, "SidewalkDest").await;

    struct Case {
        label: &'static str,
        body: fn() -> Value,
        /// (identifierType, how to read the identifier out of the GET response)
        extra_lookups: &'static [(&'static str, &'static str)],
    }

    let cases = [
        Case {
            label: "LoRaWAN with DevEui and OTAA keys",
            body: || {
                json!({
                    "Type": "LoRaWAN",
                    "Name": "dev1",
                    "Description": "a LoRaWAN device",
                    "DestinationName": "LoRaDest",
                    "Positioning": "Enabled",
                    "LoRaWAN": {
                        "DevEui": "0000000000000001",
                        "DeviceProfileId": "dp-1",
                        "ServiceProfileId": "sp-1",
                        "OtaaV1_0_x": {
                            "AppKey": "00000000000000000000000000000001",
                            "AppEui": "0000000000000002",
                        },
                    },
                })
            },
            extra_lookups: &[("DevEui", "LoRaWAN/DevEui")],
        },
        Case {
            label: "LoRaWAN with no name, no description and no LoRaWAN block",
            body: || {
                json!({
                    "Type": "LoRaWAN",
                    "DestinationName": "LoRaDest",
                })
            },
            extra_lookups: &[],
        },
        Case {
            label: "Sidewalk with a manufacturing serial number",
            body: || {
                json!({
                    "Type": "Sidewalk",
                    "Name": "sidewalk-1",
                    "DestinationName": "SidewalkDest",
                    "Positioning": "Disabled",
                    "Sidewalk": {
                        "DeviceProfileId": "dp-sidewalk",
                        "SidewalkManufacturingSn": "SMSN0000000000000001",
                    },
                })
            },
            extra_lookups: &[("SidewalkManufacturingSn", "Sidewalk/SidewalkManufacturingSn")],
        },
    ];

    for case in &cases {
        let requested = (case.body)();
        assert!(
            WIRELESS_DEVICE_TYPES
                .contains(&requested["Type"].as_str().expect("Type is a string")),
            "case {}: Type is not in the modelled catalog {WIRELESS_DEVICE_TYPES:?}",
            case.label
        );

        let (status, created) = create_device(&router, requested.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {}: expected 201, got {status} {created}",
            case.label
        );
        let id = created["Id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty(),
            "case {}: CreateWirelessDevice must mint an Id, got {created}",
            case.label
        );
        let expected_arn = format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:WirelessDevice/{id}");
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {}: expected Arn {expected_arn}, got {:?}",
            case.label,
            created["Arn"]
        );

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
            "case {}: expected 200 from GetWirelessDevice, got {status} {fetched}",
            case.label
        );
        assert_eq!(
            fetched["Id"],
            json!(id),
            "case {}: expected Id {id}, got {:?}",
            case.label,
            fetched["Id"]
        );
        assert_eq!(
            fetched["Arn"],
            json!(expected_arn),
            "case {}: GetWirelessDevice Arn must match the created ARN, got {:?}",
            case.label,
            fetched["Arn"]
        );

        // Every member the create supplied must come back byte-identical.
        for member in ["Type", "Name", "Description", "DestinationName"] {
            assert_eq!(
                fetched.get(member).cloned(),
                requested.get(member).cloned(),
                "case {}: {member} must read back as created, got {:?}",
                case.label,
                fetched.get(member)
            );
        }
        // `Positioning` is a status every device carries, so the reader reports the one
        // the create named, or this service's default when it named none.
        let expected_positioning = requested
            .get("Positioning")
            .cloned()
            .unwrap_or_else(|| json!(DEFAULT_POSITIONING_CONFIG_STATUS));
        assert_eq!(
            fetched["Positioning"],
            expected_positioning,
            "case {}: expected Positioning {expected_positioning}, got {:?}",
            case.label,
            fetched.get("Positioning")
        );
        for member in ["LoRaWAN", "Sidewalk"] {
            if let Some(expected) = requested.get(member) {
                let actual = fetched
                    .get(member)
                    .unwrap_or_else(|| panic!("case {}: {member} is missing", case.label));
                for (key, value) in expected.as_object().expect("a structure") {
                    assert_eq!(
                        actual.get(key),
                        Some(value),
                        "case {}: {member}/{key} must read back as created, got {:?}",
                        case.label,
                        actual.get(key)
                    );
                }
            }
        }

        for (identifier_type, pointer) in case.extra_lookups {
            let identifier = fetched
                .pointer(&format!("/{pointer}"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {}: {pointer} is absent from {fetched}", case.label)
                });
            let (status, by_alternate) = call(
                &router,
                Method::GET,
                &format!("/wireless-devices/{identifier}?identifierType={identifier_type}"),
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {}: identifierType={identifier_type} must resolve, got {status} \
                 {by_alternate}",
                case.label
            );
            assert_eq!(
                by_alternate["Id"],
                json!(id),
                "case {}: identifierType={identifier_type} resolved to {:?}, expected {id}",
                case.label,
                by_alternate["Id"]
            );
        }
    }
}

/// The destination link is checked against real state: a `DestinationName` the store
/// does not hold is a 404 and nothing is created.
#[tokio::test]
async fn a_device_against_an_unknown_destination_is_not_found() {
    let router = router();
    seed_destination(&router, "KnownDest").await;

    for destination in ["NoSuchDest", "knowndest", "KnownDes", "Known-Dest"] {
        let (status, body) = create_device(
            &router,
            json!({ "Type": "LoRaWAN", "DestinationName": destination }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {destination}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {destination}: expected ResourceNotFoundException, got {body}"
        );
    }

    let (_, listed) = call(&router, Method::GET, "/wireless-devices", None).await;
    assert_eq!(
        listed["WirelessDeviceList"],
        json!([]),
        "a refused create must store nothing, got {listed}"
    );

    // Every `DestinationName` in the request is the same kind of reference to the same
    // resource, so each one is resolved against the destination store: the one a
    // Sidewalk positioning configuration routes fixes to, and the one each LoRaWAN
    // application `FPort` routes frames to, not only the top-level member.
    // Source: smithy_query(query="shape", shape="SidewalkPositioning,ApplicationConfig").
    let nested = [
        (
            "Sidewalk.Positioning.DestinationName",
            json!({
                "Type": "Sidewalk",
                "DestinationName": "KnownDest",
                "Sidewalk": { "Positioning": { "DestinationName": "NoSuchDest" } },
            }),
        ),
        (
            "LoRaWAN.FPorts.Applications[0].DestinationName",
            json!({
                "Type": "LoRaWAN",
                "DestinationName": "KnownDest",
                "LoRaWAN": { "FPorts": { "Applications": [
                    { "FPort": 1, "Type": "SemtechGeolocation", "DestinationName": "NoSuchDest" },
                ] } },
            }),
        ),
        (
            "LoRaWAN.FPorts.Applications[1].DestinationName",
            json!({
                "Type": "LoRaWAN",
                "DestinationName": "KnownDest",
                "LoRaWAN": { "FPorts": { "Applications": [
                    { "FPort": 1, "Type": "SemtechGeolocation", "DestinationName": "KnownDest" },
                    { "FPort": 2, "Type": "SemtechGeolocation", "DestinationName": "NoSuchDest" },
                ] } },
            }),
        ),
    ];
    for (member, body) in &nested {
        let (status, response) = create_device(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {member}: an unknown nested destination must be 404, got {status} \
             {response}"
        );
    }

    // The same requests succeed once every referenced destination exists.
    for (member, body) in &nested {
        let mut body = body.clone();
        for pointer in [
            "/Sidewalk/Positioning/DestinationName",
            "/LoRaWAN/FPorts/Applications/0/DestinationName",
            "/LoRaWAN/FPorts/Applications/1/DestinationName",
        ] {
            if let Some(slot) = body.pointer_mut(pointer) {
                *slot = json!("KnownDest");
            }
        }
        let (status, response) = create_device(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {member}: a known nested destination must be accepted, got {status} \
             {response}"
        );
    }
}

/// A `DevEui` identifies at most one device, so a second device claiming one already
/// registered is refused and the identifier keeps resolving to the first device.
#[tokio::test]
async fn a_duplicate_dev_eui_is_refused_and_keeps_resolving_to_the_first_device() {
    let router = router();
    seed_destination(&router, "EuiDest").await;

    for dev_eui in ["0000000000000001", "ABCDEF0123456789", "ffffffffffffffff"] {
        let body = json!({
            "Type": "LoRaWAN",
            "DestinationName": "EuiDest",
            "LoRaWAN": { "DevEui": dev_eui },
        });
        let (status, first) = create_device(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {dev_eui}: first create, got {status} {first}"
        );
        let first_id = first["Id"].as_str().unwrap_or_default().to_string();

        let (status, second) = create_device(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {dev_eui}: a duplicate DevEui must be 409, got {status} {second}"
        );

        let (_, resolved) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices/{dev_eui}?identifierType=DevEui"),
            None,
        )
        .await;
        assert_eq!(
            resolved["Id"],
            json!(first_id),
            "case {dev_eui}: the identifier must still resolve to the first device, got {:?}",
            resolved["Id"]
        );
    }
}

/// Every modelled `IdentifierType` is accepted and answers 404 for an identifier no
/// device carries; a value outside the enum is a 400.
#[tokio::test]
async fn every_modelled_identifier_type_is_accepted_and_resolves_only_real_devices() {
    let router = router();
    seed_destination(&router, "IdDest").await;
    create_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "IdDest",
                "LoRaWAN": { "DevEui": "0000000000000009" } }),
    )
    .await;

    // Source: the model-generated WIRELESS_DEVICE_ID_TYPE_VALUES.
    for identifier_type in WIRELESS_DEVICE_ID_TYPES {
        let (status, body) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices/absent-identifier?identifierType={identifier_type}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {identifier_type}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {identifier_type}: expected ResourceNotFoundException, got {body}"
        );
    }

    for bad in ["wirelessdeviceid", "DeviceId", "Arn"] {
        let (status, body) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices/anything?identifierType={bad}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {bad}: an unmodelled identifierType must be 400, got {status} {body}"
        );
    }
}

/// A delete is observable by the next caller through every reader, and frees the
/// device's `DevEui`.
#[tokio::test]
async fn deleting_a_device_removes_it_from_get_list_and_the_dev_eui_index() {
    let router = router();
    seed_destination(&router, "DelDest").await;

    let (_, created) = create_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "DelDest",
                "LoRaWAN": { "DevEui": "00000000000000aa" } }),
    )
    .await;
    let id = created["Id"].as_str().expect("an Id").to_string();

    let (status, body) =
        call(&router, Method::DELETE, &format!("/wireless-devices/{id}"), None).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "expected 204 from DeleteWirelessDevice, got {status} {body}"
    );

    for uri in [
        format!("/wireless-devices/{id}?identifierType=WirelessDeviceId"),
        "/wireless-devices/00000000000000aa?identifierType=DevEui".to_string(),
    ] {
        let (status, body) = call(&router, Method::GET, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {uri}: a deleted device must be 404, got {status} {body}"
        );
    }

    let (_, listed) = call(&router, Method::GET, "/wireless-devices", None).await;
    assert_eq!(
        listed["WirelessDeviceList"],
        json!([]),
        "a deleted device must leave the list, got {listed}"
    );

    let (status, body) =
        call(&router, Method::DELETE, &format!("/wireless-devices/{id}"), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deleting an absent id must be 404, got {status} {body}"
    );

    // The DevEui is free again, so another device may claim it.
    let (status, body) = create_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "DelDest",
                "LoRaWAN": { "DevEui": "00000000000000aa" } }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the deleted device's DevEui must be claimable again, got {status} {body}"
    );
}

// ---------------------------------------------------------------------------
// Listing and pagination
// ---------------------------------------------------------------------------

/// Walk every page of `ListWirelessDevices` at `max_results`, returning the ids in the
/// order they were served and the number of pages it took.
async fn walk_pages(
    router: &MultiServiceRouter,
    max_results: i64,
    filters: &str,
) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut uri = format!("/wireless-devices?maxResults={max_results}{filters}");
        if let Some(token) = &token {
            uri.push_str(&format!("&nextToken={token}"));
        }
        let (status, body) = call(router, Method::GET, &uri, None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case maxResults={max_results}{filters}: expected 200, got {status} {body}"
        );
        pages += 1;
        let page = body["WirelessDeviceList"]
            .as_array()
            .unwrap_or_else(|| panic!("WirelessDeviceList must be a list, got {body}"))
            .clone();
        if max_results > 0 {
            assert!(
                page.len() as i64 <= max_results,
                "case maxResults={max_results}: a page returned {} entries",
                page.len()
            );
        }
        for entry in page {
            ids.push(
                entry["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("every entry carries an Id, got {entry}"))
                    .to_string(),
            );
        }
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            pages < 64,
            "case maxResults={max_results}: pagination did not terminate"
        );
    }
    (ids, pages)
}

/// Whatever the page size, paging through the list yields every stored device exactly
/// once, in one stable ascending order, with no repeats and no omissions.
#[tokio::test]
async fn pagination_covers_every_device_exactly_once_at_every_page_size() {
    let router = router();
    seed_destination(&router, "PageDest").await;

    const DEVICES: usize = 7;
    let mut expected = Vec::new();
    for index in 0..DEVICES {
        let (_, created) = create_device(
            &router,
            json!({
                "Type": if index % 2 == 0 { "LoRaWAN" } else { "Sidewalk" },
                "Name": format!("paged-{index}"),
                "DestinationName": "PageDest",
            }),
        )
        .await;
        expected.push(created["Id"].as_str().expect("an Id").to_string());
    }
    expected.sort();

    // `MaxResults` is 0..250: 0 (the modelled default, "no page size") and 250 are the
    // published bounds, and the sizes between exercise partial final pages.
    for max_results in [MAX_RESULTS_MIN, 1, 2, 3, 6, 7, 8, MAX_RESULTS_MAX] {
        let (ids, pages) = walk_pages(&router, max_results, "").await;
        assert_eq!(
            ids, expected,
            "case maxResults={max_results}: paging must serve every device exactly once in \
             ascending id order, got {ids:?} over {pages} page(s)"
        );
        let expected_pages = if max_results <= 0 || max_results as usize >= DEVICES {
            1
        } else {
            DEVICES.div_ceil(max_results as usize)
        };
        assert_eq!(
            pages, expected_pages,
            "case maxResults={max_results}: expected {expected_pages} page(s), took {pages}"
        );
    }
}

/// `MaxResults` one past its published upper bound is refused.
#[tokio::test]
async fn a_max_results_outside_the_published_range_is_refused() {
    let router = router();
    for bad in [MAX_RESULTS_MAX + 1, MAX_RESULTS_MIN - 1, 1_000] {
        let (status, body) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices?maxResults={bad}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case maxResults={bad}: expected 400, got {status} {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused rather than silently restarting
/// the list.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    seed_destination(&router, "TokenDest").await;
    create_device(
        &router,
        json!({ "Type": "LoRaWAN", "DestinationName": "TokenDest" }),
    )
    .await;

    for bad in [
        "not-a-token",
        "zz",
        "0",
        "%%%",
        // Well-formed hex that decodes to a plausible resume key but carries no marker
        // from this service: "aa", "zzz" and a hex-encoded real device id prefix.
        "6161",
        "7a7a7a",
        "30303030303030302d303030302d",
    ] {
        let (status, body) = call(
            &router,
            Method::GET,
            &format!("/wireless-devices?nextToken={bad}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case nextToken={bad:?}: expected 400, got {status} {body}"
        );
    }
}

/// Each list filter selects on the devices' own persisted fields, and the filters
/// compose.
#[tokio::test]
async fn list_filters_select_on_the_persisted_record() {
    let router = router();
    seed_destination(&router, "FilterA").await;
    seed_destination(&router, "FilterB").await;

    // (destination, type, device profile id, service profile id) -> the device's id.
    let plan = [
        ("FilterA", "LoRaWAN", "dp-1", "sp-1"),
        ("FilterA", "Sidewalk", "dp-2", ""),
        ("FilterB", "LoRaWAN", "dp-1", "sp-2"),
        ("FilterB", "LoRaWAN", "dp-3", "sp-1"),
    ];
    let mut ids = Vec::new();
    for (destination, device_type, device_profile_id, service_profile_id) in plan {
        let mut body = json!({ "Type": device_type, "DestinationName": destination });
        if device_type == "LoRaWAN" {
            // `ServiceProfileId` lives only on the LoRaWAN record (`SidewalkDevice` has
            // no such member), which is why the Sidewalk row carries none.
            body["LoRaWAN"] = json!({
                "DeviceProfileId": device_profile_id,
                "ServiceProfileId": service_profile_id,
            });
        } else {
            body["Sidewalk"] = json!({ "DeviceProfileId": device_profile_id });
        }
        let (status, created) = create_device(&router, body).await;
        assert_eq!(status, StatusCode::CREATED, "seeding {destination}/{device_type}");
        ids.push(created["Id"].as_str().expect("an Id").to_string());
    }

    let expected_for = |keep: &dyn Fn(usize) -> bool| {
        let mut selected: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(index, _)| keep(*index))
            .map(|(_, id)| id.clone())
            .collect();
        selected.sort();
        selected
    };

    struct Filter<'a> {
        query: &'a str,
        keep: &'a dyn Fn(usize) -> bool,
    }

    let filters = [
        Filter {
            query: "&destinationName=FilterA",
            keep: &|index| index < 2,
        },
        Filter {
            query: "&destinationName=FilterB",
            keep: &|index| index >= 2,
        },
        Filter {
            query: "&wirelessDeviceType=LoRaWAN",
            keep: &|index| index != 1,
        },
        Filter {
            query: "&wirelessDeviceType=Sidewalk",
            keep: &|index| index == 1,
        },
        Filter {
            query: "&destinationName=FilterB&wirelessDeviceType=LoRaWAN",
            keep: &|index| index >= 2,
        },
        Filter {
            query: "&deviceProfileId=dp-1",
            keep: &|index| index == 0 || index == 2,
        },
        Filter {
            query: "&deviceProfileId=dp-2",
            keep: &|index| index == 1,
        },
        // `ServiceProfileId` is carried on the LoRaWAN record only.
        Filter {
            query: "&serviceProfileId=sp-1",
            keep: &|index| index == 0 || index == 3,
        },
        Filter {
            query: "&serviceProfileId=sp-2",
            keep: &|index| index == 2,
        },
        Filter {
            query: "&serviceProfileId=sp-absent",
            keep: &|_| false,
        },
        Filter {
            query: "&serviceProfileId=sp-1&wirelessDeviceType=Sidewalk",
            keep: &|_| false,
        },
        Filter {
            query: "&destinationName=FilterA&wirelessDeviceType=Sidewalk",
            keep: &|index| index == 1,
        },
        Filter {
            query: "&destinationName=FilterA&deviceProfileId=dp-3",
            keep: &|_| false,
        },
        // No device in this case has been joined to a FUOTA task or a multicast group,
        // so these select nothing.
        Filter {
            query: "&fuotaTaskId=ft-1",
            keep: &|_| false,
        },
        Filter {
            query: "&multicastGroupId=mg-1",
            keep: &|_| false,
        },
    ];

    for filter in &filters {
        // Page size 1 as well as unlimited, so a filter is applied before paging.
        for max_results in [MAX_RESULTS_MIN, 1] {
            let (got, _) = walk_pages(&router, max_results, filter.query).await;
            assert_eq!(
                got,
                expected_for(filter.keep),
                "case {} maxResults={max_results}: wrong selection, got {got:?}",
                filter.query
            );
        }
    }
}

/// A list entry is the `WirelessDeviceStatistics` projection of the SAME record
/// `GetWirelessDevice` returns in full.
#[tokio::test]
async fn a_list_entry_projects_the_same_record_get_returns() {
    let router = router();
    seed_destination(&router, "ProjDest").await;
    let (_, created) = create_device(
        &router,
        json!({
            "Type": "LoRaWAN",
            "Name": "projected",
            "DestinationName": "ProjDest",
            "Positioning": "Enabled",
            "LoRaWAN": { "DevEui": "00000000000000bb", "DeviceProfileId": "dp-proj" },
        }),
    )
    .await;
    let id = created["Id"].as_str().expect("an Id").to_string();

    let (_, fetched) = call(
        &router,
        Method::GET,
        &format!("/wireless-devices/{id}?identifierType=WirelessDeviceId"),
        None,
    )
    .await;
    let (_, listed) = call(&router, Method::GET, "/wireless-devices", None).await;
    let entry = listed["WirelessDeviceList"]
        .as_array()
        .and_then(|list| list.first())
        .cloned()
        .unwrap_or_else(|| panic!("the list must carry the device, got {listed}"));

    for member in ["Id", "Arn", "Type", "Name", "DestinationName", "Positioning"] {
        assert_eq!(
            entry.get(member),
            fetched.get(member),
            "case {member}: the list entry must agree with GetWirelessDevice, got {:?} vs {:?}",
            entry.get(member),
            fetched.get(member)
        );
    }
    assert_eq!(
        entry.pointer("/LoRaWAN/DevEui"),
        fetched.pointer("/LoRaWAN/DevEui"),
        "the list entry's LoRaWAN/DevEui must agree with GetWirelessDevice, got {:?}",
        entry.pointer("/LoRaWAN/DevEui")
    );
}

// ---------------------------------------------------------------------------
// Idempotency tokens
// ---------------------------------------------------------------------------

/// `ClientRequestToken` is an `@idempotencyToken`: replaying it with the same
/// parameters completes successfully and creates nothing new, replaying it with
/// different parameters is a 409.
#[tokio::test]
async fn replaying_a_client_request_token_is_idempotent_for_both_creates() {
    let router = router();

    for (index, expression_type) in EXPRESSION_TYPES.iter().enumerate() {
        let token = format!("destination-token-{index}");
        let name = format!("TokenDest{index}");
        let mut body = destination_body(&name, expression_type);
        body["ClientRequestToken"] = json!(token);

        let (status, first) = create_destination(&router, body.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "case {token}: first create {first}");

        let (status, replay) = create_destination(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {token}: an identical replay must succeed, got {status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {token}: an identical replay must return the same resource, got {replay}"
        );

        let mut diverged = body.clone();
        diverged["Expression"] = json!("a-different-rule");
        let (status, conflict) = create_destination(&router, diverged).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {token}: a replay with different parameters must be 409, got {status} \
             {conflict}"
        );
    }

    seed_destination(&router, "DeviceTokenDest").await;
    for (index, device_type) in WIRELESS_DEVICE_TYPES.iter().enumerate() {
        let token = format!("device-token-{index}");
        let body = json!({
            "Type": device_type,
            "Name": format!("tokened-{index}"),
            "DestinationName": "DeviceTokenDest",
            "ClientRequestToken": token,
        });

        let (status, first) = create_device(&router, body.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "case {token}: first create {first}");

        let (status, replay) = create_device(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {token}: an identical replay must succeed, got {status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {token}: an identical replay must return the id the first call minted, \
             got {replay}"
        );

        let mut diverged = body.clone();
        diverged["Name"] = json!("a-different-name");
        let (status, conflict) = create_device(&router, diverged).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {token}: a replay with different parameters must be 409, got {status} \
             {conflict}"
        );
    }

    // Exactly one device per token survived: two tokens, two devices.
    let (_, listed) = call(
        &router,
        Method::GET,
        "/wireless-devices?destinationName=DeviceTokenDest",
        None,
    )
    .await;
    assert_eq!(
        listed["WirelessDeviceList"].as_array().map(Vec::len),
        Some(WIRELESS_DEVICE_TYPES.len()),
        "a replayed token must not create a second device, got {listed}"
    );
}

/// An idempotency token is released with the resource it minted: after the resource is
/// deleted the token is free, so a later create carrying it — even with different
/// parameters, which would have been a 409 while the resource lived — succeeds.
#[tokio::test]
async fn an_idempotency_token_is_released_with_the_resource_it_minted() {
    let router = router();

    let token = "released-destination-token";
    let mut body = destination_body("ReleasedDest", "RuleName");
    body["ClientRequestToken"] = json!(token);
    let (status, _) = create_destination(&router, body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "first create");

    let (status, _) =
        call(&router, Method::DELETE, "/destinations/ReleasedDest", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete the destination");

    let mut diverged = body.clone();
    diverged["Expression"] = json!("a-different-rule");
    let (status, created) = create_destination(&router, diverged).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a token whose resource was deleted must be free again, got {status} {created}"
    );
    let (_, fetched) = call(&router, Method::GET, "/destinations/ReleasedDest", None).await;
    assert_eq!(
        fetched["Expression"],
        json!("a-different-rule"),
        "the new create's parameters must be the ones stored, got {:?}",
        fetched["Expression"]
    );

    seed_destination(&router, "ReleasedDeviceDest").await;
    let device_token = "released-device-token";
    let device_body = json!({
        "Type": "LoRaWAN",
        "Name": "released",
        "DestinationName": "ReleasedDeviceDest",
        "ClientRequestToken": device_token,
    });
    let (_, first) = create_device(&router, device_body.clone()).await;
    let first_id = first["Id"].as_str().expect("an Id").to_string();
    let (status, _) = call(
        &router,
        Method::DELETE,
        &format!("/wireless-devices/{first_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete the device");

    let mut diverged = device_body.clone();
    diverged["Name"] = json!("released-again");
    let (status, second) = create_device(&router, diverged).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a device token whose device was deleted must be free again, got {status} {second}"
    );
    assert_ne!(
        second["Id"], first["Id"],
        "the second create must mint a fresh Id, got {second}"
    );
}

/// The destination store is BOUNDED: the create refuses once it is full, stores
/// nothing, and accepts again as soon as a destination is deleted.
#[tokio::test]
async fn the_destination_store_is_bounded() {
    let router = router();

    for index in 0..MAX_DESTINATIONS {
        let (status, body) =
            create_destination(&router, destination_body(&format!("Bound{index}"), "RuleName"))
                .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: a create below the ceiling must succeed, got {status} {body}"
        );
    }

    let (status, body) = create_destination(
        &router,
        destination_body(&format!("Bound{MAX_DESTINATIONS}"), "RuleName"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a create past the ceiling must be refused, got {status} {body}"
    );
    let (status, _) = call(
        &router,
        Method::GET,
        &format!("/destinations/Bound{MAX_DESTINATIONS}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a refused create must store nothing"
    );

    let (status, _) = call(&router, Method::DELETE, "/destinations/Bound0", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "free one slot");
    let (status, body) = create_destination(
        &router,
        destination_body(&format!("Bound{MAX_DESTINATIONS}"), "RuleName"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a freed slot must be usable again, got {status} {body}"
    );
}

/// Whether `candidate` has the uuid textual form `8-4-4-4-12` lowercase hex, which is
/// what `WirelessDeviceId` carries.
/// Source: `smithy_query(query="operation", operation="CreateWirelessDevice")` —
/// `Id` targets `WirelessDeviceId`, documented as the device's uuid.
pub(crate) fn is_uuid_shaped(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()))
}

/// The wireless-device store is BOUNDED, the same way the destination store is; and
/// every `Id` it mints along the way is a fresh uuid.
#[tokio::test]
async fn the_wireless_device_store_is_bounded() {
    let router = router();
    seed_destination(&router, "BoundDest").await;
    let body = json!({ "Type": "LoRaWAN", "DestinationName": "BoundDest" });

    let mut first_id = String::new();
    let mut minted = std::collections::BTreeSet::new();
    for index in 0..MAX_WIRELESS_DEVICES {
        let (status, created) = create_device(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: a create below the ceiling must succeed, got {status} {created}"
        );
        let id = created["Id"].as_str().expect("an Id").to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {index}: minted Id {id:?} is not a uuid"
        );
        assert!(
            minted.insert(id.clone()),
            "case {index}: minted Id {id:?} had already been issued"
        );
        if index == 0 {
            first_id = id;
        }
    }

    let (status, refused) = create_device(&router, body.clone()).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a create past the ceiling must be refused, got {status} {refused}"
    );
    let (_, listed) = call(
        &router,
        Method::GET,
        &format!("/wireless-devices?maxResults={MAX_RESULTS_MAX}"),
        None,
    )
    .await;
    assert_eq!(
        listed["WirelessDeviceList"].as_array().map(Vec::len),
        Some(MAX_WIRELESS_DEVICES),
        "a refused create must store nothing"
    );

    let (status, _) = call(
        &router,
        Method::DELETE,
        &format!("/wireless-devices/{first_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "free one slot");
    let (status, created) = create_device(&router, body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a freed slot must be usable again, got {status} {created}"
    );
}
