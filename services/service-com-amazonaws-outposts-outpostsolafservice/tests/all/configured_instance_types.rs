// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetOutpostInstanceTypes`, driven over the wire through the
//! same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The report is the instance types ONE Outpost's EC2 capacity is CURRENTLY
//!    CONFIGURED with, computed from the records this store holds of that
//!    configuration. A COMPLETED capacity task configures the hardware it named — one
//!    asset of the Outpost, or the Outpost itself when it named none — so the pools of
//!    the task that completed LAST on each of those are what stands configured there,
//!    and a pool of `Count` zero configures no instance of its type and so names none.
//! 2. Until a capacity task reconfigures it, what is configured is what the hardware
//!    the Outpost's uncancelled orders subscribe to arrived carrying: the largest size
//!    of each instance family the ordered configuration publishes EC2 capacity of
//!    (read off `ListCatalogItems`, which every expectation below is computed from
//!    rather than spelled out). Capacity is configured ON compute hardware, so an
//!    Outpost holding none — one whose ordered hardware is not installed yet — is
//!    configured with nothing.
//! 3. Every reported entry carries the `VCPUs` the published instance-type catalog
//!    reports for its type, so this report cannot disagree with what
//!    `ListOrderableInstanceTypes` says about an instance type; a type that catalog
//!    publishes no vCPU figure for is one no capacity task can configure at all — a
//!    configuration naming it cannot be weighed against the hardware, so the task
//!    fails and configures nothing ([`crate::capacity_task_outcome`]).
//! 4. The configuration moves on the CLOCK alone: a capacity task still being worked
//!    through has configured nothing, and reading the report never moves it.
//! 5. The report is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped, for any page size; the last
//!    page carries no token; a boundary past the end is an empty last page; and a
//!    token this operation did not issue — including one another list of this service
//!    issued — is a bad request.
//! 6. It is a REPORT: both identifier forms the modelled `OutpostId` pattern accepts
//!    answer the same thing, the answer echoes the Outpost's own id and ARN as
//!    `GetOutpost` reports them, repeating a call answers the same thing, and what it
//!    read is left exactly as it was.
//! 7. An identifier that names no Outpost of this region is 404 `NotFoundException`
//!    with the identifier echo of the value AS SUPPLIED; one that is no identifier at
//!    all, and a page size outside the modelled range, is 400 `ValidationException`.
//!
//! The WIDER report — what an Outpost's hardware could support, "including instance
//! types that are not currently configured" — is [`crate::supported_instance_types`].

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{AssetInfo, ComputeAttributes};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    nested_page_kind, CONFIGURED_INSTANCE_TYPE_PAGE,
};

use crate::capacity_task_lifecycle::{an_order, pool, settle_capacity_task, start_ok, HOST_VCPUS};
use crate::outpost_assets::record;
use crate::site_and_outpost_lifecycle::{
    deliver_orders, install_hardware, mark_outpost_active, orderable_site, query_value, refusal,
    segment, text, Outposts, Refusal, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX,
};

/// An instance type the modelled `InstanceTypeName` pattern (`^[a-z0-9\-]+\.[a-z0-9\-]+$`)
/// accepts but that this service's published catalog carries no entry for — so no
/// capacity task can configure it: there is no size to weigh a pool of it by.
const UNPUBLISHED_INSTANCE_TYPE: &str = "zz9.xlarge";

/// A `CatalogItemId` of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`) that this
/// service publishes no hardware configuration under.
const UNPUBLISHED_SKU: &str = "OR-0000000";

/// The modelled `CatalogItemStatus` a published configuration an order can still be
/// placed for carries, as `smithy_query(query="shape", shape="CatalogItemStatus")`
/// reports the value set (`AVAILABLE` | `DISCONTINUED`).
const ORDERABLE_ITEM_STATUS: &str = "AVAILABLE";

/// The route `GetOutpostInstanceTypes` binds, for one identifier form and one query
/// string.
fn route(identifier: &str, query: &str) -> String {
    format!("/outposts/{}/instanceTypes{query}", segment(identifier))
}

