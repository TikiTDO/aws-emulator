// SPDX-License-Identifier: CC0-1.0

//! The HOST-COUNT series a load balancer publishes for the target groups it routes to, and
//! the periodic health check those counts are read off.
//!
//! A load balancer does not only hold its target groups' registrations; it CHECKS them, over
//! and over, and reports how many of each group's targets it currently considers healthy and
//! unhealthy as the CloudWatch metrics [`HEALTHY_HOST_COUNT`] and [`UNHEALTHY_HOST_COUNT`] in
//! the namespace its own family publishes ([`namespace`]), dimensioned by the pair
//! [`TARGET_GROUP_DIMENSION`] + [`LOAD_BALANCER_DIMENSION`] — the two resources a count
//! belongs to. That series is how a client watches a target group: an alarm on it is what
//! drives unhealthy-host remediation, so a load balancer that publishes nothing leaves every
//! such alarm without data.
//!
//! A series belongs to the PAIR, so it exists exactly while a listener or a rule of that load
//! balancer forwards to that target group ([`super::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State::refresh_target_group_associations`]):
//! a target group nothing routes to is checked by no load balancer and publishes no series at
//! all, which is what a reader of such a group's series finds.
//!
//! The datapoints go into the CloudWatch emulator's OWN metric store — the same store
//! `PutMetricData` writes and `GetMetricStatistics` / `GetMetricData` / `ListMetrics` read —
//! reached through the shared engine, so one metric has one home and the publishers cannot
//! drift. They are appended to that store directly rather than sent through `PutMetricData`,
//! which reserves the `AWS/` namespaces for the services that own them ("you should not
//! specify a namespace that begins with `AWS/`", `PutMetricData$Namespace`).
//!
//! # What one check observes
//!
//! A health check reaches the registered target over the network, and this emulator makes no
//! outbound connection — so what one check can observe is whether the emulator itself holds
//! something that stands behind the registration, ON THE PORT the check connects to
//! ([`observe`]). That port is the group's effective health-check port — `HealthCheckPort`
//! when it names one, and the port the target receives traffic on otherwise:
//!
//! * a registration whose workload declares the ports it LISTENS on
//!   ([`super::ServedPorts`], what another emulated service's scheduler registered it with)
//!   fails every check against a port that is not one of them: a workload that is running
//!   answers nothing on a port it does not serve, which is what a health check pointed at
//!   the wrong port finds;
//! * otherwise, a registration another emulated service's SCHEDULER made for a workload it
//!   runs stands for that workload, which is running here, so the check passes;
//! * an `instance` target passes while EC2 — which owns instances — reports that instance
//!   running, and fails for one it reports stopped or holds no longer;
//! * an `ip` target passes while some network interface of the account holds that address,
//!   which is what makes the address one something in this VPC answers on, and fails for an
//!   address no interface holds;
//! * anything else observes NOTHING, and a registration nothing was observed about keeps the
//!   health it has rather than being failed on silence: a `lambda` target, whose function is
//!   another service's resource, and every target of a group whose VPC the account's EC2
//!   inventory does not hold — an inventory that knows nothing about the network the target
//!   sits in can report nothing about the target either ([`super::network`] reads every
//!   other fact this service does not own the same way).
//!
//! The verdict only MOVES once the target group's own thresholds are reached — "the number of
//! consecutive health check failures required before considering a target unhealthy"
//! (`CreateTargetGroup$UnhealthyThresholdCount`) and its healthy counterpart — which
//! [`super::target_health::TargetProbe`] folds the observations into, and a registration whose
//! checks have failed without reaching that threshold is still `initial`.
//!
//! # The clock the series is published on
//!
//! An Application Load Balancer publishes these counts once a MINUTE
//! ([`PUBLISH_INTERVAL`]), so the run-up an alarm over several evaluation periods needs is
//! minutes long, and the first flip of a target the load balancer cannot reach is
//! `UnhealthyThresholdCount` health-check intervals away — neither of which a client is going
//! to sit through against an emulator. So the series is published on an ACCELERATED
//! schedule: its checks come due [`ACCELERATED_SPACING`] apart until the series reaches the
//! present, and one publish interval apart from then on, while every datapoint keeps the
//! one-minute SPACING the real series has. A series therefore opens with
//! [`RUN_UP_PERIODS`] datapoints stamped BEFORE the instant routing began, and an alarm
//! whose evaluation periods reach back over them sees a series that has been running.
//!
//! What makes that observable without any further call is [`drive_host_count_metrics`]: the
//! write that changes what a load balancer routes to, or which targets are registered,
//! publishes what is due at that instant and starts the one bounded in-process task that
//! waits for the next due check. Nothing here is on a READ path, and nothing it publishes
//! depends on who called: the checks it materialises are the ones the stored due instants
//! have already made due. [`check_registrations`] is the one entry point another emulated
//! service's scheduler reaches, for the first check of a registration it has just created:
//! it observes those registrations and nothing else, and publishes no datapoint, so it moves
//! no other registration's probe and no series' schedule.

