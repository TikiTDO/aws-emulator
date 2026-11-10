// SPDX-License-Identifier: CC0-1.0

//! What a REAL (non-dry-run) capacity task DOES, driven over the wire through the same
//! front door as every other suite (the harness in
//! [`crate::capacity_task_lifecycle`], which owns what a started task persists and
//! reports).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A started capacity task's outcome is the configuration it asks for weighed
//!    against the instance capacity of the hardware it configures — the one asset the
//!    request names, or all of the Outpost's compute hardware when it names none. A
//!    configuration that fits beside everything already running on that hardware runs
//!    to `COMPLETED`; one the hardware cannot hold even once the instances the task may
//!    stop are stopped ends `FAILED` with the modelled
//!    `UNSUPPORTED_CAPACITY_CONFIGURATION`. That is the SAME weighing a dry run of the
//!    identical request reports ([`crate::capacity_task_dry_run`]), so the two never
//!    disagree about the configuration: every row below asks both.
//! 2. Where the configuration can run only on capacity the task's blocking instances
//!    hold — the running instances `ListBlockingInstancesForCapacityTask` recommends
//!    stopping — the modelled `TaskActionOnBlockingInstances` decides: `FAIL_TASK` is
//!    "the capacity task fails", so the task ends `FAILED` with the modelled
//!    `BLOCKING_INSTANCES_NOT_EVACUATED`, while `WAIT_FOR_EVACUATION` — and a request
//!    that named no action, which asked for no failure — leaves it
//!    `WAITING_FOR_EVACUATION`.
//! 3. Whatever the outcome, the start itself answers the non-terminal `REQUESTED`
//!    status with no failure: the outcome falls due on the clock.
//! 4. Only a COMPLETED task configures anything, so a task that failed leaves the
//!    Outpost's configured capacity exactly as it was (`GetOutpostInstanceTypes`).
//! 5. A task waiting for evacuation is work still in hand: it is the Outpost's active
//!    task, so another is refused 409, the clock never moves it on (what it waits for
//!    is instances being stopped, which no call of this API does), and cancelling it is
//!    what ends it.
//!
//! Where the hardware and the running instances come from: an asset is a physical unit
//! Amazon Web Services installs and an instance is launched through Amazon EC2, so no
//! call in this API creates either — each row records its own through the store seams
//! [`crate::outpost_assets`] and [`crate::blocking_instances`] own, and then asks over
//! the wire. Every vCPU figure a row expects is read from the service's own published
//! catalog over the wire (`ListOrderableInstanceTypes`), never from a copy of it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{AssetInfo, AssetInstance};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    blocking_instances_not_evacuated, capacity_above_available, instance_type_not_run,
    CAPACITY_TASK_CANCELLED, CAPACITY_TASK_COMPLETED, CAPACITY_TASK_FAILED,
    CAPACITY_TASK_FAILURE_BLOCKING_INSTANCES_NOT_EVACUATED,
    CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION, CAPACITY_TASK_REQUESTED,
    CAPACITY_TASK_WAITING_FOR_EVACUATION, TASK_ACTION_FAIL_TASK, TASK_ACTION_WAIT_FOR_EVACUATION,
};

use crate::blocking_instances::{instance, instance_id, own_account, OTHER_ACCOUNT};
use crate::capacity_task_dry_run::{host, published_vcpus};
use crate::capacity_task_lifecycle::{
    capacity_route, get_ok, outpost, pool, settle_capacity_task, settle_capacity_task_steps, start,
    start_ok, task_route,
};
use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// An instance type of the modelled `InstanceTypeName` form
/// (`^[a-z0-9\-]+\.[a-z0-9\-]+$`) that this service's catalog publishes nothing for, so
/// no vCPU figure exists to weigh a pool of it by.
const UNPUBLISHED_INSTANCE_TYPE: &str = "zz9.mega";

/// The wire error code a second task on one Outpost is refused with, from
/// `smithy_query(query="operation",operation="StartCapacityTask")`: `ConflictException`,
/// HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// The two `AssetId`s a row's compute hosts are recorded under, of the form the modelled
/// `AssetIdInput` declares (`^\d{10}$`).
const HOSTS: [&str; 2] = ["1000000001", "1000000002"];

