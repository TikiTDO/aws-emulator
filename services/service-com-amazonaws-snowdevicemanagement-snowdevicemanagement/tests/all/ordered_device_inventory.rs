// SPDX-License-Identifier: CC0-1.0

//! The managed-device inventory as a projection of Snow Family ORDERS, end to end over
//! HTTP.
//!
//! Snow Device Management models no device-creation operation: a device becomes
//! manageable by being ORDERED — a Snow Family job created through the Snowball API with
//! `RemoteManagement` installed. So every case here drives BOTH services over the wire,
//! mounted on one shared engine: the order is placed through Snowball's own front door
//! (`POST /` with `X-Amz-Target`), and the inventory is read through Snow Device
//! Management's (`GET /managed-devices`, `POST /managed-device/{id}/describe`). Nothing
//! reaches into either state struct, so an inventory that is not computed from the stored
//! orders cannot pass.
//!
//! The rule under test: one managed device per order that installed remote management and
//! still holds a device, in the Region the order was placed in, carrying that order's
//! `JobId` as `associatedWithJob` — which is what `ListDevices?jobId=` narrows on. Each
//! case is a TABLE whose rows differ along what the rule generalises over: the
//! `RemoteManagement` value (each entry of the modelled catalog, and absent), the
//! `SnowballType` ordered, the job type, the Region, the number of orders, and whether
//! the order still holds its device.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix the Snowball emulator answers on, per that crate's
/// OPERATIONS.md "Wire protocol" (`awsJson1_1`, `POST /`).
const ORDER_TARGET_PREFIX: &str = "AWSIESnowballJobManagementService";

/// The two signing names the router routes by: the order service's and this one's.
const ORDER_SIGNING_NAME: &str = "snowball";
const DEVICE_SIGNING_NAME: &str = "snow-device-management";

/// The `RemoteManagement` values the model admits
/// (`smithy_query(query="shape", shape="RemoteManagement")` on the Snowball model), each
/// of which is a row below. Spelled out here rather than imported, so a wrong catalog in
/// the implementation cannot make this suite pass.
const REMOTE_MANAGEMENT_VALUES: &[&str] =
    &["INSTALLED_AUTOSTART", "INSTALLED_ONLY", "NOT_INSTALLED"];

/// One order, and whether the device it ships is one this account manages remotely.
struct OrderRow {
    case: &'static str,
    region: &'static str,
    job_type: &'static str,
    snowball_type: &'static str,
    /// The `RemoteManagement` the order named, or `None` for an order that named none.
    remote_management: Option<&'static str>,
    /// Whether this order ships a device Snow Device Management manages.
    managed: bool,
}

/// Every order row: each entry of the `RemoteManagement` catalog and the absent member,
/// over `SnowballType` values from the modelled catalog, both job types a device of that
/// type serves, and two Regions — so the rule is exercised far beyond the one recorded
/// order (a `SNC1_HDD` `LOCAL_USE` job managed `INSTALLED_AUTOSTART`).
fn order_rows() -> Vec<OrderRow> {
    vec![
        OrderRow {
            case: "a Snowcone HDD ordered for local use with remote management autostarted",
            region: "us-west-2",
            job_type: "LOCAL_USE",
            snowball_type: "SNC1_HDD",
            remote_management: Some("INSTALLED_AUTOSTART"),
            managed: true,
        },
        OrderRow {
            case: "a Snowcone SSD ordered with remote management installed only",
            region: "us-west-2",
            job_type: "LOCAL_USE",
            snowball_type: "SNC1_SSD",
            remote_management: Some("INSTALLED_ONLY"),
            managed: true,
        },
        OrderRow {
            case: "an Edge Compute Optimized device ordered with remote management not installed",
            region: "us-west-2",
            job_type: "LOCAL_USE",
            snowball_type: "EDGE_C",
            remote_management: Some("NOT_INSTALLED"),
            managed: false,
        },
        OrderRow {
            case: "a Snowblade ordered without naming remote management at all",
            region: "us-west-2",
            job_type: "LOCAL_USE",
            snowball_type: "RACK_5U_C",
            remote_management: None,
            managed: false,
        },
        OrderRow {
            case: "an import order of a Storage Optimized device with remote management \
                   autostarted",
            region: "us-west-2",
            job_type: "IMPORT",
            snowball_type: "EDGE_S",
            remote_management: Some("INSTALLED_AUTOSTART"),
            managed: true,
        },
        OrderRow {
            case: "a V3_5C device ordered in another Region with remote management \
                   installed only",
            region: "eu-central-1",
            job_type: "LOCAL_USE",
            snowball_type: "V3_5C",
            remote_management: Some("INSTALLED_ONLY"),
            managed: true,
        },
        OrderRow {
            case: "an Edge device ordered in another Region with remote management not installed",
            region: "eu-central-1",
            job_type: "LOCAL_USE",
            snowball_type: "EDGE",
            remote_management: Some("NOT_INSTALLED"),
            managed: false,
        },
    ]
}

