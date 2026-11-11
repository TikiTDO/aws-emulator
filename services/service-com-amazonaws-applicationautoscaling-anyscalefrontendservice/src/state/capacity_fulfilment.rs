// SPDX-License-Identifier: CC0-1.0

//! FULFILMENT of a capacity movement: when the service that owns the scaled resource has
//! made the capacity this service set take effect.
//!
//! A capacity movement is two things, not one. This service DECIDES a capacity and sets it
//! on the resource, which is what the movement's activity records the instant it happens
//! ("Setting desired count to 3."); the owning service then has to make that capacity real,
//! which is what the same activity reports it is still waiting for ("Successfully set
//! desired count to 3. Waiting for change to be fulfilled by ecs.") until it has happened
//! ("Change successfully fulfilled by ecs."). So the activity's terminal status is NOT owed
//! by this service's own clock: a movement reaches `Successful` only once the owning service
//! reports the capacity as IN EFFECT, and for as long as that service has not made it so the
//! movement stays `InProgress` — however long that is, and for ever where the resource's own
//! estate can never satisfy it.
//!
//! Which is a fact only the owning service holds, so it is READ there, once per pass
//! ([`settle_activity_fulfilments`]), for the movements whose own span has already elapsed —
//! never before, because the capacity is written into the owning service's store after the
//! activity is recorded ([`super::policy_execution::settle_policy_executions`]), so a
//! reading taken at the recording instant would be of the capacity the resource held BEFORE.
//!
//! One arm per namespace whose owning service reports an in-effect capacity of its own, APART
//! from the capacity it was asked for: Amazon ECS does — `Service$desiredCount` is "the
//! desired number of instantiations of the task definition to keep running on the service"
//! and `Service$runningCount` "the number of tasks in the cluster that are in the RUNNING
//! state", so a desired count the cluster has nowhere to place is set and never fulfilled.
//! For a namespace whose owning service reports no such second value, there is nothing to
//! observe and the movement is fulfilled by the span this service gives it
//! ([`super::ACTIVITY_FULFILMENT`]), exactly as before.

use std::collections::BTreeSet;
use std::time::SystemTime;

use super::{
    service_state, ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState, TargetKey,
};

/// The `ecs` namespace, whose owning service reports the capacity it was asked for
/// (`desiredCount`) apart from the capacity in effect (`runningCount`).
const ECS_NAMESPACE: &str = "ecs";

/// Whether a movement of a target in this namespace has its fulfilment CONFIRMED in the
/// owning service rather than by this service's own span.
///
/// Namespace by namespace, because it is a property of the owning service and not of any one
/// request: a service that reports an in-effect capacity of its own is one whose confirmation
/// can be read, and the next such namespace is a new arm of [`owner_has_fulfilled`] plus a row
/// here.
pub fn owner_confirms_fulfilment(service_namespace: &str) -> bool {
    service_namespace == ECS_NAMESPACE
}

/// How many activities ONE pass asks the owning service about.
///
/// The pass's OWN bound, the sibling of `policy_execution::MAX_EXECUTIONS_PER_PASS` and for
/// the same reason: how many movements are awaiting confirmation is the caller's choice, so
/// the work of a pass is fixed here. Nothing starves — the activities are taken oldest first,
/// so what this pass leaves out is what the next pass takes.
const MAX_CONFIRMATIONS_PER_PASS: usize = 32;

/// One movement this pass has to ask the owning service about.
#[derive(Debug, Clone)]
struct Awaiting {
    /// The activity, by the id it is reported under.
    activity_id: String,
    target: TargetKey,
    /// The capacity the movement set, which is the capacity that has to be in effect.
    capacity: i32,
}

/// Confirm, for every capacity movement of `region` still waiting on the service that owns
/// the scaled resource, whether that service has made the capacity take effect — and end the
/// activity at the instant it has.
///
/// Call it with NO store lock held: the activities are collected under this service's lock,
/// the owning service's store is read with it released, and the confirmations are written
/// under it again.
///
/// This is work the CLOCK owes and not any request's: the capacity took effect in the owning
/// service at an instant of its own, whether or not anybody called this service afterwards,
/// so a reporting operation may materialise it.
pub fn settle_activity_fulfilments<Eng: Send + Sync + 'static>(engine: &Eng, region: &str) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    let awaiting = awaiting_confirmation(&state.lock(), region, now);
    if awaiting.is_empty() {
        return;
    }
    let confirmed: BTreeSet<String> = awaiting
        .into_iter()
        .filter(|movement| owner_has_fulfilled(engine, &movement.target, movement.capacity))
        .map(|movement| movement.activity_id)
        .collect();
    if confirmed.is_empty() {
        return;
    }
    confirm_fulfilled(&mut state.lock(), &confirmed, SystemTime::now().max(now));
}

