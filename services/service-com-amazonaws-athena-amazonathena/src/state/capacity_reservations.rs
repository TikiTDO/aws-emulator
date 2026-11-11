// SPDX-License-Identifier: CC0-1.0

//! The provisioned capacity this account reserved, and the lifecycle each reservation
//! walks.
//!
//! `CreateCapacityReservation` writes a [`CapacityReservationRecord`] into the
//! [`CapacityReservationRegistry`] on this service's store and `GetCapacityReservation`
//! answers out of that same record, so every value a read reports is one some earlier
//! write persisted rather than one derived from the reading request.
//!
//! A reservation DOES have a lifecycle the clock owes, and `CapacityReservation`
//! documents it: "When a reservation is initially created, it has no DPUs. Athena
//! allocates DPUs until the allocated amount equals the requested amount." So a create
//! stores WHEN its allocation is due ([`CapacityReservationRecord::allocation_complete_at`])
//! and every read derives the status and the allocated amount from that instant
//! ([`CapacityReservationRecord::status_at`]) rather than a reader having to move the
//! record on. An update asks for a new amount the same way, and a cancellation is owed
//! the same kind of span before it is complete.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::*;

use crate::ops::types::{CapacityAllocation, CapacityReservation};

/// The `CapacityReservationStatus` of a reservation whose FIRST allocation has not
/// completed yet — the status a create leaves it in, since a reservation "initially
/// created ... has no DPUs" (`CapacityReservation`). One of
/// `CAPACITY_RESERVATION_STATUS_VALUES` in [`crate::ops::types`].
pub const CAPACITY_RESERVATION_STATUS_PENDING: &str = "PENDING";
/// The `CapacityReservationStatus` of a reservation whose requested data processing units
/// are allocated; see [`CAPACITY_RESERVATION_STATUS_PENDING`].
pub const CAPACITY_RESERVATION_STATUS_ACTIVE: &str = "ACTIVE";
/// The `CapacityReservationStatus` of an allocated reservation with a NEW requested
/// amount still being allocated — what `UpdateCapacityReservation` ("Updates the number
/// of requested data processing units for the capacity reservation") leaves behind until
/// that allocation is due; see [`CAPACITY_RESERVATION_STATUS_PENDING`].
pub const CAPACITY_RESERVATION_STATUS_UPDATE_PENDING: &str = "UPDATE_PENDING";
/// The `CapacityReservationStatus` of a reservation whose cancellation is still in
/// progress; see [`CAPACITY_RESERVATION_STATUS_PENDING`].
pub const CAPACITY_RESERVATION_STATUS_CANCELLING: &str = "CANCELLING";
/// The `CapacityReservationStatus` of a cancelled reservation — the only status a
/// reservation can be deleted from, per `DeleteCapacityReservation` ("A reservation must
/// be cancelled before it can be deleted"); see
/// [`CAPACITY_RESERVATION_STATUS_PENDING`].
pub const CAPACITY_RESERVATION_STATUS_CANCELLED: &str = "CANCELLED";

/// The `CapacityAllocationStatus` of the allocation a reservation is still waiting for.
/// One of `CAPACITY_ALLOCATION_STATUS_VALUES` in [`crate::ops::types`].
pub const CAPACITY_ALLOCATION_STATUS_PENDING: &str = "PENDING";
/// The `CapacityAllocationStatus` of an allocation that completed; see
/// [`CAPACITY_ALLOCATION_STATUS_PENDING`].
pub const CAPACITY_ALLOCATION_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The number of data processing units a reservation holds before any allocation has
/// completed: "When a reservation is initially created, it has no DPUs"
/// (`CapacityReservation`).
pub const INITIAL_ALLOCATED_DPUS: i32 = 0;

