// SPDX-License-Identifier: CC0-1.0

//! What a DRY RUN of `StartCapacityTask` is answered with, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — and the
//! harness is [`crate::capacity_task_lifecycle`]'s, which owns what a started task
//! reports.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. Resolving the resources a request names comes BEFORE the work the request asks
//!    for, so a reference that resolves to nothing is refused 404 `NotFoundException`
//!    with the wording of the kind of resource it could not resolve, whatever else the
//!    request asked for — asking for a dry run does not change which refusal applies.
//! 2. A dry run is a normal, successful call: it is answered 200 from a projection of
//!    the request that nothing is stored for — the plan echoed back, `DryRun` true, no
//!    `CapacityTaskId`, and a terminal `CapacityTaskStatus` carrying the verdict — and
//!    it changes nothing, so the rules about a task being CREATED (one active task per
//!    Outpost and per order) are not applied to it.
//! 3. The verdict is the comparison the modelled `DryRun` documents: the capacity the
//!    requested pools ask for, against the capacity available on the hardware they
//!    would configure — the vCPU ceiling of the compute hosts in scope, the one asset
//!    the request names or all of the Outpost's when it names none. At or below it the
//!    configuration is `COMPLETED`; above it, and for an instance type that hardware
//!    does not run at all, it is `FAILED` with the modelled
//!    `UNSUPPORTED_CAPACITY_CONFIGURATION`.
//! 4. Capacity held by instances the task could not stop — ones it excludes, and ones
//!    belonging to another account — is not available to it, while capacity held by
//!    the instances it may stop is.
//!
//! Every vCPU figure a row expects is read from the service's own specification table,
//! never from a copy of it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::AssetInfo;
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    asset_not_found, capacity_above_available, instance_type_not_run, order_not_found,
    outpost_not_found, CAPACITY_TASK_COMPLETED, CAPACITY_TASK_FAILED,
    CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION, CAPACITY_TASK_STATUS_REASON_MAX,
};

use crate::blocking_instances::{instance, own_account, OTHER_ACCOUNT};
use crate::capacity_task_lifecycle::{
    an_order, capacity_route, install, outpost, pool, start, start_ok, task_route, ABSENT_ASSET_ID,
    ABSENT_ORDER_ID, INSTALLED,
};
use crate::outpost_assets::{asset, compute};
use crate::site_and_outpost_lifecycle::{text, Outposts, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX};

/// An `AssetId` of the modelled `AssetIdInput` form (`^\d{10}$`) recorded as hardware
/// installed on ANOTHER Outpost, so a row can name an asset that exists somewhere
/// without being hardware of the Outpost the task would run on.
const ELSEWHERE_ASSET_ID: &str = "2000000009";

/// How the defaulted `DryRun` member arrives: left out, sent false, or sent true.
/// Rule 1 holds for all three.
const DRY_RUN_FORMS: [(&str, Option<bool>); 3] = [
    ("the DryRun member left out", None),
    ("DryRun sent false", Some(false)),
    ("DryRun sent true", Some(true)),
];

/// `request` with the `DryRun` member as `form` says it arrives.
fn with_dry_run(request: &Value, form: Option<bool>) -> Value {
    let mut request = request.clone();
    if let Some(dry_run) = form {
        request["DryRun"] = json!(dry_run);
    }
    request
}

/// How many capacity tasks the store holds.
fn held(outposts: &Outposts) -> usize {
    outposts.store().lock().capacity_tasks.len()
}

