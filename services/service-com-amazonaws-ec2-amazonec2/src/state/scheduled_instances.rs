// SPDX-License-Identifier: CC0-1.0

//! Scheduled Instances: the capacity a Region has set aside for them, and the schedule
//! arithmetic every operation that reads that capacity shares.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! [`super::reserved_instances`] gives: the store, the words a schedule is described in and
//! the arithmetic that turns a recurrence into hours are one subject, and keeping them
//! together leaves `state.rs` readable. Everything here is re-exported by `crate::state`, so
//! a handler reaches it as `crate::state::scheduled_instance_occurrences_per_year` exactly as
//! it reaches the helpers that live in the parent module.
//!
//! ## Capacity is stored, not derived
//!
//! A Region's Availability Zones and the instance types it offers are DERIVED from the Region
//! name (see [`crate::state`], "Availability Zones are facts of a Region"), because AWS
//! reports the zones and types a Region *has*. Scheduled Instance capacity is not of that
//! kind: it is capacity AWS sets ASIDE for term purchase, so whether a Region holds any is a
//! fact about that Region's inventory rather than about its name — and nothing in the EC2 API
//! sets capacity aside, exactly as nothing in it racks the Outpost behind
//! [`ComAmazonawsEc2Amazonec2State::local_gateways`]. So it is a STORE
//! ([`ComAmazonawsEc2Amazonec2State::scheduled_instance_capacity`]) that starts EMPTY, and a
//! search of an account whose Region holds none is answered with no schedules at all — which
//! is what the recorded `DescribeScheduledInstanceAvailability` response carries: HTTP 200
//! with an empty `scheduledInstanceAvailabilitySet`.
//!
//! The schedule a search reports is then computed from the capacity record and the request:
//! the recurrence says how often a slot comes round
//! ([`scheduled_instance_occurrences_per_year`]), which is what fixes the smallest slot that
//! still meets the 1,200-hour-per-year minimum
//! ([`scheduled_instance_minimum_slot_hours`]), and the record says which zone, instance
//! type and platform the capacity is in and what an hour of it costs.
//!
//! ## A purchased Scheduled Instance IS stored
//!
//! A schedule on offer is computed, but a schedule the account has BOUGHT is a resource it
//! holds: [`ComAmazonawsEc2Amazonec2State::scheduled_instances`] holds one
//! [`ScheduledInstanceRecord`] per purchase, keyed by the `ScheduledInstanceId`
//! [`ComAmazonawsEc2Amazonec2State::insert_scheduled_instance`] mints for it, which is the
//! order `DescribeScheduledInstances` reports and pages over. The record holds the terms of
//! the purchase — the capacity it draws on, the recurrence, the slot and the term's two ends —
//! and NOT where the schedule has got to: "the time for the next schedule to start"
//! (`ScheduledInstance$NextSlotStartTime`) and "the time that the previous schedule ended or
//! will end" (`$PreviousSlotEndTime`) move with the clock on their own, so
//! [`scheduled_instance_slot_boundaries`] derives both from the stored recurrence at the
//! instant of the read and [`scheduled_instance_view`] projects the record onto the modelled
//! shape. A report therefore writes nothing, and two reads an hour apart report the same
//! purchase with the schedule each has reached.

use super::*;
use crate::ops::types::{ScheduledInstance, ScheduledInstanceRecurrence};

// ---------------------------------------------------------------------------
// The words a Scheduled Instance schedule is described in
// ---------------------------------------------------------------------------

/// A schedule that comes round every day: the `Daily` frequency of
/// `ScheduledInstanceRecurrenceRequest$Frequency` — "The frequency (`Daily`, `Weekly`, or
/// `Monthly`)".
pub const SCHEDULED_INSTANCE_FREQUENCY_DAILY: &str = "Daily";

/// A schedule that comes round every week: that member's `Weekly` frequency.
pub const SCHEDULED_INSTANCE_FREQUENCY_WEEKLY: &str = "Weekly";

/// A schedule that comes round every month: that member's `Monthly` frequency.
pub const SCHEDULED_INSTANCE_FREQUENCY_MONTHLY: &str = "Monthly";

/// Every frequency a schedule can recur at, in the order the member's own doc names them.
pub const SCHEDULED_INSTANCE_FREQUENCIES: [&str; 3] = [
    SCHEDULED_INSTANCE_FREQUENCY_DAILY,
    SCHEDULED_INSTANCE_FREQUENCY_WEEKLY,
    SCHEDULED_INSTANCE_FREQUENCY_MONTHLY,
];

/// Occurrence days counted as days of the WEEK: the `DayOfWeek` unit of
/// `ScheduledInstanceRecurrenceRequest$OccurrenceUnit` — "The unit for `OccurrenceDays`
/// (`DayOfWeek` or `DayOfMonth`)".
pub const SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_WEEK: &str = "DayOfWeek";

/// Occurrence days counted as days of the MONTH: that member's `DayOfMonth` unit.
pub const SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_MONTH: &str = "DayOfMonth";

/// Both units, in the order the member's own doc names them.
pub const SCHEDULED_INSTANCE_OCCURRENCE_UNITS: [&str; 2] = [
    SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_WEEK,
    SCHEDULED_INSTANCE_OCCURRENCE_UNIT_DAY_OF_MONTH,
];

/// The platform of a Scheduled Instance that runs Linux: one of the two values
/// `ScheduledInstanceAvailability$Platform` names — "The platform (`Linux/UNIX` or
/// `Windows`)" — and one of the two the `platform` filter selects on.
pub const SCHEDULED_INSTANCE_PLATFORM_LINUX: &str = "Linux/UNIX";

