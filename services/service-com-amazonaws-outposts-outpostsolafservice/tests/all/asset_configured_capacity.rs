// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the per-asset capacity `ListAssets` reports, driven over the
//! wire through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an
//! asset's `ComputeAttributes$InstanceTypeCapacities` is "the instance type capacities
//! configured for this asset. This can be changed through a capacity task", so a
//! compute asset reports the pools of the COMPLETED capacity task that configured that
//! hardware — one entry per instance type the task asked for an instance of, carrying
//! how many — and compute hardware no completed task has configured reports no
//! per-type capacity at all.
//!
//! Where [`crate::asset_scoped_capacity_configuration`] states WHICH hardware a
//! completed task configures and reads the result back through the Outpost-wide
//! `GetOutpostInstanceTypes`, this suite reads the same fact back per ASSET, which is
//! the only place the model exposes it — so the two reads of one configuration are
//! asserted to agree: every type an asset reports capacity of is a type the Outpost
//! reports as configured.
//!
//! The rows differ along everything the rule generalises over: how many pools a task
//! carries and at what `Count` (one, several, several of ONE type, a `Count` that
//! configures nothing), which asset of the Outpost a task names, the form factor and
//! so how much compute hardware the Outpost holds (a rack of six compute assets, a
//! server of one), a task that names no asset at all, a task beside an Outpost-wide
//! one, a second task replacing what the first configured, and an Outpost no capacity
//! task has run on.

use serde_json::{json, Value};

use crate::asset_scoped_capacity_configuration::{an_outpost, catalog, catalog_items, order};
use crate::capacity_task_lifecycle::{get_ok, pool, settle_capacity_task, start_ok};
use crate::site_and_outpost_lifecycle::{deliver_orders, segment, text, Outposts};

/// Which hardware ONE capacity task of a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The Outpost itself: the request names no `AssetId`.
    Outpost,
    /// The compute asset at this position in the Outpost's own asset order, whose
    /// `AssetId` the row reads out of `ListAssets`.
    Asset(usize),
}

/// One capacity task of a row: the hardware it names and the pools it asks for there,
/// each as `(position in the row's usable instance types, Count)`.
struct Task {
    scope: Scope,
    pools: &'static [(usize, i64)],
}

/// One row: an Outpost whose ordered hardware is installed, and the capacity tasks run
/// on it.
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
}

impl Row {
    /// How many of the Outpost's compute assets this row needs installed: one past the
    /// last position any of its tasks names.
    fn named(&self) -> usize {
        self.tasks
            .iter()
            .filter_map(|task| match task.scope {
                Scope::Asset(position) => Some(position + 1),
                Scope::Outpost => None,
            })
            .max()
            .unwrap_or_default()
    }
}

/// `ListAssets` on one Outpost, as the whole list in the Outpost's own installation
/// order.
async fn assets(outposts: &Outposts, outpost_id: &str) -> Vec<Value> {
    outposts
        .ok(
            "GET",
            &format!("/outposts/{}/assets?MaxResults=1000", segment(outpost_id)),
            None,
        )
        .await["Assets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The COMPUTE units of one such list, in that same order — the hardware a capacity
/// task configures.
fn compute_assets(listed: &[Value]) -> Vec<&Value> {
    listed
        .iter()
        .filter(|asset| text(asset, "AssetType") == "COMPUTE")
        .collect()
}

/// The `(InstanceType, Count)` of every entry of one asset's
/// `ComputeAttributes$InstanceTypeCapacities`, in the order it reported them, or
/// `None` when the asset reports no per-type capacity at all.
fn capacities(asset: &Value) -> Option<Vec<(String, Option<i64>)>> {
    asset
        .get("ComputeAttributes")?
        .get("InstanceTypeCapacities")?
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        text(entry, "InstanceType"),
                        entry.get("Count").and_then(Value::as_i64),
                    )
                })
                .collect()
        })
}

