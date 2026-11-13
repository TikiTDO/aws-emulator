// SPDX-License-Identifier: CC0-1.0

//! The fast snapshot restores the account holds: what one (snapshot, Availability Zone)
//! pair is stored as, the lifecycle the clock owes it, and the one reading of the request
//! members `EnableFastSnapshotRestores` and `DisableFastSnapshotRestores` share.
//!
//! A fast snapshot restore is a property of a PAIR — "Enables fast snapshot restores for
//! the specified snapshots in the specified Availability Zones"
//! (`EnableFastSnapshotRestores`) — so the store
//! ([`ComAmazonawsEc2Amazonec2State::fast_snapshot_restores`]) is keyed by both halves of
//! it ([`fast_snapshot_restore_key`]) in a [`std::collections::BTreeMap`], giving the one
//! stable order `DescribeFastSnapshotRestores` reports and pages over.
//!
//! The record keeps the FACTS of the pair — which snapshot, which zone in both of its
//! spellings, which account enabled it — and the instants its lifecycle moves fall due on;
//! the state, the state transition reason and the five phase timestamps are derived from
//! those instants by [`FastSnapshotRestoreRecord::view_at`]. `FastSnapshotRestoreStateCode`
//! runs `enabling` -> `optimizing` -> `enabled` after an enable and `disabling` ->
//! `disabled` after a disable, and reaching the next word of that catalog is owed by the
//! CLOCK rather than by a later call: deriving them on read means a reporting operation
//! observes the move the clock already owed without writing anything, and no two operations
//! can store two different words for one pair's state.
//!
//! A pair is keyed by a snapshot of [`ComAmazonawsEc2Amazonec2State::snapshots`] and a zone
//! of the signed Region, so the store cannot hold more records than there are stored
//! snapshots times [`super::AVAILABILITY_ZONES_PER_REGION`], and every string one retains is
//! either a zone name the service chose or a snapshot id read back out of that store —
//! nothing caller-sized — so no byte charge is made for one.

use std::collections::btree_map::Entry;
use std::time::{Instant, SystemTime};

use crate::ops::types::{
    DescribeFastSnapshotRestoreSuccessItem, DisableFastSnapshotRestoreSuccessItem,
    EnableFastSnapshotRestoreSuccessItem,
};
use crate::scaffolded::Scaffolded;

use super::{
    AvailabilityZoneRecord, ClockTransition, ComAmazonawsEc2Amazonec2State, MAX_RESOURCE_IDS,
    TRANSITION_DELAY, availability_zone_of_region, availability_zones_in_region, deduplicate_ids,
    invalid_parameter_combination, invalid_parameter_value, missing_parameter,
    system_time_to_date_time,
};

/// `FastSnapshotRestoreStateCode`'s `enabling`: the state an enable puts a pair in, from
/// which the service's own work carries it onwards
/// (`smithy_query(query="shape",shape="FastSnapshotRestoreStateCode")`).
pub const FAST_SNAPSHOT_RESTORE_STATE_ENABLING: &str = "enabling";

/// `FastSnapshotRestoreStateCode`'s `optimizing`: the pair is being optimized and is not
/// yet giving a restored volume its full performance.
pub const FAST_SNAPSHOT_RESTORE_STATE_OPTIMIZING: &str = "optimizing";

/// `FastSnapshotRestoreStateCode`'s `enabled`: the terminal state of an enable.
pub const FAST_SNAPSHOT_RESTORE_STATE_ENABLED: &str = "enabled";

/// `FastSnapshotRestoreStateCode`'s `disabling`: the state a disable puts a pair in.
pub const FAST_SNAPSHOT_RESTORE_STATE_DISABLING: &str = "disabling";

/// `FastSnapshotRestoreStateCode`'s `disabled`: the terminal state of a disable, and the
/// state a pair nothing has ever enabled is in.
pub const FAST_SNAPSHOT_RESTORE_STATE_DISABLED: &str = "disabled";

/// "`Client.UserInitiated` - The state successfully transitioned to `enabling` or
/// `disabling`" (`DescribeFastSnapshotRestoreSuccessItem$StateTransitionReason`): the reason
/// of the state a CALLER asked for.
pub const FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED: &str = "Client.UserInitiated";

/// "`Client.UserInitiated - Lifecycle state transition` - The state successfully
/// transitioned to `optimizing`, `enabled`, or `disabled`"
/// (`DescribeFastSnapshotRestoreSuccessItem$StateTransitionReason`): the reason of a state
/// the service's own lifecycle carried the pair to.
pub const FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE: &str =
    "Client.UserInitiated - Lifecycle state transition";