/// The platform of a Scheduled Instance that runs Windows: the other value of that member.
pub const SCHEDULED_INSTANCE_PLATFORM_WINDOWS: &str = "Windows";

/// Both platforms, in the order the member's own doc names them.
pub const SCHEDULED_INSTANCE_PLATFORMS: [&str; 2] = [
    SCHEDULED_INSTANCE_PLATFORM_LINUX,
    SCHEDULED_INSTANCE_PLATFORM_WINDOWS,
];

/// The network a Scheduled Instance of a Region that has only VPCs runs in:
/// `ScheduledInstanceAvailability$NetworkPlatform` ("The network platform") spelled the way
/// EC2 spells a network platform elsewhere in this API — the `EC2-VPC` of
/// `AccountAttribute`'s `supported-platforms` — since EC2-Classic is gone.
pub const SCHEDULED_INSTANCE_NETWORK_PLATFORM_VPC: &str = "EC2-VPC";

/// The term a Scheduled Instance is bought for, in days: "The minimum term. The only possible
/// value is 365 days" (`ScheduledInstanceAvailability$MinTermDurationInDays`), and the same
/// sentence on `MaxTermDurationInDays` — so one constant answers both members.
pub const SCHEDULED_INSTANCE_TERM_DURATION_DAYS: i32 = 365;

/// The hours a Scheduled Instance schedule must add up to over its term: "You must meet the
/// minimum required duration of 1,200 hours per year"
/// (`DescribeScheduledInstanceAvailability`).
pub const SCHEDULED_INSTANCE_MIN_HOURS_PER_YEAR: i32 = 1_200;

/// The duration a slot must stay BELOW, in hours: "This value must be greater than
/// MinSlotDurationInHours and less than 1,720"
/// (`DescribeScheduledInstanceAvailabilityRequest$MaxSlotDurationInHours`) — so the bound is
/// exclusive.
pub const SCHEDULED_INSTANCE_SLOT_DURATION_LIMIT_HOURS: i32 = 1_720;

/// The days one term covers, which is what [`scheduled_instance_occurrences_per_year`] counts
/// a DAILY schedule's slots over: the term is [`SCHEDULED_INSTANCE_TERM_DURATION_DAYS`] days.
pub const SCHEDULED_INSTANCE_DAYS_PER_TERM: i32 = SCHEDULED_INSTANCE_TERM_DURATION_DAYS;

/// The whole weeks one term covers, which a WEEKLY schedule's slots are counted over.
pub const SCHEDULED_INSTANCE_WEEKS_PER_TERM: i32 = SCHEDULED_INSTANCE_DAYS_PER_TERM / 7;

/// The months one term covers, which a MONTHLY schedule's slots are counted over.
pub const SCHEDULED_INSTANCE_MONTHS_PER_TERM: i32 = 12;

/// The days of the week an occurrence day may name: "for a weekly schedule, this is one or
/// more days of the week (1-7, where 1 is Sunday)"
/// (`ScheduledInstanceRecurrenceRequest$OccurrenceDays`).
pub const SCHEDULED_INSTANCE_DAYS_OF_WEEK: ::std::ops::RangeInclusive<i32> = 1..=7;

/// The days of the month an occurrence day may name: "For a monthly schedule, this is one or
/// more days of the month (1-31)" (the same member's doc).
pub const SCHEDULED_INSTANCE_DAYS_OF_MONTH: ::std::ops::RangeInclusive<i32> = 1..=31;

/// How long a purchase token stays good for, in seconds: "The purchase token. This token
/// expires in two hours" (`ScheduledInstanceAvailability$PurchaseToken`).
pub const SCHEDULED_INSTANCE_PURCHASE_TOKEN_LIFETIME_SECONDS: i64 = 2 * 60 * 60;

/// The prefix a purchase token carries, so a token this service minted is told from a value a
/// caller assembled before any of its fields are read.
const SCHEDULED_INSTANCE_PURCHASE_TOKEN_PREFIX: &str = "sipt";

/// The marker inside a purchase token saying which kind of thing its fields describe — see
/// [`encode_scheduled_instance_purchase_token`].
const SCHEDULED_INSTANCE_PURCHASE_TOKEN_VERSION: &str = "ec2-sipt-1";

// ---------------------------------------------------------------------------
// The capacity a Region has set aside
// ---------------------------------------------------------------------------

/// One block of Scheduled Instance capacity: the instances of one type, on one platform, in
/// one Availability Zone that AWS has set aside for schedules, and what an hour of one costs.
///
/// It holds what a SEARCH cannot compute — which zone, type and platform have capacity, how
/// many instances are left in it and the published price — and nothing a search computes: the
/// slot, its first start and the term's hours all follow from the request's recurrence and
/// window, so storing them would let one zone's record disagree with another's about
/// arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ScheduledInstanceCapacityRecord {
    /// The Availability Zone the capacity sits in, as
    /// [`availability_zones_in_region`] names one (`us-west-2a`). A record in a zone the
    /// request's Region does not have is no capacity that request can be answered with.
    pub availability_zone: String,
    /// The instance type the capacity is for. "You can specify one of the C3, C4, M4, or R3
    /// instance types" (`ScheduledInstanceAvailability$InstanceType`) is what AWS offered
    /// schedules for; the store keeps whatever type its writer set aside rather than a second
    /// catalog of its own.
    pub instance_type: String,
    /// `Linux/UNIX` or `Windows` — see [`SCHEDULED_INSTANCE_PLATFORMS`].
    pub platform: String,
    /// The network platform an instance launched from this capacity runs in — see
    /// [`SCHEDULED_INSTANCE_NETWORK_PLATFORM_VPC`].
    pub network_platform: String,
    /// "The number of available instances" (`ScheduledInstanceAvailability$AvailableInstanceCount`):
    /// what is left of this block, which a purchase draws down.
    pub available_instance_count: i32,
    /// "The hourly price for a single instance"
    /// (`ScheduledInstanceAvailability$HourlyPrice`), as the decimal amount that member
    /// carries. Held rather than derived: a Scheduled Instance price is AWS's published term
    /// price for the capacity, which neither the model nor a request carries.
    pub hourly_price: String,
}