/// One compute host a row records on its Outpost: its `AssetId`, the instance families
/// recorded on it (an empty list is a host that recorded none, which narrows nothing
/// about what it runs) and its `ComputeAttributes$MaxVcpus` ceiling.
type Host = (&'static str, &'static [&'static str], i32);

/// One instance a row records as RUNNING on its Outpost's first host: the instance type
/// it holds capacity in, and whether the account that initiates every task here owns it
/// — an instance of another account is on the Outpost but not this caller's to stop, so
/// its capacity is not capacity a task can free.
type Running = (&'static str, bool);

/// What the hardware a row records can hold, as the rule weighs it — used only to word
/// what a FAILED task's reason must say.
struct Figures {
    /// The vCPUs the row's pools ask for.
    requested: i64,
    /// The vCPUs free on the hardware in scope with every recorded instance still
    /// running.
    free: i64,
    /// The vCPUs free once the instances the task MAY stop have stopped: what the
    /// configuration is weighed as being above or below.
    available: i64,
}

/// Why a configuration is one the hardware cannot be configured with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unsupported {
    /// The hardware in scope does not run this instance type at all.
    NotRun(&'static str),
    /// It is above the capacity available to it.
    Above,
}

/// What the rule requires one row's task to end up as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The configuration fits: the task runs to `COMPLETED`.
    Completes,
    /// `FAILED` with the modelled `UNSUPPORTED_CAPACITY_CONFIGURATION`.
    Unsupported(Unsupported),
    /// `FAILED` with the modelled `BLOCKING_INSTANCES_NOT_EVACUATED`: the hardware could
    /// hold the configuration, but only once instances left running had stopped, and
    /// `FAIL_TASK` was asked for.
    FailsOnBlocking,
    /// `WAITING_FOR_EVACUATION`: the same configuration, with the wait asked for or
    /// nothing asked for.
    Waits,
}

/// One row: the hardware and running instances of an Outpost, the request a task is
/// started with on it, and the outcome the rule requires.
struct Weighed {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The compute hosts recorded on the Outpost, in order.
    hosts: &'static [Host],
    /// The instances recorded as running on the FIRST of those hosts.
    running: &'static [Running],
    /// The `AssetId` the request names, which scopes the configuration to that one
    /// host, or `None` for a request that names none.
    asset: Option<&'static str>,
    /// The pools the request asks for, as `(instance type, Count)`.
    pools: &'static [(&'static str, i64)],
    /// The positions in `running` the request's `InstancesToExclude` names by instance
    /// id: instances that must not be stopped, so their capacity cannot be freed.
    exclude: &'static [usize],
    /// The `TaskActionOnBlockingInstances` the request carries, or `None` for a request
    /// that names none.
    action: Option<&'static str>,
    expected: Outcome,
}

/// The `AssetInfo` of each of a row's compute hosts, as the store records them.
fn hardware(hosts: &[Host]) -> Vec<AssetInfo> {
    hosts
        .iter()
        .enumerate()
        .map(|(index, (asset_id, families, max_vcpus))| {
            host(
                asset_id,
                &format!("h-{:017x}", index + 1),
                5.0 + index as f32,
                families,
                *max_vcpus,
            )
        })
        .collect()
}

/// The `AssetInstance` of each of a row's running instances, all on its first host.
fn instances(row: &Weighed) -> Vec<AssetInstance> {
    row.running
        .iter()
        .enumerate()
        .map(|(index, (instance_type, own))| {
            instance(
                &instance_id(index as u32 + 1),
                Some(instance_type),
                row.hosts.first().map(|(asset_id, _, _)| *asset_id),
                Some(match own {
                    true => own_account(),
                    false => OTHER_ACCOUNT,
                }),
                None,
            )
        })
        .collect()
}

/// The `StartCapacityTask` request a row sends, with `dry_run` as its `DryRun`.
fn request(row: &Weighed, dry_run: bool) -> Value {
    let mut request = json!({
        "InstancePools": row.pools
            .iter()
            .map(|(instance_type, count)| pool(instance_type, *count))
            .collect::<Vec<Value>>(),
        "DryRun": dry_run,
    });
    if let Some(asset_id) = row.asset {
        request["AssetId"] = json!(asset_id);
    }
    if !row.exclude.is_empty() {
        let ids: Vec<String> = row
            .exclude
            .iter()
            .map(|position| instance_id(*position as u32 + 1))
            .collect();
        request["InstancesToExclude"] = json!({ "Instances": ids });
    }
    if let Some(action) = row.action {
        request["TaskActionOnBlockingInstances"] = json!(action);
    }
    request
}