/// `GetOutpostInstanceTypes`, requiring HTTP 200.
async fn configured(outposts: &Outposts, identifier: &str, query: &str) -> Value {
    outposts.ok("GET", &route(identifier, query), None).await
}

/// The `(InstanceType, VCPUs)` of every entry of one answer, in the order it reported
/// them — `None` for an entry that carries no count at all.
fn reported(output: &Value) -> Vec<(String, Option<i64>)> {
    output
        .get("InstanceTypes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        text(item, "InstanceType"),
                        item.get("VCPUs").and_then(Value::as_i64),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every instance type this service holds a specification of: the source every
/// `VCPUs` expectation below is computed from.
///
/// A capacity task configures hardware in any size it runs, which is the wider of the
/// service's two instance-type lists rather than the catalog
/// `ListOrderableInstanceTypes` publishes (see [`crate::instance_type_catalogs`]).
fn catalog() -> Vec<Value> {
    crate::instance_type_catalogs::specifications()
}

/// The `VCPUs` that catalog publishes for `instance_type`, or `None` for a type it
/// publishes no entry for.
fn published_vcpus(catalog: &[Value], instance_type: &str) -> Option<i64> {
    catalog
        .iter()
        .find(|entry| text(entry, "InstanceType") == instance_type)
        .and_then(|entry| entry.get("VCPUs").and_then(Value::as_i64))
}

/// What a report of the types `want` names must answer: those types, in that order,
/// each with the catalog's own count.
fn expected(catalog: &[Value], want: &[&str]) -> Vec<(String, Option<i64>)> {
    want.iter()
        .map(|instance_type| {
            (
                (*instance_type).to_string(),
                published_vcpus(catalog, instance_type),
            )
        })
        .collect()
}

/// One site and one Outpost on it carrying `hardware_type`, as that Outpost's
/// `(OutpostId, OutpostArn)`.
///
/// The Outpost reports the terminal `ACTIVE` lifecycle status, because the rows start
/// capacity tasks on the hardware they record and a task is refused on an Outpost whose
/// hardware is not in service ([`crate::precondition_gates`]).
async fn an_outpost(
    outposts: &Outposts,
    name: &str,
    hardware_type: Option<&str>,
) -> (String, String) {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(hardware_type) = hardware_type {
        request["SupportedHardwareType"] = json!(hardware_type);
    }
    let outpost = outposts.create_outpost(&request).await;
    let (outpost_id, outpost_arn) = (text(&outpost, "OutpostId"), text(&outpost, "OutpostArn"));
    mark_outpost_active(outposts, &outpost_id);
    (outpost_id, outpost_arn)
}

/// One recorded compute asset, so a capacity task can be scoped to a piece of this
/// Outpost's hardware and weighed against its capacity.
///
/// An asset is a PHYSICAL unit Amazon Web Services installs, so no call in this API
/// creates one: the rows below record their own through the same store seam
/// [`crate::outpost_assets`] records hardware through, and that module owns the
/// projection every member of one is asserted against. The vCPU ceiling is
/// [`HOST_VCPUS`], the figure the hardware this service installs really carries, so
/// what a row's task asks for has to fit on the hosts the row records — a
/// configuration above them fails instead of configuring anything.
fn compute_asset(asset_id: &str) -> AssetInfo {
    AssetInfo {
        asset_id: Some(asset_id.to_string()),
        rack_id: Some("rack-01".to_string()),
        asset_type: Some("COMPUTE".to_string()),
        compute_attributes: Some(ComputeAttributes {
            host_id: Some("h-00000000000000001".to_string()),
            state: Some("ACTIVE".to_string()),
            instance_families: None,
            instance_type_capacities: None,
            max_vcpus: Some(HOST_VCPUS),
        }),
        asset_location: None,
    }
}

/// Every entry the report serves when it is read `size` at a time, following each
/// `NextToken` to the end.
async fn paged(outposts: &Outposts, identifier: &str, size: usize) -> Vec<(String, Option<i64>)> {
    let mut collected: Vec<(String, Option<i64>)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match &token {
            Some(token) => format!("?MaxResults={size}&NextToken={}", query_value(token)),
            None => format!("?MaxResults={size}"),
        };
        let page = configured(outposts, identifier, &query).await;
        let entries = reported(&page);
        assert!(
            entries.len() <= size,
            "case size {size}: a page holds at most MaxResults entries, got {entries:?}"
        );
        collected.extend(entries);
        pages += 1;
        assert!(
            pages <= 32,
            "case size {size}: paging must terminate, got {pages} pages and {collected:?}"
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    collected
}

/// One row of the capacity-task table: the capacity tasks that configure one Outpost,
/// and the configuration they leave on it.
struct Configuring {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The `StartCapacityTask` requests to run on the Outpost, in order. A row with
    /// two runs the second only once the first has completed — one active capacity
    /// task per Outpost, as [`crate::capacity_task_lifecycle`] states.
    tasks: Vec<Value>,
    /// The `AssetId`s recorded as this Outpost's hardware, so a task scoped to one of
    /// them resolves AND the configuration its tasks ask for has hardware to be
    /// weighed against. Values the modelled `AssetIdInput` (`^\d{10}$`) accepts.
    hardware: &'static [&'static str],
    /// Whether the first task names an order placed against this Outpost.
    with_order: bool,
    /// The instance types the report must then answer, in the order it answers them.
    expected: &'static [&'static str],
}

/// Rules 1 and 3: whatever capacity tasks have completed on an Outpost, the report is
/// the configuration they left on it, each entry carrying the catalog's own `VCPUs`.
///
/// The rows differ along everything the rule generalises over: how many pools a task
/// configures (none, one, several), a `Count` at the published minimum of the modelled
/// `range(0..9999)` and inside it, a type the published catalog does not carry, how
/// many tasks have run (one, two in sequence), the hardware a task names (the Outpost
/// itself, one of its assets, both), how much hardware the Outpost holds (none, one
/// host, two), the same type configured on two scopes at once, and whether the task
/// names an order. Each row asks for a configuration the hardware it records can hold,
/// because only a COMPLETED task configures anything; that a task above that capacity
/// fails instead is [`crate::capacity_task_outcome`]'s.
#[tokio::test]
async fn a_completed_capacity_task_is_what_the_outpost_is_configured_with() {
    let outposts = Outposts::new();
    let catalog = catalog();
    assert_eq!(
        published_vcpus(&catalog, UNPUBLISHED_INSTANCE_TYPE),
        None,
        "the row that asks for {UNPUBLISHED_INSTANCE_TYPE:?} needs a type the published \
         catalog does not carry"
    );

    let rows: Vec<Configuring> = vec![
        Configuring {
            case: "one pool of one type",
            tasks: vec![json!({ "InstancePools": [pool("m5.4xlarge", 2)] })],
            hardware: &["1000000001"],
            with_order: false,
            expected: &["m5.4xlarge"],
        },
        Configuring {
            // The published MAXIMUM `Count` is asked of a start in
            // [`crate::capacity_task_lifecycle`], where what a started task persists is
            // stated: no hardware holds 9999 of an instance type, so a task asking for
            // it fails and configures nothing.
            case: "several pools, one at the published minimum Count",
            tasks: vec![json!({
                "InstancePools": [
                    pool("c5.xlarge", 1),
                    pool("m5.24xlarge", 1),
                    pool("m5.large", 0),
                ],
            })],
            hardware: &["1000000001", "1000000002"],
            with_order: false,
            // The pool at the minimum count configures no instance of its type, so it
            // names no configured instance type.
            expected: &["c5.xlarge", "m5.24xlarge"],
        },
        Configuring {
            case: "every pool at the minimum count",
            tasks: vec![json!({ "InstancePools": [pool("m5.large", 0)] })],
            hardware: &["1000000001"],
            with_order: false,
            expected: &[],
        },
        Configuring {
            // No pool asks for any capacity, so there is nothing to weigh and nothing
            // the Outpost's hardware — none at all, here — has to hold.
            case: "no pools at all",
            tasks: vec![json!({ "InstancePools": [] })],
            hardware: &[],
            with_order: false,
            expected: &[],
        },
        Configuring {
            // Rule 3: the catalog publishes no vCPU figure for this type, so the
            // configuration cannot be weighed against the hardware at all — the task
            // fails and leaves the Outpost configured with nothing.
            case: "a type the published catalog does not carry configures nothing",
            tasks: vec![json!({ "InstancePools": [pool(UNPUBLISHED_INSTANCE_TYPE, 1)] })],
            hardware: &["1000000001"],
            with_order: false,
            expected: &[],
        },
        Configuring {
            case: "a later task on the Outpost supersedes the earlier one",
            tasks: vec![
                json!({ "InstancePools": [pool("m5.large", 1)] }),
                json!({ "InstancePools": [pool("c5.9xlarge", 3)] }),
            ],
            hardware: &["1000000001", "1000000002"],
            with_order: false,
            expected: &["c5.9xlarge"],
        },
        Configuring {
            case: "one task scoped to an asset and one to the Outpost",
            tasks: vec![
                json!({ "InstancePools": [pool("m5.large", 1)], "AssetId": "1000000001" }),
                json!({ "InstancePools": [pool("c5.xlarge", 2)] }),
            ],
            hardware: &["1000000001"],
            with_order: false,
            // The Outpost's own configuration first, then each asset's.
            expected: &["c5.xlarge", "m5.large"],
        },
        Configuring {
            case: "the same type configured on two scopes is reported once",
            tasks: vec![
                json!({ "InstancePools": [pool("r5.2xlarge", 1)], "AssetId": "1000000002" }),
                json!({ "InstancePools": [pool("r5.2xlarge", 4)] }),
            ],
            hardware: &["1000000002"],
            with_order: false,
            expected: &["r5.2xlarge"],
        },
        Configuring {
            case: "a task that names the order its hardware was bought on",
            tasks: vec![json!({ "InstancePools": [pool("m5.8xlarge", 1)] })],
            hardware: &["1000000001"],
            with_order: true,
            expected: &["m5.8xlarge"],
        },
    ];

    // Every row's first task is started before anything is waited for, so the whole
    // table costs ONE wait for the clock rather than one per row.
    let mut placed: Vec<(String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let (outpost_id, outpost_arn) =
            an_outpost(&outposts, &format!("configured-{index}"), None).await;
        if !row.hardware.is_empty() {
            let assets: Vec<AssetInfo> = row.hardware.iter().copied().map(compute_asset).collect();
            record(&outposts, &outpost_id, &assets, row.case);
        }
        if let Some(first) = row.tasks.first() {
            let mut request = first.clone();
            if row.with_order {
                request["OrderId"] = json!(an_order(&outposts, &outpost_id).await);
            }
            start_ok(&outposts, &outpost_id, &request).await;
        }
        placed.push((outpost_id, outpost_arn));
    }
    settle_capacity_task();
    for (row, (outpost_id, _)) in rows.iter().zip(&placed) {
        if let Some(second) = row.tasks.get(1) {
            start_ok(&outposts, outpost_id, second).await;
        }
    }
    settle_capacity_task();

    for (row, (outpost_id, outpost_arn)) in rows.iter().zip(&placed) {
        let case = row.case;
        let answer = configured(&outposts, outpost_id, "").await;
        assert_eq!(
            reported(&answer),
            expected(&catalog, row.expected),
            "case {case:?}: expected the instance types the completed capacity tasks \
             configured, with the catalog's own VCPUs, got {answer:?}"
        );
        assert!(
            answer.get("NextToken").is_none(),
            "case {case:?}: a page that exhausts the report issues no NextToken, got {answer:?}"
        );
        // Rule 6's identifier half: the ARN form names the same Outpost.
        assert_eq!(
            reported(&configured(&outposts, outpost_arn, "").await),
            reported(&answer),
            "case {case:?}: the ARN form must answer the same report as the bare id"
        );
    }
}

/// Rule 4: a capacity task configures nothing until the clock has completed it, and a
/// read of the report never moves it.
#[tokio::test]
async fn a_capacity_task_still_running_has_configured_nothing() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let (outpost_id, _) = an_outpost(&outposts, "running", None).await;
    // Hardware the task's configuration is weighed against, so the task this row
    // follows is one that completes.
    record(
        &outposts,
        &outpost_id,
        &[compute_asset("1000000001")],
        "running",
    );
    let started = start_ok(
        &outposts,
        &outpost_id,
        &json!({ "InstancePools": [pool("m5.4xlarge", 2), pool("c5.large", 1)] }),
    )
    .await;
    assert_eq!(
        text(&started, "CapacityTaskStatus"),
        "REQUESTED",
        "case running: a just-started task is awaiting its next step, got {started:?}"
    );

    let while_running = configured(&outposts, &outpost_id, "").await;
    assert_eq!(
        reported(&while_running),
        Vec::new(),
        "case running: a task Amazon Web Services Outposts is still working through has \
         configured nothing, got {while_running:?}"
    );

    settle_capacity_task();
    let want = expected(&catalog, &["m5.4xlarge", "c5.large"]);
    let completed = configured(&outposts, &outpost_id, "").await;
    assert_eq!(
        reported(&completed),
        want,
        "case completed: the pools of the completed task are what stands configured, got \
         {completed:?}"
    );
    // Rule 6: repeating the call answers the same thing — the report moved on the
    // clock, not on being read.
    assert_eq!(
        reported(&configured(&outposts, &outpost_id, "").await),
        want,
        "case completed: a second read must answer the same configuration"
    );
}

