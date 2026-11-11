// SPDX-License-Identifier: CC0-1.0

//! The maintenance windows this service holds: "a maintenance window […] defines a
//! schedule for performing potentially disruptive actions on your nodes"
//! (`smithy_query(query="doc",shape="CreateMaintenanceWindow")`).
//!
//! `CreateMaintenanceWindow` mints one and `GetMaintenanceWindow` reports it, both
//! through the one store below
//! ([`super::ComAmazonawsSsmAmazonssmState::maintenance_windows`]), so whatever the
//! create persisted is what a later read reports.
//!
//! A window is reached through the endpoint of ONE Region — Systems Manager is a regional
//! service — so the store is keyed by `(region, window id)`, the id being the one the
//! create minted ([`mint_window_id`]). An id nothing created names no window, which is
//! what the modelled `DoesNotExistException` reports
//! ([`maintenance_window_not_found_message`]).
//!
//! What a window WILL do is read from its schedule rather than stored: "The schedule of
//! the maintenance window in the form of a cron or rate expression"
//! (`CreateMaintenanceWindow$Schedule`) names the instants the window opens at, and
//! `NextExecutionTime` is "the next time the maintenance window will actually run, taking
//! into account any specified times for the maintenance window to become active or
//! inactive" (`MaintenanceWindowIdentity$NextExecutionTime`) — so it is COMPUTED from the
//! stored schedule, the stored activation window and the clock ([`WindowSchedule`]), and
//! moves on its own as the clock does.
//!
//! The grammars are read by maintained parsers rather than by a hand-written one: the
//! `cron` crate for a `cron(...)` body normalised by [`normalised_cron`], `chrono` for an
//! `at(...)` instant and for the ISO-8601 instants that bound a window, and `chrono-tz`
//! for the IANA zone a window states its schedule in. A body carrying one of the
//! wildcards this service's own cron dialect documents and that crate's grammar does not
//! read (`L`, `W`, `#`) is accepted by the write path and names no instant here, so such
//! a window is held as asked for rather than opened at an instant guessed for it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime};

use ::chrono::{Offset, TimeZone};

use super::ComAmazonawsSsmAmazonssmState;

/// What every maintenance window id starts with, from the modelled `MaintenanceWindowId`
/// pattern (`^mw-[0-9a-f]{17}$`).
pub const WINDOW_ID_PREFIX: &str = "mw-";

/// How many lowercase hex digits a window id carries after [`WINDOW_ID_PREFIX`], from the
/// same modelled pattern — which is also what makes an id exactly the modelled length
/// (`len 20..20`).
pub const WINDOW_ID_HEX_DIGITS: usize = 17;

/// The `ResourceTypeForTagging` value whose id is a maintenance window's minted id:
/// "MaintenanceWindow: mw-012345abcde" (`AddTagsToResourceRequest$ResourceId`), from the
/// modelled enum (`crate::ops::types::RESOURCE_TYPE_FOR_TAGGING_VALUES`).
pub const TAGGING_TYPE_MAINTENANCE_WINDOW: &str = "MaintenanceWindow";

/// Whether a window a create has just minted is enabled.
///
/// `CreateMaintenanceWindow` carries no `Enabled` member at all while
/// `UpdateMaintenanceWindow` does ("Whether the maintenance window is enabled"), so a
/// created window is enabled and only an update can disable it.
pub const CREATED_ENABLED: bool = true;

/// How many maintenance windows this store holds at once, across every Region.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes: a
/// create is the only thing that admits a window, callers may run as many as they like,
/// and the store is held in memory for as long as the process runs — so without a ceiling
/// the one map would grow for as long as creates keep arriving. A create past it is
/// refused through the modelled `ResourceLimitExceededException` and writes nothing.
pub const MAX_MAINTENANCE_WINDOWS: usize = 100;

/// What a create refused by [`MAX_MAINTENANCE_WINDOWS`] reports: the emulator's own
/// ceiling, named as the emulator's, with the figure it stands at.
#[must_use]
pub fn maintenance_window_ceiling_message() -> String {
    format!(
        "This emulator holds at most {MAX_MAINTENANCE_WINDOWS} maintenance windows across \
         every Region"
    )
}

/// How this service reports a `WindowId` that names no maintenance window reachable
/// through this endpoint, through `DoesNotExistException`.
///
/// The error's own account of its class is "the specified resource, such as a maintenance
/// window or patch baseline, doesn't exist" (`smithy_query(query="doc",
/// shape="DoesNotExistException")`), so the sentence names the window the request asked
/// for and nothing else of it.
#[must_use]
pub fn maintenance_window_not_found_message(window_id: &str) -> String {
    format!("Maintenance window {window_id} does not exist")
}

/// The units a `rate(value unit)` schedule may name, and what each is worth in seconds,
/// from the rate-expression table of the service's cron-and-rate reference: "unit: The
/// unit of time […] minute | minutes | hour | hours | day | days". Both spellings of each
/// unit are accepted, as the service accepts them.
const RATE_UNITS: &[(&str, u64)] = &[
    ("minute", 60),
    ("minutes", 60),
    ("hour", 3_600),
    ("hours", 3_600),
    ("day", 86_400),
    ("days", 86_400),
];

/// How many whitespace-separated fields a `cron(...)` body carries, from the same
/// reference's cron table: Minutes, Hours, Day-of-month, Month, Day-of-week, Year — six.
const CRON_FIELDS: usize = 6;

/// The index of the day-of-month field among the six a `cron(...)` body carries.
const DAY_OF_MONTH_FIELD: usize = 2;

/// The index of the day-of-week field among those six.
const DAY_OF_WEEK_FIELD: usize = 4;

/// The indexes of the two fields a `?` ("no specific value") is a statement in —
/// day-of-month and day-of-week — which is the same statement `*` makes in the grammar the
/// `cron` crate reads.
const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [DAY_OF_MONTH_FIELD, DAY_OF_WEEK_FIELD];