use ::std::collections::{BTreeMap, HashMap, HashSet};
use ::std::sync::Arc;
use ::std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use super::target_health::{HEALTHY_STATE, UNHEALTHY_STATE};
use super::{
    load_balancer_type_code,
    ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState, LoadBalancerRecord,
    RegisteredTarget, TargetGroupRecord, TargetKey,
};

/// "The number of targets that are considered healthy" — the metric an Elastic Load
/// Balancing namespace reports per target group.
pub const HEALTHY_HOST_COUNT: &str = "HealthyHostCount";

/// "The number of targets that are considered unhealthy", the companion of
/// [`HEALTHY_HOST_COUNT`].
pub const UNHEALTHY_HOST_COUNT: &str = "UnHealthyHostCount";

/// The dimension naming the target group a count is about: its ARN from the resource type
/// onwards (`targetgroup/<name>/<id>`), which is how an Elastic Load Balancing dimension
/// names a target group.
pub const TARGET_GROUP_DIMENSION: &str = "TargetGroup";

/// The dimension naming the load balancer that did the checking: its ARN from the family
/// code onwards (`app/<name>/<id>`), which is how an Elastic Load Balancing dimension names
/// a load balancer.
pub const LOAD_BALANCER_DIMENSION: &str = "LoadBalancer";

/// The `StandardUnit` a host count carries: it is a number of targets.
pub const COUNT_UNIT: &str = "Count";

/// The namespace an Application Load Balancer's metrics live in.
pub const APPLICATION_NAMESPACE: &str = "AWS/ApplicationELB";

/// The namespace a Network Load Balancer's metrics live in.
pub const NETWORK_NAMESPACE: &str = "AWS/NetworkELB";

/// The namespace a Gateway Load Balancer's metrics live in.
pub const GATEWAY_NAMESPACE: &str = "AWS/GatewayELB";

/// How far apart the datapoints of one series are STAMPED: Elastic Load Balancing reports
/// these counts per minute, which is also the finest period an alarm over a regular-resolution
/// metric can be evaluated at.
pub const PUBLISH_INTERVAL: Duration = Duration::from_secs(60);

/// The `StorageResolution` the datapoints are stored at, in seconds: the regular resolution a
/// metric has unless its publisher asks for the high-resolution one ("This field is optional,
/// if you do not specify it the default of 60 is used", `MetricDatum$StorageResolution`).
const STORAGE_RESOLUTION_SECS: i32 = 60;

/// How many datapoints a series OPENS with, stamped one [`PUBLISH_INTERVAL`] apart ahead of
/// the instant the load balancer began routing to the target group.
///
/// The emulator's own figure, not an AWS one: the real series has been running for as long as
/// the routing has, so an alarm created straight after it sees as many periods of data as its
/// `EvaluationPeriods` asks for. Five periods is the run-up of the longest evaluation an
/// `EvaluationPeriods` of one to five asks for, and nothing is instant either way — the
/// datapoints carry the verdicts the checks really reached, in the order they reached them.
const RUN_UP_PERIODS: u32 = 5;

/// How far apart the checks of one series come DUE while its series is still catching up with
/// the present.
///
/// The emulator's own span: at this spacing the run-up of a series opened with the default
/// `UnhealthyThresholdCount` of two takes 500 ms, so a caller reading straight after the
/// write that started the routing still sees the `initial` state and not the verdict.
const ACCELERATED_SPACING: Duration = Duration::from_millis(250);

/// The shortest a publisher-clock wake-up waits, so a check already past its due instant
/// still costs a yield rather than a CPU-bound spin.
const MIN_PUBLISH_WAIT: Duration = Duration::from_millis(50);

/// The longest a publisher-clock wake-up waits, so the clock re-reads the store — which a
/// write may have changed since — at least this often.
const MAX_PUBLISH_WAIT: Duration = Duration::from_secs(5);

/// How many times one publisher clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start, and the next write
/// to a load balancer's routing or registrations starts a clock again if none is running.
const MAX_CLOCK_WAKE_UPS: usize = 2_000;

/// How many registrations ONE pass checks, over every target group the store holds.
///
/// A bound on the work one wake-up does, not a quota: the pass walks the registrations of the
/// routed groups, and a store holding more than this many hands the rest to the next wake-up
/// rather than doing unbounded work while a clock tick is being served.
const MAX_CHECKS_PER_PASS: usize = 2_000;

/// The namespace the host counts of a load balancer of `lb_type` are published into.
///
/// One namespace per FAMILY, as each family's own metric reference names it: the counts of an
/// Application Load Balancer are not the counts of a Network Load Balancer, and a reader
/// windows exactly one of them.
#[must_use]
pub fn namespace(lb_type: &str) -> &'static str {
    match load_balancer_type_code(lb_type) {
        "net" => NETWORK_NAMESPACE,
        "gwy" => GATEWAY_NAMESPACE,
        _ => APPLICATION_NAMESPACE,
    }
}