/// The key one capacity record is stored under: its zone, platform and instance type, joined
/// by a space.
///
/// Capacity is one block per zone, platform and instance type — those three are what a
/// `DescribeScheduledInstanceAvailability` filter selects on — so the key is exactly the
/// identity of a block, and a second record for the same three replaces the first instead of
/// being reported twice. The separator sorts ahead of every character a zone, platform or
/// instance type can carry, so one value being a prefix of another cannot reorder two keys —
/// the same construction [`offering_page_cursor`] uses, and the order a page of availability
/// is reported and resumed in.
pub fn scheduled_instance_capacity_key(
    availability_zone: &str,
    platform: &str,
    instance_type: &str,
) -> String {
    format!("{availability_zone} {platform} {instance_type}")
}

impl ComAmazonawsEc2Amazonec2State {
    /// Set `record`'s capacity aside, and answer the key it is stored under.
    ///
    /// The ONE writer of [`Self::scheduled_instance_capacity`], so every block is keyed the
    /// one way ([`scheduled_instance_capacity_key`]) and a reader can page over the store's
    /// own order. Nothing in the EC2 API calls it — AWS sets Scheduled Instance capacity
    /// aside itself, as it racks the Outpost behind [`Self::local_gateways`] — so it is how
    /// an account's inventory is seeded, and it charges nothing against
    /// [`Self::stored_bytes`]: that charge bounds what a REQUEST can leave in the store, and
    /// no request reaches this.
    pub fn insert_scheduled_instance_capacity(
        &mut self,
        record: ScheduledInstanceCapacityRecord,
    ) -> String {
        let key = scheduled_instance_capacity_key(
            &record.availability_zone,
            &record.platform,
            &record.instance_type,
        );
        self.scheduled_instance_capacity
            .insert(key.clone(), record);
        key
    }
}

// ---------------------------------------------------------------------------
// What a recurrence adds up to over one term
// ---------------------------------------------------------------------------

/// How many slots a schedule of `frequency` every `interval` units, on `occurrence_days`
/// days, comes round for in one term; `None` when it comes round for none at all.
///
/// A daily schedule has one slot per day of the term, a weekly one per named day of each
/// week it falls in and a monthly one per named day of each month — divided by the interval,
/// which is "the interval quantity. The interval unit depends on the value of `frequency`"
/// (`ScheduledInstanceRecurrence$Interval`). A schedule that names no occurrence day comes
/// round once per period, which is what a daily schedule always is ("You can't specify this
/// value with a daily schedule") and what a weekly or monthly one that leaves the days to AWS
/// is.
///
/// An interval longer than the term leaves no slot inside it, and that is `None` rather than
/// zero: there is no schedule to report at all, so no slot duration could make it meet the
/// 1,200 hours its term needs.
pub fn scheduled_instance_occurrences_per_year(
    frequency: &str,
    interval: i32,
    occurrence_days: usize,
) -> Option<i32> {
    let interval = interval.max(1);
    let days = i32::try_from(occurrence_days.max(1)).unwrap_or(i32::MAX);
    let periods = match frequency {
        SCHEDULED_INSTANCE_FREQUENCY_DAILY => SCHEDULED_INSTANCE_DAYS_PER_TERM / interval,
        SCHEDULED_INSTANCE_FREQUENCY_WEEKLY => SCHEDULED_INSTANCE_WEEKS_PER_TERM / interval,
        SCHEDULED_INSTANCE_FREQUENCY_MONTHLY => SCHEDULED_INSTANCE_MONTHS_PER_TERM / interval,
        // Unreachable through a validated request: a frequency outside
        // [`SCHEDULED_INSTANCE_FREQUENCIES`] names no schedule, so it comes round never
        // rather than being counted as one of the three.
        _ => 0,
    };
    // A daily schedule names no day, so its count is the periods themselves; for the other
    // two the named days multiply the periods.
    let occurrences = periods.saturating_mul(days);
    (occurrences > 0).then_some(occurrences)
}

/// The shortest slot, in hours, a schedule that comes round `occurrences_per_term` times may
/// have: the smallest whole number of hours that still adds up to
/// [`SCHEDULED_INSTANCE_MIN_HOURS_PER_YEAR`] over the term.
///
/// This is the rule "You must meet the minimum required duration of 1,200 hours per year" as
/// arithmetic, and the operation's own examples are what it answers: 4 hours for a daily
/// schedule (365 slots), 24 for a weekly one (52 slots) and 100 for a monthly one (12 slots).
pub fn scheduled_instance_minimum_slot_hours(occurrences_per_term: i32) -> Option<i32> {
    let occurrences = u32::try_from(occurrences_per_term)
        .ok()
        .filter(|occurrences| *occurrences > 0)?;
    let minimum = u32::try_from(SCHEDULED_INSTANCE_MIN_HOURS_PER_YEAR).unwrap_or(0);
    // Rounded UP: a slot one hour shorter would leave the term below the minimum. And never
    // to nothing — a slot runs for some whole number of hours, however often it comes round.
    i32::try_from(minimum.div_ceil(occurrences).max(1)).ok()
}