/// How many times one weekday can fall in a month, which is what the `<day-of-week>#<nth>`
/// form may ask for: a 31-day month holds five of one weekday at most.
const MAX_WEEKDAYS_IN_MONTH: u8 = 5;

/// The field values the normalised seven-field expression carries for every field but the
/// one under test, so a field is judged on its own.
const NEUTRAL_CRON_FIELDS: [&str; CRON_FIELDS + 1] = ["0", "0", "0", "*", "*", "*", "*"];

/// The format an `at(...)` body states its instant in: `at(yyyy-mm-ddThh:mm:ss)`, a local
/// date and time with no zone offset, since the zone is `ScheduleTimezone`'s to state.
const AT_EXPRESSION_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// The form a window's `NextExecutionTime` is reported in: ISO-8601 Extended to the
/// MINUTE, which is the resolution Systems Manager states the instants a window opens at
/// — a window's schedule names a minute of the hour at its finest (the cron grammar's
/// first field, and the smallest unit a `rate(...)` may state), so there is no finer
/// instant for one to report.
const NEXT_EXECUTION_UTC_FORMAT: &str = "%Y-%m-%dT%H:%MZ";

/// The same form for an instant read in a zone that is not at UTC, whose offset it carries
/// in place of the `Z` designator.
const NEXT_EXECUTION_OFFSET_FORMAT: &str = "%Y-%m-%dT%H:%M%:z";

/// The civil date-and-time form a window's `StartDate`/`EndDate` may state without a zone,
/// the zone then being the window's own: both are documented "in ISO-8601 Extended format"
/// (`CreateMaintenanceWindow$StartDate`).
const CIVIL_DATE_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// The zone a schedule is read in when the window names none: the two instants that bound
/// a window and the schedule itself are then read in UTC, which is the zone an ISO-8601
/// instant with no offset of its own is taken at.
const DEFAULT_ZONE: ::chrono_tz::Tz = ::chrono_tz::Tz::UTC;

/// A day, which is the unit `ScheduleOffset` states its wait in: "The number of days to
/// wait after the date and time specified by a cron expression before running the
/// maintenance window" (`CreateMaintenanceWindow$ScheduleOffset`).
const ONE_DAY: Duration = Duration::from_secs(86_400);

/// How this service states a `Schedule` that is not a schedule at all.
///
/// The one sentence Systems Manager words this error class with, declared once for the whole
/// crate beside the association schedules that first stated it
/// (`super::associations::INVALID_SCHEDULE`, quoting the modelled `InvalidSchedule`: "The
/// schedule is invalid. Verify your cron or rate expression and try again"). It is fixed
/// text for the class and interpolates nothing from the request, so every refusal of a cron
/// or rate expression this service cannot run on is worded the same way.
pub const INVALID_SCHEDULE: &str = super::associations::INVALID_SCHEDULE;

/// How this service states a `ScheduleTimezone` no IANA zone answers to, with the value
/// the request carried.
#[must_use]
pub fn invalid_schedule_timezone_message(timezone: &str) -> String {
    format!("Invalid schedule timezone {timezone}.")
}

/// Refuse a `Schedule` that states none of the forms this service documents, or states one
/// of them malformed; `None` for an expression the service can read.
///
/// The length bound (1..256) is modelled, so the boundary walk has already refused
/// anything outside it by the time a handler runs. Both operations that WRITE a window's
/// schedule are held to this one rule, so a schedule one accepts is one the other does.
#[must_use]
pub fn schedule_violation(schedule: &str) -> Option<&'static str> {
    let readable = match form_of(schedule) {
        Some((ScheduleForm::At, body)) => at_expression_states_an_instant(body),
        Some((ScheduleForm::Rate, body)) => rate_period(body).is_some(),
        Some((ScheduleForm::Cron, body)) => cron_expression_states_its_instants(body),
        None => false,
    };
    (!readable).then_some(INVALID_SCHEDULE)
}

/// Refuse a `ScheduleTimezone` that names no time zone; `None` for an absent member (the
/// schedule is then read in UTC) or a zone the IANA database carries.
///
/// "The time zone that the scheduled maintenance window executions are based on, in
/// Internet Assigned Numbers Authority (IANA) format"
/// (`CreateMaintenanceWindow$ScheduleTimezone`): a value no zone answers to leaves the
/// instants the window opens at undefined, so it is not accepted.
#[must_use]
pub fn schedule_timezone_violation(timezone: Option<&str>) -> Option<String> {
    let timezone = timezone.filter(|zone| !zone.is_empty())?;
    timezone
        .parse::<::chrono_tz::Tz>()
        .err()
        .map(|_| invalid_schedule_timezone_message(timezone))
}

/// `at` instant, `rate` period or `cron` fields: the forms a maintenance window's schedule
/// may state, keyed by the keyword that introduces each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduleForm {
    At,
    Rate,
    Cron,
}

/// The form `schedule` states and the body it carries, or `None` when it states no form
/// this service defines.
fn form_of(schedule: &str) -> Option<(ScheduleForm, &str)> {
    for (keyword, form) in [
        ("at", ScheduleForm::At),
        ("rate", ScheduleForm::Rate),
        ("cron", ScheduleForm::Cron),
    ] {
        if let Some(body) = schedule
            .trim()
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return Some((form, body));
        }
    }
    None
}

/// Whether an `at(...)` body states an instant in the documented format.
fn at_expression_states_an_instant(body: &str) -> bool {
    ::chrono::NaiveDateTime::parse_from_str(body.trim(), AT_EXPRESSION_FORMAT).is_ok()
}