/// The `ComputeAttributes$MaxVcpus` one asset reports — "the maximum number of vCPUs
/// possible for the specified asset", which is what a configuration asked of that
/// asset is weighed against.
fn ceiling(asset: &Value) -> i64 {
    asset
        .get("ComputeAttributes")
        .and_then(|compute| compute.get("MaxVcpus"))
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

/// The instance types this service publishes as offered on `form_factor` hardware,
/// each with its published vCPU count, smallest first.
///
/// Source: `ListOrderableInstanceTypes`' own `FormFactorConfigs` and `VCPUs`, read
/// over the wire rather than spelled out — so a row configures a type the hardware it
/// names really runs (which is what its `ComputeAttributes` record) and asks for a
/// number of instances that hardware can hold.
fn usable(catalog: &[Value], form_factor: &str) -> Vec<(String, i64)> {
    let mut offered: Vec<(String, i64)> = catalog
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
        .filter_map(|entry| {
            Some((
                text(entry, "InstanceType"),
                entry.get("VCPUs").and_then(Value::as_i64)?,
            ))
        })
        .collect();
    offered.sort_by(|left, right| (left.1, &left.0).cmp(&(right.1, &right.0)));
    offered
}

/// The `StartCapacityTask` request one task of a row sends.
fn request(task: &Task, assets: &[String], types: &[(String, i64)]) -> Value {
    let pools: Vec<Value> = task
        .pools
        .iter()
        .map(|(choice, count)| pool(&types[*choice].0, *count))
        .collect();
    let mut request = json!({ "InstancePools": pools });
    if let Scope::Asset(position) = task.scope {
        request["AssetId"] = json!(assets[position]);
    }
    request
}

/// How many vCPUs one task of a row asks for: each pool's `Count` times the vCPUs the
/// catalog publishes for its instance type, which is the figure the hardware it is
/// asked of has to carry for the task to complete at all.
fn asked(task: &Task, types: &[(String, i64)]) -> i64 {
    task.pools
        .iter()
        .map(|(choice, count)| count * types[*choice].1)
        .sum()
}

/// What the rule requires one compute asset to report: the pools of the task that
/// configured THAT hardware — the last one naming it, or, for the single compute asset
/// of an Outpost no task of its own has named, the last one that named the Outpost
/// itself — with pools of one type counted together and a pool that configures no
/// instance leaving no entry, so an asset left configured with nothing reports no
/// per-type capacity.
fn expectation(
    row: &Row,
    position: usize,
    sole: bool,
    types: &[(String, i64)],
) -> Option<Vec<(String, Option<i64>)>> {
    let last = |scope: Scope| row.tasks.iter().rev().find(|task| task.scope == scope);
    let standing = last(Scope::Asset(position))
        .or_else(|| sole.then(|| last(Scope::Outpost)).flatten())?;
    let mut want: Vec<(String, Option<i64>)> = Vec::new();
    for (choice, count) in standing.pools {
        if *count <= 0 {
            continue;
        }
        let instance_type = types[*choice].0.clone();
        match want.iter_mut().find(|(held, _)| *held == instance_type) {
            Some((_, held)) => *held = Some(held.unwrap_or_default() + count),
            None => want.push((instance_type, Some(*count))),
        }
    }
    (!want.is_empty()).then_some(want)
}

/// The instance types `GetOutpostInstanceTypes` reports as configured on one Outpost —
/// the Outpost-wide read of the same stored fact.
async fn configured_types(outposts: &Outposts, outpost_id: &str) -> Vec<String> {
    outposts
        .ok(
            "GET",
            &format!(
                "/outposts/{}/instanceTypes?MaxResults=1000",
                segment(outpost_id)
            ),
            None,
        )
        .await["InstanceTypes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| text(entry, "InstanceType"))
        .collect()
}

/// The rule: a compute asset reports the per-type capacity the completed capacity task
/// that configured that hardware left on it, and none where no completed task has
/// configured it.
#[tokio::test]
async fn an_asset_reports_the_capacity_the_completed_task_configured_on_it() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let racks = catalog_items(&outposts, "RACK").await;
    let servers = catalog_items(&outposts, "SERVER").await;
    assert!(
        !racks.is_empty() && !servers.is_empty(),
        "the rows order published hardware of each class, got {racks:?} and {servers:?}"
    );
    let rack_types = usable(&catalog, "RACK");
    let server_types = usable(&catalog, "SERVER");
    assert!(
        rack_types.len() >= 3 && server_types.len() >= 2,
        "the rows configure three published rack types and two server types, got \
         {rack_types:?} and {server_types:?}"
    );
    // The types the hardware of one row's form factor runs.
    let types_of = |hardware_type: &str| match hardware_type {
        "SERVER" => server_types.clone(),
        _ => rack_types.clone(),
    };

    let rows: &[Row] = &[
        Row {
            case: "one asset of a rack configured by the task that named it",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(0),
                pools: &[(0, 1)],
            }],
        },
        Row {
            case: "two assets of a rack, each configured by its own task",
            hardware_type: "RACK",
            tasks: &[
                Task {
                    scope: Scope::Asset(1),
                    pools: &[(0, 2)],
                },
                Task {
                    scope: Scope::Asset(3),
                    pools: &[(1, 3)],
                },
            ],
        },
        Row {
            case: "one asset configured with several instance types at once",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(2),
                pools: &[(0, 1), (1, 4), (2, 2)],
            }],
        },
        Row {
            case: "two pools of one instance type on one asset",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(0),
                pools: &[(1, 2), (1, 3)],
            }],
        },
        Row {
            case: "a second task on one asset replaces what the first configured",
            hardware_type: "RACK",
            tasks: &[
                Task {
                    scope: Scope::Asset(5),
                    pools: &[(0, 5)],
                },
                Task {
                    scope: Scope::Asset(5),
                    pools: &[(2, 1)],
                },
            ],
        },
        Row {
            case: "a task whose pools configure no instance leaves the asset reporting none",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Asset(4),
                pools: &[(0, 0)],
            }],
        },
        Row {
            case: "a later task of Count zero clears what an earlier one configured",
            hardware_type: "RACK",
            tasks: &[
                Task {
                    scope: Scope::Asset(2),
                    pools: &[(0, 2)],
                },
                Task {
                    scope: Scope::Asset(2),
                    pools: &[(0, 0)],
                },
            ],
        },
        Row {
            case: "the one compute asset of a server, configured by the task that named it",
            hardware_type: "SERVER",
            tasks: &[Task {
                scope: Scope::Asset(0),
                pools: &[(0, 3)],
            }],
        },
        Row {
            case: "the one compute asset of a server, configured by a task that named no asset",
            hardware_type: "SERVER",
            tasks: &[Task {
                scope: Scope::Outpost,
                pools: &[(0, 2)],
            }],
        },
        Row {
            case: "a server asset keeps what the task that named it configured, past a \
                   later Outpost-wide one",
            hardware_type: "SERVER",
            tasks: &[
                Task {
                    scope: Scope::Asset(0),
                    pools: &[(0, 2)],
                },
                Task {
                    scope: Scope::Outpost,
                    pools: &[(1, 1)],
                },
            ],
        },
        Row {
            case: "a rack-wide task, whose split between six compute assets is held of none",
            hardware_type: "RACK",
            tasks: &[Task {
                scope: Scope::Outpost,
                pools: &[(0, 2)],
            }],
        },
        Row {
            case: "an Outpost no capacity task has run on",
            hardware_type: "RACK",
            tasks: &[],
        },
    ];

    // Every row's Outpost orders the hardware of its own class, and every install those
    // orders owe is brought due at once — so the whole table costs ONE delivery rather
    // than one per row.
    let mut placed: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let outpost_id =
            an_outpost(&outposts, &format!("configured-{index}"), row.hardware_type).await;
        let item = match row.hardware_type {
            "SERVER" => &servers[0],
            _ => &racks[0],
        };
        order(&outposts, &outpost_id, &text(item, "CatalogItemId")).await;
        placed.push(outpost_id);
    }
    deliver_orders(&outposts);

    // Before any capacity task: installed hardware nothing has configured reports no
    // per-type capacity, which is what each row then configures a piece of.
    let mut hardware: Vec<Vec<String>> = Vec::new();
    for (row, outpost_id) in rows.iter().zip(&placed) {
        let case = row.case;
        let listed = assets(&outposts, outpost_id).await;
        let compute = compute_assets(&listed);
        assert!(
            compute.len() >= row.named().max(1),
            "case {case:?}: the row names {} of the Outpost's compute assets, got {compute:?}",
            row.named()
        );
        for asset in &compute {
            assert_eq!(
                capacities(asset),
                None,
                "case {case:?}: compute hardware no capacity task has configured must report \
                 no per-type capacity, got {asset:?}"
            );
        }
        // A row may only ask of hardware what that hardware can hold: a configuration
        // above it would FAIL instead of configuring anything, which is
        // [`crate::capacity_task_outcome`]'s rule and not this suite's.
        let carries = compute.iter().map(|asset| ceiling(asset)).min().unwrap_or_default();
        let types = types_of(row.hardware_type);
        for task in row.tasks {
            let wants = asked(task, &types);
            assert!(
                wants <= carries,
                "case {case:?}: the row asks for {wants} vCPUs of hardware carrying {carries}"
            );
        }
        hardware.push(compute.iter().map(|asset| text(asset, "AssetId")).collect());
    }

    // One round per task position: a second task on an Outpost starts only once the
    // first has completed, and every row's task of one round runs together.
    let rounds = rows.iter().map(|row| row.tasks.len()).max().unwrap_or_default();
    for round in 0..rounds {
        let mut started: Vec<(usize, String)> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            if let Some(task) = row.tasks.get(round) {
                let answered = start_ok(
                    &outposts,
                    &placed[index],
                    &request(task, &hardware[index], &types_of(row.hardware_type)),
                )
                .await;
                started.push((index, text(&answered, "CapacityTaskId")));
            }
        }
        settle_capacity_task();
        // Only a COMPLETED task configures anything, so each row's task is required to
        // have run before what it configured is read back.
        for (index, capacity_task_id) in started {
            let case = rows[index].case;
            let task = get_ok(&outposts, &placed[index], &capacity_task_id).await;
            assert_eq!(
                text(&task, "CapacityTaskStatus"),
                "COMPLETED",
                "case {case:?}: the row's task must complete to configure anything, got {task:?}"
            );
        }
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let outpost_id = &placed[index];
        let types = types_of(row.hardware_type);
        let listed = assets(&outposts, outpost_id).await;
        let compute = compute_assets(&listed);
        let sole = compute.len() == 1;
        for (position, asset) in compute.iter().enumerate() {
            let want = expectation(row, position, sole, &types);
            assert_eq!(
                capacities(asset),
                want,
                "case {case:?}: asset at position {position} must report the capacity the task \
                 that configured it left ({want:?}), got {asset:?}"
            );
        }
        // The two reads of one configuration agree: a type an asset reports capacity of
        // is a type the Outpost reports as configured.
        let configured = configured_types(&outposts, outpost_id).await;
        for asset in &compute {
            for (instance_type, _) in capacities(asset).unwrap_or_default() {
                assert!(
                    configured.contains(&instance_type),
                    "case {case:?}: {instance_type} is configured on an asset, so the Outpost \
                     must report it as configured, got {configured:?}"
                );
            }
        }
        // The list is a REPORT: reading it again answers the same assets.
        assert_eq!(
            assets(&outposts, outpost_id).await,
            listed,
            "case {case:?}: a second list must answer the same assets"
        );
    }
}