/// One emulator: a router carrying the Snow Family ORDER service beside this one, over
/// one shared engine, with nothing seeded — every device in it got there by being
/// ordered over HTTP.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what this suite is about; the router still routes by the SigV4
    // credential scope each call below carries.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// The SigV4 credential scope a call carries: the only place a request names a Region,
/// and what the router resolves the service from.
fn scope(region: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDSNOWTEST/20260101/{region}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=00"
    )
}

/// What the emulator answered: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    signing_name: &str,
    target: Option<&str>,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("authorization", scope(region, signing_name));
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
        builder = builder.header("content-type", "application/x-amz-json-1.1");
    } else if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One Snowball call, signed for `region`.
async fn order_call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        region,
        ORDER_SIGNING_NAME,
        Some(&format!("{ORDER_TARGET_PREFIX}.{operation}")),
        Some(body),
    )
    .await
}

/// One Snowball call that must succeed, returning its body.
async fn order_ok(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let answer = order_call(router, region, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} to answer 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body
}

/// One Snow Device Management call, signed for `region`.
async fn device_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    dispatch(
        router,
        method,
        uri,
        region,
        DEVICE_SIGNING_NAME,
        None,
        body,
    )
    .await
}

/// `ListDevices` in `region`, with `query` appended to the collection URI.
async fn list_devices(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    device_call(
        router,
        region,
        "GET",
        &format!("/managed-devices{query}"),
        None,
    )
    .await
}

