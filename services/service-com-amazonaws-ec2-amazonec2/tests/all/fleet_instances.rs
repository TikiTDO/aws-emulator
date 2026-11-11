// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeFleetInstances`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the report is the instances the EC2 Fleet is running, read out of the INSTANCE store:
//!    "Describes the running instances for the specified EC2 Fleet"
//!    (`DescribeFleetInstances`), so every instance the fleet launched that the store still
//!    holds un-terminated is one of them — an instance that has reached `terminated`, and
//!    one the store no longer holds, is not — and the type reported for one is the type
//!    `DescribeInstances` reports for the same instance;
//! 2. a `FleetId` that is not a fleet id at all is refused with `InvalidFleetId.Malformed`
//!    and a well-formed one this account holds no fleet for with `InvalidFleetId.NotFound`,
//!    in that order;
//! 3. a fleet of type `instant` is refused with `UnsupportedOperation`: this operation
//!    "does not support fleets of type instant" (`DescribeFleetInstances`);
//! 4. the report is narrowed by the one documented `instance-type` filter, over EC2's
//!    `*`/`?` wildcards and its AND/OR rule, and a filter name the operation does not
//!    document is refused;
//! 5. it is cut into `MaxResults`-sized pages a `NextToken` continues, in `InstanceId`
//!    order;
//! 6. `DryRun` answers `DryRunOperation` only for a request that would otherwise have
//!    succeeded;
//! 7. describing is a READ: two calls answer the same and nothing in the store moves.
//!
//! Each table row is a distinct input along what the rule generalises over — which
//! instance state, which instance type, which malformed id form, which fleet type, which
//! filter and which page size — so no row is the specification.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ec2::ops::types::INSTANCE_HEALTH_STATUS_VALUES;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FLEET_ACTIVITY_STATUS_FULFILLED, FLEET_ID_PREFIX,
    FLEET_STATE_ACTIVE, FLEET_TYPE_INSTANT, FLEET_TYPE_MAINTAIN, FLEET_TYPE_VALUES,
    FleetInstanceRecord, FleetSpec, INSTANCE_HEALTH_HEALTHY, InstanceStateName, MAX_PAGE_SIZE,
    PendingTransition, TRANSITION_DELAY, fleet_id_is_well_formed,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_state, error_code, filters};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. A fleet carries no Region member, so the
/// Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The image every launch here names; the instance store's own image catalog answers for
/// it, as the instance-lifecycle suite's launches do.
const AMI: &str = "ami-0123456789abcdef0";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One EC2 emulator over its own engine, with the engine handed back so a suite can seed
/// the store a handler reads.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeFleetInstances", params).await
}

/// Launch one instance of `instance_type` and answer its `InstanceId`.
async fn launch(router: &MultiServiceRouter, instance_type: &str) -> String {
    let params = vec![
        p("ImageId", AMI),
        p("MinCount", "1"),
        p("MaxCount", "1"),
        p("InstanceType", instance_type),
    ];
    let (status, body) = call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed for {instance_type:?}, got {status}: {body}"
    );
    super::ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// Store one EC2 Fleet of `fleet_type` that has launched `instances`, and answer the
/// `FleetId` the store minted for it — the write `CreateFleet` will make once it is
/// implemented, made here directly so this suite is about the REPORT.
fn seed_fleet(
    engine: &engine::InMemoryEngine,
    fleet_type: &str,
    instances: Vec<FleetInstanceRecord>,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_fleet(FleetSpec {
            fleet_type: fleet_type.to_string(),
            fleet_state: FLEET_STATE_ACTIVE.to_string(),
            activity_status: Some(FLEET_ACTIVITY_STATUS_FULFILLED.to_string()),
            client_token: None,
            create_time: SystemTime::now(),
            launch_template_configs: Vec::new(),
            target_capacity_specification: None,
            fulfilled_capacity: None,
            fulfilled_on_demand_capacity: None,
            terminate_instances_with_expiration: None,
            replace_unhealthy_instances: None,
            excess_capacity_termination_policy: None,
            valid_from: None,
            valid_until: None,
            tags: Vec::new(),
            context: None,
            spot_options: None,
            on_demand_options: None,
            pools: Vec::new(),
            instances,
        })
        .expect("the store must have room for one more fleet")
}

/// A fleet instance with no Spot request behind it — an On-Demand launch.
fn on_demand(instance_id: &str) -> FleetInstanceRecord {
    FleetInstanceRecord {
        instance_id: instance_id.to_string(),
        spot_instance_request_id: None,
        pool: 0,
    }
}