/// What separates the two halves of a pair's key ([`fast_snapshot_restore_key`]).
///
/// A character that appears in neither a snapshot id nor an Availability Zone name, and that
/// sorts below every character a snapshot id's SUFFIX can hold — every id this store keys on
/// carries the same fixed prefix, so the suffix is where two of them diverge. The key order
/// of the store is therefore the (snapshot id, zone) order itself: `snap-1/<zone>` sorts
/// before `snap-10/<zone>` exactly as `snap-1` sorts before `snap-10`.
const FAST_SNAPSHOT_RESTORE_KEY_SEPARATOR: char = '/';

/// The key one (snapshot, Availability Zone) pair is stored under.
///
/// Both halves, because a fast snapshot restore is a property of the pair: one snapshot may
/// have one in each zone, and one zone may have one for each snapshot. The zone is named by
/// its NAME rather than by its id, the two being one zone's two spellings, so a request that
/// named it either way reaches the same record.
pub fn fast_snapshot_restore_key(snapshot_id: &str, availability_zone: &str) -> String {
    format!("{snapshot_id}{FAST_SNAPSHOT_RESTORE_KEY_SEPARATOR}{availability_zone}")
}

/// A move the clock owes one fast snapshot restore, carrying the `FastSnapshotRestoreStateCode`
/// word it reaches: [`ClockTransition`], the crate's one shape for a state the clock rather
/// than a caller brings about.
pub type FastSnapshotRestoreTransition = ClockTransition<&'static str>;

/// The disable of one fast snapshot restore: when the pair entered `disabling` and when the
/// clock owes it `disabled`.
///
/// Held as ONE value rather than as separate members, so a record can never carry a
/// disabling time without the move that follows it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(bound(deserialize = "'de: 'static"))]
pub struct FastSnapshotRestoreDisable {
    /// The monotonic instant the disable was asked for, which is where the enable's own
    /// lifecycle stops: a pair disabled while still `enabling` never reaches `optimizing`,
    /// so a phase whose due instant falls after this one was never entered.
    #[serde(with = "::service_common::persist::instant")]
    pub began_at: Instant,
    /// The wall-clock instant the pair entered `disabling`, reported as its `DisablingTime`.
    pub disabling_time: SystemTime,
    /// The `disabled` move the clock owes the pair.
    pub disabled: FastSnapshotRestoreTransition,
}

/// One stored fast snapshot restore: which pair it is, who enabled it, and the instants its
/// lifecycle moves fall due on.
///
/// The state, the state transition reason and the phase timestamps are NOT stored: all of
/// them are facts of the record's age, which [`Self::view_at`] derives — see the module
/// docs.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(bound(deserialize = "'de: 'static"))]
pub struct FastSnapshotRestoreRecord {
    /// "The ID of the snapshot" (`DescribeFastSnapshotRestoreSuccessItem$SnapshotId`).
    pub snapshot_id: String,
    /// "The Availability Zone" (`…$AvailabilityZone`).
    pub availability_zone: String,
    /// "The ID of the Availability Zone" (`…$AvailabilityZoneId`), the same zone's other
    /// spelling, so a report can answer either member without resolving the zone again.
    pub availability_zone_id: String,
    /// "The ID of the Amazon Web Services account that enabled fast snapshot restores on
    /// the snapshot" (`…$OwnerId`).
    pub owner_id: String,
    /// The wall-clock instant the pair entered `enabling`, reported as its `EnablingTime`.
    pub enabling_time: SystemTime,
    /// The `optimizing` move the clock owes a pair an enable has just started.
    pub optimizing: FastSnapshotRestoreTransition,
    /// The `enabled` move that follows it, due one further [`TRANSITION_DELAY`] on: the
    /// state is "optimizing" until the optimization the service does for the pair is done,
    /// and that work is owed by the clock from the enable rather than from whenever
    /// somebody next asks.
    pub enabled: FastSnapshotRestoreTransition,
    /// The disable of this pair, once one has been asked for.
    pub disabling: Option<FastSnapshotRestoreDisable>,
}

