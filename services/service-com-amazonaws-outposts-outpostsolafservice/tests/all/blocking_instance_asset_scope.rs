// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SCOPE `ListBlockingInstancesForCapacityTask` reports within,
//! driven over the wire through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]), starting its tasks through
//! [`crate::capacity_task_lifecycle`]'s own `StartCapacityTask` helper.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: a capacity
//! task configures ONE piece of an Outpost's hardware — the asset it named, or all of
//! that Outpost's compute hardware when it named none — so the instances it would need
//! stopped are the ones running on the hardware in that scope and nothing running
//! elsewhere on the Outpost: stopping those frees no capacity the task could use. The
//! account and exclusion halves of the same rule ([`crate::blocking_instances`] states
//! them) narrow that further, and the capacity weighing a dry run of the identical
//! request reports reads the SAME rule, so the second test below asserts the two agree.
//!
//! Where the recorded hardware and instances come from: an asset is a physical unit
//! Amazon Web Services installs and an instance is launched through Amazon EC2 —
//! `RunInstances` against the Outpost — so no call in THIS API creates either. Each row
//! records its own through the store seams [`crate::capacity_task_lifecycle`] and
//! [`crate::blocking_instances`] own, and then asks over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::AssetInstance;
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CAPACITY_TASK_COMPLETED, CAPACITY_TASK_WAITING_FOR_EVACUATION,
};

use crate::blocking_instances::{instance, instance_id, own_account, OTHER_ACCOUNT};
use crate::capacity_task_dry_run::published_vcpus;
use crate::capacity_task_lifecycle::{
    get_ok, install_capacity, outpost, pool, settle_capacity_task, start_ok, HOST_VCPUS,
};
use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// The `AssetId`s every row's Outpost records compute hardware under, all of the form
/// the modelled `AssetIdInput` declares (`^\d{10}$`): two that instances run on and one
/// that nothing runs on, so a row can scope a task to hardware with no instance of its
/// own while the Outpost is busy elsewhere.
const HOSTS: [&str; 3] = ["1000000001", "1000000002", "1000000003"];

/// The instance type every recorded instance below holds its capacity in, and every
/// requested pool asks for: one type the published catalog carries a vCPU figure for, so
/// the arithmetic of the agreement test is read off that catalog rather than a copy.
const INSTANCE_TYPE: &str = "m5.4xlarge";

/// The route `ListBlockingInstancesForCapacityTask` binds.
fn route(outpost_identifier: &str, capacity_task_id: &str) -> String {
    format!(
        "/outposts/{}/capacity/{}/blockingInstances?MaxResults=1000",
        segment(outpost_identifier),
        segment(capacity_task_id)
    )
}

/// What one row's task excludes, as the three sub-lists the modelled
/// `InstancesToExclude` carries: the positions in the row's recorded instances it names
/// by id, the owning accounts it names, and the owning services it names. A sub-list
/// left empty is one the request never carries.
#[derive(Debug, Default, Clone)]
struct Exclude {
    instances: Vec<usize>,
    account_ids: Vec<String>,
    services: Vec<String>,
}

impl Exclude {
    /// The `InstancesToExclude` member a row's `StartCapacityTask` request carries, or
    /// `None` when the row excludes nothing at all.
    fn member(&self, recorded: &[AssetInstance]) -> Option<Value> {
        let mut member = serde_json::Map::new();
        if !self.instances.is_empty() {
            let ids: Vec<String> = self
                .instances
                .iter()
                .map(|position| recorded[*position].instance_id.clone().unwrap_or_default())
                .collect();
            member.insert("Instances".to_string(), json!(ids));
        }
        if !self.account_ids.is_empty() {
            member.insert("AccountIds".to_string(), json!(self.account_ids));
        }
        if !self.services.is_empty() {
            member.insert("Services".to_string(), json!(self.services));
        }
        match member.is_empty() {
            true => None,
            false => Some(Value::Object(member)),
        }
    }