/// The three figures the rule weighs a row's request by, in the vCPUs the published
/// catalog counts every instance type in.
fn figures(row: &Weighed, vcpus: &BTreeMap<String, i64>) -> Figures {
    let published = |instance_type: &str| vcpus.get(instance_type).copied().unwrap_or_default();
    let requested: i64 = row
        .pools
        .iter()
        .map(|(instance_type, count)| published(instance_type) * count)
        .sum();
    // The hardware in scope: the host the request names, or all of them when it names
    // none.
    let in_scope = |asset_id: &str| row.asset.is_none_or(|named| asset_id == named);
    let ceiling: i64 = row
        .hosts
        .iter()
        .filter(|(asset_id, _, _)| in_scope(asset_id))
        .map(|(_, _, max_vcpus)| i64::from(*max_vcpus))
        .sum();
    // Every recorded instance runs on the first host, so a request scoped to another
    // one is weighed against hardware none of them runs on.
    let first_in_scope = row
        .hosts
        .first()
        .is_some_and(|(asset_id, _, _)| in_scope(asset_id));
    let mut held = 0;
    let mut held_unstoppable = 0;
    for (position, (instance_type, own)) in row.running.iter().enumerate() {
        if !first_in_scope {
            continue;
        }
        held += published(instance_type);
        if !own || row.exclude.contains(&position) {
            held_unstoppable += published(instance_type);
        }
    }
    Figures {
        requested,
        free: (ceiling - held).max(0),
        available: (ceiling - held_unstoppable).max(0),
    }
}

/// The `Failed` member the rule requires of a row's task, and `None` for a row whose
/// task does not fail.
fn failure(row: &Weighed, figures: &Figures) -> Option<Value> {
    match row.expected {
        Outcome::Completes | Outcome::Waits => None,
        Outcome::Unsupported(Unsupported::NotRun(instance_type)) => Some(json!({
            "Reason": instance_type_not_run(instance_type),
            "Type": CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION,
        })),
        Outcome::Unsupported(Unsupported::Above) => Some(json!({
            "Reason": capacity_above_available(figures.requested, figures.available),
            "Type": CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION,
        })),
        Outcome::FailsOnBlocking => Some(json!({
            "Reason": blocking_instances_not_evacuated(figures.requested, figures.free),
            "Type": CAPACITY_TASK_FAILURE_BLOCKING_INSTANCES_NOT_EVACUATED,
        })),
    }
}

/// The `CapacityTaskStatus` the rule requires the task to settle in.
fn settled_status(expected: Outcome) -> &'static str {
    match expected {
        Outcome::Completes => CAPACITY_TASK_COMPLETED,
        Outcome::Unsupported(_) | Outcome::FailsOnBlocking => CAPACITY_TASK_FAILED,
        Outcome::Waits => CAPACITY_TASK_WAITING_FOR_EVACUATION,
    }
}

/// What a DRY RUN of the same request must answer: the configuration half of the
/// weighing on its own, which is all the modelled `DryRun` asks about — so a row whose
/// configuration the hardware cannot hold is the same failure, and a row the action
/// alone decides is below the available capacity.
fn dry_run_verdict(row: &Weighed, figures: &Figures) -> (&'static str, Option<Value>) {
    match row.expected {
        Outcome::Unsupported(_) => (CAPACITY_TASK_FAILED, failure(row, figures)),
        Outcome::Completes | Outcome::FailsOnBlocking | Outcome::Waits => {
            (CAPACITY_TASK_COMPLETED, None)
        }
    }
}

