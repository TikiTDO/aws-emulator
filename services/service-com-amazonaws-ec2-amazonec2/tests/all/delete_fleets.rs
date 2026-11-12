// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteFleets`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a deletion MOVES THE FLEET TO THE DELETED STATE `TerminateInstances` CHOOSES —
//!    `deleted_terminating` when the instances the fleet placed are terminated with it,
//!    `deleted_running` when they are left to run — and the call reports the state the fleet
//!    was in beside the one a describe now reports;
//! 2. "after you delete an EC2 Fleet request, it launches no new instances": every Spot
//!    Instance request the fleet placed is cancelled, and a deleted fleet reaches `deleted`
//!    once it has NO INSTANCE LEFT — owed to the clock and to those instances, so a fleet
//!    whose instances keep running stays `deleted_running` until something terminates them;
//! 3. an id that names no deletable fleet is ONE ENTRY of `UnsuccessfulFleetDeletions` with
//!    its `DeleteFleetErrorCode`, never a refusal of the call: the other ids are still
//!    deleted;
//! 4. a request that cannot be served at all is refused and deletes nothing — `DryRun`
//!    performs nothing, a batch above the documented bound of 125 ids deletes no fleet, and
//!    `no-terminate-instances` is refused for a fleet of type `instant`.
//!
//! Each table row is a distinct input along what the rule generalises over — which fleet
//! type, which `TerminateInstances`, how much target capacity, how many pools, whether the
//! fleet is still `submitted` when the delete lands, which unservable id — so no row is the
//! specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DELETE_FLEET_ID_DOES_NOT_EXIST,
    DELETE_FLEET_ID_DOES_NOT_EXIST_MESSAGE, DELETE_FLEET_ID_MALFORMED,
    DELETE_FLEET_NOT_IN_DELETABLE_STATE, FLEET_ACTIVITY_STATUS_FULFILLED,
    FLEET_ACTIVITY_STATUS_PENDING_TERMINATION, FLEET_STATE_ACTIVE, FLEET_STATE_DELETED,
    FLEET_STATE_DELETED_RUNNING, FLEET_STATE_DELETED_TERMINATING, FLEET_STATE_SUBMITTED,
    FLEET_TYPE_INSTANT, FLEET_TYPE_MAINTAIN, FLEET_TYPE_REQUEST, MAX_FLEETS_PER_DELETE,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_state, element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A well-formed EC2 Fleet id no fleet of the account is stored under: the mint counts from
/// one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "fleet-0000ffff-0000-0000-0000-00000000ffff";

/// Something that is not an EC2 Fleet id at all.
const MALFORMED_FLEET_ID: &str = "fleet-not-an-id";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, read out of the service's
/// OWN state: a launch template must name an image the account can resolve, so the test
/// cannot invent an id.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// Create one launch template and answer its `LaunchTemplateId`.
async fn launch_template(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "delete-fleets-suite"),
            p("LaunchTemplateData.ImageId", image_id()),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no launchTemplateId in {body}"))
}

/// Create one fleet of `fleet_type` asking for `total_target_capacity` units, with `extra`
/// parameters, and answer its `FleetId`.
async fn create_fleet(
    router: &MultiServiceRouter,
    case: &str,
    template_id: &str,
    fleet_type: &str,
    total_target_capacity: i32,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![
        p(
            "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
            template_id,
        ),
        p("Type", fleet_type),
        p(
            "TargetCapacitySpecification.TotalTargetCapacity",
            total_target_capacity.to_string(),
        ),
    ];
    params.extend(extra.iter().cloned());
    let (status, body) = call(router, REGION, "CreateFleet", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateFleet must succeed for a {fleet_type} fleet, got {status}: {body}"
    );
    element(&body, "fleetId").unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"))
}

/// One fleet of `fleet_type` with its own launch template, for a case that needs nothing else
/// of it.
async fn one_fleet(
    router: &MultiServiceRouter,
    case: &str,
    fleet_type: &str,
    total_target_capacity: i32,
) -> String {
    let template_id = launch_template(router, case).await;
    create_fleet(
        router,
        case,
        &template_id,
        fleet_type,
        total_target_capacity,
        &[],
    )
    .await
}

/// The `FleetId.N` and `TerminateInstances` fields of one deletion.
fn delete_params(ids: &[&str], terminate_instances: bool) -> Params {
    let mut params = indexed("FleetId", ids);
    params.push(p(
        "TerminateInstances",
        if terminate_instances { "true" } else { "false" },
    ));
    params
}

