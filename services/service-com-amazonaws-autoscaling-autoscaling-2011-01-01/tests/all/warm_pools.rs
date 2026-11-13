// SPDX-License-Identifier: CC0-1.0

//! The warm pool of an Auto Scaling group over the awsQuery wire: `PutWarmPool`,
//! `DescribeWarmPool`, the group's own reader, and the delete that takes the pool with
//! the group.
//!
//! The rule under test: a warm pool is a SINGLE CHILD of one Auto Scaling group, so
//! `PutWarmPool` is an upsert over that group's own record — a group with no pool has one
//! CREATED with the documented defaults for every member the request omits (`MinSize` 0,
//! `PoolState` Stopped, no prepared capacity), a group that has one has it UPDATED per
//! member (an omitted member keeps the stored value, and `MaxGroupPreparedCapacity` -1
//! clears the stored one) — and the pool it stores HOLDS instances: the account's own
//! Amazon EC2 instances, maintained at the size the model documents (the difference
//! between the prepared capacity, or the group's maximum when none is set, and the
//! group's desired capacity, with the pool's own minimum as the floor), each reported
//! under the pool's `Warmed:` state once its transition is due. The pool's readers report
//! exactly what is stored, and deleting the group takes the pool and its instances with
//! it.
//!
//! The tables below span what that rule generalises over: which members a request
//! carries, whether the call creates or updates, every published `PoolState`, both forms
//! of `InstanceReusePolicy`, a prepared capacity narrower than the group's maximum and
//! one below its desired capacity, the documented -1, the pool's minimum as the binding
//! floor, how the group is placed (by subnet or by Availability Zone alone), the group's
//! capacity changing under a stored pool, the published bounds of `MaxRecords` and one
//! past each, and the refusals a bad request earns. Every row reads the pool back through
//! `DescribeWarmPool` and resolves every id it reports in the account's own Amazon EC2
//! state, so no row can pass on an accepted-but-unstored change or on an id that names no
//! instance.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    borrowed, ec2_query, p, query, seed_subnets, tag_value, tag_values, xml_unescape, REGION,
};

/// How often a poll re-reads the warm pool.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: the lifecycle span this service owes
/// a transition and the one Amazon EC2 owes, with ample margin for a loaded test machine.
/// Derived from the services' own spans rather than written out, so the three cannot
/// drift apart.
fn poll_budget() -> Duration {
    autoscaling::state::LIFECYCLE_TRANSITION.max(ec2::TRANSITION_DELAY) + Duration::from_secs(10)
}

/// The states a warm pool may transition its instances to, from the model's own
/// `com.amazonaws.autoscaling#WarmPoolState` enum
/// (`smithy_query(query="shape",shape="WarmPoolState")`): `Stopped | Running |
/// Hibernated`.
const POOL_STATES: [&str; 3] = ["Stopped", "Running", "Hibernated"];

/// `PoolState` for a pool created without one, from the model's own documentation for
/// `PutWarmPoolType$PoolState` ("Default is Stopped").
const DEFAULT_POOL_STATE: &str = "Stopped";

/// `MinSize` for a pool created without one, from the model's own documentation for
/// `PutWarmPoolType$MinSize` ("Defaults to 0 if not specified").
const DEFAULT_MIN_SIZE: i32 = 0;

/// The `MaxGroupPreparedCapacity` that removes a stored one, from the model's own
/// documentation for that member ("To remove a value that you previously set, include the
/// property but specify -1 for the value").
const PREPARED_CAPACITY_UNSET: i32 = -1;

/// The published bounds of `DescribeWarmPool`'s `MaxRecords`: a page holds at least one
/// instance, and "the maximum value is 50"
/// (`smithy_query(query="doc",shape="DescribeWarmPoolType",member="MaxRecords")`).
const MIN_MAX_RECORDS: i32 = 1;
const MAX_MAX_RECORDS: i32 = 50;

/// The `LifecycleState` an instance of a warm pool is reported in once the pool's own
/// state is due: the pool state under the prefix the model's `LifecycleState` enum
/// spells warm-pool states with (`Warmed:Stopped`, `Warmed:Running`,
/// `Warmed:Hibernated`).
fn warmed(pool_state: &str) -> String {
    format!("Warmed:{pool_state}")
}

/// The Amazon EC2 instance state a warm pool in `pool_state` keeps its instances in:
/// only `Running` leaves them running, since the other two published states
/// (`Stopped`, `Hibernated`) are states an instance is stopped in — which is what a
/// pre-initialized pool instance costs nothing to keep means.
fn expected_ec2_state(pool_state: &str) -> &'static str {
    if pool_state == "Running" {
        "running"
    } else {
        "stopped"
    }
}