/// Rule 1: a dry run is refused for a reference that resolves to nothing exactly as a
/// real start is, with the same status, the same modelled error and the same identifier
/// echo — the member's validation-path name and the value as supplied.
#[tokio::test]
async fn a_dry_run_is_refused_for_a_reference_that_resolves_to_nothing() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "dry-run-references").await;
    let elsewhere = outpost(&outposts, "dry-run-elsewhere").await;
    install(&outposts, &outpost_id, &INSTALLED, "dry-run-references");
    install(
        &outposts,
        &elsewhere,
        &[ELSEWHERE_ASSET_ID],
        "dry-run-elsewhere",
    );
    // An order the account really placed, so a row can name one reference that
    // resolves beside one that does not.
    let order_id = an_order(&outposts, &outpost_id).await;
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    // Each row: the Outpost the call is addressed to, the request without its `DryRun`
    // member, and the exact refusal body — the wording of the KIND of resource the row
    // could not resolve: the Outpost sentence, the order sentence, or the asset
    // sentence.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "an Outpost nothing holds, named by bare id",
            ABSENT_OUTPOST_ID,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            outpost_not_found(ABSENT_OUTPOST_ID),
        ),
        (
            "an Outpost nothing holds, named by full ARN",
            &absent_outpost_arn,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            outpost_not_found(&absent_outpost_arn),
        ),
        (
            "an OrderId this store holds no order for",
            &outpost_id,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)], "OrderId": ABSENT_ORDER_ID }),
            order_not_found(ABSENT_ORDER_ID),
        ),
        (
            "an OrderId that names nothing, beside an asset that resolves",
            &outpost_id,
            json!({
                "InstancePools": [pool("c5.xlarge", 4), pool("m5.large", 2)],
                "OrderId": ABSENT_ORDER_ID,
                "AssetId": INSTALLED[0],
            }),
            order_not_found(ABSENT_ORDER_ID),
        ),
        (
            "an AssetId no Outpost of this store holds",
            &outpost_id,
            json!({ "InstancePools": [], "AssetId": ABSENT_ASSET_ID }),
            asset_not_found(ABSENT_ASSET_ID),
        ),
        (
            "an AssetId installed on another Outpost",
            &outpost_id,
            json!({ "InstancePools": [pool("m5.4xlarge", 1)], "AssetId": ELSEWHERE_ASSET_ID }),
            asset_not_found(ELSEWHERE_ASSET_ID),
        ),
        (
            "an AssetId that names nothing, beside an order that resolves",
            &outpost_id,
            json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "OrderId": order_id,
                "AssetId": ABSENT_ASSET_ID,
            }),
            asset_not_found(ABSENT_ASSET_ID),
        ),
    ];

    for (case, outpost_identifier, request, expected) in &cases {
        for (form, dry_run) in DRY_RUN_FORMS {
            let case = format!("{case} ({form})");
            let reply = start(
                &outposts,
                outpost_identifier,
                &with_dry_run(request, dry_run),
            )
            .await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: expected 404 NotFoundException, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                &text(&reply.body, "Message"),
                expected,
                "case {case:?}: expected $.Message {expected:?}, got {}",
                reply.body
            );
        }
    }

    // No row created anything: a refused start — dry run or not — stores no task.
    let stored = held(&outposts);
    assert_eq!(
        stored, 0,
        "a refused start must store no capacity task: expected 0 held, got {stored}"
    );
}

