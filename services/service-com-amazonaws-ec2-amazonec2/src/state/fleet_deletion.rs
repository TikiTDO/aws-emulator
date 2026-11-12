// SPDX-License-Identifier: CC0-1.0

//! How an EC2 Fleet is DELETED: the `FleetStateCode` members a deletion moves a fleet
//! through, the states one can be deleted from, the per-fleet codes and sentences a batch
//! delete reports, the bounds one batch is held to, and the ONE writer of a fleet's deletion.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.
//!
//! A deletion is the mirror of a Spot Fleet request's cancellation
//! ([`ComAmazonawsEc2Amazonec2State::cancel_spot_fleet_request`]): the flag the caller sends
//! decides the state the fleet enters and whether what it launched is terminated, the fleet
//! reaches its terminal state once the instances it placed are gone, and an id that names no
//! deletable fleet is reported PER FLEET rather than refusing the call.

use super::*;

/// The `deleted` member of [`crate::ops::types::FLEET_STATE_CODE_VALUES`]: where a deleted
/// EC2 Fleet ends up once no instance it placed is still held.
pub const FLEET_STATE_DELETED: &str = "deleted";

/// The `deleted_running` member of [`crate::ops::types::FLEET_STATE_CODE_VALUES`]:
/// "otherwise, it enters the `deleted_running` state, and the instances continue to run until
/// they are interrupted or you terminate them manually" (`DeleteFleets`).
pub const FLEET_STATE_DELETED_RUNNING: &str = "deleted_running";

/// The `deleted_terminating` member of [`crate::ops::types::FLEET_STATE_CODE_VALUES`]: "if
/// you choose to terminate the instances, the EC2 Fleet request enters the
/// `deleted_terminating` state" (`DeleteFleets`).
pub const FLEET_STATE_DELETED_TERMINATING: &str = "deleted_terminating";

/// The `modifying` member of [`crate::ops::types::FLEET_STATE_CODE_VALUES`]: a fleet whose
/// target capacity is being changed.
pub const FLEET_STATE_MODIFYING: &str = "modifying";

/// The `pending_termination` member of [`crate::ops::types::FLEET_ACTIVITY_STATUS_VALUES`]:
/// "`pending_termination` while instances are terminating when fleet size is decreased"
/// (`FleetData$ActivityStatus`), which is where a fleet deleted WITH its instances sits until
/// they are gone.
pub const FLEET_ACTIVITY_STATUS_PENDING_TERMINATION: &str = "pending_termination";

/// The `FleetStateCode` members an EC2 Fleet can be DELETED from: one that is holding
/// capacity, one AWS has just taken the request on, and one whose target capacity is being
/// changed. Every other member is a fleet whose deletion has already been asked for or that
/// failed, so there is nothing left to delete.
pub const FLEET_DELETABLE_STATES: &[&str] = &[
    FLEET_STATE_SUBMITTED,
    FLEET_STATE_ACTIVE,
    FLEET_STATE_MODIFYING,
];

/// Whether an EC2 Fleet in `state` can be deleted ([`FLEET_DELETABLE_STATES`]).
pub fn fleet_state_is_deletable(state: &str) -> bool {
    FLEET_DELETABLE_STATES.contains(&state)
}

/// The most EC2 Fleets of type [`FLEET_TYPE_INSTANT`] ONE `DeleteFleets` call may name: "you
/// can delete up to 25 fleets of type `instant` in a single request" (`DeleteFleets`).
pub const MAX_INSTANT_FLEETS_PER_DELETE: usize = 25;

/// The most EC2 Fleets of type [`FLEET_TYPE_MAINTAIN`] or [`FLEET_TYPE_REQUEST`] ONE
/// `DeleteFleets` call may name: "you can delete up to 100 fleets of type `maintain` or
/// `request` in a single request" (`DeleteFleets`).
pub const MAX_ASYNCHRONOUS_FLEETS_PER_DELETE: usize = 100;

/// The most EC2 Fleets ONE `DeleteFleets` call may name at all: "you can delete up to 125
/// fleets in a single request, provided you do not exceed the quota for each fleet type, as
/// specified above" (`DeleteFleets`) — which is why the two per-type bounds above are asked
/// as well as this one.
pub const MAX_FLEETS_PER_DELETE: usize =
    MAX_INSTANT_FLEETS_PER_DELETE + MAX_ASYNCHRONOUS_FLEETS_PER_DELETE;