/// How an Elastic Load Balancing dimension names the load balancer `record`:
/// `<family>/<name>/<id>`, which is its ARN from the family code onwards.
#[must_use]
pub fn load_balancer_dimension_value(record: &LoadBalancerRecord) -> String {
    format!(
        "{}/{}/{}",
        load_balancer_type_code(&record.lb_type),
        record.name,
        record.id
    )
}

/// How an Elastic Load Balancing dimension names the target group `arn` identifies:
/// `targetgroup/<name>/<id>`, which is that ARN from the resource type onwards.
///
/// `None` for a value carrying no target-group resource field, which names no target group
/// for a dimension to be about.
#[must_use]
pub fn target_group_dimension_value(arn: &str) -> Option<String> {
    let marker = ":targetgroup/";
    let at = arn.find(marker)?;
    Some(arn[at + 1..].to_string())
}

/// One series' place in its schedule: the stamp its next datapoint carries, and when that
/// datapoint comes due.
#[derive(Debug)]
struct SeriesClock {
    /// The time stamp the next datapoint carries. Advances by [`PUBLISH_INTERVAL`] per
    /// check, so the series keeps the spacing the real one has.
    next_stamp: SystemTime,
    /// The instant the next check comes due; see [`ACCELERATED_SPACING`].
    next_check_at: Instant,
}

/// Where every host-count series of this store has got to, and the one in-process clock that
/// advances them.
///
/// Held on the store so the schedule is the store's own: a series exists while the pair it
/// belongs to is routed, and the clock is claimed once however many writes drive it.
#[derive(Debug, Default)]
pub struct HostCountPublisher {
    /// The schedule of each `(load balancer ARN, target group ARN)` pair being checked.
    series: HashMap<(String, String), SeriesClock>,
    /// Whether a clock is running for this store.
    clock_running: bool,
    /// Which claim the running clock holds, so a clock that has stood down cannot release a
    /// claim a later one took.
    clock_generation: u64,
    /// The running clock's task, held so the task a request started stays stoppable.
    clock: Option<::tokio::task::JoinHandle<()>>,
}

/// One registration a pass is about to check: where it is stored, and what it stands for.
#[derive(Debug, Clone)]
struct Checked {
    /// The target group holding the registration.
    target_group_arn: String,
    /// The registration's key in that group.
    key: TargetKey,
    /// The group's `TargetType`, which says what the id names.
    target_type: String,
    /// The VPC the group's targets live in, which is the inventory a check reads.
    vpc_id: Option<String>,
    /// The registered target's id: an instance id, an address, or a function ARN.
    id: String,
    /// Whether another emulated service's scheduler owns this registration.
    scheduled: bool,
    /// Whether the workload behind the registration listens on the port this check
    /// CONNECTS to — the group's effective health-check port, which is `HealthCheckPort`
    /// when it names one and the port the target receives traffic on otherwise
    /// (`TargetGroupRecord::health_check_port_of`).
    ///
    /// `None` when that is not known: a registration whose workload declares no ports
    /// ([`super::ServedPorts`]), or a health check whose port no number names.
    serves_health_check_port: Option<bool>,
}

/// What one check observed about a registration: `Some(true)` for one something in this
/// emulator stands behind, `Some(false)` for one nothing does, and `None` when nothing was
/// observed at all.
type Observation = Option<bool>;

/// Run ONE check of exactly the registrations `registrations` names — each a
/// `(target group ARN, registration key)` of this store — and fold what each check observed
/// into that registration's own probe.
///
/// What [`drive_host_count_metrics`] does on the writes this service's OWN API makes, for a
/// registration ANOTHER emulated service's scheduler has just created through the state seam
/// (`super::TargetGroupRecord::register_for`): the first check of a registration is owed by
/// the write that created it, so the verdict its checks reach does not wait for a clock
/// wake-up and a registration whose checks fail is never reported healthy on the way to one.
/// The periodic checking is the clock's, which the create of the listener that routes to the
/// group started.
///
/// NOTHING else is touched: no other registration is observed, no series is opened or closed
/// and no datapoint is published, so a call made while materialising one new registration
/// cannot advance any other registration's probe or any series' schedule. A registration of a
/// target group no load balancer forwards to is checked by nothing and is skipped here too.
///
/// Called with NO guard of this store held: it takes the lock itself.
pub fn check_registrations<Eng: Send + Sync + 'static>(
    engine: &Eng,
    registrations: &[(String, TargetKey)],
) {
    if registrations.is_empty() {
        return;
    }
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let checked = {
        let store = state.lock();
        named_registrations(&store, registrations)
    };
    let observations = observe_all(engine, &checked);
    fold_observations(&mut state.lock(), &observations);
}