/// How long after a create or an update the requested allocation is DUE.
///
/// Athena hands back a reservation in a non-terminal status and the allocation arrives
/// later, so this is a transition the CLOCK owes: the instant is stored on the record and
/// every read derives the status from it. The span is this emulator's, not a duration AWS
/// publishes (a real allocation takes as long as the capacity takes to come up), and it is
/// never zero — a caller reading or updating straight back must still observe the
/// transient status and the refusals it carries.
pub const CAPACITY_ALLOCATION: Duration = Duration::from_secs(3);

/// How long after `CancelCapacityReservation` the cancellation is complete: the same kind
/// of emulator-chosen span as [`CAPACITY_ALLOCATION`], so the documented `CANCELLING`
/// status a cancelled reservation passes through is observable rather than skipped.
pub const CAPACITY_RESERVATION_CANCELLATION: Duration = Duration::from_secs(3);

/// How long a cancelled reservation remains in the account.
///
/// `CancelCapacityReservation` publishes the duration: "Cancelled reservations remain in
/// your account and will be deleted 45 days after cancellation. During the 45 days, you
/// cannot re-purpose or reuse a reservation that has been cancelled, but you can refer to
/// its tags and view it for historical reference." So the removal is a change the clock
/// makes due, and a read past it reports the reservation as absent.
pub const CANCELLED_CAPACITY_RESERVATION_RETENTION: Duration =
    Duration::from_secs(45 * 24 * 60 * 60);

/// How many capacity reservations one account holds in this emulator.
///
/// The EMULATOR's own bound, with no counterpart in the model: Athena publishes no limit
/// on the number of reservations, and this is only what keeps a sequence of creates from
/// growing the emulator's heap without limit. A create past it is refused with the
/// operation's own `InvalidRequestException`.
pub const MAX_CAPACITY_RESERVATIONS: usize = 1000;

/// One capacity reservation, as this service persists it.
///
/// Neither the status nor the allocated amount is stored: both follow from the stored
/// instants ([`Self::status_at`], [`Self::allocated_dpus_at`]), so a read derives them
/// from the clock instead of a reader having to move the record on.
#[derive(Debug, Clone)]
pub struct CapacityReservationRecord {
    /// The reservation name, which is also its key in the registry.
    pub name: String,
    /// The number of data processing units currently REQUESTED, reported as
    /// `CapacityReservation$TargetDpus`. The create's amount until an update asks for
    /// another.
    pub target_dpus: i32,
    /// When the reservation was created, reported as `CapacityReservation$CreationTime`.
    pub creation_time: SystemTime,
    /// When the allocation of [`Self::target_dpus`] was asked for: the create's instant,
    /// or that of the update that last changed the requested amount. Reported as
    /// `CapacityAllocation$RequestTime`.
    pub allocation_requested_at: SystemTime,
    /// When that allocation is due to be complete — [`CAPACITY_ALLOCATION`] after it was
    /// asked for. Reported as `CapacityAllocation$RequestCompletionTime` from then on.
    pub allocation_complete_at: SystemTime,
    /// How many data processing units are allocated while that allocation is still in
    /// flight: [`INITIAL_ALLOCATED_DPUS`] for the create's allocation, and the amount the
    /// previous one reached for an update's.
    pub allocated_dpus_before: i32,
    /// When the allocation BEFORE the one above completed, if there was one. `None` for a
    /// reservation whose first allocation is still in flight, which is what tells
    /// [`CAPACITY_RESERVATION_STATUS_PENDING`] from
    /// [`CAPACITY_RESERVATION_STATUS_UPDATE_PENDING`].
    pub previous_allocation_completed_at: Option<SystemTime>,
    /// When the reservation was cancelled, if it was: the instant
    /// `CancelCapacityReservation` was called, from which both the `CANCELLING` span and
    /// the [`CANCELLED_CAPACITY_RESERVATION_RETENTION`] run.
    pub cancelled_at: Option<SystemTime>,
    /// When that cancellation is due to be complete —
    /// [`CAPACITY_RESERVATION_CANCELLATION`] after [`Self::cancelled_at`].
    pub cancel_complete_at: Option<SystemTime>,
    /// The tags the create call carried; keys are unique within this list.
    pub tags: Vec<Tag>,
}