/// Rule 2: a dry run whose references all resolve is answered 200 from a projection of
/// the request that nothing was stored for, and it changes nothing a later caller can
/// see.
#[tokio::test]
async fn a_dry_run_is_answered_from_the_request_and_stores_nothing() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "dry-run-answer").await;
    install(&outposts, &outpost_id, &INSTALLED, "dry-run-answer");
    let order_id = an_order(&outposts, &outpost_id).await;

    // The rows differ along what the request can carry: how many pools (none, one,
    // several), which optional references are named, and whether the caller said what
    // to do about blocking instances.
    let cases: Vec<(&str, Value)> = vec![
        (
            "one pool and no optional member",
            json!({ "InstancePools": [pool("m5.4xlarge", 1)], "DryRun": true }),
        ),
        (
            "no pools at all",
            json!({ "InstancePools": [], "DryRun": true }),
        ),
        (
            "several pools, an order and an asset that both resolve",
            json!({
                "InstancePools": [pool("m5.large", 3), pool("c5.24xlarge", 9999)],
                "OrderId": order_id,
                "AssetId": INSTALLED[1],
                "DryRun": true,
            }),
        ),
        (
            "instances to exclude and an action for blocking instances",
            json!({
                "InstancePools": [pool("r5.2xlarge", 1)],
                "InstancesToExclude": { "Instances": ["i-0123456789abcdef0"] },
                "TaskActionOnBlockingInstances": "WAIT_FOR_EVACUATION",
                "DryRun": true,
            }),
        ),
    ];

    for (case, request) in &cases {
        let answered = start_ok(&outposts, &outpost_id, request).await;
        // The plan the verdict is about, echoed back: every member the request
        // carried, reported as the caller sent it.
        assert_eq!(
            answered.get("DryRun"),
            Some(&Value::Bool(true)),
            "case {case:?}: a dry run reports DryRun true, got {answered}"
        );
        assert_eq!(
            answered.get("OutpostId").and_then(Value::as_str),
            Some(outpost_id.as_str()),
            "case {case:?}: the verdict names the Outpost it is about, got {answered}"
        );
        assert_eq!(
            answered.get("RequestedInstancePools"),
            request.get("InstancePools"),
            "case {case:?}: the requested pools are reported back, got {answered}"
        );
        for member in ["OrderId", "AssetId", "InstancesToExclude"] {
            assert_eq!(
                answered.get(member),
                request.get(member),
                "case {case:?}: {member} is reported exactly as it arrived, got {answered}"
            );
        }
        assert_eq!(
            answered.get("TaskActionOnBlockingInstances"),
            request.get("TaskActionOnBlockingInstances"),
            "case {case:?}: the action for blocking instances is reported back, \
             got {answered}"
        );
        // Nothing was created, so no id names it.
        assert!(
            answered.get("CapacityTaskId").is_none(),
            "case {case:?}: a dry run creates no task, so it reports no CapacityTaskId, \
             got {answered}"
        );
        // The verdict itself: one of the two terminal statuses, with a failure
        // exactly when it failed, and the completion date only on the status that
        // means the configuration ran.
        assert_verdict_shape(&answered, case);
        let stamp = |member: &str| answered.get(member).and_then(Value::as_f64);
        assert_eq!(
            stamp("CreationDate"),
            stamp("LastModifiedDate"),
            "case {case:?}: a dry run is requested and answered at one instant, \
             got {answered}"
        );
        let stored = held(&outposts);
        assert_eq!(
            stored, 0,
            "case {case:?}: a dry run must store no capacity task: expected 0 held, \
             got {stored}"
        );
    }

    // The dry runs left the Outpost able to take a task, which it could not had one of
    // them left a task active on it — and the task the real start creates is the only
    // one the store holds.
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 2)], "OrderId": order_id });
    let started = start_ok(&outposts, &outpost_id, &request).await;
    let task_id = text(&started, "CapacityTaskId");
    let stored = held(&outposts);
    assert_eq!(
        stored, 1,
        "the real start is the only call that stored a task: expected 1 held, got {stored}"
    );
    let read = outposts
        .ok("GET", &task_route(&outpost_id, &task_id), None)
        .await;
    assert_eq!(
        text(&read, "CapacityTaskId"),
        task_id,
        "GetCapacityTask must answer the task the real start persisted, got {read}"
    );
    assert_eq!(
        read.get("DryRun"),
        Some(&Value::Bool(false)),
        "the persisted task is not a dry run, got {read}"
    );

    // And a dry run made WHILE that task is active is still answered with a verdict,
    // not with the one-active-task conflict: a dry run creates no task, so it is not a
    // second one.
    let answered = start_ok(
        &outposts,
        &outpost_id,
        &json!({ "InstancePools": [pool("m5.4xlarge", 1)], "DryRun": true }),
    )
    .await;
    assert_verdict_shape(&answered, "a dry run beside an active task");
    let stored = held(&outposts);
    assert_eq!(
        stored, 1,
        "that dry run stored nothing either: expected 1 held, got {stored}"
    );
}

/// Rule 2, the other side of it: with `DryRun` left out or sent false the same request
/// persists a task that `GetCapacityTask` reads back, so what a dry run withholds is
/// the task — not the answer.
#[tokio::test]
async fn a_dry_run_and_a_real_start_differ_in_what_they_persist() {
    let outposts = Outposts::new();
    for (index, (case, dry_run)) in DRY_RUN_FORMS.iter().enumerate() {
        // One Outpost per row: only one capacity task can be active on an Outpost.
        let outpost_id = outpost(&outposts, &format!("dry-run-persistence-{index}")).await;
        install(&outposts, &outpost_id, &INSTALLED, case);
        let order_id = an_order(&outposts, &outpost_id).await;
        let request = with_dry_run(
            &json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "OrderId": order_id,
                "AssetId": INSTALLED[0],
            }),
            *dry_run,
        );
        let before = held(&outposts);

        let answered = outposts
            .ok("POST", &capacity_route(&outpost_id), Some(&request))
            .await;
        assert_eq!(
            answered.get("DryRun"),
            Some(&Value::Bool(dry_run.unwrap_or(false))),
            "case {case:?}: the answer reports the DryRun the request carried, \
             got {answered}"
        );
        if dry_run.unwrap_or(false) {
            assert_eq!(
                (held(&outposts), answered.get("CapacityTaskId")),
                (before, None),
                "case {case:?}: a dry run stores no task and mints no id, got {answered}"
            );
            continue;
        }
        assert_eq!(
            held(&outposts),
            before + 1,
            "case {case:?}: a real start stores exactly one task"
        );
        let read = outposts
            .ok(
                "GET",
                &task_route(&outpost_id, &text(&answered, "CapacityTaskId")),
                None,
            )
            .await;
        assert_eq!(
            text(&read, "OrderId"),
            order_id,
            "case {case:?}: the persisted task carries the order it named, got {read}"
        );
        assert_eq!(
            text(&read, "AssetId"),
            INSTALLED[0],
            "case {case:?}: the persisted task carries the asset it named, got {read}"
        );
    }
}