// ---------------------------------------------------------------------------
// The token a purchase is made with
// ---------------------------------------------------------------------------

/// What one reported schedule would be purchased as: the capacity it draws on, the schedule
/// itself, and the instant the token naming it stops being good.
///
/// A schedule is not a stored resource — it is what a search computes from stored capacity
/// and the request — so the terms travel INSIDE the purchase token
/// ([`encode_scheduled_instance_purchase_token`]) rather than in a map keyed by it, exactly
/// as a Capacity Block offering's terms travel inside its offering id
/// ([`CapacityBlockOfferingTerms`]). That is what lets the search report a token without
/// writing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledInstancePurchaseTerms {
    /// The Availability Zone the capacity sits in.
    pub availability_zone: String,
    /// The instance type the schedule is for.
    pub instance_type: String,
    /// The platform those instances run.
    pub platform: String,
    /// When the first slot of the schedule starts, in seconds since the Unix epoch.
    pub first_slot_start_secs: i64,
    /// How many hours one slot runs for.
    pub slot_duration_in_hours: i32,
    /// When the token stops being good, in seconds since the Unix epoch:
    /// [`SCHEDULED_INSTANCE_PURCHASE_TOKEN_LIFETIME_SECONDS`] after it was minted.
    pub expires_at_secs: i64,
}

/// The `PurchaseToken` for `terms`: the one derived-id construction this service reports every
/// derived resource under ([`encode_derived_id`]), over the terms, a marker saying they are a
/// Scheduled Instance purchase and a checksum.
///
/// Deriving the token FROM the terms is what makes it resolvable without a store, and the
/// expiry it carries is what makes it expire: a token is good for
/// [`SCHEDULED_INSTANCE_PURCHASE_TOKEN_LIFETIME_SECONDS`] from the call that minted it, so two
/// searches a while apart report different tokens for the same schedule — the token is a
/// per-call credential for a purchase, not a name for the schedule.
pub fn encode_scheduled_instance_purchase_token(terms: &ScheduledInstancePurchaseTerms) -> String {
    super::encode_derived_id(
        SCHEDULED_INSTANCE_PURCHASE_TOKEN_PREFIX,
        SCHEDULED_INSTANCE_PURCHASE_TOKEN_VERSION,
        &[
            terms.first_slot_start_secs.to_string(),
            terms.slot_duration_in_hours.to_string(),
            terms.expires_at_secs.to_string(),
            super::derived_id_text_field(Some(terms.availability_zone.as_str())),
            super::derived_id_text_field(Some(terms.instance_type.as_str())),
            super::derived_id_text_field(Some(terms.platform.as_str())),
        ],
    )
}

// ---------------------------------------------------------------------------
// The Scheduled Instances the account has bought
// ---------------------------------------------------------------------------

/// The seconds one hour of a slot runs for, the unit `$SlotDurationInHours` is given in.
const SECONDS_IN_HOUR: i64 = 60 * 60;

/// How many Scheduled Instances this emulator stores before a purchase is refused.
///
/// CHOSEN, like [`MAX_STORED_RESERVED_INSTANCES`] and for the same reason: nothing reclaims a
/// stored Scheduled Instance — a term that has run out still names a purchase the account
/// made, so the record stays — so the store needs a ceiling that is not the process's memory.
/// It is well above any published per-Region Scheduled Instance quota a test would exercise.
pub const MAX_STORED_SCHEDULED_INSTANCES: usize = 4096;

/// The leading part every `ScheduledInstanceId` this store mints carries.
///
/// A Scheduled Instance id is NOT the `<prefix>-<hex>` form [`prefixed_id_is_well_formed`]
/// reads: AWS's own `ScheduledInstanceId` values read `sci-1234-1234-1234-1234-123456789abcdef`
/// — the prefix, four groups of four hexadecimal digits and a last group of fifteen — and the
/// model pins no pattern on the member (`ScheduledInstanceId` is a plain `String`), so the form
/// is decided here, in the one place such an id is minted and the one place one is read. The
/// four fixed groups are a LAYOUT, as [`RESERVED_INSTANCES_ID_LAYOUT_PREFIX`] is for a Reserved
/// Instance's UUID; the store's own monotonic counter fills the last group.
const SCHEDULED_INSTANCE_ID_LAYOUT_PREFIX: &str = "sci-ec20-0000-0000-0000";

/// The digits in each of the four leading groups of a `ScheduledInstanceId`.
const SCHEDULED_INSTANCE_ID_GROUP_DIGITS: usize = 4;

/// The digits in its last group, which is the one the store's counter fills.
const SCHEDULED_INSTANCE_ID_NODE_DIGITS: usize = 15;

/// The groups a `ScheduledInstanceId` is made of, counting the `sci` prefix.
const SCHEDULED_INSTANCE_ID_GROUPS: usize = 6;

/// `InvalidScheduledInstanceId.Malformed` (HTTP 400): the request named something that is not
/// a Scheduled Instance id at all, so there is nothing to look up.
///
/// The `Invalid<Member>.Malformed` / `.NotFound` pair EC2 answers for every resource id whose
/// own code the model does not declare (EC2's model declares no error shapes at all — see
/// [`crate::scaffolded`]), named after the request member `ScheduledInstanceId` the way
/// [`INVALID_IPAM_POOL_ID_MALFORMED`] is named after its own.
pub const INVALID_SCHEDULED_INSTANCE_ID_MALFORMED: &str = "InvalidScheduledInstanceId.Malformed";