    /// Whether these exclusions name the instance at `position` through any of the three
    /// sub-lists — alternatives, so an instance any one of them names is excluded.
    fn names(&self, recorded: &[AssetInstance], position: usize) -> bool {
        let instance = &recorded[position];
        let named = |values: &[String], value: Option<&String>| {
            value.is_some_and(|value| values.contains(value))
        };
        self.instances.contains(&position)
            || named(&self.account_ids, instance.account_id.as_ref())
            || named(&self.services, instance.aws_service_name.as_ref())
    }
}

/// An exclusion by instance position, by owning account or by owning service.
fn exclude_instances(positions: &[usize]) -> Exclude {
    Exclude {
        instances: positions.to_vec(),
        ..Exclude::default()
    }
}

/// See [`exclude_instances`].
fn exclude_accounts(accounts: &[&str]) -> Exclude {
    Exclude {
        account_ids: accounts.iter().map(|id| (*id).to_string()).collect(),
        ..Exclude::default()
    }
}

/// See [`exclude_instances`].
fn exclude_services(services: &[&str]) -> Exclude {
    Exclude {
        services: services.iter().map(|name| (*name).to_string()).collect(),
        ..Exclude::default()
    }
}

/// One row: whether the Outpost records any instance at all, the `AssetId` the task
/// names and what it excludes.
struct Scoped {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// Whether the row records the shared set of instances ([`running`]) or none at all.
    records: bool,
    /// The `AssetId` the task names, or `None` for a task that names none.
    asset: Option<&'static str>,
    exclude: Exclude,
}

/// The instances every row that records any records, in the order they are recorded.
///
/// Between them they span what the rule generalises over: instances on each of the two
/// busy hosts, another account's instance the Outpost is shared with, one owned by an
/// Amazon Web Services service, and one whose asset was never recorded at all — which is
/// on the Outpost but on no asset a task can name.
fn running() -> Vec<AssetInstance> {
    vec![
        instance(
            &instance_id(1),
            Some(INSTANCE_TYPE),
            Some(HOSTS[0]),
            Some(own_account()),
            None,
        ),
        instance(
            &instance_id(2),
            Some(INSTANCE_TYPE),
            Some(HOSTS[0]),
            Some(own_account()),
            Some("EC2"),
        ),
        instance(
            &instance_id(3),
            Some(INSTANCE_TYPE),
            Some(HOSTS[1]),
            Some(own_account()),
            None,
        ),
        instance(
            &instance_id(4),
            Some(INSTANCE_TYPE),
            Some(HOSTS[1]),
            Some(OTHER_ACCOUNT),
            None,
        ),
        instance(
            &instance_id(5),
            Some(INSTANCE_TYPE),
            None,
            Some(own_account()),
            None,
        ),
    ]
}

/// The rule, stated once: the `InstanceId`s of the instances a task scoped to `asset`
/// and excluding `exclude` would need stopped — the ones running on the hardware in that
/// scope, belonging to the account that initiated the task and named by none of its
/// exclusions — in the Outpost's own recorded order.
fn blocking(recorded: &[AssetInstance], asset: Option<&str>, exclude: &Exclude) -> Vec<String> {
    recorded
        .iter()
        .enumerate()
        .filter(|(position, instance)| {
            asset.is_none_or(|named| instance.asset_id.as_deref() == Some(named))
                && instance.account_id.as_deref() == Some(own_account())
                && !exclude.names(recorded, *position)
        })
        .map(|(_, instance)| instance.instance_id.clone().unwrap_or_default())
        .collect()
}

/// The `InstanceId`s one answer reports, in the order it reported them.
fn reported_ids(output: &Value, case: &str) -> Vec<String> {
    output
        .get("BlockingInstances")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a BlockingInstances list, got {output}"))
        .iter()
        .map(|item| text(item, "InstanceId"))
        .collect()
}

/// One Outpost whose three compute hosts are installed and whose instances are recorded,
/// as its `OutpostId`.
async fn an_outpost(outposts: &Outposts, name: &str, recorded: &[AssetInstance]) -> String {
    let outpost_id = outpost(outposts, name).await;
    install_capacity(outposts, &outpost_id, &HOSTS, name);
    crate::blocking_instances::record(outposts, &outpost_id, recorded, name);
    outpost_id
}