/// Rules 1, 2 and 3 over the whole class: every outcome a started capacity task can
/// reach, weighed off what is persisted, beside the dry run of the identical request.
///
/// The rows differ along everything the rule generalises over: how many pools (none,
/// one, several), a `Count` at the published minimum of the modelled `range(0..9999)`,
/// inside it and at its maximum, a configuration below, exactly at and above the
/// capacity available, three ways an instance type is not one the hardware runs (a
/// family it did not record, a size above its recorded ceiling, a type the catalog
/// publishes nothing for), an Outpost with no compute hardware at all, a request scoped
/// to one host of two and to the host nothing runs on, capacity held by the initiating
/// account's instances and by another account's, an instance the request excludes, and
/// each value of `TaskActionOnBlockingInstances` beside a request that names none.
#[tokio::test]
async fn a_capacity_task_ends_as_its_configuration_was_weighed() {
    let outposts = Outposts::new();
    let vcpus = published_vcpus();
    assert!(
        !vcpus.contains_key(UNPUBLISHED_INSTANCE_TYPE),
        "the row asking for {UNPUBLISHED_INSTANCE_TYPE:?} needs a type the catalog \
         publishes nothing for, got {vcpus:?}"
    );
    // One host of 96 vCPUs that recorded no instance family, so what it can hold is
    // decided by its capacity alone; and a second one beside it for the scoped rows.
    const ONE_HOST: &[Host] = &[(HOSTS[0], &[], 96)];
    const TWO_HOSTS: &[Host] = &[(HOSTS[0], &[], 96), (HOSTS[1], &[], 96)];
    // The instances a blocking row records: one of the account that initiates every task
    // here (16 vCPUs, capacity the task may free by stopping it) beside one of another
    // account the Outpost is shared with (16 vCPUs it may not).
    const SHARED: &[Running] = &[("m5.4xlarge", true), ("m5.4xlarge", false)];

    let rows: &[Weighed] = &[
        Weighed {
            case: "a configuration well inside the hardware's capacity",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[("m5.4xlarge", 2)],
            exclude: &[],
            action: None,
            expected: Outcome::Completes,
        },
        Weighed {
            case: "exactly the capacity the hardware has",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[("m5.24xlarge", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Completes,
        },
        Weighed {
            case: "pools of several types inside it",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[("m5.large", 4), ("c5.xlarge", 2), ("m5.4xlarge", 3)],
            exclude: &[],
            action: None,
            expected: Outcome::Completes,
        },
        Weighed {
            case: "no pools at all asks for no capacity",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[],
            exclude: &[],
            action: None,
            expected: Outcome::Completes,
        },
        Weighed {
            case: "every pool at the published minimum Count",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[("m5.24xlarge", 0)],
            exclude: &[],
            action: None,
            expected: Outcome::Completes,
        },
        Weighed {
            case: "one instance above the available capacity",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[("m5.24xlarge", 1), ("m5.large", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::Above),
        },
        Weighed {
            case: "the published maximum Count of one pool, far above it",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[("m5.4xlarge", 9999)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::Above),
        },
        Weighed {
            case: "an instance type the published catalog carries no vCPU figure for",
            hosts: ONE_HOST,
            running: &[],
            asset: None,
            pools: &[(UNPUBLISHED_INSTANCE_TYPE, 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::NotRun(UNPUBLISHED_INSTANCE_TYPE)),
        },
        Weighed {
            case: "an instance type of a family the hardware did not record",
            hosts: &[(HOSTS[0], &["m5"], 96)],
            running: &[],
            asset: None,
            pools: &[("c5.xlarge", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::NotRun("c5.xlarge")),
        },
        Weighed {
            case: "an instance type larger than the hardware's recorded vCPU ceiling",
            hosts: &[(HOSTS[0], &["m5"], 16)],
            running: &[],
            asset: None,
            pools: &[("m5.8xlarge", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::NotRun("m5.8xlarge")),
        },
        Weighed {
            case: "an Outpost holding no compute hardware at all",
            hosts: &[],
            running: &[],
            asset: None,
            pools: &[("m5.large", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::NotRun("m5.large")),
        },
        Weighed {
            case: "scoped to one host of two, inside that host's capacity",
            hosts: TWO_HOSTS,
            running: &[],
            asset: Some(HOSTS[0]),
            pools: &[("m5.24xlarge", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Completes,
        },
        Weighed {
            case: "scoped to one host of two, above that host's capacity",
            hosts: TWO_HOSTS,
            running: &[],
            asset: Some(HOSTS[0]),
            pools: &[("m5.24xlarge", 1), ("m5.large", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Unsupported(Unsupported::Above),
        },
        Weighed {
            case: "capacity another account's instance holds is not this task's to free",
            hosts: ONE_HOST,
            running: &[("m5.24xlarge", false)],
            asset: None,
            pools: &[("m5.large", 1)],
            exclude: &[],
            action: Some(TASK_ACTION_FAIL_TASK),
            expected: Outcome::Unsupported(Unsupported::Above),
        },
        Weighed {
            case: "a configuration that fits beside everything already running",
            hosts: ONE_HOST,
            running: SHARED,
            asset: None,
            pools: &[("m5.16xlarge", 1)],
            exclude: &[],
            action: Some(TASK_ACTION_FAIL_TASK),
            expected: Outcome::Completes,
        },
        Weighed {
            case: "it needs capacity a blocking instance holds, and FAIL_TASK was asked for",
            hosts: ONE_HOST,
            running: SHARED,
            asset: None,
            pools: &[("m5.16xlarge", 1), ("m5.2xlarge", 1)],
            exclude: &[],
            action: Some(TASK_ACTION_FAIL_TASK),
            expected: Outcome::FailsOnBlocking,
        },
        Weighed {
            case: "the same configuration, with the evacuation waited for",
            hosts: ONE_HOST,
            running: SHARED,
            asset: None,
            pools: &[("m5.16xlarge", 1), ("m5.2xlarge", 1)],
            exclude: &[],
            action: Some(TASK_ACTION_WAIT_FOR_EVACUATION),
            expected: Outcome::Waits,
        },
        Weighed {
            case: "the same configuration, with no action named at all",
            hosts: ONE_HOST,
            running: SHARED,
            asset: None,
            pools: &[("m5.16xlarge", 1), ("m5.2xlarge", 1)],
            exclude: &[],
            action: None,
            expected: Outcome::Waits,
        },
        Weighed {
            case: "the blocking instance is excluded, so its capacity cannot be freed",
            hosts: ONE_HOST,
            running: SHARED,
            asset: None,
            pools: &[("m5.16xlarge", 1), ("m5.2xlarge", 1)],
            exclude: &[0],
            action: Some(TASK_ACTION_FAIL_TASK),
            expected: Outcome::Unsupported(Unsupported::Above),
        },
        Weighed {
            case: "above the capacity even once the blocking instances stop",
            hosts: ONE_HOST,
            running: SHARED,
            asset: None,
            pools: &[("m5.24xlarge", 1), ("m5.large", 1)],
            exclude: &[],
            action: Some(TASK_ACTION_FAIL_TASK),
            expected: Outcome::Unsupported(Unsupported::Above),
        },
        Weighed {
            case: "scoped to the host nothing runs on, so the other host's instances hold \
                   no capacity against it",
            hosts: TWO_HOSTS,
            running: &[("m5.24xlarge", true)],
            asset: Some(HOSTS[1]),
            pools: &[("m5.24xlarge", 1)],
            exclude: &[],
            action: Some(TASK_ACTION_FAIL_TASK),
            expected: Outcome::Completes,
        },
    ];

    // Every row's task is started before anything is waited for, so the whole table
    // costs ONE wait for the clock rather than one per row.
    let mut started: Vec<(String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let outpost_id = outpost(&outposts, &format!("outcome-{index}")).await;
        if !row.hosts.is_empty() {
            crate::outpost_assets::record(&outposts, &outpost_id, &hardware(row.hosts), case);
        }
        if !row.running.is_empty() {
            crate::blocking_instances::record(&outposts, &outpost_id, &instances(row), case);
        }
        let figures = figures(row, &vcpus);

        // Rule 1: the dry run of the identical request reports the configuration half of
        // the weighing, and nothing about it is stored.
        let (dry_status, dry_failure) = dry_run_verdict(row, &figures);
        let dry = start_ok(&outposts, &outpost_id, &request(row, true)).await;
        assert_eq!(
            text(&dry, "CapacityTaskStatus"),
            dry_status,
            "case {case:?}: the dry run must weigh the configuration as {dry_status}, got {dry}"
        );
        assert_eq!(
            dry.get("Failed").cloned(),
            dry_failure.clone(),
            "case {case:?}: expected the dry run's failure {dry_failure:?}, got {dry}"
        );

        // Rule 3: the real start answers the non-terminal status and no failure, whatever
        // the outcome weighed above will be.
        let answered = start_ok(&outposts, &outpost_id, &request(row, false)).await;
        assert_eq!(
            text(&answered, "CapacityTaskStatus"),
            CAPACITY_TASK_REQUESTED,
            "case {case:?}: a just-started task is awaiting its next step, got {answered}"
        );
        assert_eq!(
            answered.get("Failed"),
            None,
            "case {case:?}: nothing has failed when the start answers, got {answered}"
        );
        assert_eq!(
            answered.get("CompletionDate"),
            None,
            "case {case:?}: nothing has run when the start answers, got {answered}"
        );
        started.push((outpost_id, text(&answered, "CapacityTaskId")));
    }
    settle_capacity_task();

    for (row, (outpost_id, task_id)) in rows.iter().zip(&started) {
        let case = row.case;
        let figures = figures(row, &vcpus);
        let expected_status = settled_status(row.expected);
        let expected_failure = failure(row, &figures);
        let read = get_ok(&outposts, outpost_id, task_id).await;
        assert_eq!(
            text(&read, "CapacityTaskStatus"),
            expected_status,
            "case {case:?}: expected the task to be {expected_status} once the clock owed \
             it ({:?} asked of {} vCPUs free, {} available), got {read}",
            row.expected,
            figures.free,
            figures.available
        );
        assert_eq!(
            read.get("Failed").cloned(),
            expected_failure,
            "case {case:?}: expected the failure {expected_failure:?}, got {read}"
        );
        // "The date that the specified capacity task ran successfully": only the task
        // that ran carries one.
        assert_eq!(
            read.get("CompletionDate").is_some(),
            expected_status == CAPACITY_TASK_COMPLETED,
            "case {case:?}: only a completed task is dated as having run, got {read}"
        );
        // Rule 1, the half that makes a dry run worth running: a configuration the dry
        // run called unsupported fails the real task with the SAME failure.
        let (dry_status, dry_failure) = dry_run_verdict(row, &figures);
        if dry_status == CAPACITY_TASK_FAILED {
            assert_eq!(
                (text(&read, "CapacityTaskStatus"), read.get("Failed").cloned()),
                (CAPACITY_TASK_FAILED.to_string(), dry_failure),
                "case {case:?}: the real task must answer what the dry run of the same \
                 request did, got {read}"
            );
        }
    }
}

/// Rule 4: a task the hardware could not hold configures nothing, while one it could
/// configures exactly its pools.
///
/// The two sides are driven on Outposts carrying the SAME hardware, so what separates
/// them is the configuration each task asked for and nothing else.
#[tokio::test]
async fn only_a_completed_capacity_task_configures_the_outpost() {
    let outposts = Outposts::new();
    let vcpus = published_vcpus();
    let hosts: &[Host] = &[(HOSTS[0], &[], 96)];

    // (case, the pools the task asks for, the instance types left configured)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a configuration the hardware can hold",
            json!({ "InstancePools": [pool("m5.4xlarge", 2), pool("c5.xlarge", 1)] }),
            vec!["m5.4xlarge", "c5.xlarge"],
        ),
        (
            "a configuration above the hardware's capacity",
            json!({ "InstancePools": [pool("m5.24xlarge", 2)] }),
            Vec::new(),
        ),
        (
            "a configuration of a type the hardware does not run",
            json!({ "InstancePools": [pool(UNPUBLISHED_INSTANCE_TYPE, 1)] }),
            Vec::new(),
        ),
    ];

    let mut placed: Vec<String> = Vec::new();
    for (index, (case, request, _)) in cases.iter().enumerate() {
        let outpost_id = outpost(&outposts, &format!("configures-{index}")).await;
        crate::outpost_assets::record(&outposts, &outpost_id, &hardware(hosts), case);
        start_ok(&outposts, &outpost_id, request).await;
        placed.push(outpost_id);
    }
    settle_capacity_task();

    for ((case, _, configured), outpost_id) in cases.iter().zip(&placed) {
        let report = outposts
            .ok(
                "GET",
                &format!("/outposts/{}/instanceTypes", segment(outpost_id)),
                None,
            )
            .await;
        let reported: Vec<(String, Option<i64>)> = report
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
            .unwrap_or_default();
        let expected: Vec<(String, Option<i64>)> = configured
            .iter()
            .map(|instance_type| {
                (
                    (*instance_type).to_string(),
                    vcpus.get(*instance_type).copied(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: only a completed task configures the Outpost, got {report}"
        );
    }
}

/// Rule 5: a task waiting for evacuation is work Amazon Web Services Outposts is still
/// waiting on, so it holds its Outpost, the clock never moves it on, and cancelling it
/// is what ends it.
#[tokio::test]
async fn a_task_waiting_for_evacuation_holds_its_outpost_until_it_is_cancelled() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "waiting").await;
    let hosts: &[Host] = &[(HOSTS[0], &[], 96)];
    crate::outpost_assets::record(&outposts, &outpost_id, &hardware(hosts), "waiting");
    // One instance of the initiating account, holding capacity the configuration below
    // needs: 96 vCPUs of ceiling less the 32 it holds leaves 64 free, and the task asks
    // for 96 — which it could have once that instance stopped.
    let running = vec![instance(
        &instance_id(1),
        Some("m5.8xlarge"),
        Some(HOSTS[0]),
        Some(own_account()),
        None,
    )];
    crate::blocking_instances::record(&outposts, &outpost_id, &running, "waiting");

    let request = json!({
        "InstancePools": [pool("m5.24xlarge", 1)],
        "TaskActionOnBlockingInstances": TASK_ACTION_WAIT_FOR_EVACUATION,
    });
    let task_id = text(
        &start_ok(&outposts, &outpost_id, &request).await,
        "CapacityTaskId",
    );
    settle_capacity_task();
    let waiting = get_ok(&outposts, &outpost_id, &task_id).await;
    assert_eq!(
        text(&waiting, "CapacityTaskStatus"),
        CAPACITY_TASK_WAITING_FOR_EVACUATION,
        "the task must report that it requires capacity the running instances hold, \
         got {waiting}"
    );

    // What it waits for, as the service recommends it: the instance holding the capacity
    // the task needs.
    let blocking = outposts
        .ok(
            "GET",
            &format!(
                "/outposts/{}/capacity/{}/blockingInstances",
                segment(&outpost_id),
                segment(&task_id)
            ),
            None,
        )
        .await;
    let listed: Vec<String> = blocking["BlockingInstances"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| text(entry, "InstanceId"))
        .collect();
    assert_eq!(
        listed,
        vec![instance_id(1)],
        "the task must be waiting for the instance the service recommends stopping, \
         got {blocking}"
    );

    // The clock owes it nothing: no span moves it on, because what it waits for is an
    // instance being stopped and no call of this API stops one.
    settle_capacity_task_steps(4);
    let later = get_ok(&outposts, &outpost_id, &task_id).await;
    assert_eq!(
        text(&later, "CapacityTaskStatus"),
        CAPACITY_TASK_WAITING_FOR_EVACUATION,
        "waiting for an evacuation is not a step the clock takes, got {later}"
    );

    // It is the Outpost's ACTIVE task, so a second one is refused.
    let refused = start(
        &outposts,
        &outpost_id,
        &json!({ "InstancePools": [pool("m5.large", 1)] }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, CONFLICT_CODE),
        "a task still waiting holds its Outpost: expected 409 {CONFLICT_CODE}, got {} {} {}",
        refused.status,
        refused.error_code,
        refused.body
    );

    // Cancelling is what ends the wait, and then the Outpost can take another task.
    outposts
        .ok("POST", &task_route(&outpost_id, &task_id), Some(&json!({})))
        .await;
    settle_capacity_task();
    let cancelled = get_ok(&outposts, &outpost_id, &task_id).await;
    assert_eq!(
        text(&cancelled, "CapacityTaskStatus"),
        CAPACITY_TASK_CANCELLED,
        "a cancelled task settles cancelled, got {cancelled}"
    );
    let accepted = outposts
        .ok(
            "POST",
            &capacity_route(&outpost_id),
            Some(&json!({ "InstancePools": [pool("m5.large", 1)] })),
        )
        .await;
    assert_eq!(
        text(&accepted, "CapacityTaskStatus"),
        CAPACITY_TASK_REQUESTED,
        "once the wait is cancelled the Outpost can take another task, got {accepted}"
    );
}
