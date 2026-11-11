// SPDX-License-Identifier: CC0-1.0

//! WHAT the activity log says when a scheduled action fires, and when the target's registered
//! window refuses the firing.
//!
//! The rules under test:
//!
//! * a firing is recorded as its own activity — the DISPATCH — whose `Cause` names the action
//!   that fired, whose `Description` names the `ScalableTargetAction` members it set and their
//!   values, and whose `StatusMessage` reports those bounds as set;
//! * the capacity movement the new bounds cause is a SECOND activity, caused by the bound that
//!   was set, describing the scaled property of the resource's own namespace, and recorded
//!   after the dispatch — so a log read newest-first reports the movement and then the firing
//!   that caused it. A firing whose bounds leave the capacity where it already was records no
//!   second activity, because nothing moved;
//! * the bounds a firing would leave the target with have to be a RANGE: an occurrence that
//!   would put the floor above the ceiling (or the ceiling below the floor) is refused at
//!   dispatch — the dispatch activity settles `Failed` with the inverted-window reason, the
//!   registered bounds stand and the resource's capacity does not move.
//!
//! The catalog spans what the rules generalise over rather than the one case the issue cited:
//! which bound the action sets (the floor, the ceiling, both at once), which direction the
//! movement goes in (out to the floor, in to the ceiling, nowhere), the namespace that owns
//! the scaled resource (`dynamodb` and `ecs`, whose scaled property is named differently), and
//! which end of the window the refusal comes from.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::emulator::{at_schedule_in, resource_id_for, Emulator};

/// The window between queueing an occurrence and its instant. Nothing sleeps for it: the due
/// instant is stored and the clock materialises it once it passes.
const WINDOW: Duration = Duration::from_secs(3);

/// How long a case keeps polling for the activities a firing records: well past [`WINDOW`],
/// the fulfilment span a movement takes and the quarter-second the clock waits between passes.
const POLL_FOR: Duration = Duration::from_secs(25);

/// How long a poll sleeps between reads.
const POLL_EVERY: Duration = Duration::from_millis(200);

/// The statuses of the model's own `ScalingActivityStatusCode` this suite asserts.
const SUCCESSFUL: &str = "Successful";
/// See [`SUCCESSFUL`].
const FAILED: &str = "Failed";

/// The enforcement activity a firing's bounds cause, where they move the capacity at all.
struct Enforcement {
    /// Its `Cause`: the bound that was set, and the value it was set to.
    cause: &'static str,
    /// Its `Description`: the scaled property, as the resource's own namespace names it.
    description: &'static str,
    /// Its `StatusMessage` once the owning service has fulfilled the change.
    status_message: &'static str,
}

/// One firing: the target it fires on, the bounds it sets, and what the log says about it.
struct FiringRow {
    case: &'static str,
    /// The dimension the target scales, which decides both the owning service and the noun the
    /// enforcement activity describes the movement with.
    dimension: &'static str,
    /// The capacity the resource is created with, which is the capacity the firing is measured
    /// against.
    starting: i64,
    /// The window the target is registered with.
    registered: (i64, i64),
    /// The `ScalableTargetAction` the action carries: its `MinCapacity`, its `MaxCapacity`, or
    /// both.
    scheduled: (Option<i64>, Option<i64>),
    /// The dispatch activity's `Description`.
    dispatch_description: &'static str,
    /// Its terminal `StatusCode`.
    dispatch_status: &'static str,
    /// Its `StatusMessage`.
    dispatch_message: &'static str,
    /// The enforcement activity beside it, or `None` for a firing that moved nothing.
    enforcement: Option<Enforcement>,
    /// The window the target carries afterwards.
    bounds_after: (i64, i64),
    /// The capacity the resource holds afterwards.
    capacity_after: i64,
}