/// The vCPU ceiling (`ComputeAttributes$MaxVcpus`) recorded on each compute host of
/// the WIDE hardware profile below, and how many such hosts it holds.
const WIDE_HOST_VCPUS: i32 = 96;
/// See [`WIDE_HOST_VCPUS`].
const WIDE_HOSTS: [&str; 2] = ["1000000001", "1000000002"];

/// An asset of the wide profile that is NOT compute hardware: a storage server hosts
/// no instance, so it carries no instance capacity at all.
const WIDE_STORAGE_ASSET: &str = "1000000003";

/// The instance families recorded on the wide profile's hosts
/// (`ComputeAttributes$InstanceFamilies`).
const WIDE_FAMILIES: [&str; 2] = ["m5", "c5"];

/// How many compute hosts the wide profile holds, as the arithmetic counts them.
fn hosts_in_wide_profile() -> i64 {
    i64::try_from(WIDE_HOSTS.len()).unwrap_or_default()
}

/// The vCPU ceiling recorded on the one compute host of the NARROW profile, which is
/// small enough that instance types of a family it does run are larger than it.
const NARROW_HOST_VCPUS: i32 = 16;
/// See [`NARROW_HOST_VCPUS`].
const NARROW_HOST: &str = "1000000004";

/// One compute host of a profile: a `COMPUTE` asset carrying the Dedicated Host it
/// hosts instances on, the instance families it runs and its vCPU ceiling — the two
/// `ComputeAttributes` members a capacity verdict reads.
pub(crate) fn host(
    asset_id: &str,
    host_id: &str,
    elevation: f32,
    families: &[&str],
    max_vcpus: i32,
) -> AssetInfo {
    asset(
        asset_id,
        "rack-1",
        Some("COMPUTE"),
        Some(compute(
            Some(host_id),
            Some("ACTIVE"),
            Some(families),
            None,
            Some(max_vcpus),
        )),
        Some(elevation),
    )
}

/// The hardware of the WIDE profile: two compute hosts of [`WIDE_HOST_VCPUS`] vCPUs
/// each, running the families [`WIDE_FAMILIES`] names, beside one asset that is no
/// compute hardware.
fn wide_hardware() -> Vec<AssetInfo> {
    vec![
        host(
            WIDE_HOSTS[0],
            "h-00000000000000001",
            5.0,
            &WIDE_FAMILIES,
            WIDE_HOST_VCPUS,
        ),
        host(
            WIDE_HOSTS[1],
            "h-00000000000000002",
            6.0,
            &WIDE_FAMILIES,
            WIDE_HOST_VCPUS,
        ),
        asset(
            WIDE_STORAGE_ASSET,
            "rack-1",
            Some("STORAGE"),
            None,
            Some(11.0),
        ),
    ]
}

/// The hardware of the NARROW profile: one compute host of [`NARROW_HOST_VCPUS`]
/// vCPUs, running the `m5` family.
fn narrow_hardware() -> Vec<AssetInfo> {
    vec![host(
        NARROW_HOST,
        "h-00000000000000009",
        5.0,
        &["m5"],
        NARROW_HOST_VCPUS,
    )]
}

