// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the device slice (`CreateDevice` / `GetDevices`), driven over
//! HTTP.
//!
//! The rule under test: `CreateDevice` mints a device of the global network the path
//! addresses, stores every member the request carried, and `GetDevices` answers from that
//! stored record — so the device's identity (id, ARN, `CreatedAt`) is minted rather than
//! echoed, its lifecycle state is a function of the clock, and the inventory
//! (`GetNetworkResourceCounts`) and the deletes of its parents all see the same record.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: a device is only ever read back through
//! `GetDevices`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{
    error_code, expected_scoped_arn, ids_of, is_minted_id, Emulator, PAST_SETTLE,
};

/// The documented maximum length, in characters, of `Vendor`, `Model` and `SerialNumber`:
/// "Constraints: Maximum length of 128 characters" on each
/// (`smithy_query query="doc" shape="CreateDeviceRequest" member="Vendor"`, the same
/// sentence for `Model` and `SerialNumber`).
const MAX_DESCRIPTOR_CHARS: usize = 128;

/// The modelled maximum of a `ConstrainedString`, which bounds `Description` and `Type`
/// (`smithy_query query="operation" operation="CreateDevice"`: len 0..256).
const MAX_CONSTRAINED_CHARS: usize = 256;

/// A subnet ARN of the form `AWSLocation$SubnetArn` accepts: its modelled pattern is
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:subnet\/subnet-[0-9a-f]{8,17}$`
/// (`smithy_query shape="AWSLocation"`).
const SUBNET_ARN: &str = "arn:aws:ec2:us-west-2:123456789012:subnet/subnet-0123456789abcdef0";

/// One row of the create/round-trip table: the members the request carries, with
/// `Value::Null` standing for "this row omits the member".
struct DeviceCase {
    name: &'static str,
    /// Whether the row assigns the device to the site the test created.
    sited: bool,
    aws_location: Value,
    location: Value,
    description: Value,
    device_type: Value,
    vendor: Value,
    model: Value,
    serial_number: Value,
    tags: Value,
}

impl DeviceCase {
    /// The `CreateDevice` body this row sends: every member only when the row carries it
    /// (the only required member, `GlobalNetworkId`, travels in the path).
    fn body(&self, site: &str) -> Value {
        let mut body = serde_json::Map::new();
        for (member, value) in [
            ("AWSLocation", &self.aws_location),
            ("Location", &self.location),
            ("Description", &self.description),
            ("Type", &self.device_type),
            ("Vendor", &self.vendor),
            ("Model", &self.model),
            ("SerialNumber", &self.serial_number),
            ("Tags", &self.tags),
        ] {
            if !value.is_null() {
                body.insert(member.to_string(), value.clone());
            }
        }
        if self.sited {
            body.insert("SiteId".to_string(), json!(site));
        }
        Value::Object(body)
    }

    /// The `SiteId` a read must answer: the site the row assigned, or `null` for a device
    /// created without one (`SiteId` is optional, so an unsited device is legal).
    fn expected_site(&self, site: &str) -> Value {
        if self.sited {
            json!(site)
        } else {
            Value::Null
        }
    }

    /// The `Tags` a read must answer: the list the row sent, or the empty list when it
    /// sent none (a device always has a tag list, possibly empty).
    fn expected_tags(&self) -> Value {
        if self.tags.is_null() {
            json!([])
        } else {
            self.tags.clone()
        }
    }
}

/// The rows of the create/round-trip table. They differ along whether the device is
/// assigned to a site, along the form of `AWSLocation` (both members, a zone only, a
/// subnet ARN only, absent — "for an on-premises device, you can omit this parameter"),
/// along the presence of a physical `Location`, and along the size, character set and
/// presence of every free-form descriptor: everything the storage rule generalises over.
fn device_cases() -> Vec<DeviceCase> {
    vec![
        DeviceCase {
            name: "every member, assigned to a site",
            sited: true,
            aws_location: json!({ "Zone": "us-west-2a", "SubnetArn": SUBNET_ARN }),
            location: json!({
                "Address": "1 Main St",
                "Latitude": "47.6062",
                "Longitude": "-122.3321",
            }),
            description: json!("branch router"),
            device_type: json!("router"),
            vendor: json!("AnyCompany"),
            model: json!("AC-1000"),
            serial_number: json!("SN-0001"),
            tags: json!([{ "Key": "env", "Value": "test" }, { "Key": "tier", "Value": "edge" }]),
        },
        DeviceCase {
            name: "nothing but the global network in the path",
            sited: false,
            aws_location: Value::Null,
            location: Value::Null,
            description: Value::Null,
            device_type: Value::Null,
            vendor: Value::Null,
            model: Value::Null,
            serial_number: Value::Null,
            tags: Value::Null,
        },
        DeviceCase {
            name: "an on-premises device: a physical location and no AWS location",
            sited: false,
            aws_location: Value::Null,
            location: json!({ "Address": "2 High St" }),
            description: json!("on-premises switch"),
            device_type: json!("switch"),
            vendor: json!("AnyCompany"),
            model: Value::Null,
            serial_number: json!("SN-0002"),
            tags: json!([]),
        },
        DeviceCase {
            name: "an AWS device located by zone only",
            sited: true,
            aws_location: json!({ "Zone": "usw2-az1" }),
            location: Value::Null,
            description: Value::Null,
            device_type: json!("virtual-appliance"),
            vendor: Value::Null,
            model: json!("AC-2000"),
            serial_number: Value::Null,
            tags: Value::Null,
        },
        DeviceCase {
            name: "an AWS device located by subnet ARN only",
            sited: false,
            aws_location: json!({ "SubnetArn": SUBNET_ARN }),
            location: Value::Null,
            description: json!("in-VPC appliance"),
            device_type: Value::Null,
            vendor: json!("AnyCompany"),
            model: json!("AC-3000"),
            serial_number: json!("SN-0003"),
            tags: json!([{ "Key": "owner", "Value": "net" }]),
        },
        DeviceCase {
            name: "both a site and a location, which the record keeps independently",
            sited: true,
            aws_location: json!({ "Zone": "us-west-2b" }),
            location: json!({ "Latitude": "0", "Longitude": "0" }),
            description: json!("sited and located"),
            device_type: json!("router"),
            vendor: json!("AnyCompany"),
            model: json!("AC-4000"),
            serial_number: json!("SN-0004"),
            tags: Value::Null,
        },
        DeviceCase {
            name: "descriptors at their published maximum lengths",
            sited: true,
            aws_location: Value::Null,
            location: Value::Null,
            description: json!("d".repeat(MAX_CONSTRAINED_CHARS)),
            device_type: json!("t".repeat(MAX_CONSTRAINED_CHARS)),
            vendor: json!("v".repeat(MAX_DESCRIPTOR_CHARS)),
            model: json!("m".repeat(MAX_DESCRIPTOR_CHARS)),
            serial_number: json!("s".repeat(MAX_DESCRIPTOR_CHARS)),
            tags: Value::Null,
        },
        DeviceCase {
            name: "empty, multi-line and non-ASCII descriptors",
            sited: false,
            aws_location: json!({ "Zone": "" }),
            location: json!({ "Address": "Ünter den Linden 1\nBerlin" }),
            description: json!("line one\nline two"),
            device_type: json!(""),
            vendor: json!("Ünternet"),
            model: json!("модель"),
            serial_number: json!("序列号 \u{1f310}"),
            tags: json!([{ "Key": "ключ", "Value": "значение" }]),
        },
    ]
}

/// A device record with `State` dropped, so two views of the same stored device compare
/// member for member without racing the `PENDING` -> `AVAILABLE` transition the clock owes
/// (which its own test below covers).
fn without_state(device: &Value) -> Value {
    let mut copy = device.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("State");
    }
    copy
}

/// `CreateDevice` in `network`.
async fn create_device(emulator: &Emulator, network: &str, body: Value) -> (u16, Value) {
    emulator
        .post(&format!("/global-networks/{network}/devices"), body)
        .await
}

/// `CreateDevice` that must succeed, answering the minted `DeviceId`.
pub(crate) async fn create_device_ok(emulator: &Emulator, network: &str, body: Value) -> String {
    let (status, created) = create_device(emulator, network, body.clone()).await;
    assert_eq!(
        status, 200,
        "CreateDevice({body}): expected 200, got {status} with {created}"
    );
    created["Device"]["DeviceId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDevice({body}): no DeviceId in {created}"))
        .to_string()
}

/// The `DeviceId`s `GetDevices` answers for `query` (appended to the devices URI), with
/// the status.
async fn get_devices(emulator: &Emulator, network: &str, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/devices")
    } else {
        format!("/global-networks/{network}/devices?{query}")
    };
    emulator.get(&uri).await
}

/// `ids` sorted, which is the key order `GetDevices` lists in.
fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

#[tokio::test]
async fn create_device_stores_every_member_and_get_devices_answers_from_the_stored_record() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("devices").await;
    let site = emulator.create_site(&network, "hq").await;

    let mut minted: Vec<String> = Vec::new();
    for case in device_cases() {
        let name = case.name;
        let (status, created) = create_device(&emulator, &network, case.body(&site)).await;
        assert_eq!(
            status, 200,
            "case {name}: CreateDevice expected 200, got {status} with {created}"
        );
        let device = created["Device"].clone();
        let device_id = device["DeviceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: no DeviceId in {created}"))
            .to_string();
        assert!(
            is_minted_id(&device_id, "device"),
            "case {name}: expected a device-<17 hex> id, got {device_id:?}"
        );
        assert!(
            !minted.contains(&device_id),
            "case {name}: id {device_id:?} was already minted for another device"
        );
        assert_eq!(
            device["DeviceArn"],
            json!(expected_scoped_arn("device", &network, &device_id)),
            "case {name}: the ARN must name the global network and the minted id, got {device}"
        );
        assert_eq!(
            device["GlobalNetworkId"],
            json!(network.clone()),
            "case {name}: the device must back-reference its global network, got {device}"
        );
        assert_eq!(
            device["SiteId"],
            case.expected_site(&site),
            "case {name}: SiteId must be stored as sent, got {device}"
        );
        assert_eq!(
            device["AWSLocation"], case.aws_location,
            "case {name}: AWSLocation must be stored as sent, got {device}"
        );
        assert_eq!(
            device["Location"], case.location,
            "case {name}: Location must be stored as sent, got {device}"
        );
        assert_eq!(
            device["Description"], case.description,
            "case {name}: Description must be stored as sent, got {device}"
        );
        assert_eq!(
            device["Type"], case.device_type,
            "case {name}: Type must be stored as sent, got {device}"
        );
        assert_eq!(
            device["Vendor"], case.vendor,
            "case {name}: Vendor must be stored as sent, got {device}"
        );
        assert_eq!(
            device["Model"], case.model,
            "case {name}: Model must be stored as sent, got {device}"
        );
        assert_eq!(
            device["SerialNumber"], case.serial_number,
            "case {name}: SerialNumber must be stored as sent, got {device}"
        );
        assert_eq!(
            device["Tags"],
            case.expected_tags(),
            "case {name}: Tags must be stored as sent, got {device}"
        );
        assert_eq!(
            device["State"],
            json!("PENDING"),
            "case {name}: a caller acting immediately must still see PENDING, got {device}"
        );
        assert!(
            device["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {name}: CreatedAt must be a timestamp, got {device}"
        );

        // ROUND TRIP: the stored record is what the reader answers, member for member.
        let (status, listed) = get_devices(&emulator, &network, &format!("deviceIds={device_id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name}: GetDevices expected 200, got {status} with {listed}"
        );
        assert_eq!(
            ids_of(&listed, "Devices", "DeviceId"),
            vec![device_id.clone()],
            "case {name}: GetDevices must answer exactly the created device, got {listed}"
        );
        assert_eq!(
            without_state(&listed["Devices"][0]),
            without_state(&device),
            "case {name}: GetDevices must answer the stored record, got {listed}"
        );

        minted.push(device_id);
    }

    // Every device of the global network is readable at once, in key order, each once.
    let (_, listed) = get_devices(&emulator, &network, "maxResults=500").await;
    assert_eq!(
        ids_of(&listed, "Devices", "DeviceId"),
        sorted(minted.clone()),
        "GetDevices must answer every stored device of the global network, got {listed}"
    );
}

#[tokio::test]
async fn get_devices_filters_by_the_site_and_the_ids_the_request_names() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("filters").await;
    let other_network = emulator.create_global_network("other").await;
    let site_a = emulator.create_site(&network, "a").await;
    let site_b = emulator.create_site(&network, "b").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;

    let at_a_one = create_device_ok(&emulator, &network, json!({ "SiteId": site_a })).await;
    let at_a_two = create_device_ok(
        &emulator,
        &network,
        json!({ "SiteId": site_a, "Type": "switch" }),
    )
    .await;
    let at_b = create_device_ok(&emulator, &network, json!({ "SiteId": site_b })).await;
    let unsited = create_device_ok(&emulator, &network, json!({ "Type": "router" })).await;
    let elsewhere =
        create_device_ok(&emulator, &other_network, json!({ "SiteId": other_site })).await;

    // Each row is a filter and the devices of THIS global network it selects. An absent
    // filter matches every device the global network holds; a device created without a
    // site matches no `siteId`; a device of another global network is never selected.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter at all",
            String::new(),
            sorted(vec![
                at_a_one.clone(),
                at_a_two.clone(),
                at_b.clone(),
                unsited.clone(),
            ]),
        ),
        (
            "siteId with two devices",
            format!("siteId={site_a}"),
            sorted(vec![at_a_one.clone(), at_a_two.clone()]),
        ),
        (
            "siteId with one device",
            format!("siteId={site_b}"),
            vec![at_b.clone()],
        ),
        (
            "siteId of a site in another global network",
            format!("siteId={other_site}"),
            vec![],
        ),
        (
            "one deviceId",
            format!("deviceIds={at_b}"),
            vec![at_b.clone()],
        ),
        (
            "two deviceIds, one of them unsited",
            format!("deviceIds={at_a_one}&deviceIds={unsited}"),
            sorted(vec![at_a_one.clone(), unsited.clone()]),
        ),
        (
            "a deviceId and the siteId it sits at",
            format!("deviceIds={at_a_one}&siteId={site_a}"),
            vec![at_a_one.clone()],
        ),
        (
            "a deviceId and a siteId it does not sit at",
            format!("deviceIds={at_a_one}&siteId={site_b}"),
            vec![],
        ),
        (
            "a deviceId of another global network",
            format!("deviceIds={elsewhere}"),
            vec![],
        ),
    ];

    for (case, query, expected) in rows {
        let (status, listed) = get_devices(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case} ({query:?}): expected 200, got {status} with {listed}"
        );
        assert_eq!(
            ids_of(&listed, "Devices", "DeviceId"),
            expected,
            "case {case} ({query:?}): expected {expected:?}, got {listed}"
        );
    }

    // The other global network still answers only its own device, so no filter leaked.
    let (status, listed) = get_devices(&emulator, &other_network, "").await;
    assert_eq!(
        status, 200,
        "GetDevices on the other global network: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        ids_of(&listed, "Devices", "DeviceId"),
        vec![elsewhere],
        "GetDevices must answer only its own global network's devices, got {listed}"
    );
}

#[tokio::test]
async fn create_device_refuses_a_descriptor_past_its_documented_maximum() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("descriptors").await;

    // The three members carrying the documented 128-character bound, each tested at the
    // bound (accepted) and one character past it (refused), in ASCII and in a script whose
    // characters are multi-byte — the bound is counted in characters, not bytes.
    for member in ["Vendor", "Model", "SerialNumber"] {
        for (case, character) in [("ascii", "v"), ("non-ascii", "ü")] {
            let at_bound = character.repeat(MAX_DESCRIPTOR_CHARS);
            let (status, accepted) =
                create_device(&emulator, &network, json!({ member: at_bound.clone() })).await;
            assert_eq!(
                status, 200,
                "case {member}/{case} at {MAX_DESCRIPTOR_CHARS} characters: expected 200, got {status} with {accepted}"
            );
            assert_eq!(
                accepted["Device"][member],
                json!(at_bound),
                "case {member}/{case} at the bound: the value must be stored, got {accepted}"
            );

            let past_bound = character.repeat(MAX_DESCRIPTOR_CHARS + 1);
            let (status, refused) =
                create_device(&emulator, &network, json!({ member: past_bound })).await;
            assert_eq!(
                status, 400,
                "case {member}/{case} at {} characters: expected 400, got {status} with {refused}",
                MAX_DESCRIPTOR_CHARS + 1
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {member}/{case} past the bound: wrong error code in {refused}"
            );
            assert_eq!(
                refused["Reason"].as_str(),
                Some("FieldValidationFailed"),
                "case {member}/{case} past the bound: expected Reason FieldValidationFailed, got {refused}"
            );
            assert_eq!(
                refused["Fields"][0]["Name"],
                json!(member),
                "case {member}/{case} past the bound: the field entry must name the member, got {refused}"
            );
        }
    }

    // Only the accepted creates are stored: six rows were refused, six accepted.
    let (_, listed) = get_devices(&emulator, &network, "maxResults=500").await;
    assert_eq!(
        ids_of(&listed, "Devices", "DeviceId").len(),
        6,
        "a refused CreateDevice must store nothing, got {listed}"
    );
}

#[tokio::test]
async fn a_created_device_reports_pending_until_the_settle_span_the_clock_owes_elapses() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("settling").await;
    let device = create_device_ok(&emulator, &network, json!({})).await;

    let (_, immediate) = get_devices(&emulator, &network, &format!("deviceIds={device}")).await;
    assert_eq!(
        immediate["Devices"][0]["State"],
        json!("PENDING"),
        "GetDevices straight after the create: expected PENDING, got {immediate}"
    );
    let created_at = immediate["Devices"][0]["CreatedAt"].clone();

    // The transition is owed by the clock, not by the read: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    let (_, settled) = get_devices(&emulator, &network, &format!("deviceIds={device}")).await;
    assert_eq!(
        settled["Devices"][0]["State"],
        json!("AVAILABLE"),
        "GetDevices after the settle span: expected AVAILABLE, got {settled}"
    );
    assert_eq!(
        settled["Devices"][0]["CreatedAt"], created_at,
        "the settle transition must not restamp CreatedAt, got {settled}"
    );
}

#[tokio::test]
async fn get_devices_pages_every_device_of_its_own_global_network() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("paging").await;
    let site = emulator.create_site(&network, "hq").await;

    let mut minted: Vec<String> = Vec::new();
    for index in 0..7 {
        let body = if index % 2 == 0 {
            json!({ "SiteId": site })
        } else {
            json!({})
        };
        minted.push(create_device_ok(&emulator, &network, body).await);
    }
    let expected = sorted(minted);

    // `MaxResults` is modelled `range 1..500`, so the rows are no page size at all, each
    // published bound and sizes either side of the stored count.
    for page_size in [None, Some(1), Some(2), Some(6), Some(7), Some(8), Some(500)] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref next) = token {
                query.push(format!("nextToken={next}"));
            }
            let (status, page) = get_devices(&emulator, &network, &query.join("&")).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size:?}: expected 200, got {status} with {page}"
            );
            collected.extend(ids_of(&page, "Devices", "DeviceId"));
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case MaxResults={page_size:?}: GetDevices must page every device exactly once"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused_by_get_devices() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("tokens").await;
    create_device_ok(&emulator, &network, json!({})).await;

    // Every row is a NON-empty value: the empty string is no token at all — it satisfies
    // the modelled `length 0..2048` and names no position, so it asks for the first page
    // (`crate::empty_page_token`).
    let cases: Vec<(&str, String)> = vec![
        ("not hex", "zzzz".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        (
            "hex of a token another service would mint",
            hex::encode("someone-else:device-0"),
        ),
        ("hex of non-utf8 bytes", hex::encode([0xff, 0xfe, 0xfd])),
    ];

    for (case, token) in cases {
        let (status, body) = get_devices(&emulator, &network, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for an unparseable NextToken, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: wrong error code in {body}"
        );
    }
}

#[tokio::test]
async fn the_network_resource_inventory_counts_the_devices_the_global_network_holds() {
    let emulator = Emulator::new();

    // The rows differ in how many devices the global network holds and in how they are
    // spread over its sites; every row runs on the same emulator, so a count that leaked
    // across global networks would show up here.
    for (case, sited, unsited) in [
        ("no device at all", 0, 0),
        ("one sited device", 1, 0),
        ("one unsited device", 0, 1),
        ("several of each", 3, 2),
    ] {
        let network = emulator.create_global_network(case).await;
        let site = emulator.create_site(&network, case).await;
        for _ in 0..sited {
            create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
        }
        for _ in 0..unsited {
            create_device_ok(&emulator, &network, json!({})).await;
        }

        let (status, counts) = emulator
            .get(&format!(
                "/global-networks/{network}/network-resource-count?resourceType=device"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: GetNetworkResourceCounts expected 200, got {status} with {counts}"
        );
        assert_eq!(
            counts["NetworkResourceCounts"],
            json!([{ "ResourceType": "device", "Count": sited + unsited }]),
            "case {case}: the inventory must report the {} devices stored, got {counts}",
            sited + unsited
        );
    }
}

#[tokio::test]
async fn a_parent_that_still_holds_a_device_cannot_be_deleted() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("conflicts").await;
    let held = emulator.create_site(&network, "held").await;
    let empty = emulator.create_site(&network, "empty").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": held })).await;

    // "Deletes an existing site. The site cannot be associated with any device or link."
    // (`smithy_query query="doc" shape="DeleteSite"`).
    let (status, refused) = emulator
        .delete(&format!("/global-networks/{network}/sites/{held}"))
        .await;
    assert_eq!(
        status, 400,
        "DeleteSite with a device: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "DeleteSite with a device: wrong error code in {refused}"
    );

    // A site that holds no device still deletes, so the refusal is about the device only.
    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{network}/sites/{empty}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite without devices: expected 200, got {status} with {deleted}"
    );

    // A device is a child of the global network too, so that delete is refused as well.
    let (status, refused) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 400,
        "DeleteGlobalNetwork with a device: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "DeleteGlobalNetwork with a device: wrong error code in {refused}"
    );

    // Every refusal left the device exactly where it was.
    let (status, listed) = get_devices(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "GetDevices after the refusals: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        ids_of(&listed, "Devices", "DeviceId"),
        vec![device],
        "a refused delete must leave the devices alone, got {listed}"
    );
}

#[tokio::test]
async fn creating_past_the_device_quota_is_refused_with_the_modelled_error_and_stores_nothing() {
    use service_com_amazonaws_networkmanager_networkmanager::state::MAX_DEVICES_PER_GLOBAL_NETWORK;

    let emulator = Emulator::new();
    let network = emulator.create_global_network("quota").await;
    let sibling = emulator.create_global_network("sibling").await;

    let mut devices: Vec<String> = Vec::new();
    for _ in 0..MAX_DEVICES_PER_GLOBAL_NETWORK {
        devices.push(create_device_ok(&emulator, &network, json!({})).await);
    }
    let (status, refused) = create_device(&emulator, &network, json!({})).await;
    assert_eq!(
        status, 402,
        "CreateDevice at the per-global-network quota: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "CreateDevice at the quota: wrong error code in {refused}"
    );

    let (_, listed) = get_devices(&emulator, &network, "maxResults=500").await;
    assert_eq!(
        ids_of(&listed, "Devices", "DeviceId"),
        sorted(devices),
        "a refused CreateDevice must not have stored anything, got {listed}"
    );

    // The quota is per global network, so a sibling of a full one still has room.
    let (status, accepted) = create_device(&emulator, &sibling, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateDevice on a sibling of a full global network: expected 200, got {status} with {accepted}"
    );
}