impl CapacityReservationRecord {
    /// The record a create of `name` requesting `target_dpus` at `now` registers: no DPUs
    /// allocated yet, and the allocation of the requested amount already asked for.
    pub fn created(name: String, target_dpus: i32, tags: Vec<Tag>, now: SystemTime) -> Self {
        Self {
            name,
            target_dpus,
            creation_time: now,
            allocation_requested_at: now,
            allocation_complete_at: allocation_complete_at(now),
            allocated_dpus_before: INITIAL_ALLOCATED_DPUS,
            previous_allocation_completed_at: None,
            cancelled_at: None,
            cancel_complete_at: None,
            tags,
        }
    }

    /// The `CapacityReservationStatus` this reservation is in at `now`.
    ///
    /// Derived, never written: two reads at the same instant report the same status, and a
    /// reservation nobody reads reaches `ACTIVE` exactly when one that is polled does.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if let Some(complete) = self.cancel_complete_at {
            return if now < complete {
                CAPACITY_RESERVATION_STATUS_CANCELLING
            } else {
                CAPACITY_RESERVATION_STATUS_CANCELLED
            };
        }
        if now < self.allocation_complete_at {
            return if self.previous_allocation_completed_at.is_none() {
                CAPACITY_RESERVATION_STATUS_PENDING
            } else {
                CAPACITY_RESERVATION_STATUS_UPDATE_PENDING
            };
        }
        CAPACITY_RESERVATION_STATUS_ACTIVE
    }

    /// How many data processing units are allocated at `now`, reported as
    /// `CapacityReservation$AllocatedDpus` ("The number of data processing units currently
    /// allocated").
    ///
    /// Zero once the cancellation is complete: a cancelled reservation's capacity is
    /// released, and `CancelCapacityReservation` documents that it cannot be re-purposed or
    /// reused, so nothing is currently allocated to it.
    pub fn allocated_dpus_at(&self, now: SystemTime) -> i32 {
        if self.status_at(now) == CAPACITY_RESERVATION_STATUS_CANCELLED {
            return INITIAL_ALLOCATED_DPUS;
        }
        if now < self.allocation_complete_at {
            self.allocated_dpus_before
        } else {
            self.target_dpus
        }
    }

    /// When the most recent allocation that SUCCEEDED completed, as of `now` — reported as
    /// `CapacityReservation$LastSuccessfulAllocationTime` ("The time of the most recent
    /// capacity allocation that succeeded"). `None` while the first allocation is still in
    /// flight, there being none that succeeded yet.
    pub fn last_successful_allocation_at(&self, now: SystemTime) -> Option<SystemTime> {
        if now < self.allocation_complete_at {
            self.previous_allocation_completed_at
        } else {
            Some(self.allocation_complete_at)
        }
    }

    /// The most recent allocation request as the modelled `CapacityAllocation` shape, as of
    /// `now`: its submission instant and "the most recent status of the attempted
    /// allocation".
    pub fn last_allocation_at(&self, now: SystemTime) -> CapacityAllocation {
        let pending = now < self.allocation_complete_at;
        CapacityAllocation {
            status: if pending {
                CAPACITY_ALLOCATION_STATUS_PENDING.to_string()
            } else {
                CAPACITY_ALLOCATION_STATUS_SUCCEEDED.to_string()
            },
            // `CapacityAllocation$StatusMessage` is "the status message of the capacity
            // allocation"; no allocation here fails, so there is nothing to report.
            status_message: None,
            request_time: ::aws_smithy_types::DateTime::from(self.allocation_requested_at),
            request_completion_time: (!pending)
                .then(|| ::aws_smithy_types::DateTime::from(self.allocation_complete_at)),
        }
    }

    /// The record as the modelled `CapacityReservation` shape a read answers with, as of
    /// `now`.
    pub fn to_capacity_reservation(&self, now: SystemTime) -> CapacityReservation {
        CapacityReservation {
            name: self.name.clone(),
            status: self.status_at(now).to_string(),
            target_dpus: self.target_dpus,
            allocated_dpus: self.allocated_dpus_at(now),
            last_allocation: Some(self.last_allocation_at(now)),
            last_successful_allocation_time: self
                .last_successful_allocation_at(now)
                .map(::aws_smithy_types::DateTime::from),
            creation_time: ::aws_smithy_types::DateTime::from(self.creation_time),
        }
    }

    /// Ask for `target_dpus` to be allocated, as an update does.
    ///
    /// The amount the reservation holds at `now` becomes what it holds until the new
    /// allocation is due, so an update never reports capacity it has not allocated yet and
    /// never drops the capacity it already had.
    pub fn request_allocation(&mut self, target_dpus: i32, now: SystemTime) {
        self.allocated_dpus_before = self.allocated_dpus_at(now);
        self.previous_allocation_completed_at = self.last_successful_allocation_at(now);
        self.target_dpus = target_dpus;
        self.allocation_requested_at = now;
        self.allocation_complete_at = allocation_complete_at(now);
    }

    /// Cancel the reservation at `now`, as `CancelCapacityReservation` does: the
    /// cancellation is complete [`CAPACITY_RESERVATION_CANCELLATION`] later, and the record
    /// leaves the account [`CANCELLED_CAPACITY_RESERVATION_RETENTION`] after this instant.
    pub fn cancel(&mut self, now: SystemTime) {
        self.cancelled_at = Some(now);
        self.cancel_complete_at = Some(due(now, CAPACITY_RESERVATION_CANCELLATION));
    }

    /// Whether the account no longer holds this reservation at `now` because the documented
    /// retention of a cancelled reservation has elapsed (see
    /// [`CANCELLED_CAPACITY_RESERVATION_RETENTION`]).
    pub fn is_retired_at(&self, now: SystemTime) -> bool {
        self.cancelled_at
            .is_some_and(|at| now >= due(at, CANCELLED_CAPACITY_RESERVATION_RETENTION))
    }
}