/// `InvalidScheduledInstanceId.NotFound` (HTTP 400): the request named a well-formed Scheduled
/// Instance id this account holds no purchase for.
pub const INVALID_SCHEDULED_INSTANCE_ID_NOT_FOUND: &str = "InvalidScheduledInstanceId.NotFound";

/// Whether `id` is a well-formed `ScheduledInstanceId`.
///
/// The form [`SCHEDULED_INSTANCE_ID_LAYOUT_PREFIX`] documents: the `sci` prefix, four groups of
/// four lowercase hexadecimal digits and a last group of fifteen. Any value of that form passes,
/// because an id a caller names may have been minted by AWS rather than by this store; only the
/// FORM is this service's to read. Reading a caller's value never panics and never allocates.
pub fn scheduled_instance_id_is_well_formed(id: &str) -> bool {
    let mut groups = id.split('-');
    if groups.next() != Some(SCHEDULED_INSTANCE_ID_PREFIX) {
        return false;
    }
    let hexadecimal = |group: &str, digits: usize| {
        group.len() == digits && group.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    };
    let mut seen = 1;
    for group in groups {
        seen += 1;
        let digits = if seen == SCHEDULED_INSTANCE_ID_GROUPS {
            SCHEDULED_INSTANCE_ID_NODE_DIGITS
        } else if seen < SCHEDULED_INSTANCE_ID_GROUPS {
            SCHEDULED_INSTANCE_ID_GROUP_DIGITS
        } else {
            return false;
        };
        if !hexadecimal(group, digits) {
            return false;
        }
    }
    seen == SCHEDULED_INSTANCE_ID_GROUPS
}

/// The prefix a `ScheduledInstanceId` carries, as a request member names one.
pub const SCHEDULED_INSTANCE_ID_PREFIX: &str = "sci";

/// Refuse a value that is not a Scheduled Instance id at all: HTTP 400
/// [`INVALID_SCHEDULED_INSTANCE_ID_MALFORMED`], worded with the crate's one
/// [`invalid_id_message`], and answered BEFORE anything is looked up — which is what lets a
/// caller tell "not an id of this kind" from "no such purchase".
pub fn invalid_scheduled_instance_id_malformed<E>(scheduled_instance_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SCHEDULED_INSTANCE_ID_MALFORMED,
        400,
        invalid_id_message(scheduled_instance_id),
    )
}

/// Refuse a well-formed Scheduled Instance id this account holds no purchase for: HTTP 400
/// [`INVALID_SCHEDULED_INSTANCE_ID_NOT_FOUND`], through the crate's one [`id_not_found`]
/// sentence.
pub fn scheduled_instance_id_does_not_exist<E>(scheduled_instance_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_SCHEDULED_INSTANCE_ID_NOT_FOUND,
        "Scheduled Instance ID",
        &echoed_resource_id(scheduled_instance_id),
    )
}

/// What one Scheduled Instance purchase is stored with: the terms of the purchase, everything
/// the service derives from them being left to
/// [`ComAmazonawsEc2Amazonec2State::insert_scheduled_instance`].
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ScheduledInstanceSpec {
    /// The Availability Zone the capacity the purchase draws on sits in, as
    /// [`availability_zones_in_region`] names one. It is what says which REGION holds the
    /// purchase, exactly as it does for a block of capacity.
    pub availability_zone: String,
    /// "The instance type" (`ScheduledInstance$InstanceType`).
    pub instance_type: String,
    /// `Linux/UNIX` or `Windows` — see [`SCHEDULED_INSTANCE_PLATFORMS`].
    pub platform: String,
    /// "The network platform" — see [`SCHEDULED_INSTANCE_NETWORK_PLATFORM_VPC`].
    pub network_platform: String,
    /// "The number of instances" (`ScheduledInstance$InstanceCount`) the purchase bought.
    pub instance_count: i32,
    /// "The hourly price for a single instance" (`$HourlyPrice`), the price of the capacity the
    /// purchase was made against.
    pub hourly_price: String,
    /// "The number of hours in the schedule" (`$SlotDurationInHours`).
    pub slot_duration_in_hours: i32,
    /// The schedule the purchase repeats on, as the search that offered it reported it.
    pub recurrence: ScheduledInstanceRecurrence,
    /// When the first slot of the schedule starts, in seconds since the Unix epoch: "the start
    /// date for the Scheduled Instance" (`$TermStartDate`), which is also where the term's
    /// [`SCHEDULED_INSTANCE_TERM_DURATION_DAYS`] days are counted from.
    pub term_start_secs: i64,
    /// When the purchase was made, in seconds since the Unix epoch: "the date when the
    /// Scheduled Instance was purchased" (`$CreateDate`).
    pub purchased_at_secs: i64,
}