/// The `fleetIdMalformed` member of
/// [`crate::ops::types::DELETE_FLEET_ERROR_CODE_VALUES`]: the per-fleet code `DeleteFleets`
/// reports for something that is not an EC2 Fleet id at all.
pub const DELETE_FLEET_ID_MALFORMED: &str = "fleetIdMalformed";

/// The `fleetIdDoesNotExist` member of
/// [`crate::ops::types::DELETE_FLEET_ERROR_CODE_VALUES`]: the per-fleet code for an id this
/// account holds no EC2 Fleet for.
pub const DELETE_FLEET_ID_DOES_NOT_EXIST: &str = "fleetIdDoesNotExist";

/// The `fleetNotInDeletableState` member of
/// [`crate::ops::types::DELETE_FLEET_ERROR_CODE_VALUES`]: the per-fleet code for a fleet in a
/// `FleetStateCode` it cannot be deleted from ([`FLEET_DELETABLE_STATES`]).
pub const DELETE_FLEET_NOT_IN_DELETABLE_STATE: &str = "fleetNotInDeletableState";

/// The `unexpectedError` member of [`crate::ops::types::DELETE_FLEET_ERROR_CODE_VALUES`]: the
/// per-fleet code for a fleet the deletion did not carry out for a reason that is none of the
/// others, so no id a call named is ever left out of both of its answer's sets.
pub const DELETE_FLEET_UNEXPECTED_ERROR: &str = "unexpectedError";

/// The message the per-fleet [`DELETE_FLEET_ID_DOES_NOT_EXIST`] carries.
///
/// Fixed text with no variable part: the id the entry is about is the entry's own `FleetId`
/// member, so the sentence says only what went wrong. Recorded in the wire corpus for a
/// `DeleteFleets` naming a fleet id of no fleet, which is also where the `Fleet deletion
/// failed.` opening every sentence of this family takes comes from.
pub const DELETE_FLEET_ID_DOES_NOT_EXIST_MESSAGE: &str =
    "Fleet deletion failed. Fleet ID not found.";

/// The message the per-fleet [`DELETE_FLEET_NOT_IN_DELETABLE_STATE`] carries: the family's
/// opening sentence, with the `FleetStateCode` the fleet is in read out of the STORE.
pub fn fleet_not_in_deletable_state_message(state: &str) -> String {
    format!("Fleet deletion failed. Fleet is in the {state} state.")
}

/// The message the per-fleet [`DELETE_FLEET_UNEXPECTED_ERROR`] carries.
pub const DELETE_FLEET_UNEXPECTED_ERROR_MESSAGE: &str =
    "Fleet deletion failed. An unexpected error occurred.";

/// The message the per-fleet [`DELETE_FLEET_ID_MALFORMED`] carries: the ONE wording this
/// crate answers a malformed id of any kind with ([`invalid_id_message`]), the way the
/// per-fleet malformed-id code of `CancelSpotFleetRequests` carries it
/// ([`spot_fleet_request_id_malformed_message`]) — so a request-level refusal of the same
/// value and a per-fleet error cannot word it two ways.
pub fn delete_fleet_id_malformed_message(fleet_id: &str) -> String {
    invalid_id_message(fleet_id)
}

/// Refuse a deletion that asked for a fleet of type [`FLEET_TYPE_INSTANT`] to be deleted
/// WITHOUT terminating its instances: HTTP 400 `UnsupportedOperation`.
///
/// "To let the instances continue to run after the EC2 Fleet is deleted, specify
/// `no-terminate-instances`. Supported only for fleets of type `maintain` and `request`. For
/// `instant` fleets, you cannot specify `NoTerminateInstances`. A deleted `instant` fleet with
/// running instances is not supported" (`DeleteFleetsRequest$TerminateInstances`) — so there
/// is no state such a fleet could be left in, and the call is refused before anything is
/// deleted.
pub fn no_terminate_instances_unsupported<E>() -> Scaffolded<E> {
    unsupported_operation(
        "NoTerminateInstances is supported only for fleets of type maintain and request.",
    )
}

/// Whether any instance the EC2 Fleet `record` placed is still held by the account, un-
/// terminated.
///
/// Read through the ONE projection of a fleet's instances ([`fleet_active_instances`]), so
/// "has running instances" and "the instances this fleet reports" are the same set: an
/// instance that has reached `terminated`, and one the instance store no longer holds at all,
/// is neither. That set is what a DELETED fleet's own completion waits for, the way a
/// cancelled Spot Fleet request's does
/// ([`spot_fleet_has_running_instances`]).
pub fn fleet_has_running_instances(
    store: &ComAmazonawsEc2Amazonec2State,
    record: &FleetRecord,
) -> bool {
    !fleet_active_instances(store, record).is_empty()
}