async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteFleets", params).await
}

/// One entry of `SuccessfulFleetDeletions`.
#[derive(Debug)]
struct Deleted {
    id: String,
    previous: String,
    current: String,
}

/// Every fleet a deletion reports as deleted, in document order.
fn deleted(case: &str, body: &str) -> Vec<Deleted> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "successfulFleetDeletionSet")
        .into_iter()
        .map(|item| Deleted {
            id: child_text(item, "fleetId").unwrap_or_default(),
            previous: child_text(item, "previousFleetState").unwrap_or_default(),
            current: child_text(item, "currentFleetState").unwrap_or_default(),
        })
        .collect()
}

/// One entry of `UnsuccessfulFleetDeletions`.
#[derive(Debug)]
struct NotDeleted {
    id: String,
    code: String,
    message: String,
}

/// Every fleet a deletion reports as not deleted, in document order.
fn not_deleted(case: &str, body: &str) -> Vec<NotDeleted> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "unsuccessfulFleetDeletionSet")
        .into_iter()
        .map(|item| NotDeleted {
            id: child_text(item, "fleetId").unwrap_or_default(),
            code: child(item, "error")
                .and_then(|error| child_text(error, "code"))
                .unwrap_or_default(),
            message: child(item, "error")
                .and_then(|error| child_text(error, "message"))
                .unwrap_or_default(),
        })
        .collect()
}

/// The `FleetStateCode` and the `ActivityStatus` `DescribeFleets` reports for one fleet.
///
/// The fleet is always named by id: "if a fleet is of type `instant`, you must specify the
/// fleet ID in the request" (`DescribeFleets`).
async fn described_fleet(case: &str, router: &MultiServiceRouter, fleet_id: &str) -> (String, String) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeFleets",
        &indexed("FleetId", &[fleet_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeFleets, got {status}: {body}"
    );
    (
        element(&body, "fleetState")
            .unwrap_or_else(|| panic!("case {case:?}: no fleetState reported for {fleet_id}: {body}")),
        element(&body, "activityStatus").unwrap_or_default(),
    )
}