/// `StartCapacityTask` on the Outpost, scoped to `asset` and excluding what `exclude`
/// names, returning the minted `CapacityTaskId`.
async fn a_task(
    outposts: &Outposts,
    outpost_id: &str,
    recorded: &[AssetInstance],
    asset: Option<&str>,
    exclude: &Exclude,
) -> String {
    let mut request = json!({ "InstancePools": [pool(INSTANCE_TYPE, 1)] });
    if let Some(asset_id) = asset {
        request["AssetId"] = json!(asset_id);
    }
    if let Some(member) = exclude.member(recorded) {
        request["InstancesToExclude"] = member;
    }
    text(
        &start_ok(outposts, outpost_id, &request).await,
        "CapacityTaskId",
    )
}

/// The rule over the whole class: the list names only the instances running on the
/// hardware the task configures.
///
/// The rows differ along everything the rule generalises over: a task that names no
/// asset and ones that name each of the Outpost's hosts, including the host nothing runs
/// on; instances on the named host, on another host and on no recorded host at all;
/// another account's instance inside the scope; exclusions by instance id, by owning
/// account and by owning service inside the scope; and an Outpost with no recorded
/// instance at all.
#[tokio::test]
async fn the_list_names_only_the_instances_running_on_the_hardware_the_task_configures() {
    let outposts = Outposts::new();
    let recorded = running();
    let rows: Vec<Scoped> = vec![
        Scoped {
            case: "a task that names no asset reaches every host of the Outpost",
            records: true,
            asset: None,
            exclude: Exclude::default(),
        },
        Scoped {
            case: "scoped to the first host, where two of the account's instances run",
            records: true,
            asset: Some(HOSTS[0]),
            exclude: Exclude::default(),
        },
        Scoped {
            case: "scoped to the second host, which also holds another account's instance",
            records: true,
            asset: Some(HOSTS[1]),
            exclude: Exclude::default(),
        },
        Scoped {
            case: "scoped to the host nothing runs on",
            records: true,
            asset: Some(HOSTS[2]),
            exclude: Exclude::default(),
        },
        Scoped {
            case: "scoped to the first host, excluding one of its instances by id",
            records: true,
            asset: Some(HOSTS[0]),
            exclude: exclude_instances(&[0]),
        },
        Scoped {
            case: "scoped to the first host, excluding by the service that owns an instance",
            records: true,
            asset: Some(HOSTS[0]),
            exclude: exclude_services(&["EC2"]),
        },
        Scoped {
            case: "scoped to the second host, excluding the initiating account itself",
            records: true,
            asset: Some(HOSTS[1]),
            exclude: exclude_accounts(&[own_account()]),
        },
        Scoped {
            case: "scoped to the first host, excluding an account whose instance is elsewhere",
            records: true,
            asset: Some(HOSTS[0]),
            exclude: exclude_accounts(&[OTHER_ACCOUNT]),
        },
        Scoped {
            case: "a task that names no asset, excluding the instance on no recorded host",
            records: true,
            asset: None,
            exclude: exclude_instances(&[4]),
        },
        Scoped {
            case: "scoped to a host of an Outpost nothing runs on",
            records: false,
            asset: Some(HOSTS[0]),
            exclude: Exclude::default(),
        },
    ];

    // One Outpost per row: only ONE capacity task can be active on an Outpost at a time.
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let instances: Vec<AssetInstance> = match row.records {
            true => recorded.clone(),
            false => Vec::new(),
        };
        let outpost_id = an_outpost(&outposts, &format!("scope-{index}"), &instances).await;
        let task_id = a_task(&outposts, &outpost_id, &instances, row.asset, &row.exclude).await;
        let expected = blocking(&instances, row.asset, &row.exclude);

        let answer = outposts.ok("GET", &route(&outpost_id, &task_id), None).await;
        assert_eq!(
            reported_ids(&answer, case),
            expected,
            "case {case:?}: expected the blocking instances {expected:?}, got {answer}"
        );
    }
}