/// Publish every host-count datapoint the clock has already made due, and start the one
/// bounded in-process task that waits for the next one.
///
/// Called by the writes that change WHAT a load balancer routes to or WHICH targets are
/// registered with a group it routes to — never by a read: the checks this materialises are
/// the ones the stored due instants have already made due, so what a caller sees does not
/// depend on whether anybody called.
///
/// With no async runtime to spawn onto there is no clock, and the checks due at the moment of
/// the write are then the whole series until the next write — the router always runs on one.
pub fn drive_host_count_metrics<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    publish_due_checks(&**engine);
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_clock(&mut state.lock()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a clock that ran out of
        // work has already released its claim inside the one critical section that observed
        // it.
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let Some(wait) = next_wait_or_release(&mut state.lock(), generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps a
            // due check from costing a CPU-bound spin, and it hands the runtime back
            // between wake-ups.
            ::tokio::time::sleep(wait).await;
            publish_due_checks(&*engine);
        }
        if capped {
            // Only THIS clock's claim, so a clock started since keeps its own.
            release_clock(&mut state.lock(), generation);
        }
    });
    set_clock(&mut claimed.lock(), generation, handle);
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood
/// down installs nothing and aborts nothing.
fn set_clock(store: &mut ElbState, generation: u64, handle: ::tokio::task::JoinHandle<()>) {
    if store.host_counts.clock_generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = store.host_counts.clock.replace(handle) {
        previous.abort();
    }
}