impl ComAmazonawsEc2Amazonec2State {
    /// Delete the EC2 Fleet `fleet_id` names, and answer the `FleetStateCode` it was in and
    /// the one it is in now — `None` when this store holds no such fleet or it is in a state
    /// it cannot be deleted from ([`fleet_state_is_deletable`]).
    ///
    /// The one way an EC2 Fleet is deleted, so what a deletion does cannot be built two ways:
    ///
    /// * "after you delete an EC2 Fleet request, it launches no new instances"
    ///   (`DeleteFleets`) — so every Spot Instance request the fleet placed is cancelled and
    ///   owes no fulfillment, which is what stops the next clock pass launching an instance
    ///   for it;
    /// * "if you choose to terminate the instances, the EC2 Fleet request enters the
    ///   `deleted_terminating` state. Otherwise, it enters the `deleted_running` state, and
    ///   the instances continue to run until they are interrupted or you terminate them
    ///   manually" — so `terminate_instances` decides both the state the fleet enters and
    ///   whether the instances it launched are terminated;
    /// * "when you delete an `instant` fleet, Amazon EC2 automatically terminates all its
    ///   instances" — so a fleet of that type terminates them whatever the flag says, which is
    ///   also why its caller refuses `no-terminate-instances` for one
    ///   ([`no_terminate_instances_unsupported`]);
    /// * a deleted fleet reaches [`FLEET_STATE_DELETED`] once no instance it placed is still
    ///   held, which is owed to the clock and to those instances
    ///   ([`FleetRecord::apply_due_move`]).
    ///
    /// The fleet RECORD stays in the store: a deleted fleet is still a fleet
    /// `DescribeFleets` reports, which is what its `fleet-state` filter publishing `deleted`,
    /// `deleted-running` and `deleted-terminating` means.
    pub fn delete_fleet(
        &mut self,
        fleet_id: &str,
        terminate_instances: bool,
    ) -> Option<(String, String)> {
        let record = self.fleets.get(fleet_id)?;
        let previous_state = record.fleet_state().to_string();
        if !fleet_state_is_deletable(&previous_state) {
            return None;
        }
        // "When you delete an `instant` fleet, Amazon EC2 automatically terminates all its
        // instances": the flag cannot leave one running.
        let terminate = terminate_instances || record.fleet_type() == FLEET_TYPE_INSTANT;
        let launched = record.instances.clone();

        let at = SystemTime::now();
        for placed in &launched {
            // Each Spot Instance request the fleet placed is cancelled through the ONE writer
            // of a request's cancellation, so a request cancelled with its fleet and one
            // cancelled on its own end the same way — and the fleet places no new instance for
            // a request it has cancelled. An On-Demand instance was placed through no request.
            if let Some(request_id) = placed.spot_instance_request_id.as_deref() {
                self.cancel_spot_instance_request(request_id, terminate, at);
            }
        }
        if terminate {
            // "Terminating an instance is permanent and irreversible": the instances the fleet
            // launched, through the ONE writer of a termination.
            for placed in &launched {
                self.begin_instance_termination(&placed.instance_id, at);
            }
        }

        let current_state = if terminate {
            FLEET_STATE_DELETED_TERMINATING
        } else {
            FLEET_STATE_DELETED_RUNNING
        };
        let record = self.fleets.get_mut(fleet_id)?;
        record.fleet.fleet_state = Some(current_state.to_string());
        // The `active` the clock owed a `submitted` fleet is not owed a deleted one; what it
        // owes now is the `deleted` the fleet reaches when its instances are gone.
        record.pending = Some(FleetTransition::starting_now(FLEET_STATE_DELETED));
        // Read the state and the activity status back through the one clock pass this fleet
        // owes, so the deletion reports what a describe in the same instant reports — over
        // THIS fleet alone, so one call's work stays proportional to the fleets it named
        // rather than to every fleet the account holds.
        let running_instances = self
            .fleets
            .get(fleet_id)
            .is_some_and(|record| fleet_has_running_instances(self, record));
        if let Some(record) = self.fleets.get_mut(fleet_id) {
            record.apply_due_move(Instant::now(), running_instances);
        }
        let current_state = self.fleets.get(fleet_id).map_or_else(
            || current_state.to_string(),
            |record| record.fleet_state().to_string(),
        );
        Some((previous_state, current_state))
    }
}