impl FastSnapshotRestoreRecord {
    /// A pair an enable has just put in `enabling` at `now`, which the clock owes
    /// `optimizing` and then `enabled`.
    pub fn enabling_at(
        snapshot_id: &str,
        zone: &AvailabilityZoneRecord,
        owner_id: &str,
        now: SystemTime,
    ) -> Self {
        let started_at = Instant::now();
        Self {
            snapshot_id: snapshot_id.to_string(),
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            owner_id: owner_id.to_string(),
            enabling_time: now,
            optimizing: FastSnapshotRestoreTransition {
                to: FAST_SNAPSHOT_RESTORE_STATE_OPTIMIZING,
                due_at: started_at + TRANSITION_DELAY,
                at: now + TRANSITION_DELAY,
            },
            enabled: FastSnapshotRestoreTransition {
                to: FAST_SNAPSHOT_RESTORE_STATE_ENABLED,
                due_at: started_at + TRANSITION_DELAY * 2,
                at: now + TRANSITION_DELAY * 2,
            },
            disabling: None,
        }
    }

    /// Put this pair in `disabling` at `now`, from which the clock owes it `disabled`.
    ///
    /// A pair that is already disabling or disabled keeps the disable it has: the state it
    /// is in is the state the request asks for, so there is nothing further to start.
    pub fn begin_disable(&mut self, now: SystemTime) {
        if self.disabling.is_some() {
            return;
        }
        let began_at = Instant::now();
        self.disabling = Some(FastSnapshotRestoreDisable {
            began_at,
            disabling_time: now,
            disabled: FastSnapshotRestoreTransition {
                to: FAST_SNAPSHOT_RESTORE_STATE_DISABLED,
                due_at: began_at + TRANSITION_DELAY,
                at: now + TRANSITION_DELAY,
            },
        });
    }

    /// The pair's `FastSnapshotRestoreStateCode` at `now`.
    ///
    /// A disable overrides the enable's lifecycle: the pair is `disabling` until that move
    /// falls due and `disabled` from then on, whichever word the enable had reached.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if let Some(disable) = self.disabling.as_ref() {
            return if now < disable.disabled.due_at {
                FAST_SNAPSHOT_RESTORE_STATE_DISABLING
            } else {
                disable.disabled.to
            };
        }
        if now < self.optimizing.due_at {
            FAST_SNAPSHOT_RESTORE_STATE_ENABLING
        } else if now < self.enabled.due_at {
            self.optimizing.to
        } else {
            self.enabled.to
        }
    }

    /// Why the pair is in the state it is at `now`: the caller's own request for the two
    /// states a caller asks for, and the lifecycle for the three the service's own work
    /// carries it to ([`FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED`] and
    /// [`FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE`], which is the member's whole catalog).
    pub fn state_transition_reason_at(&self, now: Instant) -> &'static str {
        match self.state_at(now) {
            FAST_SNAPSHOT_RESTORE_STATE_ENABLING | FAST_SNAPSHOT_RESTORE_STATE_DISABLING => {
                FAST_SNAPSHOT_RESTORE_REASON_USER_INITIATED
            }
            _ => FAST_SNAPSHOT_RESTORE_REASON_LIFECYCLE,
        }
    }

    /// Had the pair entered the phase `moment` names by `now`?
    ///
    /// A phase is entered when its move falls due — and only while the enable's lifecycle
    /// is still running: a pair disabled before a phase fell due never reached it, so it
    /// reports no timestamp for one.
    fn entered(&self, moment: &FastSnapshotRestoreTransition, now: Instant) -> bool {
        now >= moment.due_at
            && self
                .disabling
                .as_ref()
                .is_none_or(|disable| disable.began_at >= moment.due_at)
    }

    /// This pair as `DescribeFastSnapshotRestores` reports it at `now`: the stored facts,
    /// plus the state, the reason and the phase timestamps the clock has reached.
    ///
    /// A timestamp is reported only for a phase the pair ENTERED — "the time at which fast
    /// snapshot restores entered the `optimizing` state" (`…$OptimizingTime`) — so a pair
    /// still enabling carries one timestamp and a disabled one carries the whole walk it
    /// made.
    pub fn view_at(&self, now: Instant) -> DescribeFastSnapshotRestoreSuccessItem {
        let disable = self.disabling.as_ref();
        DescribeFastSnapshotRestoreSuccessItem {
            snapshot_id: Some(self.snapshot_id.clone()),
            availability_zone: Some(self.availability_zone.clone()),
            availability_zone_id: Some(self.availability_zone_id.clone()),
            state: Some(self.state_at(now).to_string()),
            state_transition_reason: Some(self.state_transition_reason_at(now).to_string()),
            owner_id: Some(self.owner_id.clone()),
            // "The Amazon Web Services owner alias that enabled fast snapshot restores on
            // the snapshot. THIS IS INTENDED FOR FUTURE USE" (`…$OwnerAlias`): nothing to
            // report, rather than an alias this emulator invented.
            owner_alias: None,
            enabling_time: Some(system_time_to_date_time(self.enabling_time)),
            optimizing_time: self
                .entered(&self.optimizing, now)
                .then(|| system_time_to_date_time(self.optimizing.at)),
            enabled_time: self
                .entered(&self.enabled, now)
                .then(|| system_time_to_date_time(self.enabled.at)),
            disabling_time: disable.map(|disable| system_time_to_date_time(disable.disabling_time)),
            disabled_time: disable
                .filter(|disable| now >= disable.disabled.due_at)
                .map(|disable| system_time_to_date_time(disable.disabled.at)),
        }
    }

    /// This pair as `EnableFastSnapshotRestores` reports it in `Successful` at `now`.
    pub fn enable_item_at(
        &self,
        now: Instant,
        zone_form: FastSnapshotRestoreZoneForm,
    ) -> EnableFastSnapshotRestoreSuccessItem {
        let item = self.view_at(now);
        EnableFastSnapshotRestoreSuccessItem {
            snapshot_id: item.snapshot_id,
            availability_zone: zone_form.reported_zone_name(&self.availability_zone),
            availability_zone_id: zone_form.reported_zone_id(&self.availability_zone_id),
            state: item.state,
            state_transition_reason: item.state_transition_reason,
            owner_id: item.owner_id,
            owner_alias: item.owner_alias,
            enabling_time: item.enabling_time,
            optimizing_time: item.optimizing_time,
            enabled_time: item.enabled_time,
            disabling_time: item.disabling_time,
            disabled_time: item.disabled_time,
        }
    }

    /// This pair as `DisableFastSnapshotRestores` reports it in `Successful` at `now`.
    pub fn disable_item_at(
        &self,
        now: Instant,
        zone_form: FastSnapshotRestoreZoneForm,
    ) -> DisableFastSnapshotRestoreSuccessItem {
        let item = self.enable_item_at(now, zone_form);
        DisableFastSnapshotRestoreSuccessItem {
            snapshot_id: item.snapshot_id,
            availability_zone: item.availability_zone,
            availability_zone_id: item.availability_zone_id,
            state: item.state,
            state_transition_reason: item.state_transition_reason,
            owner_id: item.owner_id,
            owner_alias: item.owner_alias,
            enabling_time: item.enabling_time,
            optimizing_time: item.optimizing_time,
            enabled_time: item.enabled_time,
            disabling_time: item.disabling_time,
            disabled_time: item.disabled_time,
        }
    }
}