/// A fleet instance placed through the Spot request `request_id`.
fn spot(instance_id: &str, request_id: &str) -> FleetInstanceRecord {
    FleetInstanceRecord {
        instance_id: instance_id.to_string(),
        spot_instance_request_id: Some(request_id.to_string()),
        pool: 0,
    }
}

/// One `instance-type` filter case: the label, the `Filters` entries it sends, and the
/// instance ids it must select.
type FilterCase = (
    &'static str,
    Vec<(&'static str, &'static [&'static str])>,
    Vec<String>,
);

/// The store as the read-only assertions compare it: every fleet with the instances it
/// names, and every instance with the state it stands in.
type StoreSnapshot = (Vec<(String, Vec<String>)>, Vec<(String, String)>);

/// One `<item>` of the `activeInstanceSet` a response carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveRow {
    instance_id: String,
    instance_type: Option<String>,
    instance_health: Option<String>,
    spot_instance_request_id: Option<String>,
}

/// Every active instance a response reports, in the order it reported them.
fn active_instances(case: &str, body: &str) -> Vec<ActiveRow> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "instanceId").is_some())
        .map(|node| ActiveRow {
            instance_id: child_text(node, "instanceId").unwrap_or_default(),
            instance_type: child_text(node, "instanceType"),
            instance_health: child_text(node, "instanceHealth"),
            spot_instance_request_id: child_text(node, "spotInstanceRequestId"),
        })
        .collect()
}

/// The `instanceId` of every active instance a response reports.
fn active_ids(case: &str, body: &str) -> Vec<String> {
    active_instances(case, body)
        .into_iter()
        .map(|row| row.instance_id)
        .collect()
}

/// The `fleetId` the response echoes.
fn reported_fleet_id(case: &str, body: &str) -> Option<String> {
    let parsed = document(case, body);
    parsed
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "fleetId")
        .and_then(|node| node.text())
        .map(str::to_string)
}

/// The `nextToken` the response carries, if any.
fn next_token(case: &str, body: &str) -> Option<String> {
    let parsed = document(case, body);
    parsed
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "nextToken")
        .and_then(|node| node.text())
        .map(str::to_string)
}