/// The two sides of the one rule agree about the scope: the instances the list
/// recommends stopping are exactly the ones the capacity weighing counts as capacity the
/// task could free.
///
/// Two Outposts recording the SAME hardware and the same running instances, differing
/// only in which host their task is scoped to: the busy one, where the configuration
/// fits only once those instances stop, and the idle one, where it fits as it is. The
/// weighing is read over the wire twice — as the dry run's verdict and as the status the
/// started task settles in — and the list beside it.
#[tokio::test]
async fn the_blocking_list_and_the_capacity_weighing_agree_about_the_task_scope() {
    let outposts = Outposts::new();
    let vcpus = published_vcpus();
    let held = vcpus
        .get(INSTANCE_TYPE)
        .copied()
        .unwrap_or_else(|| panic!("the published catalog carries a vCPU count for {INSTANCE_TYPE}"));
    let ceiling = i64::from(HOST_VCPUS);
    assert!(
        held > 0 && ceiling % held == 0,
        "the arithmetic asks for a whole host of {INSTANCE_TYPE}: {ceiling} vCPUs in \
         instances of {held}"
    );
    // The whole vCPU ceiling of ONE host, so the configuration fits on a host nothing
    // runs on and fits on the busy host only once its instances have stopped.
    let pools = json!([pool(INSTANCE_TYPE, ceiling / held)]);
    // Two of the initiating account's instances on the FIRST host, and none on the
    // second: the capacity the one task is held up by is capacity the other never lost.
    let recorded = vec![
        instance(
            &instance_id(1),
            Some(INSTANCE_TYPE),
            Some(HOSTS[0]),
            Some(own_account()),
            None,
        ),
        instance(
            &instance_id(2),
            Some(INSTANCE_TYPE),
            Some(HOSTS[0]),
            Some(own_account()),
            None,
        ),
    ];
    let on_the_busy_host: Vec<String> = recorded
        .iter()
        .map(|instance| instance.instance_id.clone().unwrap_or_default())
        .collect();

    // (case, the host the task is scoped to, the status it settles in, the instances it
    // is held up by)
    let cases: [(&str, &str, &str, Vec<String>); 2] = [
        (
            "scoped to the host its own instances run on",
            HOSTS[0],
            CAPACITY_TASK_WAITING_FOR_EVACUATION,
            on_the_busy_host,
        ),
        (
            "scoped to the host nothing runs on",
            HOSTS[1],
            CAPACITY_TASK_COMPLETED,
            Vec::new(),
        ),
    ];

    for (index, (case, asset_id, settled, blocked)) in cases.iter().enumerate() {
        let outpost_id = an_outpost(&outposts, &format!("agree-{index}"), &recorded).await;
        let request = json!({ "InstancePools": pools, "AssetId": asset_id });

        // The weighing, as the dry run of the identical request reports it: the
        // configuration is one the hardware in scope can hold either way.
        let mut dry_run = request.clone();
        dry_run["DryRun"] = json!(true);
        let verdict = start_ok(&outposts, &outpost_id, &dry_run).await;
        assert_eq!(
            verdict.get("CapacityTaskStatus").and_then(Value::as_str),
            Some(CAPACITY_TASK_COMPLETED),
            "case {case:?}: the hardware in scope can hold the configuration, got {verdict}"
        );

        // The same request for real: the task is held up only by instances in its own
        // scope, so what the weighing found it waiting for is what the list names.
        let task_id = text(
            &start_ok(&outposts, &outpost_id, &request).await,
            "CapacityTaskId",
        );
        let answer = outposts.ok("GET", &route(&outpost_id, &task_id), None).await;
        assert_eq!(
            reported_ids(&answer, case),
            *blocked,
            "case {case:?}: expected the blocking instances {blocked:?}, got {answer}"
        );

        settle_capacity_task();
        let task = get_ok(&outposts, &outpost_id, &task_id).await;
        assert_eq!(
            task.get("CapacityTaskStatus").and_then(Value::as_str),
            Some(*settled),
            "case {case:?}: expected the task to settle in {settled}, got {task}"
        );
    }
}