/// The pair as a disable of a snapshot that has no fast snapshot restore in that zone
/// reports it: `disabled`.
///
/// A pair nothing enabled is already in the state the request asks for, so the disable is
/// reported as done rather than as a failure — and it carries no lifecycle at all: no
/// account enabled it, and none of the five phase timestamps happened, so the item names
/// only the pair and the state it is in.
pub fn disabled_fast_snapshot_restore_item(
    snapshot_id: &str,
    zone: &AvailabilityZoneRecord,
    zone_form: FastSnapshotRestoreZoneForm,
) -> DisableFastSnapshotRestoreSuccessItem {
    DisableFastSnapshotRestoreSuccessItem {
        snapshot_id: Some(snapshot_id.to_string()),
        availability_zone: zone_form.reported_zone_name(&zone.zone_name),
        availability_zone_id: zone_form.reported_zone_id(&zone.zone_id),
        state: Some(FAST_SNAPSHOT_RESTORE_STATE_DISABLED.to_string()),
        state_transition_reason: None,
        owner_id: None,
        owner_alias: None,
        enabling_time: None,
        optimizing_time: None,
        enabled_time: None,
        disabling_time: None,
        disabled_time: None,
    }
}

/// Which of the two identifiers of an Availability Zone a request named its zones by.
///
/// "Either AvailabilityZone or AvailabilityZoneId must be specified in the request, but not
/// both" (`EnableFastSnapshotRestoresRequest$AvailabilityZones`), and the answer reports
/// each pair by the SAME identifier: a client that asked by zone name reads
/// `availabilityZone` back, and one that asked by zone id reads `availabilityZoneId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastSnapshotRestoreZoneForm {
    /// `AvailabilityZones`: "One or more Availability Zones. For example, us-east-2a."
    Name,
    /// `AvailabilityZoneIds`: "One or more Availability Zone IDs. For example, use2-az1."
    Id,
}