/// Describe `fleet_id` and demand HTTP 200.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    fleet_id: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![p("FleetId", fleet_id)];
    params.extend_from_slice(extra);
    let (status, body) = describe(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// The instance store is what is reported
// ---------------------------------------------------------------------------

/// The instance types this suite launches: four members of the modelled `InstanceType`
/// catalog (`smithy_query(query="shape",shape="InstanceType")`), of four different
/// families, so the type a row reports is a value of that row rather than one constant.
const LAUNCHED_TYPES: [&str; 4] = ["t3.micro", "m5.large", "c6g.xlarge", "t4g.nano"];

/// Every running instance of the fleet is reported with the type the INSTANCE store holds
/// for it, its health and the Spot request it was placed through — and the response echoes
/// the fleet it is a report of.
#[tokio::test]
async fn a_fleets_running_instances_are_reported_from_the_instance_store() {
    let (router, engine) = emulator();

    let mut launched: Vec<(String, &str)> = Vec::new();
    for instance_type in LAUNCHED_TYPES {
        launched.push((launch(&router, instance_type).await, instance_type));
    }
    // The second and fourth were placed through a Spot request; the others are On-Demand,
    // which carry no Spot request id at all.
    let fleet_id = seed_fleet(
        &engine,
        FLEET_TYPE_MAINTAIN,
        vec![
            on_demand(&launched[0].0),
            spot(&launched[1].0, "sir-00000000000000001"),
            on_demand(&launched[2].0),
            spot(&launched[3].0, "sir-00000000000000002"),
        ],
    );
    assert!(
        fleet_id_is_well_formed(&fleet_id),
        "the store must mint a well-formed fleet id, got {fleet_id:?}"
    );
    assert!(
        fleet_id.starts_with(FLEET_ID_PREFIX),
        "a fleet id must carry the {FLEET_ID_PREFIX:?} prefix, got {fleet_id:?}"
    );

    // A just-launched instance is `pending`, and it is one of the fleet's from the moment the
    // fleet launched it: the clock owes it `running`, which is a fact about the INSTANCE and
    // not about whether the fleet is running it.
    let body = described(&router, "while pending", &fleet_id, &[]).await;
    let mut pending_rows = active_ids("while pending", &body);
    pending_rows.sort();
    let mut expected_pending: Vec<String> =
        launched.iter().map(|(id, _)| id.clone()).collect();
    expected_pending.sort();
    assert_eq!(
        pending_rows, expected_pending,
        "while pending: every instance the fleet launched is one of its instances, got \
         {pending_rows:?}"
    );

    past_the_transition().await;

    let body = described(&router, "running", &fleet_id, &[]).await;
    assert_eq!(
        reported_fleet_id("running", &body).as_deref(),
        Some(fleet_id.as_str()),
        "running: the report must name the fleet it is of, got {body}"
    );
    let rows = active_instances("running", &body);
    let mut expected: Vec<ActiveRow> = launched
        .iter()
        .enumerate()
        .map(|(index, (instance_id, instance_type))| ActiveRow {
            instance_id: instance_id.clone(),
            instance_type: Some((*instance_type).to_string()),
            instance_health: Some(INSTANCE_HEALTH_HEALTHY.to_string()),
            spot_instance_request_id: match index {
                1 => Some("sir-00000000000000001".to_string()),
                3 => Some("sir-00000000000000002".to_string()),
                _ => None,
            },
        })
        .collect();
    expected.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    assert_eq!(
        rows, expected,
        "running: expected every running instance of the fleet with its own type, health \
         and Spot request, got {rows:?}"
    );
    for row in &rows {
        let health = row.instance_health.clone().unwrap_or_default();
        assert!(
            INSTANCE_HEALTH_STATUS_VALUES.contains(&health.as_str()),
            "running: {:?} reports the health {health:?}, which is not a member of the \
             modelled InstanceHealthStatus catalog {INSTANCE_HEALTH_STATUS_VALUES:?}",
            row.instance_id
        );
    }

    // The same instance, described through `DescribeInstances`: the two reports cannot
    // disagree about one instance's type, because both read the one stored record.
    for (instance_id, instance_type) in &launched {
        let (status, instances_body) = call(
            &router,
            REGION,
            "DescribeInstances",
            &[p("InstanceId.1", instance_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "DescribeInstances must succeed for {instance_id:?}, got {status}: \
             {instances_body}"
        );
        let reported = super::ec2_query::element(&instances_body, "instanceType");
        assert_eq!(
            reported.as_deref(),
            Some(*instance_type),
            "{instance_id:?}: DescribeInstances and DescribeFleetInstances must report one \
             instance's type the same way, got {reported:?}"
        );
    }
}

/// An instance of the fleet is one of its active instances until it reaches `terminated`,
/// whatever else the fleet record says — one row per member of the modelled
/// `InstanceStateName` catalog (`smithy_query(query="shape",shape="InstanceStateName")`),
/// plus an instance id the store holds no instance for at all.
#[tokio::test]
async fn an_instance_of_the_fleet_is_active_until_it_is_terminated() {
    /// The six states `InstanceStateName` models, and whether an instance in one is an
    /// instance its fleet is running.
    const STATES: [(InstanceStateName, bool); 6] = [
        (InstanceStateName::Pending, true),
        (InstanceStateName::Running, true),
        (InstanceStateName::ShuttingDown, true),
        (InstanceStateName::Terminated, false),
        (InstanceStateName::Stopping, true),
        (InstanceStateName::Stopped, true),
    ];

    let (router, engine) = emulator();
    let instance_id = launch(&router, LAUNCHED_TYPES[0]).await;
    let fleet_id = seed_fleet(&engine, FLEET_TYPE_MAINTAIN, vec![on_demand(&instance_id)]);

    for (state, is_active) in STATES {
        let case = format!("state {}", state.as_str());
        {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let instance = guard
                .instances
                .get_mut(&instance_id)
                .unwrap_or_else(|| panic!("case {case:?}: the launched instance must be stored"));
            instance.state = state;
            // Nothing is owed on it: the state above is where the instance stands.
            instance.pending_transition = None;
        }
        let body = described(&router, &case, &fleet_id, &[]).await;
        let reported = active_ids(&case, &body);
        let expected: Vec<String> = if is_active {
            vec![instance_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} to be the fleet's instances, got \
             {reported:?}"
        );
    }

    // A fleet that names an instance the store does not hold reports no instance for it:
    // the report is of what the instance store holds, not of what the fleet remembers.
    let stale_fleet = seed_fleet(
        &engine,
        FLEET_TYPE_MAINTAIN,
        vec![on_demand("i-00000000000000999")],
    );
    let body = described(&router, "stale instance id", &stale_fleet, &[]).await;
    let reported = active_ids("stale instance id", &body);
    assert!(
        reported.is_empty(),
        "stale instance id: an instance the store does not hold is not active, got \
         {reported:?}"
    );
}

/// The `pending` -> `running` move the CLOCK owes a launch is observed on the INSTANCE, not
/// on whether the fleet is running it: the fleet reports the instance it launched on both
/// sides of that move, and `DescribeInstances` reports the state it is in.
#[tokio::test]
async fn the_clock_moves_a_pending_instance_of_the_fleet_to_running() {
    let (router, engine) = emulator();
    let instance_id = launch(&router, LAUNCHED_TYPES[1]).await;
    let fleet_id = seed_fleet(&engine, FLEET_TYPE_MAINTAIN, vec![on_demand(&instance_id)]);

    // Put the instance back in `pending` with the move owed from now, so the span under
    // test is the service's own and starts at a known instant.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard
            .instances
            .get_mut(&instance_id)
            .expect("the launched instance must be stored");
        instance.state = InstanceStateName::Pending;
        instance.pending_transition = Some(PendingTransition::starting_now(
            InstanceStateName::Running,
        ));
    }

    let body = described(&router, "transition owed", &fleet_id, &[]).await;
    let before = active_ids("transition owed", &body);
    assert_eq!(
        before,
        vec![instance_id.clone()],
        "transition owed: the fleet is running the instance it launched, got {before:?}"
    );
    let state = described_instance_state(&router, REGION, &instance_id).await;
    assert_eq!(
        state,
        InstanceStateName::Pending.as_str(),
        "transition owed: the instance is still pending, got {state:?}"
    );

    past_the_transition().await;

    let body = described(&router, "transition due", &fleet_id, &[]).await;
    let after = active_ids("transition due", &body);
    assert_eq!(
        after,
        vec![instance_id.clone()],
        "transition due: the fleet is still running the same instance, got {after:?}"
    );
    let state = described_instance_state(&router, REGION, &instance_id).await;
    assert_eq!(
        state,
        InstanceStateName::Running.as_str(),
        "transition due: the clock owed the instance `running`, got {state:?}"
    );
}

// ---------------------------------------------------------------------------
// Which fleet ids the operation accepts
// ---------------------------------------------------------------------------

/// A well-formed EC2 Fleet id: `fleet-` and a UUID. Used as the "no such fleet" probe and
/// as the base every malformed form below deforms.
const WELL_FORMED_FLEET_ID: &str = "fleet-73fbd2ce-aa30-494c-8788-1cee4048d614";

/// Something that is not an EC2 Fleet id at all is refused as malformed — before any
/// lookup, so the answer never depends on what the account holds. One row per way an id
/// can fail the form, including the long-id form every OTHER EC2 resource uses, which the
/// recorded unit `ec2-fleet-describe-lifecycle#task#13` has AWS refusing as malformed.
#[tokio::test]
async fn a_fleet_id_that_is_not_one_is_refused_as_malformed() {
    const MALFORMED: [(&str, &str); 10] = [
        ("the 17-hexadecimal-digit long-id form", "fleet-1234567890abcdef0"),
        ("the 8-hexadecimal-digit short-id form", "fleet-12345678"),
        (
            "upper-case hexadecimal",
            "fleet-73FBD2CE-AA30-494C-8788-1CEE4048D614",
        ),
        (
            "a short final group",
            "fleet-73fbd2ce-aa30-494c-8788-1cee4048d61",
        ),
        ("a missing group", "fleet-73fbd2ce-aa30-494c-8788"),
        (
            "a group too many",
            "fleet-73fbd2ce-aa30-494c-8788-1cee4048d614-1cee",
        ),
        (
            "a non-hexadecimal digit",
            "fleet-73fbd2cg-aa30-494c-8788-1cee4048d614",
        ),
        ("no prefix", "73fbd2ce-aa30-494c-8788-1cee4048d614"),
        ("an id of another kind", "i-00000000000000001"),
        ("the prefix alone", "fleet-"),
    ];

    let (router, engine) = emulator();
    // A fleet the account DOES hold, so a refusal cannot be an artefact of an empty store.
    let held = seed_fleet(&engine, FLEET_TYPE_MAINTAIN, Vec::new());

    for (case, fleet_id) in MALFORMED {
        assert!(
            !fleet_id_is_well_formed(fleet_id),
            "case {case:?}: {fleet_id:?} must not read as a well-formed fleet id"
        );
        let (status, body) = describe(&router, &[p("FleetId", fleet_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {fleet_id:?}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFleetId.Malformed",
            "case {case:?}: expected InvalidFleetId.Malformed for {fleet_id:?}, got {body}"
        );
    }

    assert!(
        fleet_id_is_well_formed(&held) && fleet_id_is_well_formed(WELL_FORMED_FLEET_ID),
        "the minted id {held:?} and {WELL_FORMED_FLEET_ID:?} must both read as fleet ids"
    );
    // The held fleet still answers, so the refusals above are about the id's FORM.
    let body = described(&router, "a fleet the account holds", &held, &[]).await;
    assert_eq!(
        reported_fleet_id("a fleet the account holds", &body).as_deref(),
        Some(held.as_str()),
        "a fleet the account holds must still be reported, got {body}"
    );
}

/// A well-formed fleet id this account holds no fleet for is refused as not found — a
/// different question, asked second.
#[tokio::test]
async fn an_unknown_fleet_id_is_refused_as_not_found() {
    let (router, engine) = emulator();
    let held = seed_fleet(&engine, FLEET_TYPE_MAINTAIN, Vec::new());

    // One digit off the held id, flipped to a DIFFERENT digit whatever the store minted, so
    // the row can never collide with the id it is derived from.
    let last = held.chars().next_back().unwrap_or('0');
    let one_off = format!(
        "{}{}",
        &held[..held.len() - 1],
        if last == 'f' { '0' } else { 'f' }
    );
    assert_ne!(
        one_off, held,
        "the one-digit-off row must name a DIFFERENT fleet id than the held one"
    );

    let unknown = [
        ("a fleet nobody created", WELL_FORMED_FLEET_ID.to_string()),
        (
            "the null UUID",
            format!("{FLEET_ID_PREFIX}00000000-0000-0000-0000-000000000000"),
        ),
        ("an id one digit off a held one", one_off),
    ];
    for (case, fleet_id) in unknown {
        let (status, body) = describe(&router, &[p("FleetId", &fleet_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {fleet_id:?}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFleetId.NotFound",
            "case {case:?}: expected InvalidFleetId.NotFound for {fleet_id:?}, got {body}"
        );
    }
}

/// "Currently, DescribeFleetInstances does not support fleets of type instant"
/// (`DescribeFleetInstances`): an `instant` fleet is refused, and the other two members of
/// the modelled `FleetType` catalog are reported.
#[tokio::test]
async fn an_instant_fleet_is_not_supported_and_the_other_types_are() {
    let (router, engine) = emulator();
    let instance_id = launch(&router, LAUNCHED_TYPES[2]).await;
    past_the_transition().await;

    for fleet_type in FLEET_TYPE_VALUES.iter().copied() {
        let case = format!("fleet type {fleet_type}");
        let fleet_id = seed_fleet(&engine, fleet_type, vec![on_demand(&instance_id)]);
        let (status, body) = describe(&router, &[p("FleetId", &fleet_id)]).await;
        if fleet_type == FLEET_TYPE_INSTANT {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedOperation",
                "case {case:?}: expected UnsupportedOperation, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            active_ids(&case, &body),
            vec![instance_id.clone()],
            "case {case:?}: expected the fleet's running instance, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing and paging
// ---------------------------------------------------------------------------

/// Launch one instance of each of [`LAUNCHED_TYPES`], past the transition that makes them
/// running, into one fleet. Answers the fleet id and the instance ids in id order.
async fn fleet_of_every_type(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
) -> (String, Vec<String>) {
    let mut by_type: BTreeMap<&str, String> = BTreeMap::new();
    for instance_type in LAUNCHED_TYPES {
        by_type.insert(instance_type, launch(router, instance_type).await);
    }
    let fleet_id = seed_fleet(
        engine,
        FLEET_TYPE_MAINTAIN,
        by_type.values().map(|id| on_demand(id)).collect(),
    );
    past_the_transition().await;
    let mut ids: Vec<String> = by_type.values().cloned().collect();
    ids.sort();
    (fleet_id, ids)
}

/// The one documented filter narrows the report: EC2 joins several values with OR and
/// several filters with AND, and matches each value with its `*`/`?` wildcards
/// (`Filter$Values`). One row per shape of selection.
#[tokio::test]
async fn the_instance_type_filter_narrows_the_report() {
    let (router, engine) = emulator();
    let mut launched: BTreeMap<&str, String> = BTreeMap::new();
    for instance_type in LAUNCHED_TYPES {
        launched.insert(instance_type, launch(&router, instance_type).await);
    }
    let fleet_id = seed_fleet(
        &engine,
        FLEET_TYPE_MAINTAIN,
        launched.values().map(|id| on_demand(id)).collect(),
    );
    past_the_transition().await;

    let id_of = |instance_type: &str| launched[instance_type].clone();
    let cases: Vec<FilterCase> = vec![
        (
            "one exact type",
            vec![("instance-type", &["m5.large"][..])],
            vec![id_of("m5.large")],
        ),
        (
            "two values are joined with OR",
            vec![("instance-type", &["m5.large", "t4g.nano"][..])],
            {
                let mut ids = vec![id_of("m5.large"), id_of("t4g.nano")];
                ids.sort();
                ids
            },
        ),
        (
            "a trailing wildcard",
            vec![("instance-type", &["t3.*"][..])],
            vec![id_of("t3.micro")],
        ),
        (
            "a single-character wildcard",
            vec![("instance-type", &["?3.micro"][..])],
            vec![id_of("t3.micro")],
        ),
        (
            "a wildcard spanning families",
            vec![("instance-type", &["t*"][..])],
            {
                let mut ids = vec![id_of("t3.micro"), id_of("t4g.nano")];
                ids.sort();
                ids
            },
        ),
        (
            "a type no instance of the fleet has",
            vec![("instance-type", &["x2gd.medium"][..])],
            Vec::new(),
        ),
        (
            "two filters are joined with AND",
            vec![
                ("instance-type", &["t*"][..]),
                ("instance-type", &["*nano"][..]),
            ],
            vec![id_of("t4g.nano")],
        ),
    ];

    for (case, entries, expected) in cases {
        let params: Vec<(String, String)> = std::iter::once(p("FleetId", &fleet_id))
            .chain(filters(&entries))
            .collect();
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = active_ids(case, &body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused, whether or not any instance
/// would have matched it, so an unsupported filter can never read as "no results".
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    const UNDOCUMENTED: [&str; 5] = [
        "instance-id",
        "instance-health",
        "Instance-Type",
        "tag:Name",
        "",
    ];

    let (router, engine) = emulator();
    let (fleet_id, _) = fleet_of_every_type(&router, &engine).await;

    for name in UNDOCUMENTED {
        let params: Vec<(String, String)> = std::iter::once(p("FleetId", &fleet_id))
            .chain(filters(&[(name, &["anything"][..])]))
            .collect();
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// The report is cut into `MaxResults`-sized pages a `NextToken` continues, in `InstanceId`
/// order, with every instance reported exactly once however the pages fall.
#[tokio::test]
async fn the_report_pages_by_max_results_and_next_token() {
    let (router, engine) = emulator();
    let (fleet_id, expected) = fleet_of_every_type(&router, &engine).await;

    for page_size in [None, Some(1), Some(2), Some(3), Some(4), Some(5)] {
        let case = format!("page size {page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more round than there are instances, so a token that never clears is caught
        // instead of looping forever.
        for _ in 0..=expected.len() {
            let mut params = vec![p("FleetId", &fleet_id)];
            if let Some(size) = page_size {
                params.push(p("MaxResults", size.to_string()));
            }
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = active_ids(&case, &body);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page must hold at most {size} instances, got {page:?}"
                );
            }
            walked.extend(page);
            token = next_token(&case, &body);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end with no token, got {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every instance once in id order, got {walked:?}"
        );
    }
}

/// A page of none is refused and each page size the model leaves unbounded above is
/// served: 1 and [`MAX_PAGE_SIZE`] are pages, so is one past the ceiling, and 0 is not.
#[tokio::test]
async fn a_page_size_outside_the_bounds_is_refused() {
    let (router, engine) = emulator();
    let (fleet_id, expected) = fleet_of_every_type(&router, &engine).await;

    for (case, max_results) in [
        ("the lower bound", 1usize),
        ("the upper bound", MAX_PAGE_SIZE),
        // The model declares no `@range` on this operation's `MaxResults`, so a size
        // above the largest page asks for that page rather than for a refusal.
        ("above the upper bound", MAX_PAGE_SIZE + 1),
    ] {
        let body = described(
            &router,
            case,
            &fleet_id,
            &[p("MaxResults", max_results.to_string())],
        )
        .await;
        let page = active_ids(case, &body);
        assert_eq!(
            page.len(),
            max_results.min(expected.len()),
            "case {case:?}: expected a page of {} instances, got {page:?}",
            max_results.min(expected.len())
        );
    }

    for (case, max_results) in [
        ("below the lower bound", "0".to_string()),
        ("negative", "-1".to_string()),
    ] {
        let (status, body) = describe(
            &router,
            &[p("FleetId", &fleet_id), p("MaxResults", &max_results)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for MaxResults={max_results}, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused rather than read as a position.
#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    const BOGUS: [&str; 4] = ["not-a-token", "!!!!", "aaaa", "i-00000000000000001"];

    let (router, engine) = emulator();
    let (fleet_id, _) = fleet_of_every_type(&router, &engine).await;

    for token in BOGUS {
        let (status, body) = describe(
            &router,
            &[p("FleetId", &fleet_id), p("NextToken", token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun, and that describing is a read
// ---------------------------------------------------------------------------

/// `DryRun` answers `DryRunOperation` only for a request that would otherwise have
/// succeeded; one that would have been refused is refused with THAT reason.
#[tokio::test]
async fn dry_run_answers_the_reason_the_request_would_have_been_refused() {
    let (router, engine) = emulator();
    let (fleet_id, _) = fleet_of_every_type(&router, &engine).await;
    let instant_fleet = seed_fleet(&engine, FLEET_TYPE_INSTANT, Vec::new());

    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a request that would have succeeded",
            fleet_id.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "a malformed fleet id",
            "fleet-1234567890abcdef0".to_string(),
            400,
            "InvalidFleetId.Malformed",
        ),
        (
            "an unknown fleet id",
            WELL_FORMED_FLEET_ID.to_string(),
            400,
            "InvalidFleetId.NotFound",
        ),
        (
            "an instant fleet",
            instant_fleet,
            400,
            "UnsupportedOperation",
        ),
    ];

    for (case, fleet, expected_status, expected_code) in cases {
        let (status, body) = describe(
            &router,
            &[p("FleetId", &fleet), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}

/// Describing a fleet's instances is a READ: two calls answer the same, and neither the
/// fleet store nor the instance store moves because somebody asked.
#[tokio::test]
async fn describing_a_fleet_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let (fleet_id, expected) = fleet_of_every_type(&router, &engine).await;

    /// The store as the assertions below compare it: every fleet with the instances it
    /// names, and every instance with the state it stands in.
    fn snapshot(engine: &engine::InMemoryEngine) -> StoreSnapshot {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let fleets = guard
            .fleets
            .iter()
            .map(|(fleet_id, record)| {
                (
                    fleet_id.clone(),
                    record
                        .instances
                        .iter()
                        .map(|instance| instance.instance_id.clone())
                        .collect(),
                )
            })
            .collect();
        let mut instances: Vec<(String, String)> = guard
            .instances
            .values()
            .map(|instance| (instance.instance_id.clone(), instance.state.as_str().to_string()))
            .collect();
        instances.sort();
        (fleets, instances)
    }

    let first = active_instances(
        "first read",
        &described(&router, "first read", &fleet_id, &[]).await,
    );
    // The snapshot is taken AFTER the first call: the `pending` -> `running` move the clock
    // owed each launch was due at that instant whether or not anybody called, and the first
    // caller is simply who observed it. What must not move is anything after that.
    let before = snapshot(&engine);
    let second = active_instances(
        "second read",
        &described(&router, "second read", &fleet_id, &[]).await,
    );
    let third = active_instances(
        "third read",
        &described(&router, "third read", &fleet_id, &[]).await,
    );
    let after = snapshot(&engine);

    assert_eq!(
        first, second,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert_eq!(
        second, third,
        "a read must not change what a later caller sees: {second:?} then {third:?}"
    );
    assert_eq!(
        first.iter().map(|row| row.instance_id.clone()).collect::<Vec<_>>(),
        expected,
        "the first read must report every running instance of the fleet, got {first:?}"
    );
    assert_eq!(
        before, after,
        "describing must not write to the store: {before:?} then {after:?}"
    );
}