/// One row of the order table: the hardware an Outpost's orders subscribe to, and the
/// configuration that leaves on it.
struct Ordered {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The Outpost's `SupportedHardwareType`, which is the hardware the service
    /// installs on it — `None` for an Outpost that names no configuration, and so
    /// settles holding no hardware at all.
    hardware_type: Option<&'static str>,
    /// The orders to place against it: the `CatalogItemId` of each line item and how
    /// many units of it. An index into the published catalog rather than a SKU spelled
    /// out, so a row names hardware this service really publishes; `None` is
    /// [`UNPUBLISHED_SKU`].
    orders: &'static [&'static [(Option<usize>, i64)]],
    /// Whether every order placed is then cancelled.
    cancelled: bool,
    /// Whether the row's own expectation is the EMPTY report, so a row that must
    /// report something cannot pass by computing nothing to expect.
    expects_nothing: bool,
}

/// Rules 2 and 3: before any capacity task, an Outpost is configured with the largest
/// size of each instance family the configurations its orders subscribe to publish EC2
/// capacity of.
///
/// The rows differ along everything the rule generalises over: how many orders (none,
/// one, two), how many line items an order carries, how many units of a configuration
/// it buys, a SKU this service publishes nothing under, a cancelled order, an Outpost
/// of each form factor, and an Outpost that holds no compute hardware at all.
#[tokio::test]
async fn an_outpost_no_capacity_task_has_touched_is_configured_by_its_orders() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let published = outposts.ok("GET", "/catalog/items", None).await["CatalogItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !published
            .iter()
            .any(|item| text(item, "CatalogItemId") == UNPUBLISHED_SKU),
        "the row that orders {UNPUBLISHED_SKU:?} needs a SKU this service publishes nothing \
         under, got {published:?}"
    );
    // The published configurations an order can be placed FOR: every row below orders
    // the configuration it names, and an order is only taken for a configuration the
    // catalog still offers (`crate::order_catalog_item_status`), so a row's position
    // indexes that part of the catalog. Which configurations those are is read off the
    // catalog's own `ItemStatus` rather than spelled out here.
    let catalog_items: Vec<Value> = published
        .into_iter()
        .filter(|item| text(item, "ItemStatus") == ORDERABLE_ITEM_STATUS)
        .collect();
    assert!(
        catalog_items.len() >= 2,
        "the published catalog must hold configurations to order, got {catalog_items:?}"
    );
    assert!(
        !catalog_items
            .iter()
            .any(|item| text(item, "CatalogItemId") == UNPUBLISHED_SKU),
        "the row that orders {UNPUBLISHED_SKU:?} needs a SKU this service publishes nothing \
         under, got {catalog_items:?}"
    );

    let rows: &[Ordered] = &[
        Ordered {
            case: "one order of one published configuration",
            hardware_type: Some("RACK"),
            orders: &[&[(Some(0), 1)]],
            cancelled: false,
            expects_nothing: false,
        },
        Ordered {
            case: "an order on an Outposts server",
            hardware_type: Some("SERVER"),
            orders: &[&[(Some(1), 1)]],
            cancelled: false,
            expects_nothing: false,
        },
        Ordered {
            case: "several units of one configuration",
            hardware_type: Some("RACK"),
            orders: &[&[(Some(0), 3)]],
            cancelled: false,
            expects_nothing: false,
        },
        Ordered {
            case: "two orders of two configurations",
            hardware_type: Some("RACK"),
            orders: &[&[(Some(0), 1)], &[(Some(1), 2)]],
            cancelled: false,
            expects_nothing: false,
        },
        Ordered {
            case: "one order of two configurations at once",
            hardware_type: Some("RACK"),
            orders: &[&[(Some(0), 1), (Some(1), 1)]],
            cancelled: false,
            expects_nothing: false,
        },
        Ordered {
            case: "a SKU this service publishes nothing under",
            hardware_type: Some("RACK"),
            orders: &[&[(None, 1)]],
            cancelled: false,
            expects_nothing: true,
        },
        Ordered {
            case: "a published configuration beside an unpublished SKU",
            hardware_type: Some("RACK"),
            orders: &[&[(None, 1), (Some(0), 1)]],
            cancelled: false,
            expects_nothing: false,
        },
        Ordered {
            case: "no order at all",
            hardware_type: Some("RACK"),
            orders: &[],
            cancelled: false,
            expects_nothing: true,
        },
        Ordered {
            case: "a cancelled order",
            hardware_type: Some("RACK"),
            orders: &[&[(Some(0), 1)]],
            cancelled: true,
            expects_nothing: true,
        },
        Ordered {
            case: "an Outpost that holds no hardware",
            hardware_type: None,
            orders: &[&[(Some(0), 1)]],
            cancelled: false,
            expects_nothing: true,
        },
    ];

    let mut placed: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let (outpost_id, _) =
            an_outpost(&outposts, &format!("ordered-{index}"), row.hardware_type).await;
        for line_items in row.orders {
            let requested: Vec<Value> = line_items
                .iter()
                .map(|(published, quantity)| {
                    let catalog_item_id = match published {
                        Some(position) => text(&catalog_items[*position], "CatalogItemId"),
                        None => UNPUBLISHED_SKU.to_string(),
                    };
                    json!({ "CatalogItemId": catalog_item_id, "Quantity": quantity })
                })
                .collect();
            let order = outposts
                .ok(
                    "POST",
                    "/orders",
                    Some(&json!({
                        "OutpostIdentifier": outpost_id,
                        "PaymentOption": "ALL_UPFRONT",
                        "LineItems": requested,
                    })),
                )
                .await;
            if row.cancelled {
                let order_id = text(&order["Order"], "OrderId");
                outposts
                    .ok(
                        "POST",
                        &format!("/orders/{}/cancel", segment(&order_id)),
                        None,
                    )
                    .await;
            }
        }
        placed.push(outpost_id);
    }

    // Capacity is configured ON hardware, so an Outpost whose ordered hardware has not
    // been installed is configured with nothing however much it has ordered — asserted
    // BEFORE that hardware is delivered and installed.
    for (row, outpost_id) in rows.iter().zip(&placed) {
        let answer = configured(&outposts, outpost_id, "").await;
        assert_eq!(
            reported(&answer),
            Vec::new(),
            "case {:?}: an Outpost holding no hardware yet has nothing configured on it, \
             got {answer:?}",
            row.case
        );
    }

    deliver_orders(&outposts);
    for (row, outpost_id) in rows.iter().zip(&placed) {
        let case = row.case;
        // What the rule expects, computed from the published catalog: the largest size
        // of each family each ordered configuration carries capacity of, in the order
        // the orders name them, each type once.
        let mut want: Vec<String> = Vec::new();
        if row.hardware_type.is_some() && !row.cancelled {
            for line_items in row.orders {
                for (published, _) in line_items.iter() {
                    let Some(position) = published else {
                        continue;
                    };
                    for capacity in catalog_items[*position]
                        .get("EC2Capacities")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        // `Family` names the family and `MaxSize` the size alone, so
                        // the instance type the capacity names is the join of them.
                        let instance_type = format!(
                            "{}.{}",
                            text(capacity, "Family"),
                            text(capacity, "MaxSize")
                        );
                        if !want.contains(&instance_type) {
                            want.push(instance_type);
                        }
                    }
                }
            }
        }
        let want: Vec<&str> = want.iter().map(String::as_str).collect();
        assert_eq!(
            want.is_empty(),
            row.expects_nothing,
            "case {case:?}: the expectation computed from the published catalog must have \
             entries exactly where the row says it does, got {want:?}"
        );
        let answer = configured(&outposts, outpost_id, "").await;
        assert_eq!(
            reported(&answer),
            expected(&catalog, &want),
            "case {case:?}: expected the largest size of each family the ordered hardware \
             carries capacity of, got {answer:?}"
        );
    }
}