/// Every capacity reservation this account holds.
///
/// A struct of its own rather than a loose map on the store, so the bound above and the
/// documented retention are applied in the ONE place a reservation is written and the ONE
/// place one is looked up, and cannot be honoured on one operation's path and skipped on
/// another's.
#[derive(Debug, Default)]
pub struct CapacityReservationRegistry {
    /// The reservations, keyed by name — unique per account, since every operation of this
    /// slice addresses one by `Name` alone. A `BTreeMap` so a walk of the registry is in
    /// one stable order.
    reservations: BTreeMap<String, CapacityReservationRecord>,
}

impl CapacityReservationRegistry {
    /// The reservation `name` names at `now`, or `None` when the account holds none — which
    /// includes one whose cancelled retention has elapsed, since a deleted reservation
    /// "cannot be called by `GetCapacityReservation`".
    pub fn reservation(&self, name: &str, now: SystemTime) -> Option<&CapacityReservationRecord> {
        self.reservations
            .get(name)
            .filter(|record| !record.is_retired_at(now))
    }

    /// The same reservation, for a caller that is going to move it on (an update or a
    /// cancellation).
    pub fn reservation_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut CapacityReservationRecord> {
        self.reservations
            .get_mut(name)
            .filter(|record| !record.is_retired_at(now))
    }

    /// How many reservations the account holds at `now`, the count
    /// [`MAX_CAPACITY_RESERVATIONS`] bounds.
    pub fn len_at(&self, now: SystemTime) -> usize {
        self.reservations
            .values()
            .filter(|record| !record.is_retired_at(now))
            .count()
    }