/// The ids of the instances the fleet `fleet_id` placed, in launch order, read out of the
/// store the handlers share.
///
/// Read from the STATE because `DescribeFleetInstances` "does not support fleets of type
/// instant", and the rows below span every fleet type; each instance's own STATE is then read
/// back through `DescribeInstances`.
fn placed_instance_ids(engine: &engine::InMemoryEngine, case: &str, fleet_id: &str) -> Vec<String> {
    let guard = engine.state::<Ec2State>();
    let store = guard.lock();
    store
        .fleets
        .get(fleet_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the fleet {fleet_id}"))
        .instances
        .iter()
        .map(|placed| placed.instance_id.clone())
        .collect()
}

/// Every Spot Instance request the account holds, as `(state, status code)`.
async fn spot_requests(case: &str, router: &MultiServiceRouter) -> Vec<(String, String)> {
    let (status, body) = call(router, REGION, "DescribeSpotInstanceRequests", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotInstanceRequests, got {status}: {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && child(*node, "spotInstanceRequestId").is_some()
                && child(*node, "state").is_some()
        })
        .map(|item| {
            (
                child_text(item, "state").unwrap_or_default(),
                child(item, "status")
                    .and_then(|status| child_text(status, "code"))
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// One row of the deletion table: a fleet that differs from the others along what rule 1
/// generalises over.
struct DeleteCase {
    case: &'static str,
    fleet_type: &'static str,
    total_target_capacity: i32,
    /// Extra `CreateFleet` parameters — a second pool, a weighted capacity.
    extra: fn() -> Params,
    /// Whether the fleet is left to become `active` before the delete lands, which is what
    /// makes the state it was deleted FROM differ.
    active_first: bool,
    terminate_instances: bool,
    expected_previous: &'static str,
    expected_current: &'static str,
    expected_activity: &'static str,
    /// How many instances the fleet placed, every one of which has to be in the state below.
    expected_instances: usize,
    expected_instance_state: &'static str,
}

/// The rows of the deletion table. No row repeats another's combination of fleet type,
/// `TerminateInstances`, target capacity, pool count and state at deletion.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a maintain fleet deleted with its instances before it is active",
        fleet_type: FLEET_TYPE_MAINTAIN,
        total_target_capacity: 1,
        extra: Vec::new,
        active_first: false,
        terminate_instances: true,
        expected_previous: FLEET_STATE_SUBMITTED,
        expected_current: FLEET_STATE_DELETED_TERMINATING,
        // "pending_termination while instances are terminating when fleet size is decreased"
        // (`FleetData$ActivityStatus`).
        expected_activity: FLEET_ACTIVITY_STATUS_PENDING_TERMINATION,
        expected_instances: 1,
        expected_instance_state: "shutting-down",
    },
    DeleteCase {
        case: "an active maintain fleet deleted leaving its two instances running",
        fleet_type: FLEET_TYPE_MAINTAIN,
        total_target_capacity: 2,
        extra: Vec::new,
        active_first: true,
        terminate_instances: false,
        expected_previous: FLEET_STATE_ACTIVE,
        expected_current: FLEET_STATE_DELETED_RUNNING,
        // Nothing is terminating, and the fleet placed the whole capacity its target asked
        // for.
        expected_activity: FLEET_ACTIVITY_STATUS_FULFILLED,
        expected_instances: 2,
        expected_instance_state: "running",
    },
    DeleteCase {
        case: "an active request fleet of two pools deleted with its instances",
        fleet_type: FLEET_TYPE_REQUEST,
        total_target_capacity: 2,
        extra: || {
            vec![
                p("LaunchTemplateConfigs.1.Overrides.1.InstanceType", "t3.small"),
                p("LaunchTemplateConfigs.1.Overrides.2.InstanceType", "c5.large"),
            ]
        },
        active_first: true,
        terminate_instances: true,
        expected_previous: FLEET_STATE_ACTIVE,
        expected_current: FLEET_STATE_DELETED_TERMINATING,
        expected_activity: FLEET_ACTIVITY_STATUS_PENDING_TERMINATION,
        expected_instances: 2,
        expected_instance_state: "shutting-down",
    },
    DeleteCase {
        case: "an instant fleet, whose pool provides two units per instance, deleted",
        fleet_type: FLEET_TYPE_INSTANT,
        total_target_capacity: 4,
        extra: || {
            vec![p(
                "LaunchTemplateConfigs.1.Overrides.1.WeightedCapacity",
                "2.0",
            )]
        },
        active_first: false,
        terminate_instances: true,
        // An `instant` fleet is active as soon as it is created.
        expected_previous: FLEET_STATE_ACTIVE,
        expected_current: FLEET_STATE_DELETED_TERMINATING,
        // "For fleets of type `instant`, the status is `fulfilled` … (this is the only
        // possible status for `instant` fleets)" (`FleetData$ActivityStatus`).
        expected_activity: FLEET_ACTIVITY_STATUS_FULFILLED,
        expected_instances: 2,
        expected_instance_state: "shutting-down",
    },
    DeleteCase {
        case: "a maintain fleet that has placed no capacity yet",
        fleet_type: FLEET_TYPE_MAINTAIN,
        total_target_capacity: 0,
        extra: Vec::new,
        active_first: false,
        terminate_instances: true,
        expected_previous: FLEET_STATE_SUBMITTED,
        expected_current: FLEET_STATE_DELETED_TERMINATING,
        // No instance is terminating, so the deletion leaves the status the create derived.
        expected_activity: FLEET_ACTIVITY_STATUS_FULFILLED,
        expected_instances: 0,
        expected_instance_state: "running",
    },
];

#[tokio::test]
async fn a_deletion_moves_the_fleet_to_the_state_terminate_instances_chooses() {
    for row in DELETE_CASES {
        let case = row.case;
        let (router, engine) = emulator();
        let template_id = launch_template(&router, case).await;
        let fleet_id = create_fleet(
            &router,
            case,
            &template_id,
            row.fleet_type,
            row.total_target_capacity,
            &(row.extra)(),
        )
        .await;
        let instance_ids = placed_instance_ids(&engine, case, &fleet_id);
        assert_eq!(
            instance_ids.len(),
            row.expected_instances,
            "case {case:?}: expected the fleet to have placed {} instance(s), got \
             {instance_ids:?}",
            row.expected_instances
        );
        if row.active_first {
            // One span is what the fleet's `submitted` -> `active` move takes, and what its
            // instances take to reach `running`.
            past_the_transition().await;
        }

        let (status, body) = delete(
            &router,
            &delete_params(&[fleet_id.as_str()], row.terminate_instances),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteFleets, got {status}: {body}"
        );
        let reported = deleted(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one deleted fleet, got {reported:?}"
        );
        let reported = &reported[0];
        assert_eq!(
            reported.id, fleet_id,
            "case {case:?}: expected the deleted fleet {fleet_id}, got {reported:?}"
        );
        assert_eq!(
            reported.previous, row.expected_previous,
            "case {case:?}: expected the previous state {:?}, got {reported:?}",
            row.expected_previous
        );
        assert_eq!(
            reported.current, row.expected_current,
            "case {case:?}: expected the current state {:?}, got {reported:?}",
            row.expected_current
        );
        assert!(
            not_deleted(case, &body).is_empty(),
            "case {case:?}: a deleted fleet is in no unsuccessful set, got {:?}",
            not_deleted(case, &body)
        );

        // What the call said is what a describe says.
        let (state, activity) = described_fleet(case, &router, &fleet_id).await;
        assert_eq!(
            state, row.expected_current,
            "case {case:?}: expected the describe to report {:?}, got {state:?}",
            row.expected_current
        );
        assert_eq!(
            activity, row.expected_activity,
            "case {case:?}: expected the activity status {:?}, got {activity:?}",
            row.expected_activity
        );

        // Every instance the fleet placed is in the state the flag chose, read back through
        // `DescribeInstances`.
        for instance_id in &instance_ids {
            let state = described_instance_state(&router, REGION, instance_id).await;
            assert_eq!(
                state, row.expected_instance_state,
                "case {case:?}: expected the instance {instance_id} in {:?}, got {state:?}",
                row.expected_instance_state
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_fleet_launches_no_new_instances() {
    // Every Spot Instance request the fleet placed is cancelled, whichever way the fleet was
    // deleted, so the clock launches nothing more for it.
    for terminate_instances in [true, false] {
        let case = if terminate_instances {
            "deleted with its instances"
        } else {
            "deleted leaving its instances running"
        };
        let (router, _engine) = emulator();
        let fleet_id = one_fleet(&router, case, FLEET_TYPE_MAINTAIN, 2).await;
        let before = spot_requests(case, &router).await;
        assert_eq!(
            before.len(),
            2,
            "case {case:?}: the fleet placed one Spot request per unit of capacity, got \
             {before:?}"
        );

        let (status, body) =
            delete(&router, &delete_params(&[fleet_id.as_str()], terminate_instances)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteFleets, got {status}: {body}"
        );
        past_the_transition().await;
        let after = spot_requests(case, &router).await;
        assert_eq!(
            after.len(),
            before.len(),
            "case {case:?}: a deletion places no further Spot request, got {after:?}"
        );
        for (state, status_code) in &after {
            assert_eq!(
                state, "cancelled",
                "case {case:?}: every Spot request of a deleted fleet is cancelled, got \
                 {after:?}"
            );
            assert!(
                !status_code.is_empty(),
                "case {case:?}: a cancelled request carries the status code for the way it \
                 ended, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_fleet_reaches_deleted_when_its_instances_are_gone() {
    // The fleet's own termination, a caller's, and a fleet that placed nothing at all: either
    // way the fleet is `deleted` once no instance it placed is still held, and not before.
    for terminate_with_the_fleet in [true, false] {
        let case = if terminate_with_the_fleet {
            "the fleet terminates its own instance"
        } else {
            "the instance is terminated manually afterwards"
        };
        let (router, engine) = emulator();
        let fleet_id = one_fleet(&router, case, FLEET_TYPE_MAINTAIN, 1).await;
        let instance_id = placed_instance_ids(&engine, case, &fleet_id)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the fleet must have placed an instance"));

        let (status, body) = delete(
            &router,
            &delete_params(&[fleet_id.as_str()], terminate_with_the_fleet),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteFleets, got {status}: {body}"
        );

        past_the_transition().await;
        let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
        if terminate_with_the_fleet {
            assert_eq!(
                state, FLEET_STATE_DELETED,
                "case {case:?}: the fleet whose instance has terminated is \
                 {FLEET_STATE_DELETED:?}, got {state:?}"
            );
            continue;
        }
        assert_eq!(
            state, FLEET_STATE_DELETED_RUNNING,
            "case {case:?}: a fleet whose instance still runs stays \
             {FLEET_STATE_DELETED_RUNNING:?}, got {state:?}"
        );
        let (status, body) = call(
            &router,
            REGION,
            "TerminateInstances",
            &indexed("InstanceId", &[instance_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from TerminateInstances, got {status}: {body}"
        );
        past_the_transition().await;
        past_the_transition().await;
        let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
        assert_eq!(
            state, FLEET_STATE_DELETED,
            "case {case:?}: once its instance is gone the fleet is {FLEET_STATE_DELETED:?}, \
             got {state:?}"
        );
    }

    // "A target capacity of 0 is supported only for fleets of type maintain", so that is the
    // type of the fleet that waits for no instance at all.
    let case = "a fleet that placed no capacity at all";
    let (router, _engine) = emulator();
    let fleet_id = one_fleet(&router, case, FLEET_TYPE_MAINTAIN, 0).await;
    let (status, body) = delete(&router, &delete_params(&[fleet_id.as_str()], true)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteFleets, got {status}: {body}"
    );
    past_the_transition().await;
    let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
    assert_eq!(
        state, FLEET_STATE_DELETED,
        "case {case:?}: a deleted fleet with no instance to wait for reaches \
         {FLEET_STATE_DELETED:?}, got {state:?}"
    );
}

/// One row of the per-fleet error table: an id that names no deletable fleet, the
/// `DeleteFleetErrorCode` it is reported with and the message that carries it. The fixed text
/// of each message is EC2's; the id in one, and the state in another, come from the request
/// and the store.
struct UnservableCase {
    case: &'static str,
    /// Whether the fleet the row names has to be deleted before the call under test, which is
    /// what leaves it in a state it cannot be deleted from.
    delete_first: bool,
    /// The id to name, or `None` to name the fleet this row deleted first.
    id: Option<&'static str>,
    expected_code: &'static str,
    /// The message, given the id the call named.
    expected_message: fn(&str) -> String,
}

fn unservable_cases() -> Vec<UnservableCase> {
    vec![
        UnservableCase {
            case: "something that is not an EC2 Fleet id at all",
            delete_first: false,
            id: Some(MALFORMED_FLEET_ID),
            expected_code: DELETE_FLEET_ID_MALFORMED,
            expected_message: |id| format!("Invalid id: \"{id}\""),
        },
        UnservableCase {
            case: "a well-formed id the account holds no fleet for",
            delete_first: false,
            id: Some(UNKNOWN_FLEET_ID),
            expected_code: DELETE_FLEET_ID_DOES_NOT_EXIST,
            expected_message: |_| DELETE_FLEET_ID_DOES_NOT_EXIST_MESSAGE.to_string(),
        },
        UnservableCase {
            case: "the null fleet id, which is a well-formed id of no fleet",
            delete_first: false,
            id: Some("fleet-00000000-0000-0000-0000-000000000000"),
            expected_code: DELETE_FLEET_ID_DOES_NOT_EXIST,
            expected_message: |_| DELETE_FLEET_ID_DOES_NOT_EXIST_MESSAGE.to_string(),
        },
        UnservableCase {
            case: "a fleet that has already been deleted",
            delete_first: true,
            id: None,
            expected_code: DELETE_FLEET_NOT_IN_DELETABLE_STATE,
            expected_message: |_| {
                format!("Fleet deletion failed. Fleet is in the {FLEET_STATE_DELETED_RUNNING} state.")
            },
        },
    ]
}

#[tokio::test]
async fn an_id_that_names_no_deletable_fleet_is_reported_per_fleet() {
    for row in unservable_cases() {
        let case = row.case;
        let (router, _engine) = emulator();
        let template_id = launch_template(&router, case).await;
        // One fleet that CAN be deleted, named in the same batch: the per-fleet error must not
        // stop it being deleted.
        let healthy = create_fleet(&router, case, &template_id, FLEET_TYPE_MAINTAIN, 1, &[]).await;
        let other = create_fleet(&router, case, &template_id, FLEET_TYPE_REQUEST, 1, &[]).await;
        if row.delete_first {
            let (status, body) = delete(&router, &delete_params(&[other.as_str()], false)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first deletion must succeed, got {status}: {body}"
            );
        }
        let named = row.id.unwrap_or(other.as_str()).to_string();
        let (status, body) = delete(
            &router,
            &delete_params(&[named.as_str(), healthy.as_str()], false),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a per-fleet error does not refuse the call, got {status}: {body}"
        );
        let failed = not_deleted(case, &body);
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one unsuccessful fleet, got {failed:?}"
        );
        let failed = &failed[0];
        assert_eq!(
            failed.id, named,
            "case {case:?}: expected the unsuccessful fleet {named}, got {failed:?}"
        );
        assert_eq!(
            failed.code, row.expected_code,
            "case {case:?}: expected the code {:?}, got {failed:?}",
            row.expected_code
        );
        assert_eq!(
            failed.message,
            (row.expected_message)(&named),
            "case {case:?}: expected the message {:?}, got {failed:?}",
            (row.expected_message)(&named)
        );
        let reported = deleted(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the deletable fleet of the batch is still deleted, got {reported:?}"
        );
        assert_eq!(
            reported[0].id, healthy,
            "case {case:?}: expected {healthy} deleted, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_cannot_be_served_deletes_nothing() {
    let case = "a dry run";
    let (router, _engine) = emulator();
    let fleet_id = one_fleet(&router, case, FLEET_TYPE_MAINTAIN, 1).await;
    let mut params = delete_params(&[fleet_id.as_str()], true);
    params.push(p("DryRun", "true"));
    let (status, body) = delete(&router, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 for a dry run, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got the body {body}"
    );
    let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
    assert_eq!(
        state, FLEET_STATE_SUBMITTED,
        "case {case:?}: a dry run deletes nothing, got {state:?}"
    );

    // "You can delete up to 125 fleets in a single request … If you exceed the specified
    // number of fleets to delete, no fleets are deleted."
    let case = "one id more than the documented bound";
    let mut ids: Vec<String> = vec![fleet_id.clone()];
    while ids.len() <= MAX_FLEETS_PER_DELETE {
        ids.push(UNKNOWN_FLEET_ID.to_string());
    }
    let named: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, body) = delete(&router, &delete_params(&named, true)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for {} ids, got {status}: {body}",
        named.len()
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got the body {body}"
    );
    assert!(
        error_message(&body).contains(&MAX_FLEETS_PER_DELETE.to_string()),
        "case {case:?}: the refusal names the bound, got {:?}",
        error_message(&body)
    );
    let (state, _activity) = described_fleet(case, &router, &fleet_id).await;
    assert_eq!(
        state, FLEET_STATE_SUBMITTED,
        "case {case:?}: an oversized batch deletes no fleet, got {state:?}"
    );

    // One fleet named twice asks for one deletion.
    let case = "one fleet named twice";
    let (status, body) = delete(
        &router,
        &delete_params(&[fleet_id.as_str(), fleet_id.as_str()], true),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteFleets, got {status}: {body}"
    );
    let reported = deleted(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: one fleet gets one entry however often it is named, got {reported:?}"
    );

    // "For `instant` fleets, you cannot specify `NoTerminateInstances`"
    // (`DeleteFleetsRequest$TerminateInstances`).
    let case = "no-terminate-instances for an instant fleet";
    let (router, _engine) = emulator();
    let template_id = launch_template(&router, case).await;
    let instant = create_fleet(&router, case, &template_id, FLEET_TYPE_INSTANT, 1, &[]).await;
    let maintain = create_fleet(&router, case, &template_id, FLEET_TYPE_MAINTAIN, 1, &[]).await;
    let (status, body) = delete(
        &router,
        &delete_params(&[instant.as_str(), maintain.as_str()], false),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "UnsupportedOperation",
        "case {case:?}: expected UnsupportedOperation, got the body {body}"
    );
    for (fleet_id, expected) in [
        (instant.as_str(), FLEET_STATE_ACTIVE),
        (maintain.as_str(), FLEET_STATE_SUBMITTED),
    ] {
        let (state, _activity) = described_fleet(case, &router, fleet_id).await;
        assert_eq!(
            state, expected,
            "case {case:?}: the refused call deletes neither fleet, expected {fleet_id} in \
             {expected:?}, got {state:?}"
        );
    }
    // The same fleets ARE deleted when the call terminates their instances.
    let (status, body) = delete(
        &router,
        &delete_params(&[instant.as_str(), maintain.as_str()], true),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 once the instances are terminated with the fleets, \
         got {status}: {body}"
    );
    assert_eq!(
        deleted(case, &body).len(),
        2,
        "case {case:?}: both fleets are deleted, got {:?}",
        deleted(case, &body)
    );
}