/// Claim the single publisher clock for this store: the claim's GENERATION when the caller
/// may start one, `None` when one already runs.
fn claim_clock(store: &mut ElbState) -> Option<u64> {
    if store.host_counts.clock_running {
        return None;
    }
    store.host_counts.clock_running = true;
    store.host_counts.clock_generation = store.host_counts.clock_generation.wrapping_add(1);
    Some(store.host_counts.clock_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock that ended on the
/// wake-up cap cannot clear a claim a clock started since is holding.
fn release_clock(store: &mut ElbState, generation: u64) {
    if store.host_counts.clock_generation == generation {
        store.host_counts.clock_running = false;
    }
}

/// How long the clock waits before its next pass, or `None` — having RELEASED its claim —
/// when the store holds no series to publish.
///
/// Observing "nothing is owed" and standing down happen in ONE critical section, which is
/// what makes the hand-over race-free: a write holds this same lock to store the routing
/// BEFORE it tries to claim the clock, so either this call sees that series and the running
/// clock keeps it, or the write's claim finds the flag cleared and starts a clock of its own.
fn next_wait_or_release(store: &mut ElbState, generation: u64) -> Option<Duration> {
    let now = Instant::now();
    let earliest = store
        .host_counts
        .series
        .values()
        .map(|series| series.next_check_at)
        .min();
    // A registration whose checks have FAILED without reaching the unhealthy verdict yet is
    // checked again at the ACCELERATED spacing, however far off the next datapoint of the
    // series it is counted into is: the verdict is a number of CHECKS away
    // (`UnhealthyThresholdCount` of them), so it is the check spacing that decides how long
    // such a registration sits in `initial` rather than the publish schedule.
    let failing = a_registration_is_failing(store);
    match earliest {
        Some(due) => {
            let wait = due.saturating_duration_since(now);
            let wait = if failing {
                wait.min(ACCELERATED_SPACING)
            } else {
                wait
            };
            Some(wait.clamp(MIN_PUBLISH_WAIT, MAX_PUBLISH_WAIT))
        }
        None => {
            release_clock(store, generation);
            None
        }
    }
}

/// Whether some registration a load balancer of this store checks has a failing check and no
/// verdict yet ([`super::TargetProbe::is_failing`]).
fn a_registration_is_failing(store: &ElbState) -> bool {
    store.target_groups.values().any(|group| {
        !group.load_balancer_arns.is_empty()
            && group.targets.values().any(|target| target.probe.is_failing())
    })
}

/// Check every registration of a routed target group, fold what the checks observed into the
/// store, and publish the datapoint each due series is reported as.
///
/// Four steps, so no store guard is held over another service's lock and the stores are never
/// locked at once:
///
/// 1. under THIS service's guard, open a series for every pair that is routed and close the
///    ones that are not, and snapshot the registrations to check;
/// 2. with NO guard held, read what the account's EC2 inventory holds and compute every
///    observation;
/// 3. under this service's guard again, fold the observations into the registrations and take
///    the datapoints the due series are published as;
/// 4. with the metric store's guard, append them and make the alarms over them due.
fn publish_due_checks<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    let instant = Instant::now();
    let checked = {
        let mut store = state.lock();
        sync_series(&mut store, now, instant);
        registrations_to_check(&store)
    };
    let observations = observe_all(engine, &checked);
    let published = {
        let mut store = state.lock();
        fold_observations(&mut store, &observations);
        due_datapoints(&mut store, now, instant)
    };
    append_datapoints(engine, published, now);
}

/// Open a series for every `(load balancer, target group)` pair the store now routes, and
/// close the series of every pair it no longer does.
///
/// A series opens with its run-up already behind it ([`RUN_UP_PERIODS`]) and its first check
/// due at once, so the write that started the routing publishes the series' first datapoints
/// itself.
fn sync_series(store: &mut ElbState, now: SystemTime, instant: Instant) {
    let mut routed: HashSet<(String, String)> = HashSet::new();
    for (target_group_arn, group) in &store.target_groups {
        for load_balancer_arn in &group.load_balancer_arns {
            if store.load_balancers.contains_key(load_balancer_arn) {
                routed.insert((load_balancer_arn.clone(), target_group_arn.clone()));
            }
        }
    }
    store
        .host_counts
        .series
        .retain(|pair, _| routed.contains(pair));
    let run_up = PUBLISH_INTERVAL.saturating_mul(RUN_UP_PERIODS);
    for pair in routed {
        store
            .host_counts
            .series
            .entry(pair)
            .or_insert_with(|| SeriesClock {
                next_stamp: now.checked_sub(run_up).unwrap_or(now),
                next_check_at: instant,
            });
    }
}

/// Every registration of a target group some load balancer of this store routes to, up to
/// [`MAX_CHECKS_PER_PASS`].
fn registrations_to_check(store: &ElbState) -> Vec<Checked> {
    let mut checked: Vec<Checked> = Vec::new();
    for (target_group_arn, group) in &store.target_groups {
        if group.load_balancer_arns.is_empty() {
            continue;
        }
        for (key, target) in &group.targets {
            if checked.len() >= MAX_CHECKS_PER_PASS {
                return checked;
            }
            checked.push(checked_registration(target_group_arn, group, key, target));
        }
    }
    checked
}

/// The registrations `registrations` names, as the checks of them this store would run: one
/// per `(target group ARN, key)` that is still held by a target group a load balancer
/// forwards to, and up to [`MAX_CHECKS_PER_PASS`] of them.
///
/// A pair naming a group or a registration this store no longer holds contributes nothing: a
/// deregistration may have taken it out since the caller read it.
fn named_registrations(store: &ElbState, registrations: &[(String, TargetKey)]) -> Vec<Checked> {
    registrations
        .iter()
        .take(MAX_CHECKS_PER_PASS)
        .filter_map(|(target_group_arn, key)| {
            let group = store.target_groups.get(target_group_arn)?;
            if group.load_balancer_arns.is_empty() {
                return None;
            }
            let target = group.targets.get(key)?;
            Some(checked_registration(target_group_arn, group, key, target))
        })
        .collect()
}

/// What one check of the registration `key` of `group` reads: where it is stored, what it
/// stands for, and the port the check connects to.
fn checked_registration(
    target_group_arn: &str,
    group: &TargetGroupRecord,
    key: &TargetKey,
    target: &RegisteredTarget,
) -> Checked {
    Checked {
        target_group_arn: target_group_arn.to_string(),
        key: key.clone(),
        target_type: group.target_type.clone(),
        vpc_id: group.vpc_id.clone(),
        id: target.id.clone(),
        scheduled: target.registered_for.is_some(),
        // The port this group's check connects to for THIS registration, read against the
        // ports the workload behind it serves.
        serves_health_check_port: group
            .health_check_port_of(target)
            .and_then(|port| port.parse::<i32>().ok())
            .and_then(|port| target.serves(port)),
    }
}

/// What one check of each registration in `checked` observed.
///
/// The account's EC2 inventory is read ONCE for the whole pass — the VPCs it holds, the
/// addresses its network interfaces hold and the instances it reports running — so the cost
/// of a pass does not grow with the number of registrations times the size of the inventory.
fn observe_all<Eng: Send + Sync + 'static>(
    engine: &Eng,
    checked: &[Checked],
) -> Vec<(String, TargetKey, bool)> {
    if checked.is_empty() {
        return Vec::new();
    }
    let inventory = ec2::service_state(engine);
    let held: Option<Inventory> = inventory.as_ref().map(|state| {
        let store = state.lock();
        Inventory {
            vpcs: store.vpcs.keys().cloned().collect(),
            addresses: store
                .network_interfaces
                .values()
                .flat_map(interface_addresses)
                .collect(),
            running: store
                .instances
                .values()
                .filter(|instance| instance_is_running(instance))
                .map(|instance| instance.instance_id.clone())
                .collect(),
        }
    });
    checked
        .iter()
        .filter_map(|target| {
            observe(target, held.as_ref()).map(|observed| {
                (
                    target.target_group_arn.clone(),
                    target.key.clone(),
                    observed,
                )
            })
        })
        .collect()
}

/// What the account's EC2 inventory holds that a health check reads: the VPCs of the
/// account, the addresses its network interfaces hold, and the instances it reports running.
#[derive(Debug, Default)]
struct Inventory {
    /// Every `VpcId` the inventory holds.
    vpcs: HashSet<String>,
    /// Every private IPv4 and IPv6 address a network interface of the account holds.
    addresses: HashSet<String>,
    /// Every instance the inventory reports running.
    running: HashSet<String>,
}