/// The period a `rate(...)` body states, or `None` when it states none: a positive whole
/// number of a unit the service names ([`RATE_UNITS`]), and nothing else.
///
/// A figure so large that the first occurrence of the series falls off the calendar this
/// module places instants on states no period either ([`is_schedulable`]): such a series has
/// no date to open at, so it is refused with the rest of the unreadable schedules rather
/// than stored as a schedule nothing could ever report.
fn rate_period(body: &str) -> Option<Duration> {
    let mut fields = body.split_whitespace();
    let (Some(value), Some(unit), None) = (fields.next(), fields.next(), fields.next()) else {
        return None;
    };
    let value: u64 = value.parse().ok()?;
    if value == 0 {
        return None;
    }
    let unit_seconds = RATE_UNITS
        .iter()
        .find(|(name, _)| *name == unit)
        .map(|(_, seconds)| *seconds)?;
    // A figure of units no span can represent states no period at all: it is refused
    // rather than saturated into one the series could never move past.
    let seconds = value.checked_mul(unit_seconds)?;
    is_schedulable(seconds).then(|| Duration::from_secs(seconds))
}

/// Whether a span of `seconds` from the epoch lands on an instant this module can place on
/// the calendar at all — the range the date library represents, rather than the range a
/// `SystemTime` happens to hold.
///
/// A rate schedule's period is the one caller-supplied figure that can carry an occurrence
/// past that range, so it is taken against this before it is stored.
fn is_schedulable(seconds: u64) -> bool {
    ::std::time::UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .and_then(calendar_instant)
        .is_some()
}

/// Whether a `cron(...)` body states the instants it has to: six fields, each one the cron
/// grammar reads in the place it stands.
///
/// The `cron` crate reads the seven-field form (seconds first), so a literal `0` seconds
/// field is prepended, and a `?` in the day-of-month or day-of-week field is the `*` that
/// grammar spells the same statement with. A field stating one of this service's own
/// dialect forms ([`names_service_wildcard`]) is a statement that grammar does not read at
/// all, so it is accepted on the service's grammar instead of being refused on the
/// parser's.
fn cron_expression_states_its_instants(body: &str) -> bool {
    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return false;
    }
    let service_dialect = fields
        .iter()
        .enumerate()
        .any(|(index, field)| names_service_wildcard(index, field));
    for (index, field) in fields.iter().enumerate() {
        if !cron_field_parses(index, field) && !names_service_wildcard(index, field) {
            return false;
        }
    }
    // Every field reads on its own; the whole expression is read once more so a
    // combination of fields the grammar refuses is refused too — except for a body in this
    // service's own dialect, which that grammar cannot read as a whole either.
    service_dialect || normalised_cron(&fields).parse::<::cron::Schedule>().is_ok()
}

/// Whether `field`, standing at `index` among the six a `cron(...)` body carries, states one
/// of the forms this service's own cron dialect adds to the grammar the `cron` crate reads.
///
/// The forms, from the wildcards table of the service's cron-and-rate reference, each only
/// in the field it is documented for: `L` (the last day) in day-of-month and in day-of-week,
/// `<day>W` (the nearest weekday to a day of the month) in day-of-month, and
/// `<day-of-week>#<nth>` (the nth weekday of the month) in day-of-week. A dialect token
/// anywhere else states nothing the service documents, so it is left to that grammar — which
/// refuses it.
///
/// Matching the form is what keeps an ordinary token out of this: `WED` ends in no `W` of
/// its own and `JUL` is not the `L` wildcard, so both are still read — and refused — by that
/// grammar rather than waved through as dialect. The day a form names is read by that same
/// grammar in the field it stands in, so a day of the month or of the week that does not
/// exist is no dialect form either.
fn names_service_wildcard(index: usize, field: &str) -> bool {
    // The last day of the month or of the week, on its own or as the last weekday of the
    // month (`LW`).
    if field == "L" || field == "LW" {
        return NO_SPECIFIC_VALUE_FIELDS.contains(&index);
    }
    // The nearest weekday to a day of the month: the day, then `W`.
    if let Some(day) = field.strip_suffix('W') {
        return index == DAY_OF_MONTH_FIELD && cron_field_parses(DAY_OF_MONTH_FIELD, day);
    }
    // The nth weekday of the month: the day of the week, then `#`, then which one.
    if let Some((day_of_week, nth)) = field.split_once('#') {
        return index == DAY_OF_WEEK_FIELD
            && cron_field_parses(DAY_OF_WEEK_FIELD, day_of_week)
            && nth
                .parse::<u8>()
                .is_ok_and(|nth| (1..=MAX_WEEKDAYS_IN_MONTH).contains(&nth));
    }
    false
}

/// The six documented fields as the seven-field expression the `cron` crate reads.
fn normalised_cron(fields: &[&str]) -> String {
    let mut expression = String::from(NEUTRAL_CRON_FIELDS[0]);
    for (index, field) in fields.iter().enumerate() {
        expression.push(' ');
        expression.push_str(cron_field_as_parsed(index, field));
    }
    expression
}

/// One field as the `cron` crate's grammar reads it: `?` ("no specific value") is `*` in
/// that grammar, in the two fields it is a statement in.
fn cron_field_as_parsed(index: usize, field: &str) -> &str {
    if field == "?" && NO_SPECIFIC_VALUE_FIELDS.contains(&index) {
        "*"
    } else {
        field
    }
}

/// Whether `field` is one the cron grammar reads in the place `index` names, judged by
/// parsing it there with every other field neutral — so the answer is about that field and
/// not about the rest of the expression.
fn cron_field_parses(index: usize, field: &str) -> bool {
    let mut expression = NEUTRAL_CRON_FIELDS;
    expression[index + 1] = cron_field_as_parsed(index, field);
    expression.join(" ").parse::<::cron::Schedule>().is_ok()
}

/// The zone `timezone` names, or the default one when it names none — which the write path
/// has already refused anything unreadable for
/// ([`schedule_timezone_violation`]).
fn zone_of(timezone: Option<&str>) -> ::chrono_tz::Tz {
    timezone
        .filter(|zone| !zone.is_empty())
        .and_then(|zone| zone.parse().ok())
        .unwrap_or(DEFAULT_ZONE)
}