/// The default vCPU count this service specifies for every instance type it holds a
/// specification of (`DetailedInstanceTypeItem$VCPUs`, the figure every capacity
/// figure of this service is counted in) — so what a row expects follows the
/// service's own table rather than a copy of it.
///
/// Read from that table rather than from the catalog `ListOrderableInstanceTypes`
/// publishes, which is the narrower one entry per family capacity is ORDERED in: a
/// pool configures hardware in any size it runs (see
/// [`crate::instance_type_catalogs`]).
pub(crate) fn published_vcpus() -> BTreeMap<String, i64> {
    let entries = crate::instance_type_catalogs::specifications();
    assert!(
        !entries.is_empty(),
        "the service's specification table is what every row's arithmetic reads"
    );
    entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("InstanceType")?.as_str()?.to_string(),
                entry.get("VCPUs")?.as_i64()?,
            ))
        })
        .collect()
}

/// An instance type of the modelled `InstanceTypeName` form
/// (`^[a-z0-9\-]+\.[a-z0-9\-]+$`) that this service's catalog publishes nothing for,
/// so no vCPU figure exists to weigh a pool of it by.
const UNPUBLISHED_INSTANCE_TYPE: &str = "zz9.mega";

/// What a row expects the verdict to be.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The configuration is at or below the capacity available in its scope.
    Fits,
    /// It is above the `available` vCPUs its scope has.
    Above { available: i64 },
    /// The hardware in scope does not run the instance type named.
    NotRun(&'static str),
}

/// One verdict row: the request's scope, the pools it asks for, and the verdict.
struct Verdict {
    case: &'static str,
    /// Whether the row asks about the wide profile or the narrow one.
    narrow: bool,
    /// The `AssetId` the request names, which scopes the verdict to that one host.
    asset: Option<&'static str>,
    /// The pools the request carries, as (instance type, count).
    pools: &'static [(&'static str, i64)],
    expected: Expected,
}

