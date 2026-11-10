// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SCOPE a completed capacity task configures, driven over
//! the wire through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: capacity
//! is configured on a PIECE of hardware, so a completed capacity task configures the
//! hardware it named — one asset of the Outpost, or the Outpost itself when it named
//! none — and leaves the configuration of every other piece of that Outpost's compute
//! hardware standing, which for hardware no task has named is what the configurations
//! its uncancelled orders subscribe to arrived carrying. `GetOutpostInstanceTypes`
//! therefore reports what stands across all of that hardware together: the Outpost's
//! own configuration first, then each reconfigured asset's in the assets' own order.
//!
//! Where [`crate::configured_instance_types`] states what the report IS — the two
//! records that configure an Outpost, the `VCPUs` each entry carries, its paging and
//! its refusals — this suite states WHICH hardware each of those records configures,
//! on Outposts that hold both: order-derived configuration on installed hardware, and
//! capacity tasks run against pieces of it.
//!
//! The rows differ along everything the rule generalises over: how many of the
//! Outpost's compute assets a task names (one, two, every one of them), the form
//! factor and so how much compute hardware the Outpost holds (a rack of six compute
//! assets, a server of one), a task that names no asset at all, a task beside an
//! Outpost-wide one, pools at a `Count` that configures nothing, and a task
//! configuring a type the ordered hardware already carries.

use serde_json::{json, Value};

use crate::capacity_task_lifecycle::{pool, settle_capacity_task, start_ok};
use crate::site_and_outpost_lifecycle::{deliver_orders, orderable_site, segment, text, Outposts};

/// Which hardware ONE capacity task of a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The Outpost itself: the request names no `AssetId`.
    Outpost,
    /// The compute asset at this position in the Outpost's own asset order, whose
    /// `AssetId` the row reads out of `ListAssets`.
    Asset(usize),
}

/// Which instance type one pool of a row configures — chosen from what this service
/// publishes rather than spelled out, so a row names hardware facts the service
/// really holds.
#[derive(Debug, Clone, Copy)]
enum Configured {
    /// A published instance type NO published configuration carries capacity of, so it
    /// stands on an Outpost only where a capacity task put it.
    Spare(usize),
    /// A type the Outpost's ordered hardware arrived carrying, at this position in the
    /// order-derived configuration.
    Ordered(usize),
}

/// One capacity task of a row: the hardware it names and the pools it configures
/// there, each as `(instance type, Count)`.
///
/// The pools of an ASSET-scoped task are weighed against that one asset's own
/// capacity, and an Outpost-wide one's against all of its compute hardware, so a row
/// asks for a configuration the hardware it names can hold — a task above it would
/// FAIL instead of configuring anything ([`crate::capacity_task_outcome`]).
struct Task {
    scope: Scope,
    pools: &'static [(Configured, i64)],
}

/// One row: an Outpost whose ordered hardware is installed, the capacity tasks run on
/// it, and whether the configuration that hardware arrived carrying still stands.
struct Row {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The `SupportedHardwareType` the Outpost is created with, which is the hardware
    /// this service installs on it.
    hardware_type: &'static str,
    /// The tasks to run, in order. A row with two runs the second only once the first
    /// has completed — one active capacity task per Outpost, as
    /// [`crate::capacity_task_lifecycle`] states.
    tasks: &'static [Task],
    /// Whether the Outpost still holds compute hardware no task of its own has
    /// reconfigured, so the configuration that hardware arrived carrying goes on
    /// standing.
    keeps_ordered: bool,
}

/// The route `GetOutpostInstanceTypes` binds.
fn route(identifier: &str) -> String {
    format!("/outposts/{}/instanceTypes?MaxResults=1000", segment(identifier))
}

/// `GetOutpostInstanceTypes`, requiring HTTP 200.
async fn configured(outposts: &Outposts, identifier: &str) -> Value {
    outposts.ok("GET", &route(identifier), None).await
}

/// The `(InstanceType, VCPUs)` of every entry of one answer, in the order it reported
/// them.
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
/// The catalog `ListOrderableInstanceTypes` publishes is the narrower one entry per
/// family capacity is ORDERED in, while a configuration names any size the hardware
/// runs (see [`crate::instance_type_catalogs`]), so the wider table is the one a size
/// is sized from.
pub(crate) fn catalog() -> Vec<Value> {
    crate::instance_type_catalogs::specifications()
}