impl FastSnapshotRestoreZoneForm {
    /// The request member this form reads, for a refusal that names the parameter it is
    /// about.
    pub fn request_member(self) -> &'static str {
        match self {
            Self::Name => "AvailabilityZones",
            Self::Id => "AvailabilityZoneIds",
        }
    }

    /// Which member of [`AvailabilityZoneRecord`] this form reads, for the crate's one zone
    /// lookup ([`availability_zone_of_region`]).
    fn select(self) -> fn(&AvailabilityZoneRecord) -> &str {
        match self {
            Self::Name => |zone| zone.zone_name.as_str(),
            Self::Id => |zone| zone.zone_id.as_str(),
        }
    }

    /// The `AvailabilityZone` an answer reports for a pair: the zone's name when that is
    /// what the request named it by, and nothing when it asked by zone id.
    pub fn reported_zone_name(self, zone_name: &str) -> Option<String> {
        matches!(self, Self::Name).then(|| zone_name.to_string())
    }

    /// The `AvailabilityZoneId` an answer reports for a pair, the mirror of
    /// [`Self::reported_zone_name`].
    pub fn reported_zone_id(self, zone_id: &str) -> Option<String> {
        matches!(self, Self::Id).then(|| zone_id.to_string())
    }
}

/// The pairs one enable or disable request names: the snapshots, the zones, and which
/// identifier the request named those zones by.
#[derive(Debug, Clone)]
pub struct FastSnapshotRestoreRequest {
    /// Which identifier the request named its zones by, which every answer mirrors.
    pub zone_form: FastSnapshotRestoreZoneForm,
    /// The snapshot ids the request named, in request order with repeats dropped.
    pub snapshot_ids: Vec<String>,
    /// The zones of the signed Region the request named, in request order with repeats
    /// dropped.
    pub zones: Vec<AvailabilityZoneRecord>,
}