/// Rule 3: the verdict is the requested capacity weighed against the capacity
/// available on the hardware in scope.
///
/// The rows differ along everything the rule generalises over: how many pools (none,
/// one, several), the `Count` of a pool (zero, one, the modelled maximum), how big the
/// instance type is, whether the request is below, exactly at or above what is
/// available, which hardware is in scope (the whole Outpost, one compute host, an
/// asset that is no compute host), and three ways an instance type is not one the
/// hardware runs — a family it did not record, a size above its recorded ceiling, and
/// a type the catalog publishes nothing for. Every row runs against the SAME two
/// Outposts: a dry run changes nothing, so no row can disturb the next.
#[tokio::test]
async fn a_dry_run_weighs_the_requested_pools_against_the_available_capacity() {
    let outposts = Outposts::new();
    let wide = outpost(&outposts, "verdict-wide").await;
    let narrow = outpost(&outposts, "verdict-narrow").await;
    crate::outpost_assets::record(&outposts, &wide, &wide_hardware(), "verdict-wide");
    crate::outpost_assets::record(&outposts, &narrow, &narrow_hardware(), "verdict-narrow");
    let vcpus = published_vcpus();
    // What each scope has available with nothing running on it: the sum of the vCPU
    // ceilings of the compute hosts in that scope.
    let wide_available = i64::from(WIDE_HOST_VCPUS) * hosts_in_wide_profile();
    let one_host = i64::from(WIDE_HOST_VCPUS);
    let narrow_available = i64::from(NARROW_HOST_VCPUS);

    let cases: Vec<Verdict> = vec![
        Verdict {
            case: "no pools at all asks for no capacity",
            narrow: false,
            asset: None,
            pools: &[],
            expected: Expected::Fits,
        },
        Verdict {
            case: "one pool well inside the available capacity",
            narrow: false,
            asset: None,
            pools: &[("m5.4xlarge", 1)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "pools of several types inside it",
            narrow: false,
            asset: None,
            pools: &[("m5.large", 4), ("c5.xlarge", 2), ("m5.4xlarge", 3)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "a pool of Count zero asks for no capacity",
            narrow: false,
            asset: None,
            pools: &[("m5.24xlarge", 0)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "exactly the available capacity",
            narrow: false,
            asset: None,
            pools: &[("m5.24xlarge", 2)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "one instance above the available capacity",
            narrow: false,
            asset: None,
            pools: &[("m5.24xlarge", 2), ("m5.large", 1)],
            expected: Expected::Above {
                available: wide_available,
            },
        },
        Verdict {
            case: "the modelled maximum Count of one pool, far above it",
            narrow: false,
            asset: None,
            pools: &[("m5.4xlarge", 9999)],
            expected: Expected::Above {
                available: wide_available,
            },
        },
        Verdict {
            case: "scoped to one compute host, inside that host's capacity",
            narrow: false,
            asset: Some(WIDE_HOSTS[0]),
            pools: &[("m5.24xlarge", 1)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "scoped to one compute host, above that host's capacity",
            narrow: false,
            asset: Some(WIDE_HOSTS[0]),
            pools: &[("m5.24xlarge", 1), ("m5.large", 1)],
            expected: Expected::Above {
                available: one_host,
            },
        },
        Verdict {
            case: "scoped to an asset that is no compute hardware",
            narrow: false,
            asset: Some(WIDE_STORAGE_ASSET),
            pools: &[("m5.4xlarge", 1)],
            expected: Expected::NotRun("m5.4xlarge"),
        },
        Verdict {
            case: "an instance type of a family the hardware did not record",
            narrow: false,
            asset: None,
            pools: &[("r5.4xlarge", 1)],
            expected: Expected::NotRun("r5.4xlarge"),
        },
        Verdict {
            case: "the pool the hardware cannot run beside pools it can",
            narrow: false,
            asset: None,
            pools: &[("m5.large", 1), ("r5.4xlarge", 1)],
            expected: Expected::NotRun("r5.4xlarge"),
        },
        Verdict {
            case: "an instance type the published catalog knows nothing about",
            narrow: false,
            asset: None,
            pools: &[(UNPUBLISHED_INSTANCE_TYPE, 1)],
            expected: Expected::NotRun(UNPUBLISHED_INSTANCE_TYPE),
        },
        Verdict {
            case: "a type at exactly the narrow host's recorded vCPU ceiling",
            narrow: true,
            asset: None,
            pools: &[("m5.4xlarge", 1)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "a type of a family it runs, but larger than its vCPU ceiling",
            narrow: true,
            asset: None,
            pools: &[("m5.8xlarge", 1)],
            expected: Expected::NotRun("m5.8xlarge"),
        },
        Verdict {
            case: "the narrow host's whole capacity, in its smallest size",
            narrow: true,
            asset: Some(NARROW_HOST),
            pools: &[("m5.large", 8)],
            expected: Expected::Fits,
        },
        Verdict {
            case: "one instance above the narrow host's capacity",
            narrow: true,
            asset: Some(NARROW_HOST),
            pools: &[("m5.large", 9)],
            expected: Expected::Above {
                available: narrow_available,
            },
        },
    ];

    for case in &cases {
        let outpost_identifier = match case.narrow {
            true => &narrow,
            false => &wide,
        };
        let mut request = json!({
            "InstancePools": case.pools
                .iter()
                .map(|(instance_type, count)| pool(instance_type, *count))
                .collect::<Vec<Value>>(),
            "DryRun": true,
        });
        if let Some(asset_id) = case.asset {
            request["AssetId"] = json!(asset_id);
        }

        let answered = start_ok(&outposts, outpost_identifier, &request).await;
        // What the row asked for, in the vCPUs the published catalog counts each
        // instance type in.
        let requested: i64 = case
            .pools
            .iter()
            .map(|(instance_type, count)| {
                vcpus.get(*instance_type).copied().unwrap_or_default() * count
            })
            .sum();
        assert_expected(&answered, case.case, case.expected, requested);
        let stored = held(&outposts);
        assert_eq!(
            stored, 0,
            "case {:?}: a dry run stores no capacity task: expected 0 held, got {stored}",
            case.case
        );
    }
}

/// One capacity-in-use row: what the request excludes, and which of the recorded
/// instances the task therefore could not stop.
struct InUse {
    case: &'static str,
    /// The `InstancesToExclude` member the request carries, or `None` for a request
    /// that excludes nothing.
    exclude: Option<Value>,
    /// The instance types of the recorded instances this row's task could not stop:
    /// capacity that is NOT available to it.
    unstoppable: &'static [&'static str],
}

/// Rule 4: capacity held by instances the task could not stop is not available to it,
/// while capacity held by the instances it may stop is.
///
/// The rows differ along every way an instance can be one the task cannot stop: it
/// belongs to another account, or the request excludes it by instance id, by owning
/// account or by owning service — the three sub-lists the modelled
/// `InstancesToExclude` declares. Each row is asserted at the boundary from both
/// sides: a request for exactly what is left over fits, and one instance more than
/// that is above it.
#[tokio::test]
async fn capacity_held_by_instances_that_cannot_be_stopped_is_not_available() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "verdict-in-use").await;
    crate::outpost_assets::record(&outposts, &outpost_id, &wide_hardware(), "verdict-in-use");
    // The instances running on that hardware: two of the account that initiates every
    // capacity task here (one of them owned by an Amazon Web Services service), and
    // one of another account the Outpost is shared with.
    let running = vec![
        instance(
            "i-00000000000000001",
            Some("m5.4xlarge"),
            Some(WIDE_HOSTS[0]),
            Some(own_account()),
            None,
        ),
        instance(
            "i-00000000000000002",
            Some("m5.4xlarge"),
            Some(WIDE_HOSTS[1]),
            Some(own_account()),
            Some("EC2"),
        ),
        instance(
            "i-00000000000000003",
            Some("m5.24xlarge"),
            Some(WIDE_HOSTS[1]),
            Some(OTHER_ACCOUNT),
            None,
        ),
    ];
    crate::blocking_instances::record(&outposts, &outpost_id, &running, "verdict-in-use");
    let vcpus = published_vcpus();
    let ceiling = i64::from(WIDE_HOST_VCPUS) * hosts_in_wide_profile();

    let cases: Vec<InUse> = vec![
        InUse {
            case: "nothing excluded: only the other account's instance holds capacity",
            exclude: None,
            unstoppable: &["m5.24xlarge"],
        },
        InUse {
            case: "one instance excluded by its id",
            exclude: Some(json!({ "Instances": ["i-00000000000000001"] })),
            unstoppable: &["m5.24xlarge", "m5.4xlarge"],
        },
        InUse {
            case: "the initiating account excluded by account id",
            exclude: Some(json!({ "AccountIds": [own_account()] })),
            unstoppable: &["m5.24xlarge", "m5.4xlarge", "m5.4xlarge"],
        },
        InUse {
            case: "the service that owns an instance excluded by service name",
            exclude: Some(json!({ "Services": ["EC2"] })),
            unstoppable: &["m5.24xlarge", "m5.4xlarge"],
        },
        InUse {
            case: "an exclusion naming nothing that is running excludes nothing",
            exclude: Some(json!({ "Instances": ["i-00000000000000009"] })),
            unstoppable: &["m5.24xlarge"],
        },
    ];

    for case in &cases {
        let in_use: i64 = case
            .unstoppable
            .iter()
            .map(|instance_type| vcpus.get(*instance_type).copied().unwrap_or_default())
            .sum();
        let available = ceiling - in_use;
        let smallest = vcpus.get("m5.large").copied().unwrap_or_default();
        let instances = available / smallest.max(1);
        assert!(
            available > 0 && instances * smallest == available,
            "case {:?}: the row's arithmetic must leave whole instances to ask for, \
             got {available} vCPUs",
            case.case
        );

        // Exactly what is left over fits, and one instance more does not.
        for (edge, count) in [
            ("exactly the capacity left over", instances),
            ("one instance more than that", instances + 1),
        ] {
            let case_name = format!("{} ({edge})", case.case);
            let mut request = json!({
                "InstancePools": [pool("m5.large", count)],
                "DryRun": true,
            });
            if let Some(exclude) = &case.exclude {
                request["InstancesToExclude"] = exclude.clone();
            }
            let answered = start_ok(&outposts, &outpost_id, &request).await;
            let requested = smallest * count;
            let expected = match requested > available {
                true => Expected::Above { available },
                false => Expected::Fits,
            };
            assert_expected(&answered, &case_name, expected, requested);
        }
    }
}

/// The longest `InstanceTypeName` the model admits, from
/// `smithy_query(query="shape",shape="InstanceTypeCapacity")`: its `InstanceType`
/// member is `length(min: 1, max: 64)` with the pattern
/// `^[a-z0-9\-]+\.[a-z0-9\-]+$`.
const INSTANCE_TYPE_NAME_MAX: usize = 64;

/// An instance type of `length` characters, of the alphabet the modelled pattern
/// accepts.
fn instance_type_of(length: usize) -> String {
    format!("m5.{}", "x".repeat(length.saturating_sub(3)))
}

/// The reason a failed verdict words is a value this service REPORTS, so it fits the
/// modelled `CapacityTaskStatusReason` (`length(0..128)`) for any instance type that
/// reaches it — and an instance type the model admits in full is named in full.
///
/// The lengths span the class: well inside the modelled maximum, at it, one past it,
/// and far past the reason's own maximum. Over the wire the boundary refuses a type
/// past the modelled maximum before any verdict is worded at all, which is the other
/// half of the same rule.
#[tokio::test]
async fn the_reason_a_failed_verdict_words_fits_the_modelled_member() {
    for length in [
        8,
        INSTANCE_TYPE_NAME_MAX,
        INSTANCE_TYPE_NAME_MAX + 1,
        4 * CAPACITY_TASK_STATUS_REASON_MAX,
    ] {
        let instance_type = instance_type_of(length);
        let reason = instance_type_not_run(&instance_type);
        assert!(
            reason.len() <= CAPACITY_TASK_STATUS_REASON_MAX,
            "case length {length}: the reason must fit the modelled maximum of \
             {CAPACITY_TASK_STATUS_REASON_MAX}, got {} characters in {reason:?}",
            reason.len()
        );
        if length <= INSTANCE_TYPE_NAME_MAX {
            assert!(
                reason.ends_with(&instance_type),
                "case length {length}: an instance type the model admits is named in \
                 full, got {reason:?}"
            );
        }
    }

    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "verdict-reason").await;
    let request = json!({
        "InstancePools": [pool(&instance_type_of(INSTANCE_TYPE_NAME_MAX + 1), 1)],
        "DryRun": true,
    });
    let reply = start(&outposts, &outpost_id, &request).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "an instance type past the modelled maximum is refused at the boundary, \
         got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
}

/// Assert the SHAPE of a verdict, whatever it decided: one of the two terminal
/// statuses a verdict can be, a `Failed` exactly when it failed — carrying the
/// modelled unsupported-configuration type — and a `CompletionDate` only on the
/// status that means the configuration ran.
fn assert_verdict_shape(answered: &Value, case: &str) {
    let status = text(answered, "CapacityTaskStatus");
    assert!(
        status == CAPACITY_TASK_COMPLETED || status == CAPACITY_TASK_FAILED,
        "case {case:?}: a dry run's verdict is terminal, expected \
         {CAPACITY_TASK_COMPLETED} or {CAPACITY_TASK_FAILED}, got {status:?}"
    );
    let failed = answered.get("Failed");
    assert_eq!(
        failed.is_some(),
        status == CAPACITY_TASK_FAILED,
        "case {case:?}: a verdict reports a failure exactly when it failed, \
         got {answered}"
    );
    if let Some(failed) = failed {
        assert_eq!(
            text(failed, "Type"),
            CAPACITY_TASK_FAILURE_UNSUPPORTED_CONFIGURATION,
            "case {case:?}: a capacity verdict fails as an unsupported configuration, \
             got {answered}"
        );
    }
    assert_eq!(
        answered.get("CompletionDate").is_some(),
        status == CAPACITY_TASK_COMPLETED,
        "case {case:?}: only the successful verdict ran, so only it carries a \
         CompletionDate, got {answered}"
    );
}

/// Assert one row's whole verdict: the shape above, the status it decided, and — for a
/// failure — the reason this service words that failure with.
fn assert_expected(answered: &Value, case: &str, expected: Expected, requested: i64) {
    assert_verdict_shape(answered, case);
    let status = text(answered, "CapacityTaskStatus");
    let expected_status = match expected {
        Expected::Fits => CAPACITY_TASK_COMPLETED,
        _ => CAPACITY_TASK_FAILED,
    };
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected the verdict {expected_status}, got {answered}"
    );
    let expected_reason = match expected {
        Expected::Fits => return,
        Expected::Above { available } => capacity_above_available(requested, available),
        Expected::NotRun(instance_type) => instance_type_not_run(instance_type),
    };
    assert_eq!(
        text(
            answered
                .get("Failed")
                .unwrap_or_else(|| panic!("case {case:?}: a failed verdict carries Failed")),
            "Reason"
        ),
        expected_reason,
        "case {case:?}: expected the reason {expected_reason:?}, got {answered}"
    );
}