/// The movements of `region` whose fulfilment the owning service has still to confirm, and
/// whose own span has already elapsed.
fn awaiting_confirmation(
    store: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    region: &str,
    now: SystemTime,
) -> Vec<Awaiting> {
    store
        .activities
        .iter()
        .filter(|activity| activity.awaiting_owner && activity.target.region == region)
        .filter(|activity| activity.fulfilled_at.is_none_or(|due| due <= now))
        .take(MAX_CONFIRMATIONS_PER_PASS)
        .map(|activity| Awaiting {
            activity_id: activity.activity_id.clone(),
            target: activity.target.clone(),
            capacity: activity.capacity,
        })
        .collect()
}

/// End each confirmed movement at `at`: the capacity it set is in effect, so the activity
/// reaches the terminal status it was recorded with and its `EndTime` is that instant.
fn confirm_fulfilled(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    confirmed: &BTreeSet<String>,
    at: SystemTime,
) {
    for activity in store
        .activities
        .iter_mut()
        .filter(|activity| confirmed.contains(&activity.activity_id))
    {
        activity.awaiting_owner = false;
        // Never earlier than the instant the movement started, so an activity cannot report
        // having ended before it began.
        activity.fulfilled_at = Some(at.max(activity.start_time));
    }
}

/// Whether the service that OWNS the scaled resource has made `capacity` take effect on it —
/// or reports nothing this movement waits on, which is as far as the waiting can go.
///
/// One arm per namespace whose owning service distinguishes the capacity it was asked for
/// from the capacity it has reached; see the module docs.
fn owner_has_fulfilled<Eng: Send + Sync + 'static>(
    engine: &Eng,
    target: &TargetKey,
    capacity: i32,
) -> bool {
    match target.service_namespace.as_str() {
        ECS_NAMESPACE => ecs_has_fulfilled(engine, &target.resource_id, capacity),
        _ => true,
    }
}

/// Whether the ECS service an `ecs` `ResourceId` names is RUNNING the task count a movement
/// set on it: "the number of tasks in the cluster that are in the RUNNING state"
/// (`Service$runningCount`) against the count the service was asked to keep running
/// (`Service$desiredCount`), which is what `ecs:service:DesiredCount` scales.
///
/// There is nothing to wait on — so the movement is as fulfilled as it will get — in the cases
/// where that count says nothing about this movement: a resource id that names no ECS service,
/// an engine that holds no ECS store, a service that is no longer active, one whose tasks
/// Amazon ECS does not place at all (an external deployment controller runs them, so no count
/// of the scheduler's own ever catches up), and one whose desired count is no longer the
/// capacity this movement set, which something else has moved on from.
///
/// The owning service's clock is advanced first: the tasks its scheduler placed start at an
/// instant of their own, which arrives whether or not anybody calls that service, and
/// `advance_clock` is the pass that materialises exactly what it has already made due.
fn ecs_has_fulfilled<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_id: &str,
    capacity: i32,
) -> bool {
    use ::service_com_amazonaws_ecs_amazonec2containerservicev20141113 as ecs;

    let Some((cluster, service_name)) = super::split_ecs_resource_id(resource_id) else {
        return true;
    };
    let Some(store) = ecs::state::service_state(engine) else {
        return true;
    };
    let mut store = store.lock();
    store.advance_clock(SystemTime::now());
    let Some((places_tasks, desired_count)) = store
        .services
        .get(&(cluster.to_string(), service_name.to_string()))
        .filter(|service| service.is_active())
        .map(|service| (service.places_tasks(), service.desired_count))
    else {
        return true;
    };
    if !places_tasks || desired_count != capacity {
        return true;
    }
    // Counted in the same lock the desired count was read under, so the two cannot disagree
    // about the task set they describe.
    store.service_tally(cluster, service_name).running == capacity
}