/// The instants one window's schedule names, read in the zone the window states them in.
#[derive(Debug, Clone)]
pub enum WindowSchedule {
    /// `at(<instant>)`: the ONE instant that expression names, already resolved against
    /// the window's zone.
    Once(SystemTime),
    /// `rate(<value> <unit>)`: one occurrence every period, the series anchored at the
    /// instant the window was created, since a rate expression states the INTERVAL and no
    /// first instant of its own.
    Rate(Duration),
    /// `cron(<six fields>)`: an occurrence at each instant the expression names, read in
    /// the window's own zone.
    Cron(Box<::cron::Schedule>, ::chrono_tz::Tz),
}

impl WindowSchedule {
    /// The occurrences `schedule` names, read in `timezone`, or `None` when it names none
    /// this module can compute — text stating no documented form, or a `cron(...)` body in
    /// this service's own dialect.
    #[must_use]
    pub fn parse(schedule: &str, timezone: Option<&str>) -> Option<Self> {
        let zone = zone_of(timezone);
        match form_of(schedule)? {
            (ScheduleForm::At, body) => at_instant(body, zone).map(Self::Once),
            (ScheduleForm::Rate, body) => rate_period(body).map(Self::Rate),
            (ScheduleForm::Cron, body) => {
                let fields: Vec<&str> = body.split_whitespace().collect();
                if fields.len() != CRON_FIELDS {
                    return None;
                }
                normalised_cron(&fields)
                    .parse()
                    .ok()
                    .map(|parsed| Self::Cron(Box::new(parsed), zone))
            }
        }
    }

    /// The first occurrence strictly AFTER `after`, or `None` when this schedule names no
    /// instant beyond it — a one-time window whose instant has passed, or a cron
    /// expression whose year field is behind the clock.
    ///
    /// `anchor` is where a rate series is measured from (the instant the window was
    /// created), so the occurrences of one window's rate schedule are the same series
    /// whenever it is asked for rather than a period on from each caller.
    #[must_use]
    pub fn next_after(&self, after: SystemTime, anchor: SystemTime) -> Option<SystemTime> {
        match self {
            Self::Once(at) => (*at > after).then_some(*at),
            Self::Rate(period) => {
                // Computed rather than stepped: the search start is caller-influenced (a
                // window's activation date may be far ahead of the anchor), so the
                // occurrence is found by arithmetic on the period and never by a loop
                // whose length a request could choose.
                let period_seconds = period.as_secs().max(1);
                let elapsed = after
                    .duration_since(anchor)
                    .map(|since| since.as_secs())
                    .unwrap_or_default();
                let periods = elapsed
                    .checked_div(period_seconds)?
                    .checked_add(1)?
                    .checked_mul(period_seconds)?;
                anchor.checked_add(Duration::from_secs(periods))
            }
            Self::Cron(schedule, zone) => {
                let from = calendar_instant(after)?.with_timezone(zone);
                schedule
                    .after(&from)
                    .next()
                    .map(|instant| SystemTime::from(instant.with_timezone(&::chrono::Utc)))
            }
        }
    }

    /// Whether this schedule is one a `ScheduleOffset` applies to: the offset is "the
    /// number of days to wait after the date and time specified by a cron expression"
    /// (`CreateMaintenanceWindow$ScheduleOffset`), so it moves the occurrences of a cron
    /// schedule and of nothing else.
    #[must_use]
    pub fn takes_schedule_offset(&self) -> bool {
        matches!(self, Self::Cron(_, _))
    }
}

/// One clock reading as a calendar instant, or `None` when it falls outside the range the
/// date library represents.
///
/// Every reading of a `SystemTime` as a date goes through here rather than through the
/// library's own infallible conversion, which resolves the instant with an `unwrap`: a
/// `SystemTime` holds spans no calendar date exists for, and a schedule's period is
/// caller-supplied, so an instant past the calendar has to answer "no date" instead of
/// ending the process.
fn calendar_instant(at: SystemTime) -> Option<::chrono::DateTime<::chrono::Utc>> {
    let (seconds, nanos) = match at.duration_since(::std::time::UNIX_EPOCH) {
        Ok(since) => (i64::try_from(since.as_secs()).ok()?, since.subsec_nanos()),
        // Before the epoch: the span back to it, as a negative second count.
        Err(earlier) => (
            i64::try_from(earlier.duration().as_secs())
                .ok()?
                .checked_neg()?,
            0,
        ),
    };
    ::chrono::Utc.timestamp_opt(seconds, nanos).single()
}

/// The instant an `at(...)` body names, read in `zone`.
///
/// A local time a zone's offset change leaves AMBIGUOUS (a clock that goes back over it)
/// resolves to the EARLIER of its two instants, and one that zone skips entirely names no
/// instant at all — the reading `chrono` gives a local time, so nothing here invents an
/// offset for either case.
fn at_instant(body: &str, zone: ::chrono_tz::Tz) -> Option<SystemTime> {
    let naive = ::chrono::NaiveDateTime::parse_from_str(body.trim(), AT_EXPRESSION_FORMAT).ok()?;
    let local = zone.from_local_datetime(&naive).earliest()?;
    Some(SystemTime::from(local.with_timezone(&::chrono::Utc)))
}

/// The instant an ISO-8601 Extended `MaintenanceWindowStringDateTime` names, read in
/// `zone` when the text carries no offset of its own, or `None` for text that names none.
///
/// `StartDate` and `EndDate` are modelled as plain STRINGS, so the framework parses
/// neither and the stored text is echoed back exactly as the request stated it; this is
/// only how the stored text is read when the next execution has to be placed inside the
/// window the two of them bound.
fn window_instant(value: &str, zone: ::chrono_tz::Tz) -> Option<SystemTime> {
    let value = value.trim();
    if let Ok(fixed) = ::chrono::DateTime::parse_from_rfc3339(value) {
        return Some(SystemTime::from(fixed.with_timezone(&::chrono::Utc)));
    }
    let naive = ::chrono::NaiveDateTime::parse_from_str(value, CIVIL_DATE_TIME_FORMAT).ok()?;
    let local = zone.from_local_datetime(&naive).earliest()?;
    Some(SystemTime::from(local.with_timezone(&::chrono::Utc)))
}