/// The catalog. Every row is a firing of ONE action, so the activities it is asserted on are
/// exactly the ones that firing produced.
const FIRING_ROWS: &[FiringRow] = &[
    FiringRow {
        case: "a floor above the live capacity, on a table's read capacity",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 1,
        registered: (1, 20),
        scheduled: (Some(4), None),
        dispatch_description: "Setting min capacity to 4",
        dispatch_status: SUCCESSFUL,
        dispatch_message: "Successfully set min capacity to 4",
        enforcement: Some(Enforcement {
            cause: "minimum capacity was set to 4",
            description: "Setting read capacity units to 4.",
            status_message: "Successfully set read capacity units to 4. Change successfully \
                             fulfilled by dynamodb.",
        }),
        bounds_after: (4, 20),
        capacity_after: 4,
    },
    FiringRow {
        case: "a ceiling below the live capacity, on a table's write capacity",
        dimension: "dynamodb:table:WriteCapacityUnits",
        starting: 8,
        registered: (1, 20),
        scheduled: (None, Some(3)),
        dispatch_description: "Setting max capacity to 3",
        dispatch_status: SUCCESSFUL,
        dispatch_message: "Successfully set max capacity to 3",
        enforcement: Some(Enforcement {
            cause: "maximum capacity was set to 3",
            description: "Setting write capacity units to 3.",
            status_message: "Successfully set write capacity units to 3. Change successfully \
                             fulfilled by dynamodb.",
        }),
        bounds_after: (1, 3),
        capacity_after: 3,
    },
    FiringRow {
        case: "a floor on an ECS service's desired count",
        dimension: "ecs:service:DesiredCount",
        starting: 1,
        registered: (1, 20),
        scheduled: (Some(3), None),
        dispatch_description: "Setting min capacity to 3",
        dispatch_status: SUCCESSFUL,
        dispatch_message: "Successfully set min capacity to 3",
        enforcement: Some(Enforcement {
            cause: "minimum capacity was set to 3",
            description: "Setting desired count to 3.",
            status_message: "Successfully set desired count to 3. Change successfully fulfilled \
                             by ecs.",
        }),
        bounds_after: (3, 20),
        capacity_after: 3,
    },
    FiringRow {
        case: "both bounds set by one action",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 1,
        registered: (1, 20),
        scheduled: (Some(5), Some(9)),
        dispatch_description: "Setting min capacity to 5 and max capacity to 9",
        dispatch_status: SUCCESSFUL,
        dispatch_message: "Successfully set min capacity to 5 and max capacity to 9",
        enforcement: Some(Enforcement {
            cause: "minimum capacity was set to 5",
            description: "Setting read capacity units to 5.",
            status_message: "Successfully set read capacity units to 5. Change successfully \
                             fulfilled by dynamodb.",
        }),
        bounds_after: (5, 9),
        capacity_after: 5,
    },
    FiringRow {
        case: "a window the live capacity is already inside, which moves nothing",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 5,
        registered: (1, 20),
        scheduled: (None, Some(10)),
        dispatch_description: "Setting max capacity to 10",
        dispatch_status: SUCCESSFUL,
        dispatch_message: "Successfully set max capacity to 10",
        enforcement: None,
        bounds_after: (1, 10),
        capacity_after: 5,
    },
    FiringRow {
        case: "a floor above the registered ceiling",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 1,
        registered: (1, 10),
        scheduled: (Some(25), None),
        dispatch_description: "Setting min capacity to 25",
        dispatch_status: FAILED,
        dispatch_message: "Failed to set min capacity to 25. Reason: Maximum capacity cannot be \
                           less than minimum capacity",
        enforcement: None,
        bounds_after: (1, 10),
        capacity_after: 1,
    },
    FiringRow {
        case: "a ceiling below the registered floor",
        dimension: "dynamodb:table:WriteCapacityUnits",
        starting: 5,
        registered: (4, 20),
        scheduled: (None, Some(2)),
        dispatch_description: "Setting max capacity to 2",
        dispatch_status: FAILED,
        dispatch_message: "Failed to set max capacity to 2. Reason: Maximum capacity cannot be \
                           less than minimum capacity",
        enforcement: None,
        bounds_after: (4, 20),
        capacity_after: 5,
    },
];

#[tokio::test]
async fn a_firing_records_its_dispatch_and_the_movement_it_causes() {
    for row in FIRING_ROWS {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = given_target_at(&emulator, row).await;

        let mut action = target.clone();
        action["ScheduledActionName"] = json!("the-action");
        action["Schedule"] = json!(at_schedule_in(WINDOW).0);
        let mut scalable_target_action = json!({});
        if let Some(min) = row.scheduled.0 {
            scalable_target_action["MinCapacity"] = json!(min);
        }
        if let Some(max) = row.scheduled.1 {
            scalable_target_action["MaxCapacity"] = json!(max);
        }
        action["ScalableTargetAction"] = scalable_target_action;
        let (status, body) = emulator.call("PutScheduledAction", action).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScheduledAction to answer 200, got {status} {body:?}"
        );

        // The log is read newest-first, so the activities a firing produced are the newest
        // ones: the movement, then the dispatch that caused it.
        let wanted = 1 + usize::from(row.enforcement.is_some());
        let activities = poll_for_terminal_activities(&emulator, &target, wanted).await;
        assert_eq!(
            activities.len(),
            wanted,
            "case {case:?}: expected the firing to record {wanted} activity(ies), got \
             {activities:?}"
        );

        let dispatch = &activities[activities.len() - 1];
        let dispatch_cause = "scheduled action name the-action was triggered";
        assert_eq!(
            field(dispatch, "Cause"),
            dispatch_cause,
            "case {case:?}: expected the dispatch activity to name the action that fired, got \
             {dispatch:?}"
        );
        assert_eq!(
            field(dispatch, "Description"),
            row.dispatch_description,
            "case {case:?}: expected the dispatch to describe the bounds it set, got {dispatch:?}"
        );
        assert_eq!(
            field(dispatch, "StatusCode"),
            row.dispatch_status,
            "case {case:?}: expected the dispatch to settle {}, got {dispatch:?}",
            row.dispatch_status
        );
        assert_eq!(
            field(dispatch, "StatusMessage"),
            row.dispatch_message,
            "case {case:?}: expected the dispatch's own message, got {dispatch:?}"
        );

        match &row.enforcement {
            None => {}
            Some(enforcement) => {
                let movement = &activities[0];
                assert_eq!(
                    field(movement, "Cause"),
                    enforcement.cause,
                    "case {case:?}: expected the movement to be caused by the bound that was \
                     set, got {movement:?}"
                );
                assert_eq!(
                    field(movement, "Description"),
                    enforcement.description,
                    "case {case:?}: expected the movement to describe the scaled property, got \
                     {movement:?}"
                );
                assert_eq!(
                    field(movement, "StatusCode"),
                    SUCCESSFUL,
                    "case {case:?}: expected the movement to settle {SUCCESSFUL}, got {movement:?}"
                );
                assert_eq!(
                    field(movement, "StatusMessage"),
                    enforcement.status_message,
                    "case {case:?}: expected the fulfilled movement's message, got {movement:?}"
                );
            }
        }

        // And the resource and the registration afterwards: a refused firing left both exactly
        // as they were.
        let bounds = registered_bounds(&emulator, &target).await;
        assert_eq!(
            bounds, row.bounds_after,
            "case {case:?}: expected the target to carry the bounds {:?}, got {bounds:?}",
            row.bounds_after
        );
        let capacity = capacity_of(&emulator, &target).await;
        assert_eq!(
            capacity, row.capacity_after,
            "case {case:?}: expected the resource to hold {} units, got {capacity}",
            row.capacity_after
        );
    }
}

