// SPDX-License-Identifier: CC0-1.0

//! The Dedicated Host RESERVATION store: the reservations `PurchaseHostReservation` buys and
//! `DescribeHostReservations` reports.
//!
//! A reservation covers the Dedicated Hosts one purchase named — "the IDs of the Dedicated
//! Hosts associated with the reservation" (`HostReservation$HostIdSet`) — for one term of one
//! published offering, so the stored record is the modelled [`HostReservation`] shape itself
//! plus the two instants its `State` follows from:
//!
//! * `State` is a fact of the reservation's AGE rather than something two writers each store a
//!   word for, so [`HostReservationRecord::state_at`] derives it on read. "This action results
//!   in the specified reservation being purchased and charged to your account"
//!   (`PurchaseHostReservation`), and that charge is not settled the instant the call answers:
//!   the reservation is `payment-pending` until [`TRANSITION_DELAY`] has passed and `active`
//!   after it — the two words `DescribeHostReservationsRequest$Filter` documents either side
//!   of a settled payment, on the same monotonic clock and the same span as every other
//!   clock-owed move of this crate.
//! * A term ENDS: "the date and time that the reservation ends" (`HostReservation$End`) is its
//!   start plus its term, and a reservation past that instant is `retired` — the state that
//!   same filter documents for one whose term has run out — whether or not anybody called in
//!   between. That instant is wall-clock, because it is the one the reservation REPORTS.
//!
//! Nothing here writes on a read: both words are computed from the stored instants, so a
//! describe reports the state the clock has already reached without touching the store.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;

/// A reservation whose upfront charge has not settled yet: the `payment-pending` member of the
/// modelled `ReservationState` enum (`smithy_query(query="shape",shape="ReservationState")`),
/// which `DescribeHostReservationsRequest$Filter` documents first of the states a Dedicated
/// Host reservation takes.
pub const RESERVATION_STATE_PAYMENT_PENDING: &str = "payment-pending";

/// A reservation in force: that same enum's `active` member, the state the clock owes a
/// `payment-pending` reservation one [`TRANSITION_DELAY`] after the purchase that bought it.
pub const RESERVATION_STATE_ACTIVE: &str = "active";

/// A reservation whose term has run out: that same enum's `retired` member, which that same
/// filter documents last of a Dedicated Host reservation's states.
pub const RESERVATION_STATE_RETIRED: &str = "retired";

/// The id prefix a Dedicated Host reservation carries, as AWS's own reservation ids read
/// (`hr-0d41d1f9e8d5e9e8a`, the form `HostReservation$HostReservationId` names).
pub const HOST_RESERVATION_ID_PREFIX: &str = "hr";

/// The one resource type a `PurchaseHostReservation` `TagSpecification` may name: "the tags to
/// apply to the Dedicated Host Reservation during purchase"
/// (`PurchaseHostReservationRequest$TagSpecifications`), which is the `host-reservation` member
/// of the modelled `ResourceType` enum ([`crate::ops::types::RESOURCE_TYPE_VALUES`]) — the
/// catalog's own spelling, which is not how that member's prose names the resource.
pub const HOST_RESERVATION_TAGGABLE_RESOURCE: &str = "host-reservation";

/// How many Dedicated Host reservations this emulator stores before a purchase is refused.
///
/// CHOSEN, like [`MAX_STORED_DEDICATED_HOSTS`] and for the same reason: nothing in this
/// emulator reclaims a stored reservation, so the store needs a ceiling that is not the
/// process's memory. A count ceiling is no bound on the caller-chosen bytes each record holds,
/// so those are charged against [`MAX_STORED_BYTES`] too
/// ([`host_reservation_payload_bytes`]).
pub const MAX_STORED_HOST_RESERVATIONS: usize = 1_024;

/// The error code a purchase refuses with once the store is full, in the form EC2 names every
/// quota refusal with (`HostLimitExceeded`, `VpcLimitExceeded`): `<Resource>LimitExceeded`.
/// The ceiling it reports is this store's ([`MAX_STORED_HOST_RESERVATIONS`]), not a published
/// AWS quota.
pub const HOST_RESERVATION_LIMIT_EXCEEDED: &str = "ReservationLimitExceeded";