/// One emulator with BOTH services on one engine: a warm pool's instances live in the
/// same account's Amazon EC2 store, so one engine is what makes the two views one
/// account.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Create one Amazon EC2 launch template over the wire, so the instances a warm pool
/// launches have a launch source of the account to run.
async fn create_launch_template(router: &MultiServiceRouter, name: &str) {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", "ami-00000000000000041"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
}

/// What one `PutWarmPool` request carries beyond the group's name.
#[derive(Debug, Clone, Copy, Default)]
struct Put {
    min_size: Option<i32>,
    prepared_capacity: Option<i32>,
    pool_state: Option<&'static str>,
    reuse_on_scale_in: Option<bool>,
}

impl Put {
    /// This request's own parameters, as the wire names them.
    fn params(self) -> Vec<(String, String)> {
        let mut params = Vec::new();
        if let Some(min_size) = self.min_size {
            params.push(p("MinSize", min_size.to_string()));
        }
        if let Some(prepared) = self.prepared_capacity {
            params.push(p("MaxGroupPreparedCapacity", prepared.to_string()));
        }
        if let Some(pool_state) = self.pool_state {
            params.push(p("PoolState", pool_state));
        }
        if let Some(reuse) = self.reuse_on_scale_in {
            params.push(p(
                "InstanceReusePolicy.ReuseOnScaleIn",
                if reuse { "true" } else { "false" },
            ));
        }
        params
    }
}

/// Post one `PutWarmPool` for `group`, answering the status and body.
async fn put_warm_pool(
    router: &MultiServiceRouter,
    group: &str,
    put: Put,
) -> (u16, String) {
    let mut params = vec![p("AutoScalingGroupName", group)];
    params.extend(put.params());
    query(router, "PutWarmPool", &borrowed(&params)).await
}

/// One group, created with a launch template of the account and the placement and bounds
/// a row asks for.
async fn create_group(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    placement: &[(String, String)],
    min_size: i32,
    max_size: i32,
    desired_capacity: i32,
) {
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", template),
        p("MinSize", min_size.to_string()),
        p("MaxSize", max_size.to_string()),
        p("DesiredCapacity", desired_capacity.to_string()),
    ];
    params.extend_from_slice(placement);
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
}

/// The warm pool as `DescribeWarmPool` reports it: the configuration members as the wire
/// spelled them (`""` for one the answer omits), and the instances with their states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReportedPool {
    min_size: String,
    prepared_capacity: String,
    pool_state: String,
    status: String,
    reuse_on_scale_in: String,
    instance_ids: Vec<String>,
    lifecycle_states: Vec<String>,
}

/// Read one answer body into [`ReportedPool`].
fn reported_pool(body: &str) -> ReportedPool {
    let configuration = tag_value(body, "WarmPoolConfiguration");
    ReportedPool {
        min_size: tag_value(&configuration, "MinSize"),
        prepared_capacity: tag_value(&configuration, "MaxGroupPreparedCapacity"),
        pool_state: tag_value(&configuration, "PoolState"),
        status: tag_value(&configuration, "Status"),
        reuse_on_scale_in: tag_value(
            &tag_value(&configuration, "InstanceReusePolicy"),
            "ReuseOnScaleIn",
        ),
        instance_ids: tag_values(body, "InstanceId"),
        lifecycle_states: tag_values(body, "LifecycleState"),
    }
}

/// Read `group`'s warm pool once.
async fn describe_warm_pool(router: &MultiServiceRouter, group: &str) -> (u16, String) {
    query(
        router,
        "DescribeWarmPool",
        &[("AutoScalingGroupName", group)],
    )
    .await
}