/// Read the pairs an enable or a disable names out of its request members, or answer the
/// refusal EC2 gives for a request that names none the service can work with.
///
/// ONE reading for both operations, which declare the SAME members
/// (`SourceSnapshotIds`, `AvailabilityZones`, `AvailabilityZoneIds`) with the same
/// documentation, so the two cannot refuse one request differently or report one pair by
/// different identifiers.
pub fn fast_snapshot_restore_request<E>(
    source_snapshot_ids: &[String],
    availability_zones: Option<&[String]>,
    availability_zone_ids: Option<&[String]>,
    region: &str,
) -> Result<FastSnapshotRestoreRequest, Scaffolded<E>> {
    // "The IDs of one or more snapshots" (`SourceSnapshotIds`, `@required`): the model
    // refuses a request that omits the member, and one carrying an EMPTY list names no
    // snapshot at all, which is the same missing parameter. ec2Query cannot express an empty
    // list (an omitted member is what one reaches the wire as), so over that protocol the
    // boundary's own refusal is what a caller sees; this arm is what answers the same
    // request from any other reader of this function.
    if source_snapshot_ids.is_empty() {
        return Err(missing_parameter(
            "The request must contain at least one SourceSnapshotId",
        ));
    }
    // The model publishes no bound on the list, and every id named adds one item per zone
    // to the answer, so the number one request may name is capped here — the same ceiling
    // the rest of this crate puts on a caller-chosen id list — before any per-pair work is
    // done.
    if source_snapshot_ids.len() > MAX_RESOURCE_IDS {
        return Err(invalid_parameter_value(format!(
            "SourceSnapshotIds carries {supplied} ids, above the maximum of {maximum}",
            supplied = source_snapshot_ids.len(),
            maximum = MAX_RESOURCE_IDS,
        )));
    }

    // "Either AvailabilityZone or AvailabilityZoneId must be specified in the request, but
    // not both" (`AvailabilityZones` / `AvailabilityZoneIds`). The form the request chose is
    // also the form the answer reports each pair in, so a caller reads back the identifier
    // it asked with.
    let named = availability_zones.unwrap_or_default();
    let identified = availability_zone_ids.unwrap_or_default();
    let (zone_form, requested_zones) = match (named.is_empty(), identified.is_empty()) {
        (false, false) => {
            return Err(invalid_parameter_combination(
                "Either availabilityZone or availabilityZoneId may be specified, not both",
            ));
        }
        (true, true) => {
            return Err(missing_parameter(
                "Either availabilityZone or availabilityZoneId must be specified",
            ));
        }
        (false, true) => (FastSnapshotRestoreZoneForm::Name, named),
        (true, false) => (FastSnapshotRestoreZoneForm::Id, identified),
    };
    if requested_zones.len() > MAX_RESOURCE_IDS {
        return Err(invalid_parameter_value(format!(
            "{member} carries {supplied} zones, above the maximum of {maximum}",
            member = zone_form.request_member(),
            supplied = requested_zones.len(),
            maximum = MAX_RESOURCE_IDS,
        )));
    }

    // Each zone must be one the Region the caller SIGNED for has: EC2 is a regional service
    // reached through a regional endpoint, and the naming rule for a Region's zones lives in
    // one place, `availability_zones_in_region`. A zone that Region does not have is the
    // caller's mistake about the request rather than a per-pair failure, so it refuses the
    // whole call — through `availability_zone_of_region`, the crate's ONE lookup for both
    // spellings of a zone, so this surface and the ones beside it cannot answer an unknown
    // zone two ways.
    //
    // One zone gets one answer however many times the request names it, the same reason
    // `deduplicate_ids` gives for an id list: a repeat would report the same pair twice.
    let held = availability_zones_in_region(region);
    let mut zones = Vec::with_capacity(requested_zones.len());
    for zone in deduplicate_ids(requested_zones) {
        zones.push(
            availability_zone_of_region(
                &held,
                region,
                zone_form.request_member(),
                &zone,
                zone_form.select(),
            )?
            .clone(),
        );
    }

    Ok(FastSnapshotRestoreRequest {
        zone_form,
        snapshot_ids: deduplicate_ids(source_snapshot_ids),
        zones,
    })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Enable fast snapshot restores for one pair at `now`, and answer the record the pair
    /// now has.
    ///
    /// A pair with no record, and one whose restore has been disabled, start the lifecycle
    /// over from `enabling`. A pair that is already enabling, optimizing or enabled keeps
    /// the record it has: the enable it is asking for is the one already running, so
    /// repeating the call reports that pair's progress rather than restarting it.
    pub fn enable_fast_snapshot_restore(
        &mut self,
        snapshot_id: &str,
        zone: &AvailabilityZoneRecord,
        owner_id: &str,
        now: SystemTime,
    ) -> &FastSnapshotRestoreRecord {
        let key = fast_snapshot_restore_key(snapshot_id, &zone.zone_name);
        match self.fast_snapshot_restores.entry(key) {
            Entry::Vacant(slot) => slot.insert(FastSnapshotRestoreRecord::enabling_at(
                snapshot_id,
                zone,
                owner_id,
                now,
            )),
            Entry::Occupied(slot) => {
                let held = slot.into_mut();
                if held.disabling.is_some() {
                    *held =
                        FastSnapshotRestoreRecord::enabling_at(snapshot_id, zone, owner_id, now);
                }
                held
            }
        }
    }

    /// Disable fast snapshot restores for one pair at `now`, and answer the record the pair
    /// now has — `None` when the account has none for it, there being nothing to disable.
    ///
    /// The record is KEPT rather than dropped: `FastSnapshotRestoreStateCode` has a word for
    /// a pair whose restore was turned off (`disabled`), and a describe that no longer
    /// reported the pair could not answer the `state` filter that names it.
    pub fn disable_fast_snapshot_restore(
        &mut self,
        snapshot_id: &str,
        availability_zone: &str,
        now: SystemTime,
    ) -> Option<&FastSnapshotRestoreRecord> {
        let record = self
            .fast_snapshot_restores
            .get_mut(&fast_snapshot_restore_key(snapshot_id, availability_zone))?;
        record.begin_disable(now);
        Some(record)
    }

    /// Drop every pair of the snapshot `snapshot_id`, whichever zone it is in.
    ///
    /// For the snapshot's DELETION, not for a disable: a disable keeps the record because
    /// `FastSnapshotRestoreStateCode` has a word for a pair whose restore was turned off
    /// ([`FAST_SNAPSHOT_RESTORE_STATE_DISABLED`]), while a deleted snapshot leaves no pair at
    /// all — a fast snapshot restore is a property of a (snapshot, Availability Zone) pair,
    /// so one half of it being gone is not a state the pair can be in. A describe therefore
    /// reports no pair for a snapshot that no longer exists, instead of one naming an id
    /// nothing resolves.
    pub fn forget_fast_snapshot_restores(&mut self, snapshot_id: &str) {
        self.fast_snapshot_restores
            .retain(|_, record| record.snapshot_id != snapshot_id);
    }
}