/// What one check of `target` observed, given what the account's EC2 inventory holds — see
/// the module docs.
///
/// A check reads the inventory of the VPC the target group belongs to, so an inventory
/// holding no such VPC is one nothing can be observed in: a verdict is never assumed from
/// silence, which is how this service reads every other fact it does not own
/// ([`super::network`]).
fn observe(target: &Checked, held: Option<&Inventory>) -> Observation {
    if target.serves_health_check_port == Some(false) {
        // The check connects to the group's effective health-check port, and the workload
        // behind this registration does not listen on it: nothing answers there, whatever
        // else stands behind the registration. This is the one condition a check can
        // observe about the PORT, and it is read before what stands behind the target,
        // because a workload that is running answers nothing on a port it does not serve.
        return Some(false);
    }
    if target.scheduled {
        // The workload another service's scheduler registered this target for runs in this
        // emulator, and that is what stands behind the registration.
        return Some(true);
    }
    let held = held?;
    if !held.vpcs.contains(target.vpc_id.as_deref()?) {
        return None;
    }
    match target.target_type.as_str() {
        "ip" => Some(held.addresses.contains(&target.id)),
        "instance" => Some(held.running.contains(&target.id)),
        // A `lambda` target is a function of another service, and a target type this
        // service does not read is one no check here observes anything about.
        _ => None,
    }
}

/// Every private IPv4 and IPv6 address one network interface holds.
fn interface_addresses(interface: &ec2_types::NetworkInterface) -> Vec<String> {
    let private = interface
        .private_ip_addresses
        .iter()
        .flatten()
        .filter_map(|address| address.private_ip_address.clone());
    let ipv6 = interface
        .ipv6_addresses
        .iter()
        .flatten()
        .filter_map(|address| address.ipv6_address.clone());
    interface
        .private_ip_address
        .clone()
        .into_iter()
        .chain(private)
        .chain(ipv6)
        .collect()
}

/// Whether EC2 reports this instance as one that is running, which is the state an instance
/// serving traffic is in.
fn instance_is_running(instance: &ec2::InstanceRecord) -> bool {
    instance.state == ec2::InstanceStateName::Running
}

/// Fold what the checks observed into the registrations they were about.
///
/// A registration the store no longer holds is skipped: the pass ran with no guard held, so a
/// deregistration may have taken it out in the meantime.
fn fold_observations(store: &mut ElbState, observations: &[(String, TargetKey, bool)]) {
    for (target_group_arn, key, observed) in observations {
        let Some(group) = store.target_groups.get_mut(target_group_arn) else {
            continue;
        };
        let (healthy_threshold, unhealthy_threshold) = (
            group.health_check.healthy_threshold,
            group.health_check.unhealthy_threshold,
        );
        if let Some(target) = group.targets.get_mut(key) {
            target
                .probe
                .observed(*observed, healthy_threshold, unhealthy_threshold);
        }
    }
}

/// Take the datapoints every series that is due at `instant` is published as, and advance
/// each of those series to its next check.
fn due_datapoints(
    store: &mut ElbState,
    now: SystemTime,
    instant: Instant,
) -> Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> {
    let due: Vec<(String, String)> = store
        .host_counts
        .series
        .iter()
        .filter(|(_, series)| instant >= series.next_check_at)
        .map(|(pair, _)| pair.clone())
        .collect();
    let mut published: Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)> =
        Vec::with_capacity(due.len() * 2);
    for pair in due {
        let (load_balancer_arn, target_group_arn) = &pair;
        let Some(load_balancer) = store.load_balancers.get(load_balancer_arn) else {
            continue;
        };
        let (metric_namespace, load_balancer_value) = (
            namespace(&load_balancer.lb_type),
            load_balancer_dimension_value(load_balancer),
        );
        let Some(target_group_value) = target_group_dimension_value(target_group_arn) else {
            continue;
        };
        let Some(group) = store.target_groups.get_mut(target_group_arn) else {
            continue;
        };
        // What the clock already owes the group: a registration whose draining span is over
        // has left it, so it is counted in neither host count.
        group.settle_draining(now);
        let (healthy, unhealthy) = group.host_counts(now);
        let Some(series) = store.host_counts.series.get_mut(&pair) else {
            continue;
        };
        let stamp = advance(series);
        for (metric_name, count) in [
            (HEALTHY_HOST_COUNT, healthy),
            (UNHEALTHY_HOST_COUNT, unhealthy),
        ] {
            let dimensions: BTreeMap<String, String> = [
                (
                    TARGET_GROUP_DIMENSION.to_string(),
                    target_group_value.clone(),
                ),
                (
                    LOAD_BALANCER_DIMENSION.to_string(),
                    load_balancer_value.clone(),
                ),
            ]
            .into_iter()
            .collect();
            published.push((
                cloudwatch::MetricKey {
                    namespace: metric_namespace.to_string(),
                    metric_name: metric_name.to_string(),
                    dimensions,
                },
                cloudwatch::StoredDatum {
                    timestamp_secs: epoch_secs(stamp),
                    // One observation: this check's own count of the group's targets.
                    sample_count: 1.0,
                    sum: count,
                    minimum: count,
                    maximum: count,
                    unit: Some(COUNT_UNIT.to_string()),
                    storage_resolution: STORAGE_RESOLUTION_SECS,
                },
            ));
        }
    }
    published
}