    /// Store `record`. The one place a reservation enters the registry.
    pub fn insert(&mut self, record: CapacityReservationRecord) {
        self.reservations.insert(record.name.clone(), record);
    }

    /// Remove the reservation `name` names, as `DeleteCapacityReservation` does ("A deleted
    /// reservation is immediately removed from your account"). Also how a reservation whose
    /// retention elapsed stops taking room, so the registry does not hold records no
    /// operation can reach.
    pub fn remove(&mut self, name: &str) -> Option<CapacityReservationRecord> {
        self.reservations.remove(name)
    }

    /// Drop every reservation whose cancelled retention elapsed at `now`.
    ///
    /// Materialises a removal the CLOCK already made due — it was owed at the same instant
    /// even if nobody had called — so it belongs on a write path only; a read answers
    /// through [`Self::reservation`], which reports such a record as absent without
    /// touching the registry. Called from the ONE path that grows the registry (the
    /// create), which is what keeps [`MAX_CAPACITY_RESERVATIONS`] a bound on records that
    /// can still be reached rather than on records that have left the account.
    pub fn retire_cancelled(&mut self, now: SystemTime) {
        self.reservations
            .retain(|_, record| !record.is_retired_at(now));
    }
}

/// When an allocation asked for at `now` is due to be complete.
///
/// Checked addition, so a clock far enough in the future to overflow yields an allocation
/// that is already complete rather than a panic.
pub fn allocation_complete_at(now: SystemTime) -> SystemTime {
    due(now, CAPACITY_ALLOCATION)
}

/// `now` plus `span`, or `now` when that instant is not representable; see
/// [`allocation_complete_at`].
fn due(now: SystemTime, span: Duration) -> SystemTime {
    now.checked_add(span).unwrap_or(now)
}

/// The refusal message for an operation whose named capacity reservation this account does
/// not hold.
///
/// The template every Athena resource miss in this crate follows — the kind of entity, the
/// name the request addressed, and "was not found" (see [`data_catalog_not_found`]) — so
/// the same absent reservation reads the same way whichever operation of this slice was
/// called.
pub fn capacity_reservation_not_found(name: &str) -> String {
    format!("CapacityReservation {name} was not found.")
}

/// The refusal message a create under a name this account already holds a reservation
/// under answers with: the template this crate's creates share (see the data-catalog
/// create), with the name the request carried as its one variable part.
pub fn capacity_reservation_already_exists(name: &str) -> String {
    format!("CapacityReservation {name} already exists.")
}

/// The refusal message for an update or a cancellation the addressed reservation's STATUS
/// does not allow.
///
/// ONE sentence for the class — the state the reservation is in is its only variable part —
/// so an update and a cancellation refused by the status read the same way and a client can
/// tell "wait and retry" from a request it must not repeat. Both calls would have CHANGED
/// the reservation, which is how the service words what it refused.
pub fn capacity_reservation_wrong_state(status: &str) -> String {
    format!("Reservation cannot be modified when state is {status}")
}

/// The refusal message a delete of a reservation that is not
/// [`CAPACITY_RESERVATION_STATUS_CANCELLED`] answers with.
///
/// A class of its own rather than the sentence above: what the delete needs is not a
/// reservation that can be changed but one that has been cancelled, which is the rule
/// `DeleteCapacityReservation` states — "A reservation must be cancelled before it can be
/// deleted" — so the refusal states that rule and no other.
pub fn capacity_reservation_not_cancelled() -> String {
    "Reservation must be cancelled before it can be deleted".to_string()
}

/// The refusal message for a create that would take the account past
/// [`MAX_CAPACITY_RESERVATIONS`].
pub fn too_many_capacity_reservations() -> String {
    format!(
        "Cannot create another capacity reservation: this account already holds the \
         maximum of {MAX_CAPACITY_RESERVATIONS} capacity reservations."
    )
}