/// Read `group`'s warm pool until it holds `wanted` instances and every one of them
/// reports `state`, or a generous deadline passes — the CONDITION, never a fixed wait.
async fn settled_pool(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    wanted: usize,
    state: &str,
) -> ReportedPool {
    let deadline = Instant::now() + poll_budget();
    loop {
        let (status, body) = describe_warm_pool(router, group).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeWarmPool expected 200, got {status}: {body}"
        );
        let pool = reported_pool(&body);
        let settled = pool.instance_ids.len() == wanted
            && pool.lifecycle_states.len() == wanted
            && pool.lifecycle_states.iter().all(|held| held == state);
        if settled || Instant::now() >= deadline {
            return pool;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What Amazon EC2 reports for ONE instance id: its state, or `None` when EC2 resolves
/// the id to no instance at all.
async fn ec2_instance_state(router: &MultiServiceRouter, instance_id: &str) -> Option<String> {
    let (status, body) =
        ec2_query(router, "DescribeInstances", &[p("InstanceId.1", instance_id)]).await;
    if status != 200 {
        return None;
    }
    let state = tag_value(&tag_value(&body, "instanceState"), "name");
    (!state.is_empty()).then_some(state)
}

/// Wait until Amazon EC2 reports `instance_id` in `state`, or a generous deadline passes;
/// answers what it last reported.
async fn ec2_instance_settled(
    router: &MultiServiceRouter,
    instance_id: &str,
    state: &str,
) -> Option<String> {
    let deadline = Instant::now() + poll_budget();
    loop {
        let held = ec2_instance_state(router, instance_id).await;
        if held.as_deref() == Some(state) || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the warm pool table.
struct Row {
    case: &'static str,
    /// How many subnets the group's `VPCZoneIdentifier` names; 0 places it by
    /// Availability Zone alone.
    subnets: usize,
    max_size: i32,
    desired_capacity: i32,
    put: Put,
    /// How many instances the pool must hold, by the documented size rule.
    expected_size: usize,
    /// The state those instances must settle in.
    expected_pool_state: &'static str,
}

/// A warm pool is stored as asked, with the documented defaults for the members the
/// request omits, and holds the instances the documented size rule says it maintains.
#[tokio::test]
async fn a_put_stores_the_pool_and_maintains_its_instances() {
    let rows = [
        Row {
            case: "no member at all: the defaults, and the group's own maximum",
            subnets: 1,
            max_size: 2,
            desired_capacity: 0,
            put: Put::default(),
            expected_size: 2,
            expected_pool_state: DEFAULT_POOL_STATE,
        },
        Row {
            case: "a minimum above the group's own difference, kept running",
            subnets: 2,
            max_size: 1,
            desired_capacity: 1,
            put: Put {
                min_size: Some(2),
                pool_state: Some(POOL_STATES[1]),
                reuse_on_scale_in: Some(true),
                ..Put::default()
            },
            expected_size: 2,
            expected_pool_state: POOL_STATES[1],
        },
        Row {
            case: "a prepared capacity narrower than the group's maximum, hibernated",
            subnets: 3,
            max_size: 3,
            desired_capacity: 1,
            put: Put {
                min_size: Some(0),
                prepared_capacity: Some(2),
                pool_state: Some(POOL_STATES[2]),
                reuse_on_scale_in: Some(false),
            },
            expected_size: 1,
            expected_pool_state: POOL_STATES[2],
        },
        Row {
            case: "a desired capacity above the prepared capacity: a pool of none",
            subnets: 1,
            max_size: 3,
            desired_capacity: 2,
            put: Put {
                prepared_capacity: Some(1),
                pool_state: Some(POOL_STATES[0]),
                ..Put::default()
            },
            expected_size: 0,
            expected_pool_state: POOL_STATES[0],
        },
        Row {
            case: "a group placed by Availability Zone alone",
            subnets: 0,
            max_size: 2,
            desired_capacity: 0,
            put: Put {
                min_size: Some(1),
                prepared_capacity: Some(2),
                ..Put::default()
            },
            expected_size: 2,
            expected_pool_state: DEFAULT_POOL_STATE,
        },
        Row {
            case: "the documented -1 leaves the size to the group's maximum",
            subnets: 2,
            max_size: 2,
            desired_capacity: 0,
            put: Put {
                prepared_capacity: Some(PREPARED_CAPACITY_UNSET),
                ..Put::default()
            },
            expected_size: 2,
            expected_pool_state: DEFAULT_POOL_STATE,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: each row is its own account, so the ids of one row never
        // explain another's.
        let (router, engine) = emulator();
        let subnets = seed_subnets(&engine, row.subnets);
        let template = format!("lt-warm-{index}");
        create_launch_template(&router, &template).await;
        let group = format!("asg-warm-{index}");
        let placement = if row.subnets > 0 {
            vec![p(
                "VPCZoneIdentifier",
                subnets
                    .iter()
                    .map(|subnet| subnet.id.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            )]
        } else {
            vec![p(
                "AvailabilityZones.member.1",
                ec2::availability_zones_in_region(REGION)
                    .first()
                    .map(|zone| zone.zone_name.clone())
                    .expect("the Region has Availability Zones"),
            )]
        };
        create_group(
            &router,
            &group,
            &template,
            &placement,
            0,
            row.max_size,
            row.desired_capacity,
        )
        .await;

        let (status, body) = put_warm_pool(&router, &group, row.put).await;
        assert_eq!(
            status, 200,
            "case {:?}: PutWarmPool expected 200, got {status}: {body}",
            row.case
        );

        let expected_state = warmed(row.expected_pool_state);
        let pool = settled_pool(
            &router,
            row.case,
            &group,
            row.expected_size,
            &expected_state,
        )
        .await;

        // The configuration is what the request asked for, with the documented defaults
        // for the members it omitted.
        assert_eq!(
            pool.min_size,
            row.put.min_size.unwrap_or(DEFAULT_MIN_SIZE).to_string(),
            "case {:?}: expected the stored MinSize, got {pool:?}",
            row.case
        );
        assert_eq!(
            pool.pool_state, row.expected_pool_state,
            "case {:?}: expected the stored PoolState, got {pool:?}",
            row.case
        );
        let expected_prepared = row
            .put
            .prepared_capacity
            .filter(|value| *value != PREPARED_CAPACITY_UNSET)
            .map(|value| value.to_string())
            .unwrap_or_default();
        assert_eq!(
            pool.prepared_capacity, expected_prepared,
            "case {:?}: expected MaxGroupPreparedCapacity {expected_prepared:?}, got {pool:?}",
            row.case
        );
        let expected_reuse = row
            .put
            .reuse_on_scale_in
            .map(|reuse| reuse.to_string())
            .unwrap_or_default();
        assert_eq!(
            pool.reuse_on_scale_in, expected_reuse,
            "case {:?}: expected ReuseOnScaleIn {expected_reuse:?}, got {pool:?}",
            row.case
        );
        // `Status` reports "the status of a warm pool that is marked for deletion"
        // (`smithy_query(query="shape",shape="WarmPoolConfiguration")`), and a stored
        // pool is not.
        assert!(
            pool.status.is_empty(),
            "case {:?}: a stored pool reports no Status, got {pool:?}",
            row.case
        );

        // The pool holds the instances the size rule says it maintains, each settled in
        // the pool's own state.
        assert_eq!(
            pool.instance_ids.len(),
            row.expected_size,
            "case {:?}: expected {} warm pool instance(s), got {pool:?}",
            row.case,
            row.expected_size
        );
        assert!(
            pool.lifecycle_states
                .iter()
                .all(|held| *held == expected_state),
            "case {:?}: expected every instance in {expected_state:?}, got {pool:?}",
            row.case
        );

        // Every id the pool reports is an instance OF THE ACCOUNT: Amazon EC2's own
        // reader resolves it, and reports it in the state the pool's own state keeps it
        // in — running only for a `Running` pool.
        let expected_ec2 = expected_ec2_state(row.expected_pool_state);
        for instance_id in &pool.instance_ids {
            let state = ec2_instance_settled(&router, instance_id, expected_ec2).await;
            assert_eq!(
                state.as_deref(),
                Some(expected_ec2),
                "case {:?}: the pool reports {instance_id}, which Amazon EC2 must resolve to an \
                 instance {expected_ec2}, got {state:?}",
                row.case
            );
        }

        // The group's own reader reports the pool beside its own instances — and its
        // `Instances` list holds only the group's own: "to retrieve information about the
        // instances in a warm pool, you must call the DescribeWarmPool API"
        // (`DescribeAutoScalingGroups`'s own documentation).
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", group.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "WarmPoolSize"),
            row.expected_size.to_string(),
            "case {:?}: expected WarmPoolSize {}, got {body}",
            row.case,
            row.expected_size
        );
        let configuration = tag_value(&body, "WarmPoolConfiguration");
        assert_eq!(
            tag_value(&configuration, "PoolState"),
            row.expected_pool_state,
            "case {:?}: the group reports the pool's own state, got {body}",
            row.case
        );
        let group_instances = tag_values(&body, "InstanceId");
        assert_eq!(
            group_instances.len(),
            usize::try_from(row.desired_capacity).expect("a capacity is a count"),
            "case {:?}: the group reports only its own instances, got {group_instances:?} in \
             {body}",
            row.case
        );
        for instance_id in &pool.instance_ids {
            assert!(
                !group_instances.contains(instance_id),
                "case {:?}: the warm pool instance {instance_id} is not one of the group's own, \
                 got {group_instances:?}",
                row.case
            );
        }
    }
}

/// One row of the update table: the member the second put carries, and what the pool must
/// report afterwards.
struct UpdateRow {
    case: &'static str,
    update: Put,
    expected_min_size: i32,
    /// `""` when the pool must report no prepared capacity.
    expected_prepared: &'static str,
    expected_pool_state: &'static str,
    expected_reuse: &'static str,
}

/// A second `PutWarmPool` UPDATES the stored pool per member: a member it omits keeps the
/// stored value, a member it supplies overwrites it, and `MaxGroupPreparedCapacity` -1
/// clears the stored one.
#[tokio::test]
async fn a_put_updates_only_the_members_it_names() {
    // What every row starts from: every member supplied, so an omitted member in the
    // update has a stored value to keep.
    let created = Put {
        min_size: Some(1),
        prepared_capacity: Some(3),
        pool_state: Some(POOL_STATES[0]),
        reuse_on_scale_in: Some(true),
    };

    let rows = [
        UpdateRow {
            case: "the group name alone keeps every stored member",
            update: Put::default(),
            expected_min_size: 1,
            expected_prepared: "3",
            expected_pool_state: POOL_STATES[0],
            expected_reuse: "true",
        },
        UpdateRow {
            case: "a new minimum alone",
            update: Put {
                min_size: Some(2),
                ..Put::default()
            },
            expected_min_size: 2,
            expected_prepared: "3",
            expected_pool_state: POOL_STATES[0],
            expected_reuse: "true",
        },
        UpdateRow {
            case: "a new pool state alone",
            update: Put {
                pool_state: Some(POOL_STATES[1]),
                ..Put::default()
            },
            expected_min_size: 1,
            expected_prepared: "3",
            expected_pool_state: POOL_STATES[1],
            expected_reuse: "true",
        },
        UpdateRow {
            case: "a new prepared capacity alone",
            update: Put {
                prepared_capacity: Some(2),
                ..Put::default()
            },
            expected_min_size: 1,
            expected_prepared: "2",
            expected_pool_state: POOL_STATES[0],
            expected_reuse: "true",
        },
        UpdateRow {
            case: "the documented -1 clears the stored prepared capacity",
            update: Put {
                prepared_capacity: Some(PREPARED_CAPACITY_UNSET),
                ..Put::default()
            },
            expected_min_size: 1,
            expected_prepared: "",
            expected_pool_state: POOL_STATES[0],
            expected_reuse: "true",
        },
        UpdateRow {
            case: "a reuse policy turned off",
            update: Put {
                reuse_on_scale_in: Some(false),
                ..Put::default()
            },
            expected_min_size: 1,
            expected_prepared: "3",
            expected_pool_state: POOL_STATES[0],
            expected_reuse: "false",
        },
        UpdateRow {
            case: "every member at once",
            update: Put {
                min_size: Some(3),
                prepared_capacity: Some(3),
                pool_state: Some(POOL_STATES[2]),
                reuse_on_scale_in: Some(false),
            },
            expected_min_size: 3,
            expected_prepared: "3",
            expected_pool_state: POOL_STATES[2],
            expected_reuse: "false",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator();
        let subnets = seed_subnets(&engine, 1);
        let template = format!("lt-update-{index}");
        create_launch_template(&router, &template).await;
        let group = format!("asg-update-{index}");
        create_group(
            &router,
            &group,
            &template,
            &[p("VPCZoneIdentifier", subnets[0].id.clone())],
            0,
            3,
            1,
        )
        .await;

        let (status, body) = put_warm_pool(&router, &group, created).await;
        assert_eq!(
            status, 200,
            "case {:?}: the create expected 200, got {status}: {body}",
            row.case
        );
        // The pool the create left behind: prepared 3 minus the group's desired 1.
        let settled = settled_pool(&router, row.case, &group, 2, &warmed(POOL_STATES[0])).await;
        assert_eq!(
            settled.instance_ids.len(),
            2,
            "case {:?}: the created pool holds the size rule's count, got {settled:?}",
            row.case
        );

        let (status, body) = put_warm_pool(&router, &group, row.update).await;
        assert_eq!(
            status, 200,
            "case {:?}: the update expected 200, got {status}: {body}",
            row.case
        );

        // What the pool must hold afterwards, by the same documented rule: the prepared
        // capacity it now carries (or the group's own maximum when it carries none) less
        // the group's desired capacity, with the pool's minimum as the floor.
        let prepared = if row.expected_prepared.is_empty() {
            3
        } else {
            row.expected_prepared
                .parse::<i32>()
                .expect("the expected prepared capacity is a number")
        };
        let expected_size = usize::try_from((prepared - 1).max(row.expected_min_size).max(0))
            .expect("a pool size is a count");
        let expected_state = warmed(row.expected_pool_state);
        let pool = settled_pool(&router, row.case, &group, expected_size, &expected_state).await;

        assert_eq!(
            pool.min_size,
            row.expected_min_size.to_string(),
            "case {:?}: expected MinSize {}, got {pool:?}",
            row.case,
            row.expected_min_size
        );
        assert_eq!(
            pool.prepared_capacity, row.expected_prepared,
            "case {:?}: expected MaxGroupPreparedCapacity {:?}, got {pool:?}",
            row.case, row.expected_prepared
        );
        assert_eq!(
            pool.pool_state, row.expected_pool_state,
            "case {:?}: expected PoolState {:?}, got {pool:?}",
            row.case, row.expected_pool_state
        );
        assert_eq!(
            pool.reuse_on_scale_in, row.expected_reuse,
            "case {:?}: expected ReuseOnScaleIn {:?}, got {pool:?}",
            row.case, row.expected_reuse
        );
        assert_eq!(
            pool.instance_ids.len(),
            expected_size,
            "case {:?}: expected {expected_size} warm pool instance(s), got {pool:?}",
            row.case
        );
        assert!(
            pool.lifecycle_states
                .iter()
                .all(|held| *held == expected_state),
            "case {:?}: expected every instance in {expected_state:?}, got {pool:?}",
            row.case
        );
        let expected_ec2 = expected_ec2_state(row.expected_pool_state);
        for instance_id in &pool.instance_ids {
            let state = ec2_instance_settled(&router, instance_id, expected_ec2).await;
            assert_eq!(
                state.as_deref(),
                Some(expected_ec2),
                "case {:?}: the pool reports {instance_id}, which Amazon EC2 must resolve to an \
                 instance {expected_ec2} once the updated pool state is due, got {state:?}",
                row.case
            );
        }
    }
}

/// "The size of the warm pool is dynamic"
/// (`smithy_query(query="doc",shape="PutWarmPoolType",member="MaxGroupPreparedCapacity")`):
/// the pool follows the group's desired capacity without another `PutWarmPool`.
#[tokio::test]
async fn the_pool_size_follows_the_groups_capacity() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    create_launch_template(&router, "lt-dynamic").await;
    create_group(
        &router,
        "asg-dynamic",
        "lt-dynamic",
        &[p(
            "VPCZoneIdentifier",
            format!("{},{}", subnets[0].id, subnets[1].id),
        )],
        0,
        3,
        0,
    )
    .await;

    let (status, body) = put_warm_pool(&router, "asg-dynamic", Put::default()).await;
    assert_eq!(status, 200, "PutWarmPool expected 200, got {status}: {body}");

    // The group's maximum less its desired capacity, at each capacity the group is moved
    // through: a pool of 3 at capacity 0, and of 1 at capacity 2.
    let expected_state = warmed(DEFAULT_POOL_STATE);
    let pool = settled_pool(&router, "capacity 0", "asg-dynamic", 3, &expected_state).await;
    assert_eq!(
        pool.instance_ids.len(),
        3,
        "a pool with no prepared capacity holds the group's maximum less its desired capacity, \
         got {pool:?}"
    );

    for (capacity, expected_size) in [(2, 1), (3, 0), (0, 3)] {
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", "asg-dynamic"),
                ("DesiredCapacity", capacity.to_string().as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {capacity}: SetDesiredCapacity expected 200, got {status}: {body}"
        );
        let case = format!("capacity {capacity}");
        let pool = settled_pool(
            &router,
            &case,
            "asg-dynamic",
            expected_size,
            &expected_state,
        )
        .await;
        assert_eq!(
            pool.instance_ids.len(),
            expected_size,
            "case {case:?}: expected a pool of {expected_size}, got {pool:?}"
        );
    }
}

/// A forced delete takes the warm pool with the group: "if the group has a warm pool, the
/// force delete option also deletes the warm pool"
/// (`smithy_query(query="operation",operation="DeleteAutoScalingGroup")`), so the pool's
/// instances are terminated where they live and the pool answers no reader.
#[tokio::test]
async fn a_forced_delete_takes_the_warm_pool_with_the_group() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    create_launch_template(&router, "lt-cascade").await;
    create_group(
        &router,
        "asg-cascade",
        "lt-cascade",
        &[p("VPCZoneIdentifier", subnets[0].id.clone())],
        0,
        2,
        1,
    )
    .await;

    let (status, body) = put_warm_pool(
        &router,
        "asg-cascade",
        Put {
            min_size: Some(2),
            ..Put::default()
        },
    )
    .await;
    assert_eq!(status, 200, "PutWarmPool expected 200, got {status}: {body}");
    let pool = settled_pool(
        &router,
        "the cascade",
        "asg-cascade",
        2,
        &warmed(DEFAULT_POOL_STATE),
    )
    .await;
    assert_eq!(
        pool.instance_ids.len(),
        2,
        "the pool must hold its own minimum before the delete, got {pool:?}"
    );

    // Without `ForceDelete` the group is in use: its warm pool is holding instances, which
    // the refusal says in a sentence of its own after the one it states for the group's
    // own instances.
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-cascade")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a group whose warm pool holds instances is in use, got {status}: {body}"
    );
    assert!(
        body.contains("<Code>ResourceInUse</Code>"),
        "expected the ResourceInUse query error code, got {body}"
    );
    assert_eq!(
        xml_unescape(&tag_value(&body, "Message")),
        "You cannot delete an AutoScalingGroup while there are instances or pending Spot \
         instance request(s) still in the group. Also, you can\u{2019}t delete it while there \
         are instances that are part of a warm pool.",
        "expected the in-use sentence a warm pool adds its own to, got {body}"
    );

    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-cascade"),
            ("ForceDelete", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200, "a forced delete expected 200, got {status}: {body}");

    // The pool answers no reader: the group it belonged to is gone.
    let (status, body) = describe_warm_pool(&router, "asg-cascade").await;
    assert_eq!(
        status, 400,
        "the pool of a deleted group answers no reader, got {status}: {body}"
    );
    assert_eq!(
        xml_unescape(&tag_value(&body, "Message")),
        "AutoScalingGroup name not found - No such Auto Scaling group: asg-cascade",
        "expected the absent-group sentence this read answers with, got {body}"
    );

    // And the instances it was holding are terminated in the account's own Amazon EC2
    // state rather than left running there.
    for instance_id in &pool.instance_ids {
        let state = ec2_instance_settled(&router, instance_id, "terminated").await;
        assert_eq!(
            state.as_deref(),
            Some("terminated"),
            "a forced delete terminates the warm pool instance {instance_id}, got {state:?}"
        );
    }
}

/// One row of the refusal table.
struct Refusal {
    case: &'static str,
    action: &'static str,
    /// The parameters beyond the group name.
    params: Vec<(String, String)>,
    /// The group name the request carries; `None` names the stored group.
    absent_group: Option<&'static str>,
    status: u16,
    code: &'static str,
    message: String,
}

/// A request the service cannot satisfy is refused with the sentence it states for that
/// error class, and the stored pool is left alone.
#[tokio::test]
async fn an_unsatisfiable_request_is_refused() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    create_launch_template(&router, "lt-refusals").await;
    create_group(
        &router,
        "asg-refusals",
        "lt-refusals",
        &[p("VPCZoneIdentifier", subnets[0].id.clone())],
        0,
        1,
        0,
    )
    .await;
    let (status, body) = put_warm_pool(
        &router,
        "asg-refusals",
        Put {
            min_size: Some(1),
            ..Put::default()
        },
    )
    .await;
    assert_eq!(status, 200, "PutWarmPool expected 200, got {status}: {body}");
    settled_pool(
        &router,
        "the stored pool",
        "asg-refusals",
        1,
        &warmed(DEFAULT_POOL_STATE),
    )
    .await;

    // The ceiling this process holds one group to, which is what a warm pool size past it
    // is refused against — read from the service rather than written out.
    let ceiling = autoscaling::state::MAX_GROUP_CAPACITY;
    let rows = vec![
        Refusal {
            case: "a put naming no stored group",
            action: "PutWarmPool",
            params: Vec::new(),
            absent_group: Some("asg-absent"),
            status: 400,
            code: "ValidationError",
            message: "AutoScalingGroup name not found - null".to_string(),
        },
        Refusal {
            // The group is resolved before any member of the request is checked, so a
            // name no group answers to is refused for THAT and not for the size it
            // carried — which the two single-fault rows cannot tell apart.
            case: "a put naming no stored group AND a size past the ceiling",
            action: "PutWarmPool",
            params: vec![p("MinSize", (ceiling + 1).to_string())],
            absent_group: Some("asg-absent"),
            status: 400,
            code: "ValidationError",
            message: "AutoScalingGroup name not found - null".to_string(),
        },
        Refusal {
            case: "a read naming no stored group",
            action: "DescribeWarmPool",
            params: Vec::new(),
            absent_group: Some("asg-absent"),
            status: 400,
            code: "ValidationError",
            message: "AutoScalingGroup name not found - No such Auto Scaling group: asg-absent"
                .to_string(),
        },
        Refusal {
            case: "a pool state outside the published enum",
            action: "PutWarmPool",
            params: vec![p("PoolState", "Warmed")],
            absent_group: None,
            status: 400,
            code: "ValidationError",
            // `PoolState` targets the model's own `WarmPoolState` enum, so the boundary
            // validator refuses a value outside it and states the set it had to satisfy.
            message: format!(
                "1 validation error detected: Value 'Warmed' at 'poolState' failed to satisfy \
                 constraint: Member must satisfy enum value set: [{}]",
                POOL_STATES.join(", ")
            ),
        },
        Refusal {
            case: "a minimum past the ceiling one group is held to",
            action: "PutWarmPool",
            params: vec![p("MinSize", (ceiling + 1).to_string())],
            absent_group: None,
            status: 400,
            code: "LimitExceeded",
            message: format!(
                "The warm pool size {} exceeds the limit of {ceiling} instances per Auto Scaling \
                 group",
                ceiling + 1
            ),
        },
        Refusal {
            case: "a prepared capacity past that same ceiling",
            action: "PutWarmPool",
            params: vec![p("MaxGroupPreparedCapacity", (ceiling + 7).to_string())],
            absent_group: None,
            status: 400,
            code: "LimitExceeded",
            message: format!(
                "The warm pool size {} exceeds the limit of {ceiling} instances per Auto Scaling \
                 group",
                ceiling + 7
            ),
        },
        Refusal {
            case: "a page of no instances at all",
            action: "DescribeWarmPool",
            params: vec![p("MaxRecords", (MIN_MAX_RECORDS - 1).to_string())],
            absent_group: None,
            status: 400,
            code: "ValidationError",
            message: format!(
                "1 validation error detected: Value '{}' at 'maxRecords' failed to satisfy \
                 constraint: Member must have value greater than or equal to {MIN_MAX_RECORDS}",
                MIN_MAX_RECORDS - 1
            ),
        },
        Refusal {
            case: "a page past the published maximum",
            action: "DescribeWarmPool",
            params: vec![p("MaxRecords", (MAX_MAX_RECORDS + 1).to_string())],
            absent_group: None,
            status: 400,
            code: "ValidationError",
            message: format!(
                "1 validation error detected: Value '{}' at 'maxRecords' failed to satisfy \
                 constraint: Member must have value less than or equal to {MAX_MAX_RECORDS}",
                MAX_MAX_RECORDS + 1
            ),
        },
        Refusal {
            case: "a token this service never issued",
            action: "DescribeWarmPool",
            params: vec![p("NextToken", "not-a-token")],
            absent_group: None,
            status: 400,
            code: "InvalidNextToken",
            message: "The token 'not-a-token' was invalid".to_string(),
        },
    ];

    for row in &rows {
        let mut params = vec![p(
            "AutoScalingGroupName",
            row.absent_group.unwrap_or("asg-refusals"),
        )];
        params.extend(row.params.iter().cloned());
        let (status, body) = query(&router, row.action, &borrowed(&params)).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status}: {body}",
            row.case, row.status
        );
        assert_eq!(
            tag_value(&body, "Code"),
            row.code,
            "case {:?}: expected the {} code, got {body}",
            row.case,
            row.code
        );
        assert_eq!(
            xml_unescape(&tag_value(&body, "Message")),
            row.message,
            "case {:?}: expected the message {:?}, got {body}",
            row.case,
            row.message
        );
    }

    // Each published bound of `MaxRecords` is ACCEPTED, so the refusals above cannot pass
    // by refusing every page — and the stored pool survived every refusal.
    for max_records in [MIN_MAX_RECORDS, MAX_MAX_RECORDS] {
        let (status, body) = query(
            &router,
            "DescribeWarmPool",
            &[
                ("AutoScalingGroupName", "asg-refusals"),
                ("MaxRecords", max_records.to_string().as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {max_records}: a published bound is accepted, got {status}: {body}"
        );
        let pool = reported_pool(&body);
        assert_eq!(
            pool.min_size, "1",
            "case {max_records}: the refused requests left the stored pool alone, got {pool:?}"
        );
        assert_eq!(
            pool.instance_ids.len(),
            1,
            "case {max_records}: the pool still holds its instance, got {pool:?}"
        );
    }
}