/// One Scheduled Instance the account holds: the terms it was bought on, and nothing a read
/// derives from them.
///
/// The two timeline members of the modelled shape — "the time for the next schedule to start"
/// and "the time that the previous schedule ended or will end" — are NOT stored: they move with
/// the clock on their own, so holding them would let a record go stale between two reads.
/// [`scheduled_instance_slot_boundaries`] derives both from the recurrence at the instant of the
/// read, which is what lets a report write nothing.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ScheduledInstanceRecord {
    /// The id this purchase is stored under, minted by
    /// [`ComAmazonawsEc2Amazonec2State::insert_scheduled_instance`].
    pub scheduled_instance_id: String,
    /// The terms of the purchase, as its buyer made it.
    pub spec: ScheduledInstanceSpec,
    /// When the term ends, in seconds since the Unix epoch: "the end date for the Scheduled
    /// Instance" (`$TermEndDate`), which is [`SCHEDULED_INSTANCE_TERM_DURATION_DAYS`] days after
    /// it started — "the minimum term. The only possible value is 365 days".
    pub term_end_secs: i64,
    /// "The total number of hours for a single instance for the entire term"
    /// (`$TotalScheduledInstanceHours`): the slot's hours times the slots the recurrence comes
    /// round for in one term ([`scheduled_instance_occurrences_per_year`]).
    pub total_scheduled_instance_hours: i32,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Scheduled Instances fit inside the store's ceiling
    /// ([`MAX_STORED_SCHEDULED_INSTANCES`]).
    ///
    /// A purchase must ask this BEFORE inserting anything and refuse when it answers `false`,
    /// the way a Reserved Instance purchase asks [`Self::has_reserved_instances_capacity`]: one
    /// call can buy more than one schedule, so the whole batch is gated rather than each record
    /// as it is written. [`Self::insert_scheduled_instance`] refuses on its own too, so no path
    /// can grow the store past the ceiling even if it forgets to ask.
    pub fn has_scheduled_instance_capacity(&self, count: usize) -> bool {
        self.scheduled_instances.len().saturating_add(count) <= MAX_STORED_SCHEDULED_INSTANCES
    }

    /// A fresh, unused `ScheduledInstanceId`, in the form
    /// [`SCHEDULED_INSTANCE_ID_LAYOUT_PREFIX`] documents.
    ///
    /// The last group is the store's own monotonic counter — the source [`Self::next_id`] draws
    /// on — so an id is unique within a run, the same across runs, and never taken from the
    /// request. That group holds fifteen hexadecimal digits, so the store's ceiling
    /// ([`MAX_STORED_SCHEDULED_INSTANCES`]) is reached long before the layout could repeat one.
    pub fn mint_scheduled_instance_id(&mut self) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!(
            "{SCHEDULED_INSTANCE_ID_LAYOUT_PREFIX}-{counter:0digits$x}",
            counter = self.id_counter,
            digits = SCHEDULED_INSTANCE_ID_NODE_DIGITS,
        )
    }

    /// Store a Scheduled Instance bought on `spec`, and answer the `ScheduledInstanceId` it was
    /// given, or `None` when the store is already at its ceiling.
    ///
    /// The ONE writer of [`Self::scheduled_instances`], so every purchase is keyed by its id and
    /// everything the service itself owns is derived here rather than taken from a caller — the
    /// id, the end of the term (its start plus the one term AWS sells, 365 days), the hours the
    /// term adds up to (the slot times the slots the recurrence comes round for) and the
    /// recurrence in the one form a report of it takes: a schedule that names no interval repeats
    /// every period, which is an interval of one, and a daily schedule has no occurrence day and
    /// no unit to count days in ("you can't specify this value with a daily schedule" on both
    /// members). That is the same resolution `DescribeScheduledInstanceAvailability` reports a
    /// schedule with, so a purchase cannot describe its schedule differently from the search that
    /// offered it.
    ///
    /// `PurchaseScheduledInstances` is the operation that calls it, as
    /// `PurchaseReservedInstancesOffering` calls [`Self::insert_reserved_instances`].
    ///
    /// Every value a caller could size is bounded HERE, so the store's count ceiling really is
    /// what bounds the store and a read's work is a function of the stored terms rather than of
    /// what a request carried. The occurrence days are the one variable-length member of a
    /// schedule, so they are cut down to the days the period PUBLISHES — "one or more days of
    /// the week (1-7, where 1 is Sunday)" for a weekly schedule and "one or more days of the
    /// month (1-31)" for a monthly one ([`SCHEDULED_INSTANCE_DAYS_OF_WEEK`],
    /// [`SCHEDULED_INSTANCE_DAYS_OF_MONTH`]) — sorted and with repeats dropped, which leaves at
    /// most one entry per day of the period however many a request named. The slot is held
    /// inside the duration the model publishes for one ("less than 1,720",
    /// [`SCHEDULED_INSTANCE_SLOT_DURATION_LIMIT_HOURS`]) and the instance count at or above
    /// none, so no term AWS does not sell can be stored and nothing derived from either can
    /// run away. There is nothing else on the record to charge against [`Self::stored_bytes`].
    pub fn insert_scheduled_instance(&mut self, mut spec: ScheduledInstanceSpec) -> Option<String> {
        if !self.has_scheduled_instance_capacity(1) {
            return None;
        }
        let scheduled_instance_id = self.mint_scheduled_instance_id();
        let frequency = spec
            .recurrence
            .frequency
            .clone()
            .unwrap_or_else(|| SCHEDULED_INSTANCE_FREQUENCY_DAILY.to_string());
        let daily = frequency == SCHEDULED_INSTANCE_FREQUENCY_DAILY;
        let occurrence_days: Vec<i32> = if daily {
            Vec::new()
        } else {
            let published = if frequency == SCHEDULED_INSTANCE_FREQUENCY_WEEKLY {
                SCHEDULED_INSTANCE_DAYS_OF_WEEK
            } else {
                SCHEDULED_INSTANCE_DAYS_OF_MONTH
            };
            let mut days: Vec<i32> = spec
                .recurrence
                .occurrence_day_set
                .clone()
                .unwrap_or_default()
                .into_iter()
                .filter(|day| published.contains(day))
                .collect();
            days.sort_unstable();
            days.dedup();
            days
        };
        spec.instance_count = spec.instance_count.max(0);
        spec.slot_duration_in_hours = spec
            .slot_duration_in_hours
            .clamp(0, SCHEDULED_INSTANCE_SLOT_DURATION_LIMIT_HOURS - 1);
        spec.recurrence = ScheduledInstanceRecurrence {
            frequency: Some(frequency.clone()),
            interval: Some(spec.recurrence.interval.unwrap_or(1).max(1)),
            occurrence_day_set: (!occurrence_days.is_empty()).then(|| occurrence_days.clone()),
            occurrence_relative_to_end: Some(
                spec.recurrence.occurrence_relative_to_end.unwrap_or(false),
            ),
            occurrence_unit: (!daily)
                .then(|| spec.recurrence.occurrence_unit.clone())
                .flatten(),
        };
        let occurrences = scheduled_instance_occurrences_per_year(
            &frequency,
            spec.recurrence.interval.unwrap_or(1),
            occurrence_days.len(),
        )
        .unwrap_or(0);
        let record = ScheduledInstanceRecord {
            scheduled_instance_id: scheduled_instance_id.clone(),
            term_end_secs: spec.term_start_secs.saturating_add(
                i64::from(SCHEDULED_INSTANCE_TERM_DURATION_DAYS) * 24 * SECONDS_IN_HOUR,
            ),
            total_scheduled_instance_hours: spec
                .slot_duration_in_hours
                .saturating_mul(occurrences),
            spec,
        };
        self.scheduled_instances
            .insert(scheduled_instance_id.clone(), record);
        Some(scheduled_instance_id)
    }
}