/// The `devices` list of a `ListDevices` that must have succeeded.
async fn listed_devices(
    router: &MultiServiceRouter,
    region: &str,
    query: &str,
    case: &str,
) -> Vec<Value> {
    let answer = list_devices(router, region, query).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListDevices{query} in {region} to answer 200, got {} with {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("devices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `associatedWithJob` of every listed device, as a set.
fn jobs_of(devices: &[Value]) -> BTreeSet<String> {
    devices
        .iter()
        .map(|device| {
            device
                .get("associatedWithJob")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A member of a device summary, as a string.
fn member(device: &Value, name: &str) -> String {
    device
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The country a Region's carrier delivers to, for each Region this suite orders in: a
/// Snow order is refused a destination outside its Region's shipping area ("Delivery of
/// the device from this region to this country is not available"), so each row picks a
/// country in that Region's own area (source: the shipping-area table in the order
/// service's `src/ops/create_address.rs`).
const DELIVERY_COUNTRIES: &[(&str, &str)] = &[
    ("us-west-2", "US"),
    ("us-east-2", "US"),
    ("eu-central-1", "DE"),
    ("ap-southeast-2", "AU"),
    ("ca-central-1", "CA"),
];

/// Create an address in `region` and answer the `AddressId` the order service minted, so
/// an order has somewhere to ship to.
async fn create_address(router: &MultiServiceRouter, region: &str) -> String {
    let country = DELIVERY_COUNTRIES
        .iter()
        .find(|(area, _)| *area == region)
        .map(|(_, country)| *country)
        .unwrap_or_else(|| panic!("this suite knows the shipping area of {region}"));
    let created = order_ok(
        router,
        region,
        "CreateAddress",
        json!({
            "Address": {
                "Name": "Ordered Device Receiver",
                "Street1": "123 Main Street",
                "City": "Seattle",
                "Country": country,
                "PostalCode": "98101",
                "PhoneNumber": "425-555-0100",
            }
        }),
        "creating the address every order in this suite ships to",
    )
    .await;
    created
        .get("AddressId")
        .and_then(Value::as_str)
        .expect("CreateAddress reports the id it minted")
        .to_string()
}

/// The `CreateJob` body one row orders with. An order that moves data between Amazon S3
/// and the device also names the buckets it transfers and the role it transfers them
/// under; an order for local use needs neither.
fn order_body(row: &OrderRow, address_id: &str) -> Value {
    let mut body = json!({
        "JobType": row.job_type,
        "SnowballType": row.snowball_type,
        "ShippingOption": "NEXT_DAY",
        "AddressId": address_id,
    });
    if let Some(remote_management) = row.remote_management {
        body["RemoteManagement"] = json!(remote_management);
    }
    if row.job_type == "IMPORT" || row.job_type == "EXPORT" {
        body["RoleARN"] = json!("arn:aws:iam::123456789012:role/snow-device-orders");
        body["Resources"] = json!({
            "S3Resources": [{ "BucketArn": "arn:aws:s3:::snow-device-orders" }]
        });
    }
    body
}

/// Place one row's order and answer the `JobId` it was given.
async fn place_order(router: &MultiServiceRouter, row: &OrderRow, address_id: &str) -> String {
    let created = order_ok(
        router,
        row.region,
        "CreateJob",
        order_body(row, address_id),
        row.case,
    )
    .await;
    created
        .get("JobId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {:?}: CreateJob reports the JobId it minted", row.case))
        .to_string()
}

/// Every order of [`order_rows`], placed over HTTP, with the `JobId` each was given.
async fn place_every_order(router: &MultiServiceRouter) -> Vec<(OrderRow, String)> {
    let mut placed = Vec::new();
    let mut addresses: Vec<(&str, String)> = Vec::new();
    for row in order_rows() {
        let address_id = match addresses.iter().find(|(region, _)| *region == row.region) {
            Some((_, address_id)) => address_id.clone(),
            None => {
                let address_id = create_address(router, row.region).await;
                addresses.push((row.region, address_id.clone()));
                address_id
            }
        };
        let job_id = place_order(router, &row, &address_id).await;
        placed.push((row, job_id));
    }
    placed
}

/// The whole rule on the table: each order that installed remote management is one
/// managed device of its own Region, carrying that order's `JobId`, and no order that
/// installed none produces a device at all.
#[tokio::test]
async fn each_remote_management_order_ships_one_managed_device_in_its_own_region() {
    let covered: BTreeSet<&str> = order_rows()
        .iter()
        .filter_map(|row| row.remote_management)
        .collect();
    assert_eq!(
        covered,
        REMOTE_MANAGEMENT_VALUES.iter().copied().collect::<BTreeSet<&str>>(),
        "case \"the table\": expected a row for every RemoteManagement value the model admits, \
         got {covered:?}"
    );

    let router = emulator();
    let placed = place_every_order(&router).await;

    let regions: BTreeSet<&str> = placed.iter().map(|(row, _)| row.region).collect();
    for region in regions {
        let expected: BTreeSet<String> = placed
            .iter()
            .filter(|(row, _)| row.region == region && row.managed)
            .map(|(_, job_id)| job_id.clone())
            .collect();
        let devices = listed_devices(&router, region, "", region).await;
        assert_eq!(
            jobs_of(&devices),
            expected,
            "case {region:?}: expected ListDevices to report exactly the devices ordered with \
             remote management in that Region, got {devices:?}"
        );
        assert_eq!(
            devices.len(),
            expected.len(),
            "case {region:?}: expected one device per such order, got {devices:?}"
        );
    }

    for (row, job_id) in &placed {
        let case = row.case;
        // "The ID of the job used to order the device": the filter narrows the inventory
        // to the devices of that order — exactly one device for an order that ships one,
        // and none at all for an order that installed no remote management.
        let filtered = listed_devices(
            &router,
            row.region,
            &format!("?jobId={job_id}"),
            case,
        )
        .await;
        if !row.managed {
            assert!(
                filtered.is_empty(),
                "case {case:?}: expected ListDevices?jobId={job_id} to report no device for an \
                 order that installed no remote management, got {filtered:?}"
            );
            continue;
        }
        assert_eq!(
            filtered.len(),
            1,
            "case {case:?}: expected ListDevices?jobId={job_id} to report the one device that \
             order ships, got {filtered:?}"
        );
        let device = &filtered[0];
        assert_eq!(
            member(device, "associatedWithJob"),
            *job_id,
            "case {case:?}: expected associatedWithJob to be the ordering job {job_id}, got \
             {device:?}"
        );
        let managed_device_id = member(device, "managedDeviceId");
        assert!(
            !managed_device_id.is_empty(),
            "case {case:?}: expected the listed device to carry a managedDeviceId, got {device:?}"
        );
        assert_eq!(
            member(device, "managedDeviceArn"),
            format!(
                "arn:aws:snow-device-management:{}:000000000000:managed-device/{managed_device_id}",
                row.region
            ),
            "case {case:?}: expected the device ARN to name the Region the order was placed in, \
             got {device:?}"
        );

        // The device the order ships describes itself: the same order, and the device
        // type that order was placed for.
        let described = device_call(
            &router,
            row.region,
            "POST",
            &format!("/managed-device/{managed_device_id}/describe"),
            None,
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeDevice on {managed_device_id} to answer 200, got {} \
             with {}",
            described.status, described.body
        );
        assert_eq!(
            member(&described.body, "associatedWithJob"),
            *job_id,
            "case {case:?}: expected DescribeDevice to report the ordering job {job_id}, got {}",
            described.body
        );
        assert_eq!(
            member(&described.body, "deviceType"),
            row.snowball_type,
            "case {case:?}: expected the device type to be the ordered SnowballType, got {}",
            described.body
        );

        // The device became manageable when its ORDER was placed, so the instants it
        // reports having last reached out and last pushed an update at are the order's
        // own creation instant — not the instant some call first looked at it.
        let order = order_ok(
            &router,
            row.region,
            "DescribeJob",
            json!({ "JobId": job_id }),
            case,
        )
        .await;
        let ordered_at = order["JobMetadata"]["CreationDate"].clone();
        assert!(
            ordered_at.is_number(),
            "case {case:?}: expected DescribeJob to report the order's CreationDate, got {order}"
        );
        for instant in ["lastReachedOutAt", "lastUpdatedAt"] {
            assert_eq!(
                described.body[instant],
                ordered_at,
                "case {case:?}: expected {instant} to be the instant the order was placed \
                 ({ordered_at}), got {}",
                described.body
            );
        }

        // The device is this order's and belongs to the Region the order was placed in,
        // so no other Region's inventory holds it.
        for other in order_rows()
            .iter()
            .map(|other| other.region)
            .collect::<BTreeSet<&str>>()
        {
            if other == row.region {
                continue;
            }
            let elsewhere = listed_devices(&router, other, "", case).await;
            assert!(
                !jobs_of(&elsewhere).contains(job_id),
                "case {case:?}: expected the device ordered in {} to be invisible in {other}, got \
                 {elsewhere:?}",
                row.region
            );
        }
    }
}

/// A `jobId` no order was ever minted under narrows the inventory to nothing, rather than
/// to a device of another order.
#[tokio::test]
async fn a_job_id_no_order_was_placed_under_narrows_to_no_device() {
    let router = emulator();
    let placed = place_every_order(&router).await;
    let region = placed[0].0.region;

    // Each row is a well-formed `JobId` (`@length(min: 1, max: 64)`) that names no order
    // this emulator placed: the longest the member admits, the shortest, a value in the
    // `JID` form of a real job id, and the id of an order placed in ANOTHER Region.
    let foreign = placed
        .iter()
        .find(|(row, _)| row.region != region && row.managed)
        .map(|(_, job_id)| job_id.clone())
        .expect("the table places a managed order in a second Region");
    let rows: Vec<(&str, String)> = vec![
        ("the longest JobId the member admits", "J".repeat(64)),
        ("the shortest JobId the member admits", "J".to_string()),
        (
            "an unused id in the JID form a job id carries",
            "JIDffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        ("the id of an order placed in another Region", foreign),
    ];
    for (case, job_id) in rows {
        let devices = listed_devices(&router, region, &format!("?jobId={job_id}"), case).await;
        assert!(
            devices.is_empty(),
            "case {case:?}: expected ListDevices?jobId={job_id} in {region} to report no device, \
             got {devices:?}"
        );
    }
}

/// A cluster order ships one managed device per node job, each carrying its own node's
/// `JobId`.
#[tokio::test]
async fn a_cluster_order_ships_one_managed_device_per_node_job() {
    // Each row is one cluster order: the number of node jobs it asks for (the cluster's
    // whole quorum, and fewer) and the `RemoteManagement` it installs on them, which
    // every node inherits.
    let rows: Vec<(&str, i64, &str)> = vec![
        ("a single-node cluster managed INSTALLED_AUTOSTART", 1, "INSTALLED_AUTOSTART"),
        ("a two-node cluster managed INSTALLED_ONLY", 2, "INSTALLED_ONLY"),
        ("a five-node cluster managed INSTALLED_AUTOSTART", 5, "INSTALLED_AUTOSTART"),
        ("a two-node cluster managed NOT_INSTALLED", 2, "NOT_INSTALLED"),
    ];
    for (case, nodes, remote_management) in rows {
        let router = emulator();
        let region = "ap-southeast-2";
        let address_id = create_address(&router, region).await;
        let created = order_ok(
            &router,
            region,
            "CreateCluster",
            json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "AddressId": address_id,
                "InitialClusterSize": nodes,
                "RemoteManagement": remote_management,
            }),
            case,
        )
        .await;
        let node_jobs: BTreeSet<String> = created
            .get("JobListEntries")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| member(entry, "JobId"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            node_jobs.len(),
            usize::try_from(nodes).expect("a non-negative node count"),
            "case {case:?}: expected CreateCluster to report one node job per node, got {created}"
        );

        if remote_management == "NOT_INSTALLED" {
            let answer = list_devices(&router, region, "").await;
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected a Region whose only order installed no remote management \
                 to hold no inventory, got {} with {}",
                answer.status, answer.body
            );
            continue;
        }

        let devices = listed_devices(&router, region, "", case).await;
        assert_eq!(
            jobs_of(&devices),
            node_jobs,
            "case {case:?}: expected one managed device per node job, got {devices:?}"
        );
        for node_job in &node_jobs {
            let filtered =
                listed_devices(&router, region, &format!("?jobId={node_job}"), case).await;
            assert_eq!(
                filtered.len(),
                1,
                "case {case:?}: expected ListDevices?jobId={node_job} to report that node's one \
                 device, got {filtered:?}"
            );
        }
    }
}

/// An order that no longer holds a device has no managed device either: a cancelled order
/// never has an appliance prepared for it, while every other order of the same account
/// keeps its device.
#[tokio::test]
async fn an_order_that_no_longer_holds_its_device_leaves_the_inventory() {
    let router = emulator();
    let region = "us-east-2";
    let address_id = create_address(&router, region).await;

    // Three orders of the same account in one Region, so cancelling one shows the
    // inventory following that ONE order rather than the Region's whole inventory.
    let mut job_ids = Vec::new();
    for snowball_type in ["SNC1_HDD", "SNC1_SSD", "EDGE_CG"] {
        let row = OrderRow {
            case: "an order that is later cancelled",
            region,
            job_type: "LOCAL_USE",
            snowball_type,
            remote_management: Some("INSTALLED_AUTOSTART"),
            managed: true,
        };
        job_ids.push(place_order(&router, &row, &address_id).await);
    }
    let listed = jobs_of(&listed_devices(&router, region, "", "before any cancellation").await);
    assert_eq!(
        listed,
        job_ids.iter().cloned().collect::<BTreeSet<String>>(),
        "case \"before any cancellation\": expected one device per order, got {listed:?}"
    );

    let cancelled = job_ids
        .first()
        .expect("the three orders above were placed")
        .clone();
    order_ok(
        &router,
        region,
        "CancelJob",
        json!({ "JobId": cancelled }),
        "cancelling the first order",
    )
    .await;

    let remaining = jobs_of(&listed_devices(&router, region, "", "after the cancellation").await);
    assert!(
        !remaining.contains(&cancelled),
        "case \"after the cancellation\": expected the cancelled order's device to be gone, got \
         {remaining:?}"
    );
    for job_id in job_ids.iter().skip(1) {
        assert!(
            remaining.contains(job_id),
            "case \"after the cancellation\": expected the order {job_id} to keep its device, got \
             {remaining:?}"
        );
    }
    let filtered = listed_devices(
        &router,
        region,
        &format!("?jobId={cancelled}"),
        "the cancelled order's own filter",
    )
    .await;
    assert!(
        filtered.is_empty(),
        "case \"the cancelled order's own filter\": expected no device, got {filtered:?}"
    );
}

/// The device an order ships is reachable from the whole device/task surface: a task can
/// be created on it, read back, and its execution reported, and its ARN can be tagged.
#[tokio::test]
async fn an_ordered_device_serves_the_device_and_task_surface() {
    // Each row is one command the modelled `Command` union carries
    // (`smithy_query(query="shape", shape="Command")`: `unlock`, `reboot`), on a device
    // ordered with a different `RemoteManagement` value of the two that install it.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("an unlock task", json!({ "unlock": {} }), "INSTALLED_AUTOSTART"),
        ("a reboot task", json!({ "reboot": {} }), "INSTALLED_ONLY"),
    ];
    for (case, command, remote_management) in rows {
        let router = emulator();
        let region = "ca-central-1";
        let address_id = create_address(&router, region).await;
        let row = OrderRow {
            case,
            region,
            job_type: "LOCAL_USE",
            snowball_type: "SNC1_SSD",
            remote_management: Some(remote_management),
            managed: true,
        };
        let job_id = place_order(&router, &row, &address_id).await;
        let devices = listed_devices(&router, region, "", case).await;
        assert_eq!(
            devices.len(),
            1,
            "case {case:?}: expected the ordered device to be listed, got {devices:?}"
        );
        let managed_device_id = member(&devices[0], "managedDeviceId");
        let device_arn = member(&devices[0], "managedDeviceArn");

        let created = device_call(
            &router,
            region,
            "POST",
            "/task",
            Some(json!({
                "targets": [managed_device_id],
                "command": command,
                "description": "the task this suite created on an ordered device",
            })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateTask on the ordered device to answer 200, got {} with \
             {}",
            created.status, created.body
        );
        let task_id = member(&created.body, "taskId");
        assert!(
            !task_id.is_empty(),
            "case {case:?}: expected CreateTask to mint a taskId, got {}",
            created.body
        );

        let described = device_call(
            &router,
            region,
            "POST",
            &format!("/task/{task_id}"),
            None,
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeTask to answer 200, got {} with {}",
            described.status, described.body
        );
        assert_eq!(
            described
                .body
                .get("targets")
                .and_then(Value::as_array)
                .map(|targets| targets.iter().map(|t| t.as_str().unwrap_or_default().to_string()).collect::<Vec<String>>()),
            Some(vec![managed_device_id.clone()]),
            "case {case:?}: expected the task to target the ordered device, got {}",
            described.body
        );

        let executions = device_call(
            &router,
            region,
            "GET",
            &format!("/executions?taskId={task_id}"),
            None,
        )
        .await;
        assert_eq!(
            executions.status, 200,
            "case {case:?}: expected ListExecutions to answer 200, got {} with {}",
            executions.status, executions.body
        );
        let execution_devices: Vec<String> = executions
            .body
            .get("executions")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|execution| member(execution, "managedDeviceId"))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            execution_devices,
            vec![managed_device_id.clone()],
            "case {case:?}: expected one execution on the ordered device, got {}",
            executions.body
        );

        // The device's own ARN is a resource of this service, so it takes tags, and the
        // ARN names the Region the order was placed in.
        let tag_uri = format!("/tags/{}", device_arn.replace('/', "%2F"));
        let tagged = device_call(
            &router,
            region,
            "POST",
            &tag_uri,
            Some(json!({ "tags": { "ordered-by": job_id } })),
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected TagResource on the ordered device to answer 200, got {} with \
             {}",
            tagged.status, tagged.body
        );
        let read_back = device_call(&router, region, "GET", &tag_uri, None).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {} with {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.body.get("tags"),
            Some(&json!({ "ordered-by": job_id })),
            "case {case:?}: expected the tag written on the ordered device to read back, got {}",
            read_back.body
        );
    }
}