/// Take the time stamp of the check that is due and schedule the next one.
fn advance(series: &mut SeriesClock) -> SystemTime {
    let stamp = series.next_stamp;
    series.next_stamp = stamp.checked_add(PUBLISH_INTERVAL).unwrap_or(stamp);
    // While the series is still behind the present it is published at the accelerated
    // spacing; once it has caught up, the next check is due when the clock reaches its
    // stamp, which is one publish interval apart. Never longer than one interval: that is
    // the most the schedule can owe, so a clock that has stepped backwards costs one slow
    // check rather than an unbounded wait.
    let wait = series
        .next_stamp
        .duration_since(SystemTime::now())
        .unwrap_or(ACCELERATED_SPACING)
        .min(PUBLISH_INTERVAL);
    series.next_check_at = Instant::now()
        .checked_add(wait)
        .unwrap_or_else(Instant::now);
    stamp
}

/// Append the datapoints this pass published to the metric store, and make every alarm over
/// the series they belong to due for evaluation.
///
/// Nothing is published on an engine that holds no metric store to reach: a datapoint has one
/// home, and this path never makes a second one. An alarm is evaluated FROM these datapoints,
/// so storing a series it watches is what makes its evaluation due — an alarm cannot tell a
/// service's own metric from one a client published.
fn append_datapoints<Eng: Send + Sync + 'static>(
    engine: &Eng,
    published: Vec<(cloudwatch::MetricKey, cloudwatch::StoredDatum)>,
    now: SystemTime,
) {
    if published.is_empty() {
        return;
    }
    let Some(state) = cloudwatch::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    let mut written: Vec<cloudwatch::WrittenSeries> = Vec::with_capacity(published.len());
    for (key, datum) in published {
        let watched = key.clone();
        let unit = datum.unit().to_string();
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The routing itself is already stored,
            // so nothing is failed for it; the dropped count is reported where a developer
            // can see it.
            ::tracing::warn!(
                metric = watched.metric_name.as_str(),
                "the CloudWatch metric store is at capacity, so this host count was dropped"
            );
            continue;
        }
        written.push(cloudwatch::WrittenSeries {
            key: watched,
            units: vec![unit],
        });
    }
    store.make_alarms_due(&written, epoch_secs(now));
}

/// `at` as the epoch seconds a stored datapoint carries, which is the unit the metric store's
/// own readers window by. An instant before the epoch (which no clock reports) reads as the
/// epoch itself rather than failing the publish.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