/// How many periods of `frequency` one term covers, which is how far the walk below looks for
/// the slot a schedule has reached: the days, whole weeks (plus the partial one a term can end
/// in) or months of a term of [`SCHEDULED_INSTANCE_TERM_DURATION_DAYS`] days.
fn scheduled_instance_periods_per_term(frequency: &str) -> i64 {
    match frequency {
        SCHEDULED_INSTANCE_FREQUENCY_WEEKLY => i64::from(SCHEDULED_INSTANCE_WEEKS_PER_TERM) + 1,
        SCHEDULED_INSTANCE_FREQUENCY_MONTHLY => i64::from(SCHEDULED_INSTANCE_MONTHS_PER_TERM),
        // A daily schedule, and anything a validated request cannot carry: one period per day of
        // the term.
        _ => i64::from(SCHEDULED_INSTANCE_DAYS_PER_TERM),
    }
}

/// The slots `recurrence` puts in the `period`th period of the term, in order, each at the
/// time of day the term started at.
///
/// A daily schedule has one slot per period ("you can't specify this value with a daily
/// schedule" on `$OccurrenceDays`). A weekly or monthly one has a slot on each day it names
/// — "for a weekly schedule, this is one or more days of the week (1-7, where 1 is Sunday)"
/// and "for a monthly schedule, this is one or more days of the month (1-31)" — or, where it
/// names none, one on the same day of the period the term itself started on. A day counted
/// from the end of the period is the day that far back from its last ("indicates whether the
/// occurrence is relative to the end of the specified week or month"), and a day a shorter
/// month does not have is no slot in that month at all.
fn scheduled_instance_period_slots(
    frequency: &str,
    term_start: ::chrono::NaiveDateTime,
    period: i64,
    days: &[i32],
    relative_to_end: bool,
) -> Vec<::chrono::NaiveDateTime> {
    use ::chrono::{Datelike, Days, Months, NaiveDate};

    let time_of_day = term_start.time();
    let start_date = term_start.date();
    let forward = |date: NaiveDate, days_ahead: i64| -> Option<NaiveDate> {
        date.checked_add_days(Days::new(u64::try_from(days_ahead).ok()?))
    };
    let at = |date: NaiveDate| date.and_time(time_of_day);
    // The day of the month a month-long period's last day is, which a day counted from the end
    // of the month is measured back from.
    let month_length = |month_start: NaiveDate| -> Option<u32> {
        month_start
            .checked_add_months(Months::new(1))
            .and_then(|next| next.checked_sub_days(Days::new(1)))
            .map(|last| last.day())
    };

    let mut slots: Vec<::chrono::NaiveDateTime> = match frequency {
        SCHEDULED_INSTANCE_FREQUENCY_WEEKLY => {
            let Some(week_offset) = period.checked_mul(7) else {
                return Vec::new();
            };
            if days.is_empty() {
                forward(start_date, week_offset).map(at).into_iter().collect()
            } else {
                // "1 is Sunday", so the week a slot's day is counted inside begins on the Sunday
                // of the week the term started in.
                let Some(week_start) = start_date
                    .checked_sub_days(Days::new(u64::from(
                        start_date.weekday().num_days_from_sunday(),
                    )))
                    .and_then(|sunday| forward(sunday, week_offset))
                else {
                    return Vec::new();
                };
                days.iter()
                    .filter_map(|day| {
                        let offset = if relative_to_end {
                            7 - i64::from(*day)
                        } else {
                            i64::from(*day) - 1
                        };
                        (0..7).contains(&offset).then_some(offset)?;
                        forward(week_start, offset).map(at)
                    })
                    .collect()
            }
        }
        SCHEDULED_INSTANCE_FREQUENCY_MONTHLY => {
            let Some(month_start) = start_date
                .with_day(1)
                .and_then(|first| {
                    first.checked_add_months(Months::new(u32::try_from(period).ok()?))
                })
            else {
                return Vec::new();
            };
            let Some(length) = month_length(month_start) else {
                return Vec::new();
            };
            if days.is_empty() {
                month_start
                    .with_day(start_date.day())
                    .map(at)
                    .into_iter()
                    .collect()
            } else {
                days.iter()
                    .filter_map(|day| {
                        let day = u32::try_from(*day).ok()?;
                        let day = if relative_to_end {
                            length.checked_sub(day.checked_sub(1)?)?
                        } else {
                            day
                        };
                        (1..=length).contains(&day).then_some(day)?;
                        month_start.with_day(day).map(at)
                    })
                    .collect()
            }
        }
        // A daily schedule, and anything a validated request cannot carry: one slot, `period`
        // days after the term started.
        _ => forward(start_date, period).map(at).into_iter().collect(),
    };
    // A period's slots are walked in time order, whichever order the request named its days in
    // and whichever end they are counted from.
    slots.sort_unstable();
    slots
}