/// One instant as a `MaintenanceWindowStringDateTime`: ISO-8601 Extended to the minute
/// ([`NEXT_EXECUTION_UTC_FORMAT`]), in `zone` — so a window whose schedule is read in a
/// zone reports the instants it opens at in that same zone, and one read at UTC reports
/// them with the `Z` designator rather than with a zero offset.
///
/// `None` for an instant outside the calendar ([`calendar_instant`]): such an instant has no
/// ISO-8601 rendering, so nothing is reported for it.
fn rendered_instant(at: SystemTime, zone: ::chrono_tz::Tz) -> Option<String> {
    let local = calendar_instant(at)?.with_timezone(&zone);
    let format = if local.offset().fix().local_minus_utc() == 0 {
        NEXT_EXECUTION_UTC_FORMAT
    } else {
        NEXT_EXECUTION_OFFSET_FORMAT
    };
    Some(local.format(format).to_string())
}

/// One maintenance window the account created, as the store holds it.
///
/// Every member a read of a window reports is here or derived from here, so a read
/// projects this record rather than inventing anything: what the create persisted is what
/// a later read reports.
#[derive(Debug, Clone)]
pub struct MaintenanceWindowRecord {
    /// The server-minted id this record is stored under ([`mint_window_id`]).
    pub window_id: String,
    /// The Region whose endpoint this window was created through.
    pub region: String,
    /// "The name of the maintenance window" — required on create, so a window always has
    /// one.
    pub name: String,
    /// "An optional description for the maintenance window", or `None` for a create that
    /// carried none.
    pub description: Option<String>,
    /// "The date and time, in ISO-8601 Extended format, for when you want the maintenance
    /// window to become active", held exactly as the request stated it.
    pub start_date: Option<String>,
    /// "The date and time, in ISO-8601 Extended format, for when you want the maintenance
    /// window to become inactive", held exactly as the request stated it.
    pub end_date: Option<String>,
    /// "The schedule of the maintenance window in the form of a cron or rate expression" —
    /// required on create.
    pub schedule: String,
    /// "The time zone that the scheduled maintenance window executions are based on, in
    /// Internet Assigned Numbers Authority (IANA) format", or `None` when the window
    /// states none and its schedule is read in UTC.
    pub schedule_timezone: Option<String>,
    /// "The number of days to wait after the date and time specified by a cron expression
    /// before running the maintenance window", or `None` for a window that waits none.
    pub schedule_offset: Option<i32>,
    /// "The duration of the maintenance window in hours" — required on create.
    pub duration: i32,
    /// "The number of hours before the end of the maintenance window that […] Systems
    /// Manager stops scheduling new tasks for execution" — required on create, defaulted
    /// to 0 by the model.
    pub cutoff: i32,
    /// "Enables a maintenance window task to run on managed nodes, even if you haven't
    /// registered those nodes as targets" — required on create, defaulted to false by the
    /// model.
    pub allow_unassociated_targets: bool,
    /// "Indicates whether the maintenance window is enabled"
    /// ([`CREATED_ENABLED`] until an update says otherwise).
    pub enabled: bool,
    /// The "user-provided idempotency token" the create carried, or `None` when it carried
    /// none: what makes a retry of that create report the window it already minted rather
    /// than minting a second one.
    pub client_token: Option<String>,
    /// The window's tags, keyed by tag key, as the create supplied them and as the tag
    /// operations rewrite them (`super::tagging`).
    pub tags: BTreeMap<String, String>,
    /// When the create ran, which is also where a `rate(...)` schedule's series is
    /// anchored.
    pub created_date: SystemTime,
    /// When the window was last written. Equal to [`created_date`](Self::created_date)
    /// until something updates the window.
    pub modified_date: SystemTime,
}

impl MaintenanceWindowRecord {
    /// The zone this window's schedule and bounding instants are read in.
    fn zone(&self) -> ::chrono_tz::Tz {
        zone_of(self.schedule_timezone.as_deref())
    }

    /// The instants this window's stored schedule names, or `None` when the stored
    /// schedule names none this emulator can place (this service's own cron dialect).
    #[must_use]
    pub fn schedule_occurrences(&self) -> Option<WindowSchedule> {
        WindowSchedule::parse(&self.schedule, self.schedule_timezone.as_deref())
    }