/// Every configuration `ListCatalogItems` publishes of one class, in the order it
/// publishes them.
pub(crate) async fn catalog_items(outposts: &Outposts, item_class: &str) -> Vec<Value> {
    outposts
        .ok(
            "GET",
            &format!("/catalog/items?ItemClassFilter={item_class}&MaxResults=1000"),
            None,
        )
        .await["CatalogItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// What a report of the types `want` names must answer: those types, in that order,
/// each with the published catalog's own count.
fn expected(catalog: &[Value], want: &[String]) -> Vec<(String, Option<i64>)> {
    want.iter()
        .map(|instance_type| {
            (
                instance_type.clone(),
                catalog
                    .iter()
                    .find(|entry| text(entry, "InstanceType") == *instance_type)
                    .and_then(|entry| entry.get("VCPUs").and_then(Value::as_i64)),
            )
        })
        .collect()
}

/// The largest size of each instance family one published configuration carries EC2
/// capacity of, as the INSTANCE TYPE it names — `Family` carries the family and
/// `MaxSize` the size alone, so the type is the join of the two — in the order the
/// configuration publishes them: the configuration the hardware it describes arrives
/// carrying.
fn max_sizes(item: &Value) -> Vec<String> {
    item.get("EC2Capacities")
        .and_then(Value::as_array)
        .map(|capacities| {
            let mut sizes: Vec<String> = Vec::new();
            for capacity in capacities {
                let instance_type =
                    format!("{}.{}", text(capacity, "Family"), text(capacity, "MaxSize"));
                push_once(&mut sizes, &instance_type);
            }
            sizes
        })
        .unwrap_or_default()
}

/// Append `value` unless it is already there: the report answers each configured type
/// once, in the position it was first configured in.
fn push_once(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|held| held == value) {
        values.push(value.to_string());
    }
}

/// One site and one Outpost on it carrying `hardware_type`, as that Outpost's
/// `OutpostId`. An Outpost belongs to a site, so the site comes first.
pub(crate) async fn an_outpost(outposts: &Outposts, name: &str, hardware_type: &str) -> String {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let outpost = outposts
        .create_outpost(&json!({
            "Name": name,
            "SiteId": text(&site, "SiteId"),
            "SupportedHardwareType": hardware_type,
        }))
        .await;
    text(&outpost, "OutpostId")
}

/// Order one unit of `catalog_item_id` for an Outpost, exactly as a caller places an
/// order: the hardware whose configuration then arrives on it.
pub(crate) async fn order(outposts: &Outposts, outpost_id: &str, catalog_item_id: &str) {
    outposts
        .ok(
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": catalog_item_id, "Quantity": 1 }],
            })),
        )
        .await;
}