/// Where `record`'s schedule has got to at `now_secs`: when its next slot starts, and when the
/// slot before that one ends.
///
/// "The time for the next schedule to start" (`ScheduledInstance$NextSlotStartTime`) is the
/// first slot of the term that has not started yet, and `None` once the term holds no further
/// slot. "The time that the previous schedule ended or will end" (`$PreviousSlotEndTime`) is
/// the end of the latest slot that HAS started — which is still in the future while that slot
/// is running, as the member's own wording allows — and `None` before the term's first slot
/// starts, because no schedule has ended yet.
///
/// Derived rather than stored, so a purchase cannot go stale, and bounded: the walk looks at
/// the periods of ONE term ([`scheduled_instance_periods_per_term`]) and stops at the first
/// slot ahead of `now_secs`.
pub fn scheduled_instance_slot_boundaries(
    record: &ScheduledInstanceRecord,
    now_secs: i64,
) -> (Option<i64>, Option<i64>) {
    let Some(term_start) = ::chrono::DateTime::from_timestamp(record.spec.term_start_secs, 0)
        .map(|at| at.naive_utc())
    else {
        return (None, None);
    };
    let recurrence = &record.spec.recurrence;
    let frequency = recurrence
        .frequency
        .as_deref()
        .unwrap_or(SCHEDULED_INSTANCE_FREQUENCY_DAILY);
    let interval = usize::try_from(recurrence.interval.unwrap_or(1).max(1)).unwrap_or(1);
    let relative_to_end = recurrence.occurrence_relative_to_end.unwrap_or(false);
    let mut days: Vec<i32> = recurrence.occurrence_day_set.clone().unwrap_or_default();
    days.sort_unstable();
    days.dedup();
    // A daily schedule comes round every day, so it names no day at all whatever a request
    // carried beside it ("you can't specify this value with a daily schedule").
    if frequency == SCHEDULED_INSTANCE_FREQUENCY_DAILY {
        days.clear();
    }

    let slot_seconds = i64::from(record.spec.slot_duration_in_hours.max(0)) * SECONDS_IN_HOUR;
    let ended_at = |start: Option<i64>| start.map(|start| start.saturating_add(slot_seconds));
    let mut previous_start: Option<i64> = None;
    // "The interval quantity. The interval unit depends on the value of `frequency`": every
    // `interval`th period of the term holds the schedule's slots, and the ones between hold
    // none.
    for period in (0..=scheduled_instance_periods_per_term(frequency)).step_by(interval.max(1)) {
        for slot in scheduled_instance_period_slots(
            frequency,
            term_start,
            period,
            &days,
            relative_to_end,
        ) {
            let start = slot.and_utc().timestamp();
            if start < record.spec.term_start_secs {
                continue;
            }
            if start > record.term_end_secs {
                return (None, ended_at(previous_start));
            }
            if start > now_secs {
                return (Some(start), ended_at(previous_start));
            }
            previous_start = Some(start);
        }
    }
    (None, ended_at(previous_start))
}

/// `record` as `DescribeScheduledInstances` reports it at `now_secs`: the modelled
/// [`ScheduledInstance`] shape, with the terms read off the stored purchase and the two
/// timeline members derived from its recurrence ([`scheduled_instance_slot_boundaries`]).
///
/// The ONE projection of a stored purchase, so every report of one — a describe's, and the
/// purchase's own answer — says the same thing about it.
pub fn scheduled_instance_view(record: &ScheduledInstanceRecord, now_secs: i64) -> ScheduledInstance {
    let (next_slot_start_secs, previous_slot_end_secs) =
        scheduled_instance_slot_boundaries(record, now_secs);
    let at = ::aws_smithy_types::DateTime::from_secs;
    ScheduledInstance {
        availability_zone: Some(record.spec.availability_zone.clone()),
        create_date: Some(at(record.spec.purchased_at_secs)),
        hourly_price: Some(record.spec.hourly_price.clone()),
        instance_count: Some(record.spec.instance_count),
        instance_type: Some(record.spec.instance_type.clone()),
        network_platform: Some(record.spec.network_platform.clone()),
        next_slot_start_time: next_slot_start_secs.map(at),
        platform: Some(record.spec.platform.clone()),
        previous_slot_end_time: previous_slot_end_secs.map(at),
        recurrence: Some(record.spec.recurrence.clone()),
        scheduled_instance_id: Some(record.scheduled_instance_id.clone()),
        slot_duration_in_hours: Some(record.spec.slot_duration_in_hours),
        term_end_date: Some(at(record.term_end_secs)),
        term_start_date: Some(at(record.spec.term_start_secs)),
        total_scheduled_instance_hours: Some(record.total_scheduled_instance_hours),
    }
}