    /// "The next time the maintenance window will actually run, taking into account any
    /// specified times for the maintenance window to become active or inactive"
    /// (`MaintenanceWindowIdentity$NextExecutionTime`), or `None` when the window will not
    /// run again.
    ///
    /// Computed from the stored schedule and the clock, so it moves on its own as the
    /// clock does rather than being frozen at the instant of a write. A window will not
    /// run again when it is disabled ("whether the maintenance window is enabled"), when
    /// its schedule names no further instant, when that instant falls outside the interval
    /// its `StartDate` and `EndDate` bound, or when a bound it states names no instant this
    /// reader can place — and a window whose activation is still ahead first runs at the
    /// first instant after it becomes active.
    #[must_use]
    pub fn next_execution_time(&self, now: SystemTime) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let zone = self.zone();
        let occurrences = self.schedule_occurrences()?;
        // "for when you want the maintenance window to become active […] allows you to
        // delay activation of the maintenance window until the specified future date": the
        // search starts at the later of the clock and that activation. A bound the window
        // STATES but that names no instant this reader can place leaves the interval the
        // window would run in unknown, so nothing is reported for it rather than an instant
        // computed as though the window carried no bound at all.
        let mut after = now;
        if let Some(stated) = self.stated_start_date() {
            after = after.max(window_instant(stated, zone)?);
        }
        let mut at = occurrences.next_after(after, self.created_date)?;
        // The offset waits whole days after the instant the cron expression named.
        if occurrences.takes_schedule_offset() {
            if let Some(days) = self.schedule_offset.filter(|days| *days > 0) {
                at = at.checked_add(ONE_DAY * u32::try_from(days).ok()?)?;
            }
        }
        // "for when you want the maintenance window to become inactive": the window runs
        // no more once that instant has passed.
        if let Some(stated) = self.stated_end_date() {
            if at > window_instant(stated, zone)? {
                return None;
            }
        }
        rendered_instant(at, zone)
    }

    /// The activation instant this window STATES, or `None` when it states none — a member
    /// sent EMPTY names nothing, the way this crate reads every optional string off the wire
    /// (`super::non_empty`).
    fn stated_start_date(&self) -> Option<&str> {
        self.start_date
            .as_deref()
            .map(str::trim)
            .filter(|stated| !stated.is_empty())
    }

    /// The inactivation instant this window states, read the same way.
    fn stated_end_date(&self) -> Option<&str> {
        self.end_date
            .as_deref()
            .map(str::trim)
            .filter(|stated| !stated.is_empty())
    }

    /// The members a create SUPPLIED, rendered as one canonical string.
    ///
    /// This is what "the parameters don't match the original call to the API with the same
    /// idempotency token" (`IdempotentParameterMismatch`) is decided on: two creates
    /// carrying one idempotency token are the same call when their supplied members render
    /// the same way. The minted id, the Region, the token itself and the instants are not
    /// supplied members, so they are not in it.
    #[must_use]
    pub fn creation_fingerprint(&self) -> String {
        let mut rendered = String::new();
        let _ = write!(
            rendered,
            "name={}\ndescription={}\nstart={}\nend={}\nschedule={}\ntimezone={}\n\
             offset={}\nduration={}\ncutoff={}\nunassociated={}\n",
            self.name,
            self.description.as_deref().unwrap_or_default(),
            self.start_date.as_deref().unwrap_or_default(),
            self.end_date.as_deref().unwrap_or_default(),
            self.schedule,
            self.schedule_timezone.as_deref().unwrap_or_default(),
            self.schedule_offset.unwrap_or_default(),
            self.duration,
            self.cutoff,
            self.allow_unassociated_targets,
        );
        rendered.push_str("tags=");
        for (key, value) in &self.tags {
            let _ = write!(rendered, "[{key}={value}]");
        }
        rendered
    }
}

/// Mint the `WindowId` of a maintenance window being created: [`WINDOW_ID_PREFIX`]
/// followed by [`WINDOW_ID_HEX_DIGITS`] lowercase hex digits.
///
/// Through the crate's one minter of a prefixed hex identifier
/// ([`super::minted_prefixed_hex_id`]): the digits are the leading bytes of a digest of the
/// store's own mint counter and the instant of the create, so two windows of one store
/// never carry one id and an id carries nothing from the request that created it.
#[must_use]
pub fn mint_window_id(sequence: u64, at: SystemTime) -> String {
    super::minted_prefixed_hex_id(WINDOW_ID_PREFIX, WINDOW_ID_HEX_DIGITS, sequence, at)
}

impl ComAmazonawsSsmAmazonssmState {
    /// The maintenance window `window_id` names in `region`, or `None` when that Region
    /// holds none — which is what a not-found refusal reports.
    pub fn maintenance_window(
        &self,
        region: &str,
        window_id: &str,
    ) -> Option<&MaintenanceWindowRecord> {
        self.maintenance_windows
            .get(&(region.to_string(), window_id.to_string()))
    }

    /// The same window, to write it.
    pub fn maintenance_window_mut(
        &mut self,
        region: &str,
        window_id: &str,
    ) -> Option<&mut MaintenanceWindowRecord> {
        self.maintenance_windows
            .get_mut(&(region.to_string(), window_id.to_string()))
    }

    /// Store `record` as a maintenance window of its own Region, keyed by its id.
    pub fn insert_maintenance_window(&mut self, record: MaintenanceWindowRecord) {
        let key = (record.region.clone(), record.window_id.clone());
        self.maintenance_windows.insert(key, record);
    }

    /// Whether this store can take another maintenance window
    /// ([`MAX_MAINTENANCE_WINDOWS`]).
    pub fn can_hold_another_maintenance_window(&self) -> bool {
        self.maintenance_windows.len() < MAX_MAINTENANCE_WINDOWS
    }

    /// Mint the id of a maintenance window being created in this store.
    pub fn next_maintenance_window_id(&mut self, at: SystemTime) -> String {
        self.maintenance_window_sequence += 1;
        mint_window_id(self.maintenance_window_sequence, at)
    }

    /// The window `region` holds under the idempotency token `client_token`, or `None`
    /// when no create carrying that token has run there.
    pub fn maintenance_window_with_client_token(
        &self,
        region: &str,
        client_token: &str,
    ) -> Option<&MaintenanceWindowRecord> {
        self.maintenance_windows
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, record)| record)
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }
}

#[cfg(test)]
mod tests {
    //! The rules this module owns, each over a SET of inputs rather than one recorded
    //! schedule. The same rules are driven through real requests in
    //! `tests/all/maintenance_windows.rs`.

    use super::*;

    /// One row of [`the_next_execution_is_the_schedules_next_instant`]: what a failing
    /// assertion calls it, the schedule, the zone it is read in, the days it waits, and
    /// the instant reported.
    type NextExecutionRow = (
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<i32>,
        &'static str,
    );