/// Register the target `row` fires on, with its resource created at `row.starting` units — so
/// the capacity the firing is measured against is not the registered floor.
async fn given_target_at(emulator: &Emulator, row: &FiringRow) -> Value {
    let namespace = row.dimension.split(':').next().unwrap_or_default();
    let resource_id = resource_id_for(row.dimension);
    emulator
        .given_resource(namespace, &resource_id, row.starting)
        .await;
    let target = json!({
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": row.dimension,
    });
    let mut registration = target.clone();
    registration["MinCapacity"] = json!(row.registered.0);
    registration["MaxCapacity"] = json!(row.registered.1);
    let (status, body) = emulator.call("RegisterScalableTarget", registration).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}",
        row.case
    );
    target
}

/// Poll the activity log until it holds at least `wanted` activities for `target` and every one
/// of them has reached a terminal status, and answer them newest first.
async fn poll_for_terminal_activities(
    emulator: &Emulator,
    target: &Value,
    wanted: usize,
) -> Vec<Value> {
    let settled = |activities: &[Value]| {
        activities.len() >= wanted
            && activities
                .iter()
                .all(|activity| field(activity, "StatusCode") != "InProgress")
    };
    let deadline = SystemTime::now() + POLL_FOR;
    let mut last = activities(emulator, target).await;
    while !settled(&last) && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        last = activities(emulator, target).await;
    }
    last
}

/// The activities recorded for one target, newest first.
async fn activities(emulator: &Emulator, target: &Value) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingActivities", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the activity log\": expected 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One string member of an activity, or the empty string where it carries none.
fn field(activity: &Value, member: &str) -> String {
    activity[member].as_str().unwrap_or_default().to_string()
}

/// The window `DescribeScalableTargets` reports for `target`.
async fn registered_bounds(emulator: &Emulator, target: &Value) -> (i64, i64) {
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({
                "ServiceNamespace": target["ServiceNamespace"],
                "ResourceIds": [target["ResourceId"]],
                "ScalableDimension": target["ScalableDimension"],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the registration\": expected 200, got {status} {body:?}"
    );
    let reported = &body["ScalableTargets"][0];
    (
        reported["MinCapacity"].as_i64().unwrap_or_default(),
        reported["MaxCapacity"].as_i64().unwrap_or_default(),
    )
}

/// The capacity the service that OWNS the resource `target` scales reports for it.
async fn capacity_of(emulator: &Emulator, target: &Value) -> i64 {
    let dimension = target["ScalableDimension"].as_str().unwrap_or_default();
    let resource_id = target["ResourceId"].as_str().unwrap_or_default();
    match target["ServiceNamespace"].as_str().unwrap_or_default() {
        "dynamodb" => {
            emulator
                .provisioned_capacity(resource_id, dimension.ends_with("ReadCapacityUnits"))
                .await
        }
        "ecs" => {
            let (cluster, service) = resource_id
                .trim_start_matches("service/")
                .split_once('/')
                .expect("an ECS resource id names its cluster and its service");
            emulator.ecs_desired_count(cluster, service).await
        }
        other => panic!("case {other:?}: this suite reads no capacity for that namespace"),
    }
}