/// Refuse a purchase the store has no room for: HTTP 400
/// [`HOST_RESERVATION_LIMIT_EXCEEDED`].
pub fn host_reservation_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(HOST_RESERVATION_LIMIT_EXCEEDED, 400, message)
}

/// The caller-chosen bytes one stored reservation retains for the process's lifetime: the tags
/// a purchase applied to it.
///
/// The ONE definition of that charge: a purchase asks for exactly it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_host_reservation`] charges exactly it. It takes the
/// TAGS rather than the stored view, so the gate can ask before the view exists.
pub fn host_reservation_payload_bytes(tags: &[Tag]) -> usize {
    tag_list_bytes(tags)
}

/// One stored Dedicated Host reservation: the modelled shape the describe reports, and the two
/// instants its `State` follows from (see the module docs).
///
/// `view.state` is NOT stored: the word a reader sees comes from [`Self::state_at`], so the
/// record cannot hold a state the clock has already moved past.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct HostReservationRecord {
    /// The reservation as `DescribeHostReservations` reports it, less its `State`.
    pub view: HostReservation,
    /// When the upfront charge settles: `payment-pending` before this instant,
    /// [`RESERVATION_STATE_ACTIVE`] from it on. One [`TRANSITION_DELAY`] after the purchase.
    #[serde(with = "::service_common::persist::instant")]
    pub active_at: Instant,
    /// When the term runs out — the instant [`HostReservation::end`] reports — from which the
    /// reservation is [`RESERVATION_STATE_RETIRED`].
    pub ends_at: SystemTime,
}

impl HostReservationRecord {
    /// The reservation's `State` at `now` (monotonic) and `wall` (the clock its term is quoted
    /// on): see the module docs for why each of the two instants decides one of the words.
    pub fn state_at(&self, now: Instant, wall: SystemTime) -> &'static str {
        if wall >= self.ends_at {
            return RESERVATION_STATE_RETIRED;
        }
        if now >= self.active_at {
            return RESERVATION_STATE_ACTIVE;
        }
        RESERVATION_STATE_PAYMENT_PENDING
    }

    /// This reservation as the modelled `HostReservation` shape, with the `State` the clock has
    /// reached by `now`/`wall`.
    pub fn view_at(&self, now: Instant, wall: SystemTime) -> HostReservation {
        HostReservation {
            state: Some(self.state_at(now, wall).to_string()),
            ..self.view.clone()
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether one more Dedicated Host reservation fits inside the store's ceiling
    /// ([`MAX_STORED_HOST_RESERVATIONS`]).
    ///
    /// A purchase must ask this BEFORE writing anything and refuse with
    /// [`host_reservation_limit_exceeded`] when it answers `false`.
    pub fn has_host_reservation_capacity(&self) -> bool {
        self.host_reservations.len() < MAX_STORED_HOST_RESERVATIONS
    }

    /// Store one purchased reservation, charging the caller-chosen bytes it holds, and answer
    /// its id — or `None` when the store is full.
    ///
    /// `None` means the store is at [`MAX_STORED_HOST_RESERVATIONS`] and NOTHING was stored:
    /// the ceiling is enforced HERE as well as by [`Self::has_host_reservation_capacity`], the
    /// way [`Self::insert_dedicated_host`] enforces its own, so a writer that forgets the gate
    /// cannot grow the store without bound either. The record's own
    /// [`STORED_RECORD_OVERHEAD_BYTES`] are charged beside its caller-chosen bytes, so a
    /// reservation with no tags is not free against the byte budget.
    pub fn insert_host_reservation(&mut self, record: HostReservationRecord) -> Option<String> {
        let reservation_id = record.view.host_reservation_id.clone().unwrap_or_default();
        if !self.host_reservations.contains_key(&reservation_id)
            && !self.has_host_reservation_capacity()
        {
            return None;
        }
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES.saturating_add(host_reservation_payload_bytes(
                record.view.tags.as_deref().unwrap_or_default(),
            )),
        );
        self.host_reservations.insert(reservation_id.clone(), record);
        Some(reservation_id)
    }
}