    /// One row of [`a_window_that_will_not_run_reports_no_next_execution`]: what a failing
    /// assertion calls it, and how that row's window differs from the base one.
    type WindowVariantRow = (&'static str, fn(&mut MaintenanceWindowRecord));

    /// One row of [`the_creation_fingerprint_covers_every_supplied_member`]: a
    /// [`WindowVariantRow`] beside whether the two creates are the same call.
    type FingerprintRow = (&'static str, fn(&mut MaintenanceWindowRecord), bool);

    /// An instant as a clock reading, which the rows below measure from.
    fn epoch(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// Every documented form of a schedule, well-formed and malformed, is read as the
    /// service documents it — and a refusal states the expression the request carried.
    #[test]
    fn a_schedule_states_one_of_the_documented_forms() {
        // (case, schedule, states a schedule)
        let rows: [(&str, &str, bool); 32] = [
            ("a rate of minutes", "rate(30 minutes)", true),
            ("a rate of one minute", "rate(1 minute)", true),
            ("a plural unit with one", "rate(1 minutes)", true),
            ("a singular unit with five", "rate(5 minute)", true),
            ("a rate of hours", "rate(12 hours)", true),
            ("a rate of days", "rate(7 days)", true),
            ("a rate of a thousand days", "rate(1000 days)", true),
            ("a rate of zero", "rate(0 minutes)", false),
            ("a rate with no unit", "rate(5)", false),
            ("a rate in a unit nothing measures", "rate(5 fortnights)", false),
            ("a fractional rate", "rate(1.5 hours)", false),
            // A period whose first occurrence falls off the calendar: no date exists for
            // such a series to open at, so it states no schedule.
            ("a rate past the calendar", "rate(99999999999 days)", false),
            ("a rate of the largest figure there is", "rate(18446744073709551615 minutes)", false),
            ("a six-field cron", "cron(0 16 ? * TUE *)", true),
            ("a cron naming weekdays", "cron(0 18 ? * MON-FRI *)", true),
            ("a cron over a step", "cron(0/15 * * * ? *)", true),
            // The wildcards this service's own dialect documents.
            ("a cron on the last day of the month", "cron(0 8 L * ? *)", true),
            ("a cron on the nearest weekday", "cron(0 8 15W * ? *)", true),
            ("a cron on the third Tuesday", "cron(30 23 ? * TUE#3 *)", true),
            // Ordinary tokens that merely CARRY a dialect character are still read by the
            // shared grammar: one it names is served and one it does not is refused.
            ("a cron naming one weekday by name", "cron(0 8 ? * WED *)", true),
            ("a cron naming a weekday that is not one", "cron(0 8 ? * WEDX *)", false),
            ("a cron naming a day of the month that is not one", "cron(0 8 32W * ? *)", false),
            ("a cron on a weekday of no such ordinal", "cron(0 8 ? * TUE#x *)", false),
            ("a cron of five fields", "cron(0 8 1 * ?)", false),
            ("a cron of prose", "cron(not a cron)", false),
            // A dialect form states something only in the field it is documented for, so one
            // standing anywhere else is read \u2014 and refused \u2014 by the shared grammar.
            ("the last day as a minute", "cron(L 8 ? * MON *)", false),
            ("the nearest weekday as an hour", "cron(0 15W ? * MON *)", false),
            ("the nth weekday as a month", "cron(0 8 ? TUE#3 ? *)", false),
            ("the last day as a day of the week", "cron(0 8 ? * L *)", true),
            ("an at instant", "at(2030-01-01T00:00:00)", true),
            ("an at instant carrying a zone", "at(2030-01-01T00:00:00Z)", false),
            ("no form at all", "every 30 minutes", false),
        ];

        for (case, schedule, states_a_schedule) in rows {
            let violation = schedule_violation(schedule);
            assert_eq!(
                violation.is_none(),
                states_a_schedule,
                "case {case:?}: expected states_a_schedule={states_a_schedule}, got \
                 {violation:?}"
            );
            if let Some(message) = violation {
                assert_eq!(
                    message, INVALID_SCHEDULE,
                    "case {case:?}: the refusal is worded as this error class is, got \
                     {message:?}"
                );
            }
        }
    }

    /// A schedule time zone is one the IANA database carries, and an absent member is no
    /// violation at all.
    #[test]
    fn a_schedule_timezone_names_an_iana_zone() {
        // (case, timezone, names a zone)
        let rows: [(&str, Option<&str>, bool); 7] = [
            ("no zone at all", None, true),
            ("UTC", Some("UTC"), true),
            ("an American zone", Some("America/Los_Angeles"), true),
            ("an Asian zone", Some("Asia/Seoul"), true),
            ("a zone nothing names", Some("Not/AZone"), false),
            ("an offset rather than a zone", Some("+01:00"), false),
            ("prose", Some("here"), false),
        ];

        for (case, timezone, names_a_zone) in rows {
            let violation = schedule_timezone_violation(timezone);
            assert_eq!(
                violation.is_none(),
                names_a_zone,
                "case {case:?}: expected names_a_zone={names_a_zone}, got {violation:?}"
            );
        }
    }

    /// One window, so the rows below differ only along the member each is about.
    fn window(schedule: &str) -> MaintenanceWindowRecord {
        MaintenanceWindowRecord {
            window_id: mint_window_id(1, epoch(1_893_456_000)),
            region: "us-east-1".to_string(),
            name: "a-window".to_string(),
            description: None,
            start_date: None,
            end_date: None,
            schedule: schedule.to_string(),
            schedule_timezone: None,
            schedule_offset: None,
            duration: 2,
            cutoff: 1,
            allow_unassociated_targets: false,
            enabled: CREATED_ENABLED,
            client_token: None,
            tags: BTreeMap::new(),
            // 2030-01-01T00:00:00Z, a Tuesday.
            created_date: epoch(1_893_456_000),
            modified_date: epoch(1_893_456_000),
        }
    }

    /// The next execution is the first instant the stored schedule names after the clock,
    /// read in the window's own zone; the rows differ along the form of the schedule, the
    /// zone it is read in and the offset it waits.
    #[test]
    fn the_next_execution_is_the_schedules_next_instant() {
        // 2030-01-01T00:00:00Z, a Tuesday.
        let now = epoch(1_893_456_000);
        // (case, schedule, timezone, offset, the instant reported)
        let rows: [NextExecutionRow; 7] = [
            ("a cron at an hour of the day", "cron(0 8 * * ? *)", None, None, "2030-01-01T08:00Z"),
            (
                "the same cron in a western zone",
                "cron(0 8 * * ? *)",
                Some("America/Los_Angeles"),
                None,
                "2030-01-01T08:00-08:00",
            ),
            (
                "a cron on one day of the week",
                "cron(0 0 ? * WED *)",
                None,
                None,
                "2030-01-02T00:00Z",
            ),
            (
                "the same cron waiting two days",
                "cron(0 0 ? * WED *)",
                None,
                Some(2),
                "2030-01-04T00:00Z",
            ),
            ("a rate of hours", "rate(6 hours)", None, None, "2030-01-01T06:00Z"),
            // An offset states a wait after a CRON instant, so a rate schedule keeps its
            // own series.
            ("a rate carrying an offset", "rate(6 hours)", None, Some(3), "2030-01-01T06:00Z"),
            (
                "a one-time instant still ahead",
                "at(2030-06-01T12:00:00)",
                None,
                None,
                "2030-06-01T12:00Z",
            ),
        ];

        for (case, schedule, timezone, offset, expected) in rows {
            let mut held = window(schedule);
            held.schedule_timezone = timezone.map(str::to_string);
            held.schedule_offset = offset;
            let next = held.next_execution_time(now);
            assert_eq!(
                next.as_deref(),
                Some(expected),
                "case {case:?}: expected the next execution at {expected:?}, got {next:?}"
            );
        }
    }

    /// A window that will not run again reports no next execution: the rows differ along
    /// the reason it will not.
    #[test]
    fn a_window_that_will_not_run_reports_no_next_execution() {
        let now = epoch(1_893_456_000);
        // (case, how the window differs)
        let rows: [WindowVariantRow; 7] = [
            ("a disabled window", |held| held.enabled = false),
            ("a one-time instant already passed", |held| {
                held.schedule = "at(2020-01-01T00:00:00)".to_string();
            }),
            ("an activation window already closed", |held| {
                held.end_date = Some("2029-12-31T00:00:00Z".to_string());
            }),
            // A bound the window STATES that names no instant leaves the interval it would
            // run in unknown, so nothing is reported rather than an instant computed as
            // though the bound were absent.
            ("an activation stated in no readable form", |held| {
                held.start_date = Some("not-a-date".to_string());
            }),
            ("an inactivation stated in no readable form", |held| {
                held.end_date = Some("whenever".to_string());
            }),
            ("an activation stated as a date with no time", |held| {
                held.start_date = Some("2030-03-01".to_string());
            }),
            // This service's own cron dialect names instants the shared grammar cannot
            // place, so no instant is reported for one.
            ("a schedule in the service's own dialect", |held| {
                held.schedule = "cron(0 8 L * ? *)".to_string();
            }),
        ];

        for (case, differ) in rows {
            let mut held = window("cron(0 8 * * ? *)");
            differ(&mut held);
            let next = held.next_execution_time(now);
            assert_eq!(
                next, None,
                "case {case:?}: expected no next execution, got {next:?}"
            );
        }
    }

    /// A window whose activation is still ahead first runs after it becomes active, and
    /// one whose activation has passed runs on its own schedule.
    #[test]
    fn an_activation_date_moves_the_first_execution() {
        let now = epoch(1_893_456_000);
        // (case, start date, the instant reported)
        let rows: [(&str, Option<&str>, &str); 5] = [
            ("no activation at all", None, "2030-01-01T08:00Z"),
            ("an activation already passed", Some("2029-01-01T00:00:00Z"), "2030-01-01T08:00Z"),
            ("an activation stated empty", Some(""), "2030-01-01T08:00Z"),
            ("an activation still ahead", Some("2030-03-01T00:00:00Z"), "2030-03-01T08:00Z"),
            // A civil instant with no offset of its own is read in the window's zone.
            ("an activation stated without a zone", Some("2030-03-01T00:00:00"), "2030-03-01T08:00Z"),
        ];

        for (case, start_date, expected) in rows {
            let mut held = window("cron(0 8 * * ? *)");
            held.start_date = start_date.map(str::to_string);
            let next = held.next_execution_time(now);
            assert_eq!(
                next.as_deref(),
                Some(expected),
                "case {case:?}: expected the next execution at {expected:?}, got {next:?}"
            );
        }
    }

    /// A rate schedule's series is anchored at the instant the window was created, so the
    /// next occurrence is the first period boundary after the search start — computed, not
    /// stepped, however far ahead a caller's activation date puts that start.
    #[test]
    fn a_rate_series_is_anchored_at_the_create() {
        let now = epoch(1_893_456_000);
        let mut held = window("rate(1 minute)");
        // A century past the anchor: a series stepped one minute at a time would walk
        // 52 million periods to reach it.
        held.start_date = Some("2130-01-01T00:00:00Z".to_string());
        let next = held.next_execution_time(now);
        assert_eq!(
            next.as_deref(),
            Some("2130-01-01T00:01Z"),
            "expected the first period boundary after the activation, got {next:?}"
        );
    }

    /// Two creates carrying one idempotency token are the same call exactly when the
    /// members they supplied are the same, whichever member differs.
    #[test]
    fn the_creation_fingerprint_covers_every_supplied_member() {
        // (case, how the second create differs, renders the same way)
        let rows: [FingerprintRow; 9] = [
            ("nothing at all", |_| {}, true),
            // Neither the minted id nor the instants are supplied members.
            ("the minted id", |held| held.window_id = mint_window_id(9, epoch(1)), true),
            ("the instants", |held| held.created_date = epoch(42), true),
            ("the name", |held| held.name = "another-window".to_string(), false),
            ("the description", |held| held.description = Some("new".to_string()), false),
            ("the schedule", |held| held.schedule = "rate(1 day)".to_string(), false),
            ("the duration", |held| held.duration = 3, false),
            ("the cutoff", |held| held.cutoff = 0, false),
            ("a tag", |held| {
                held.tags.insert("Env".to_string(), "prod".to_string());
            }, false),
        ];

        for (case, differ, same_call) in rows {
            let first = window("cron(0 8 * * ? *)");
            let mut second = window("cron(0 8 * * ? *)");
            differ(&mut second);
            let matches = first.creation_fingerprint() == second.creation_fingerprint();
            assert_eq!(
                matches, same_call,
                "case {case:?}: expected same_call={same_call}, got {matches} for \
                 {:?} and {:?}",
                first.creation_fingerprint(),
                second.creation_fingerprint()
            );
        }
    }
}