/// The `AssetId` of every COMPUTE unit installed on one Outpost, in the order
/// `ListAssets` reports them — the hardware a capacity task can be scoped to.
async fn compute_assets(outposts: &Outposts, outpost_id: &str) -> Vec<String> {
    let answer = outposts
        .ok(
            "GET",
            &format!("/outposts/{}/assets?MaxResults=1000", segment(outpost_id)),
            None,
        )
        .await;
    answer["Assets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|asset| text(asset, "AssetType") == "COMPUTE")
        .map(|asset| text(asset, "AssetId"))
        .collect()
}

/// The instance type one pool of a row configures.
fn instance_type(choice: Configured, ordered: &[String], spare: &[String]) -> String {
    match choice {
        Configured::Spare(position) => spare[position].clone(),
        Configured::Ordered(position) => ordered[position].clone(),
    }
}

/// The `StartCapacityTask` request one task of a row sends.
fn request(task: &Task, assets: &[String], ordered: &[String], spare: &[String]) -> Value {
    let pools: Vec<Value> = task
        .pools
        .iter()
        .map(|(choice, count)| pool(&instance_type(*choice, ordered, spare), *count))
        .collect();
    let mut request = json!({ "InstancePools": pools });
    if let Scope::Asset(position) = task.scope {
        request["AssetId"] = json!(assets[position]);
    }
    request
}

/// The types `task` leaves configured on the hardware it named: those of its pools
/// that configure an instance at all, since a pool of `Count` zero configures none of
/// its type and so names none.
fn configures(want: &mut Vec<String>, task: &Task, ordered: &[String], spare: &[String]) {
    for (choice, count) in task.pools {
        if *count > 0 {
            push_once(want, &instance_type(*choice, ordered, spare));
        }
    }
}

/// What the rule requires the report to answer for one row: the configuration that
/// stands on the Outpost itself — the last task that named no asset, or, while the
/// Outpost still holds compute hardware no task has reconfigured, what that hardware
/// arrived carrying — and then the configuration standing on each asset a task did
/// name, in those assets' own order.
fn expectation(row: &Row, ordered: &[String], spare: &[String]) -> Vec<String> {
    let mut want: Vec<String> = Vec::new();
    let standing_on = |scope: Scope| row.tasks.iter().rev().find(|task| task.scope == scope);
    match standing_on(Scope::Outpost) {
        Some(task) => configures(&mut want, task, ordered, spare),
        None => {
            if row.keeps_ordered {
                for instance_type in ordered {
                    push_once(&mut want, instance_type);
                }
            }
        }
    }
    let mut positions: Vec<usize> = row
        .tasks
        .iter()
        .filter_map(|task| match task.scope {
            Scope::Asset(position) => Some(position),
            Scope::Outpost => None,
        })
        .collect();
    positions.sort_unstable();
    positions.dedup();
    for position in positions {
        if let Some(task) = standing_on(Scope::Asset(position)) {
            configures(&mut want, task, ordered, spare);
        }
    }
    want
}

/// The published instance types one FORM FACTOR's hardware runs that NO published
/// configuration carries capacity of — so a task configuring one of them configures
/// something the ordered hardware did not arrive carrying, on hardware that runs it.
///
/// Which types a piece of hardware runs is what its `ComputeAttributes` record, and
/// this service installs on an Outpost the families and vCPU ceiling its catalog
/// publishes as orderable on that form factor — so a type offered on another form
/// factor is one this hardware does not run, and a task asking for it would FAIL
/// rather than configure anything ([`crate::capacity_task_outcome`]). The form factor
/// each type is offered on is the catalog's own `FormFactorConfigs`, read over the
/// wire like every other expectation here.
fn spares(catalog: &[Value], ordered_anywhere: &[String], form_factor: &str) -> Vec<String> {
    catalog
        .iter()
        .filter(|entry| {
            entry
                .get("FormFactorConfigs")
                .and_then(Value::as_array)
                .is_some_and(|configs| {
                    configs
                        .iter()
                        .any(|config| text(config, "FormFactor") == form_factor)
                })
        })
        .map(|entry| text(entry, "InstanceType"))
        .filter(|instance_type| !ordered_anywhere.contains(instance_type))
        .collect()
}

/// The rule: a completed capacity task reconfigures the hardware it named and nothing
/// else, so the configuration of every other piece of the Outpost's compute hardware
/// goes on standing.
#[tokio::test]
async fn a_capacity_task_configures_only_the_hardware_it_named() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let racks = catalog_items(&outposts, "RACK").await;
    let servers = catalog_items(&outposts, "SERVER").await;
    assert!(
        !racks.is_empty() && !servers.is_empty(),
        "the rows order published hardware of each class, got {racks:?} and {servers:?}"
    );
    // Instance types NO published configuration carries capacity of, so a task that
    // configures one of them configures something the ordered hardware did not arrive
    // carrying — taken per form factor, because a row's task may only ask for types the
    // hardware it runs on runs.
    let ordered_anywhere: Vec<String> = racks
        .iter()
        .chain(servers.iter())
        .flat_map(max_sizes)
        .collect();
    let rack_spare = spares(&catalog, &ordered_anywhere, "RACK");
    let server_spare = spares(&catalog, &ordered_anywhere, "SERVER");
    assert!(
        rack_spare.len() >= 2 && !server_spare.is_empty(),
        "the rows need two published rack types and one server type that no published \
         configuration carries capacity of, got {rack_spare:?} and {server_spare:?}"
    );
    // The spare types of the form factor one row's Outpost holds.
    let spare_of = |hardware_type: &str| match hardware_type {
        "SERVER" => server_spare.clone(),
        _ => rack_spare.clone(),
    };

    let rows: &[Row] = &[
        Row {
            case: "one asset of a rack reconfigured, the other five as they arrived",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(0),
                pools: &[(Configured::Spare(0), 1)],
            }],
            keeps_ordered: true,
        },
        Row {
            case: "two assets of a rack reconfigured, each by its own task",
            hardware_type: "RACK",
            tasks: &[
                Task {
                    scope: Scope::Asset(1),
                    pools: &[(Configured::Spare(0), 1)],
                },
                Task {
                    scope: Scope::Asset(3),
                    pools: &[(Configured::Spare(1), 1)],
                },
            ],
            keeps_ordered: true,
        },
        Row {
            case: "the one compute asset of a server reconfigured",
            hardware_type: "SERVER",
            tasks: &[Task {
                scope: Scope::Asset(0),
                pools: &[(Configured::Spare(0), 1)],
            }],
            keeps_ordered: false,
        },
        Row {
            case: "a task that names no asset reconfigures the whole Outpost",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Outpost,
                pools: &[(Configured::Spare(1), 3)],
            }],
            keeps_ordered: false,
        },
        Row {
            case: "an asset-scoped task of pools that configure no instance",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(2),
                pools: &[(Configured::Spare(0), 0)],
            }],
            keeps_ordered: true,
        },
        Row {
            case: "an asset configured with a type the ordered hardware already carries",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(4),
                pools: &[(Configured::Ordered(0), 1)],
            }],
            keeps_ordered: true,
        },
        Row {
            case: "an Outpost-wide task beside one scoped to an asset",
            hardware_type: "RACK",
            tasks: &[
                Task {
                    scope: Scope::Asset(5),
                    pools: &[(Configured::Spare(0), 1)],
                },
                Task {
                    scope: Scope::Outpost,
                    pools: &[(Configured::Spare(1), 1)],
                },
            ],
            keeps_ordered: false,
        },
    ];

    // Every row's Outpost orders the hardware of its own class, and every install those
    // orders owe is brought due at once — so the whole table costs ONE delivery rather
    // than one per row.
    let mut placed: Vec<(String, Vec<String>)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let outpost_id = an_outpost(&outposts, &format!("scoped-{index}"), row.hardware_type).await;
        let item = match row.hardware_type {
            "SERVER" => &servers[0],
            _ => &racks[0],
        };
        let ordered = max_sizes(item);
        assert!(
            !ordered.is_empty(),
            "case {case:?}: the ordered configuration must carry EC2 capacity, got {item:?}"
        );
        order(&outposts, &outpost_id, &text(item, "CatalogItemId")).await;
        placed.push((outpost_id, ordered));
    }
    deliver_orders(&outposts);

    // Before any capacity task: the installed hardware stands configured as it
    // arrived, which is what each row then reconfigures a piece of.
    let mut hardware: Vec<Vec<String>> = Vec::new();
    for (row, (outpost_id, ordered)) in rows.iter().zip(&placed) {
        let case = row.case;
        let answer = configured(&outposts, outpost_id).await;
        assert_eq!(
            reported(&answer),
            expected(&catalog, ordered),
            "case {case:?}: installed hardware stands configured as it arrived, got {answer:?}"
        );
        let assets = compute_assets(&outposts, outpost_id).await;
        let named = row
            .tasks
            .iter()
            .filter_map(|task| match task.scope {
                Scope::Asset(position) => Some(position + 1),
                Scope::Outpost => None,
            })
            .max()
            .unwrap_or_default();
        assert!(
            assets.len() >= named,
            "case {case:?}: the row names {named} of the Outpost's compute assets, got {assets:?}"
        );
        hardware.push(assets);
    }

    // One round per task position: a second task on an Outpost starts only once the
    // first has completed, and every row's task of one round runs together.
    let rounds = rows.iter().map(|row| row.tasks.len()).max().unwrap_or_default();
    for round in 0..rounds {
        for ((row, (outpost_id, ordered)), assets) in rows.iter().zip(&placed).zip(&hardware) {
            if let Some(task) = row.tasks.get(round) {
                start_ok(
                    &outposts,
                    outpost_id,
                    &request(task, assets, ordered, &spare_of(row.hardware_type)),
                )
                .await;
            }
        }
        settle_capacity_task();
    }

    for (row, (outpost_id, ordered)) in rows.iter().zip(&placed) {
        let case = row.case;
        let want = expectation(row, ordered, &spare_of(row.hardware_type));
        let answer = configured(&outposts, outpost_id).await;
        assert_eq!(
            reported(&answer),
            expected(&catalog, &want),
            "case {case:?}: expected the configuration standing on the Outpost and on each \
             asset a task named ({want:?}), got {answer:?}"
        );
        // The report moved on the completed tasks, not on being read: a second read
        // answers the same configuration.
        assert_eq!(
            reported(&configured(&outposts, outpost_id).await),
            expected(&catalog, &want),
            "case {case:?}: a second read must answer the same configuration"
        );
    }
}