/// Rule 5: the report pages, for any page size, and a token it did not issue is a bad
/// request.
#[tokio::test]
async fn the_report_pages_and_refuses_a_token_it_did_not_issue() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let (outpost_id, _) = an_outpost(&outposts, "paging", None).await;
    // See [`a_capacity_task_still_running_has_configured_nothing`]: the pools below
    // are weighed against this hardware.
    record(
        &outposts,
        &outpost_id,
        &[compute_asset("1000000001")],
        "paging",
    );
    let pools = ["m5.large", "m5.4xlarge", "c5.xlarge", "r5.2xlarge"];
    start_ok(
        &outposts,
        &outpost_id,
        &json!({
            "InstancePools": pools
                .iter()
                .copied()
                .map(|instance_type| pool(instance_type, 1))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    settle_capacity_task();

    let whole = expected(&catalog, &pools);
    assert_eq!(
        reported(&configured(&outposts, &outpost_id, "").await),
        whole,
        "the unpaged report must answer every configured type"
    );
    // Every page size from one entry at a time to one past the whole report: the pages
    // a caller walks are exactly the report, in the same order.
    for size in 1..=whole.len() + 1 {
        assert_eq!(
            paged(&outposts, &outpost_id, size).await,
            whole,
            "case size {size}: paging must serve exactly the report, in order"
        );
    }

    // A boundary past the end of the report is an empty last page, not a refusal. The
    // report is this Outpost's own, so the stamp its tokens carry names the Outpost as
    // well as the kind of report (`state::nested_page_kind`).
    let past_the_end = configured(
        &outposts,
        &outpost_id,
        &format!(
            "?NextToken={}",
            query_value(&format!(
                "{}##{}",
                whole.len() + 1,
                nested_page_kind(CONFIGURED_INSTANCE_TYPE_PAGE, &[&outpost_id])
            ))
        ),
    )
    .await;
    assert_eq!(
        (reported(&past_the_end).len(), past_the_end.get("NextToken")),
        (0, None),
        "a boundary past the end of the report is an empty last page, got {past_the_end:?}"
    );

    let first = configured(&outposts, &outpost_id, "?MaxResults=1").await;
    let token = text(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "a report of {} entries served one at a time issues a NextToken, got {first:?}",
        whole.len()
    );

    let cases: Vec<Refusal> = vec![
        refusal(
            "a token stamped for ListOrderableInstanceTypes",
            "GET",
            route(&outpost_id, "?NextToken=1%23%23instanceTypes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for GetOutpostSupportedInstanceTypes",
            "GET",
            route(&outpost_id, "?NextToken=1%23%23supportedInstanceTypes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a well-formed token with a nonsense boundary",
            "GET",
            route(&outpost_id, "?NextToken=abc%23%23outpostInstanceTypes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size below the modelled minimum",
            "GET",
            route(&outpost_id, "?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size above the modelled maximum",
            "GET",
            route(&outpost_id, "?MaxResults=1001"),
            400,
            "ValidationException",
        ),
    ];
    outposts.expect_refusals(&cases).await;

    // The other direction: this report's token is not a boundary of the catalog list.
    let replayed = outposts
        .call(
            "GET",
            &format!("/instanceTypes?NextToken={}", query_value(&token)),
            None,
        )
        .await;
    assert_eq!(
        (replayed.status, replayed.error_code.as_str()),
        (400, "ValidationException"),
        "an outpost-instance-types token replayed on the catalog list is refused, got {:?}",
        replayed.body
    );
}

/// Rule 6: what the report says about the Outpost itself, and that reading it changes
/// nothing a later caller sees.
#[tokio::test]
async fn the_report_names_its_outpost_and_leaves_everything_as_it_was() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = an_outpost(&outposts, "reporting", Some("RACK")).await;
    install_hardware(&outposts, &outpost_id).await;
    let started = start_ok(
        &outposts,
        &outpost_id,
        &json!({ "InstancePools": [pool("m5.4xlarge", 2)] }),
    )
    .await;
    let task_id = text(&started, "CapacityTaskId");
    settle_capacity_task();

    for (case, identifier) in [("bare id", &outpost_id), ("ARN", &outpost_arn)] {
        let answer = configured(&outposts, identifier, "").await;
        assert_eq!(
            (text(&answer, "OutpostId"), text(&answer, "OutpostArn")),
            (outpost_id.clone(), outpost_arn.clone()),
            "case {case:?}: the report must name the Outpost as GetOutpost reports it, got \
             {answer:?}"
        );
    }
    let outpost = outposts.get_outpost(&outpost_id).await;
    assert_eq!(
        (
            text(&outpost, "OutpostId"),
            text(&outpost, "OutpostArn"),
            text(&outpost, "LifeCycleStatus")
        ),
        (outpost_id.clone(), outpost_arn.clone(), "ACTIVE".to_string()),
        "the Outpost itself is untouched by the report, got {outpost:?}"
    );
    // The hardware and the capacity task the report read are left exactly as they
    // were: a read of a report is not a write.
    let assets_route = format!("/outposts/{}/assets", segment(&outpost_id));
    let task_route = format!(
        "/outposts/{}/capacity/{}",
        segment(&outpost_id),
        segment(&task_id)
    );
    let assets = outposts.ok("GET", &assets_route, None).await;
    let task = outposts.ok("GET", &task_route, None).await;
    configured(&outposts, &outpost_id, "").await;
    assert_eq!(
        outposts.ok("GET", &assets_route, None).await,
        assets,
        "reading the report must leave the Outpost's hardware as it was"
    );
    assert_eq!(
        outposts.ok("GET", &task_route, None).await,
        task,
        "reading the report must leave the capacity task that configured it as it was"
    );
}

/// Rule 7: what the store does not hold, and what is no identifier at all.
#[tokio::test]
async fn an_outpost_this_store_does_not_hold_is_refused() {
    let outposts = Outposts::new();
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let echoes: Vec<(&str, String, String)> = vec![
        (
            "an Outpost nothing holds, by bare id",
            route(ABSENT_OUTPOST_ID, ""),
            format!("outpostId={ABSENT_OUTPOST_ID}"),
        ),
        (
            "an Outpost nothing holds, by ARN",
            route(&absent_outpost_arn, ""),
            format!("outpostId={absent_outpost_arn}"),
        ),
    ];
    for (case, path, expected) in &echoes {
        let reply = outposts.call("GET", path, None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            *expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {:?}",
            reply.body
        );
    }

    // An Outpost that existed and was then deleted is gone to this report too.
    let (deleted, _) = an_outpost(&outposts, "deleted", Some("RACK")).await;
    outposts
        .ok("DELETE", &format!("/outposts/{}", segment(&deleted)), None)
        .await;

    let cases: Vec<Refusal> = vec![
        refusal(
            "an Outpost identifier that is not one",
            "GET",
            route("op-not-an-outpost", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a site id where an Outpost identifier belongs",
            "GET",
            route("os-00000000000000000", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost that was deleted",
            "GET",
            route(&deleted, ""),
            404,
            "NotFoundException",
        ),
    ];
    outposts.expect_refusals(&cases).await;
}