impl super::TargetGroupRecord {
    /// How many of this group's registrations the load balancer considers healthy, and how
    /// many it considers unhealthy, at `now`.
    ///
    /// Counted off the very health [`super::TargetGroupRecord::health_of`] reports, so the
    /// series and `DescribeTargetHealth` cannot disagree about one registration. A
    /// registration in neither state — one still coming up, draining, or in a group nothing
    /// checks — is counted in neither total, which is what "considered healthy" and
    /// "considered unhealthy" leave out.
    #[must_use]
    pub fn host_counts(&self, now: SystemTime) -> (f64, f64) {
        let mut healthy = 0u32;
        let mut unhealthy = 0u32;
        for target in self.targets.values() {
            match self.health_of(target, now).state.as_deref() {
                Some(HEALTHY_STATE) => healthy += 1,
                Some(UNHEALTHY_STATE) => unhealthy += 1,
                _ => {}
            }
        }
        (f64::from(healthy), f64::from(unhealthy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a dimension names each resource by its ARN from the resource type onwards, and
    /// the namespace follows the load balancer's FAMILY.
    ///
    /// The rows span the three families a load balancer can be of and the ARN forms a
    /// dimension is read off, including a value that names no target group at all.
    #[test]
    fn a_series_is_addressed_by_family_and_arn_suffix() {
        for (case, lb_type, expected_namespace) in [
            ("an application load balancer", "application", APPLICATION_NAMESPACE),
            ("a network load balancer", "network", NETWORK_NAMESPACE),
            ("a gateway load balancer", "gateway", GATEWAY_NAMESPACE),
            ("a type no family names", "", APPLICATION_NAMESPACE),
        ] {
            let found = namespace(lb_type);
            assert_eq!(
                found, expected_namespace,
                "case {case:?}: expected {expected_namespace}, got {found}"
            );
            let record = LoadBalancerRecord {
                name: "lb".to_string(),
                id: "0123456789abcdef".to_string(),
                lb_type: lb_type.to_string(),
                ..probe_load_balancer()
            };
            let value = load_balancer_dimension_value(&record);
            let expected = format!("{}/lb/0123456789abcdef", load_balancer_type_code(lb_type));
            assert_eq!(
                value, expected,
                "case {case:?}: expected {expected}, got {value}"
            );
        }

        for (case, arn, expected) in [
            (
                "a target group ARN",
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:targetgroup/tg/abc123",
                Some("targetgroup/tg/abc123"),
            ),
            (
                "a load balancer ARN",
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/lb/abc123",
                None,
            ),
            ("a value that is no ARN", "targetgroup", None),
        ] {
            let found = target_group_dimension_value(arn);
            assert_eq!(
                found.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {found:?}"
            );
        }
    }

    /// A load balancer record with the members these tests do not vary.
    fn probe_load_balancer() -> LoadBalancerRecord {
        LoadBalancerRecord {
            arn: String::new(),
            id: String::new(),
            name: String::new(),
            region: "us-east-1".to_string(),
            dns_name: String::new(),
            created_time: SystemTime::UNIX_EPOCH,
            scheme: "internal".to_string(),
            vpc_id: None,
            state_code: "active".to_string(),
            active_at: SystemTime::UNIX_EPOCH,
            lb_type: "application".to_string(),
            availability_zones: Vec::new(),
            security_groups: Vec::new(),
            ip_address_type: "ipv4".to_string(),
            customer_owned_ipv4_pool: None,
            enforce_security_group_inbound_rules_on_private_link_traffic: None,
            enable_prefix_for_ipv6_source_nat: None,
            ipv4_ipam_pool_id: None,
            capacity_reservation_modified_at: None,
            network_interface_ids: Vec::new(),
            attributes: HashMap::new(),
            tags: HashMap::new(),
            listener_arns: Vec::new(),
        }
    }

    /// RULE: what one check observes is whether the emulator holds something that stands
    /// behind the registration AND answers on the port the check connects to, and nothing
    /// at all where there is no inventory to read.
    ///
    /// The rows span the target types a check reads, a registration another service's
    /// scheduler owns, whether the workload behind it listens on the health-check port, and
    /// an account whose inventory does not hold the group's VPC.
    #[test]
    fn a_check_observes_only_what_the_emulator_holds() {
        let held = Inventory {
            vpcs: HashSet::from(["vpc-0123456789abcdef0".to_string()]),
            addresses: HashSet::from(["10.0.1.7".to_string()]),
            running: HashSet::from(["i-0123456789abcdef0".to_string()]),
        };
        for (case, target_type, id, scheduled, serves, expected) in [
            ("an address an interface holds", "ip", "10.0.1.7", false, None, Some(true)),
            (
                "an address no interface holds",
                "ip",
                "10.0.1.9",
                false,
                None,
                Some(false),
            ),
            (
                "a running instance",
                "instance",
                "i-0123456789abcdef0",
                false,
                None,
                Some(true),
            ),
            (
                "an instance the account does not hold",
                "instance",
                "i-00000000000000000",
                false,
                None,
                Some(false),
            ),
            (
                "a function of another service",
                "lambda",
                "arn:aws:lambda:us-east-1:000000000000:function:f",
                false,
                None,
                None,
            ),
            (
                "a registration a scheduler owns",
                "ip",
                "10.0.9.9",
                true,
                None,
                Some(true),
            ),
            (
                "a scheduler's registration whose workload listens on the checked port",
                "ip",
                "10.0.9.9",
                true,
                Some(true),
                Some(true),
            ),
            (
                "a scheduler's registration whose workload does not listen on the \
                 checked port",
                "ip",
                "10.0.9.9",
                true,
                Some(false),
                Some(false),
            ),
            (
                "an address an interface holds, checked on a port nothing behind it serves",
                "ip",
                "10.0.1.7",
                false,
                Some(false),
                Some(false),
            ),
        ] {
            let checked = Checked {
                target_group_arn: "tg".to_string(),
                key: (id.to_string(), Some(443)),
                target_type: target_type.to_string(),
                vpc_id: Some("vpc-0123456789abcdef0".to_string()),
                id: id.to_string(),
                scheduled,
                serves_health_check_port: serves,
            };
            let observed = observe(&checked, Some(&held));
            assert_eq!(
                observed, expected,
                "case {case:?}: expected {expected:?}, got {observed:?}"
            );
            // With no inventory to read, and with an inventory that holds no such VPC,
            // only a scheduler's own registration is observed — and a check against a port
            // nothing serves fails whatever the inventory holds.
            let expected_unknown = if serves == Some(false) {
                Some(false)
            } else {
                scheduled.then_some(true)
            };
            let without = observe(&checked, None);
            assert_eq!(
                without, expected_unknown,
                "case {case:?} with no EC2 inventory: expected {expected_unknown:?}, got \
                 {without:?}"
            );
            let elsewhere = Checked {
                vpc_id: Some("vpc-00000000000000000".to_string()),
                ..checked.clone()
            };
            let foreign = observe(&elsewhere, Some(&held));
            assert_eq!(
                foreign, expected_unknown,
                "case {case:?} in a VPC the inventory does not hold: expected \
                 {expected_unknown:?}, got {foreign:?}"
            );
        }
    }
}
