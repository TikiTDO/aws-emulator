// SPDX-License-Identifier: CC0-1.0

//! WHEN a schedule runs: the instants one `ScheduleExpression` names, read in the
//! zone the schedule states them in.
//!
//! "The expression that defines when the schedule runs. The following formats are
//! supported. at expression - at(yyyy-mm-ddThh:mm:ss) rate expression -
//! rate(value unit) cron expression - cron(fields)"
//! (`CreateSchedule$ScheduleExpression`), each read "in the timezone in which the
//! scheduling expression is evaluated"
//! (`CreateSchedule$ScheduleExpressionTimezone`). An `at(...)` expression names ONE
//! instant — "you can use at expressions to create one-time schedules that invoke a
//! target once, at the time and in the time zone, that you specify" — while a
//! `rate(...)` and a `cron(...)` name a series.
//!
//! This module answers only the question "which instant comes next"; whether that
//! instant falls inside the schedule's `StartDate`/`EndDate` window, and when this
//! emulator performs the invocation it is due, belong to [`super::invocation`].
//!
//! The grammars are read by the SAME maintained parsers the write path validates
//! them with ([`super::schedule_expression_violation`]) and from the SAME tables:
//! `chrono` for the `at(...)` instant under
//! [`super::schedule_definition::AT_EXPRESSION_FORMAT`], `chrono-tz` for the zone,
//! and the `cron` crate for a `cron(...)` body normalised by
//! [`super::schedule_definition::normalised_cron`], over the units
//! [`super::schedule_definition::RATE_UNITS`] names — so an expression that module
//! accepts is one this names instants for. A body carrying one of the wildcards
//! this service's own cron dialect documents and that crate's grammar does not read
//! (`L`, `W`, `#`) names no instant HERE, so such a schedule is accepted by the
//! control plane and left to the issue that owns that dialect rather than fired at
//! an instant guessed for it.

use std::time::{Duration, SystemTime};

use ::chrono::TimeZone;

use super::schedule_definition::{
    normalised_cron, AT_EXPRESSION_FORMAT, CRON_FIELDS, RATE_UNITS,
};

/// The zone an expression is read in when the schedule names none.
///
/// "The timezone in which the scheduling expression is evaluated"
/// (`CreateSchedule$ScheduleExpressionTimezone`) is optional, and the two instants
/// that bound a schedule are documented "in UTC" — so an expression with no zone
/// of its own is read in UTC.
const DEFAULT_ZONE: ::chrono_tz::Tz = ::chrono_tz::Tz::UTC;

/// The instants one `ScheduleExpression` names, in the zone it is read in.
#[derive(Debug, Clone)]
pub enum Occurrences {
    /// `at(<instant>)`: the ONE instant that expression names, already resolved
    /// against the schedule's zone.
    Once(SystemTime),
    /// `rate(<value> <unit>)`: one occurrence every period, the series being
    /// anchored by the caller rather than by the expression (AWS states no instant
    /// for a rate schedule's first invocation).
    Rate(Duration),
    /// `cron(<six fields>)`: an occurrence at each instant the expression names,
    /// read in the schedule's own zone.
    Cron(Box<::cron::Schedule>, ::chrono_tz::Tz),
}

impl Occurrences {
    /// The occurrences `expression` names, read in `timezone`, or `None` when it
    /// names none this module can compute — text stating no documented form, or a
    /// `cron(...)` body in the service's own dialect.
    #[must_use]
    pub fn parse(expression: &str, timezone: Option<&str>) -> Option<Self> {
        let zone = zone_of(timezone)?;
        let expression = expression.trim();
        if let Some(body) = body_of(expression, "at") {
            return at_instant(body, zone).map(Self::Once);
        }
        if let Some(body) = body_of(expression, "rate") {
            return rate_period(body).map(Self::Rate);
        }
        let body = body_of(expression, "cron")?;
        cron_schedule(body).map(|schedule| Self::Cron(Box::new(schedule), zone))
    }

    /// The first occurrence strictly AFTER `after`, or `None` when this expression
    /// names no instant beyond it — a one-time schedule whose instant has passed,
    /// or a cron expression whose year field is behind the clock.
    ///
    /// A `rate(...)` series is anchored on `after`: "rate expressions […] invoke a
    /// target at regular intervals" states the INTERVAL and no first instant, so
    /// the next occurrence of one is simply a period on from where the caller
    /// starts it.
    #[must_use]
    pub fn next_after(&self, after: SystemTime) -> Option<SystemTime> {
        match self {
            Self::Once(at) => (*at > after).then_some(*at),
            Self::Rate(period) => after.checked_add(*period),
            Self::Cron(schedule, zone) => {
                let from = ::chrono::DateTime::<::chrono::Utc>::from(after).with_timezone(zone);
                schedule
                    .after(&from)
                    .next()
                    .map(|instant| SystemTime::from(instant.with_timezone(&::chrono::Utc)))
            }
        }
    }

    /// The first occurrence AT or after `from`, or `None` when this expression names
    /// no instant from there on.
    ///
    /// What a floor on the series asks for: a `rate(...)` series opens AT the floor
    /// (its expression states an interval and no instant of its own), while an
    /// `at(...)` instant and a `cron(...)` instant are the expression's own and are
    /// only admitted from the floor on.
    #[must_use]
    pub fn next_at_or_after(&self, from: SystemTime) -> Option<SystemTime> {
        match self {
            Self::Once(at) => (*at >= from).then_some(*at),
            Self::Rate(_) => Some(from),
            Self::Cron(..) => {
                // The shared grammar's own iterator is strictly after its argument,
                // so an instant exactly AT the floor is admitted by probing from the
                // tick before it.
                let probe = from
                    .checked_sub(Duration::from_nanos(1))
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                self.next_after(probe)
            }
        }
    }

    /// Whether this expression names at most ONE occurrence, which is what an
    /// `at(...)` expression states: "one-time schedules that invoke a target once".
    #[must_use]
    pub fn is_one_time(&self) -> bool {
        matches!(self, Self::Once(_))
    }
}

/// The zone `timezone` names, or `None` when it names none the IANA database
/// carries — which the write path has already refused
/// ([`super::schedule_expression_timezone_violation`]), so a stored schedule
/// carrying one was stored before it did.
fn zone_of(timezone: Option<&str>) -> Option<::chrono_tz::Tz> {
    match timezone.filter(|zone| !zone.is_empty()) {
        Some(zone) => zone.parse().ok(),
        None => Some(DEFAULT_ZONE),
    }
}

/// The body `keyword(...)` carries, or `None` when `expression` states another form.
fn body_of<'e>(expression: &'e str, keyword: &str) -> Option<&'e str> {
    expression
        .strip_prefix(keyword)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
}

/// The instant an `at(...)` body names, read in `zone`.
///
/// A local time a zone's offset change leaves AMBIGUOUS (a clock that goes back
/// over it) resolves to the EARLIER of its two instants, and one that zone skips
/// entirely names no instant at all — the reading `chrono` gives a local time, so
/// nothing here invents an offset for either case.
fn at_instant(body: &str, zone: ::chrono_tz::Tz) -> Option<SystemTime> {
    let naive = ::chrono::NaiveDateTime::parse_from_str(body.trim(), AT_EXPRESSION_FORMAT).ok()?;
    let local = zone.from_local_datetime(&naive).earliest()?;
    Some(SystemTime::from(local.with_timezone(&::chrono::Utc)))
}

/// The period a `rate(...)` body states, or `None` when it states none.
fn rate_period(body: &str) -> Option<Duration> {
    let mut fields = body.split_whitespace();
    let (Some(value), Some(unit), None) = (fields.next(), fields.next(), fields.next()) else {
        return None;
    };
    let value: u64 = value.parse().ok()?;
    if value == 0 {
        return None;
    }
    let seconds = RATE_UNITS
        .iter()
        .find(|(name, _)| *name == unit)
        .map(|(_, seconds)| *seconds)?;
    // A figure of units no span can represent states no period at all: it is
    // refused rather than saturated into one the series could never move past.
    Some(Duration::from_secs(value.checked_mul(seconds)?))
}

/// The schedule a `cron(...)` body states, or `None` for a body the shared parser's
/// grammar does not read.
///
/// The six documented fields become the seven-field form that grammar reads through
/// the write path's own normalisation ([`normalised_cron`]), so both sides read one
/// expression.
fn cron_schedule(body: &str) -> Option<::cron::Schedule> {
    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != CRON_FIELDS {
        return None;
    }
    normalised_cron(&fields).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instant `2027-03-01T00:00:00Z` as a clock reading, which the rows below
    /// measure from.
    fn epoch(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// An `at(...)` expression names exactly one instant, resolved in the zone the
    /// schedule states — so the same body names two different instants in two
    /// zones, and names nothing once its instant has passed.
    #[test]
    fn a_one_time_expression_names_one_instant_in_its_own_zone() {
        // (case, zone, the instant the expression names as seconds since the epoch)
        let rows: [(&str, Option<&str>, u64); 4] = [
            // 2030-01-01T00:00:00Z.
            ("no zone at all is UTC", None, 1_893_456_000),
            ("UTC named explicitly", Some("UTC"), 1_893_456_000),
            // UTC-05:00 on that date: five hours later in absolute terms.
            ("a western zone", Some("America/New_York"), 1_893_456_000 + 5 * 3_600),
            // UTC+01:00 on that date: one hour earlier.
            ("an eastern zone", Some("Europe/Berlin"), 1_893_456_000 - 3_600),
        ];
        for (case, zone, expected) in rows {
            let occurrences = Occurrences::parse("at(2030-01-01T00:00:00)", zone)
                .unwrap_or_else(|| panic!("case {case:?}: the expression names occurrences"));
            assert!(
                occurrences.is_one_time(),
                "case {case:?}: an at expression names one occurrence, got {occurrences:?}"
            );
            let next = occurrences.next_after(epoch(1_800_000_000));
            assert_eq!(
                next,
                Some(epoch(expected)),
                "case {case:?}: expected the instant at {expected}, got {next:?}"
            );
            assert_eq!(
                occurrences.next_after(epoch(expected)),
                None,
                "case {case:?}: an at expression names nothing after its own instant, got {:?}",
                occurrences.next_after(epoch(expected))
            );
        }
    }

    /// A `rate(...)` expression names one occurrence per period, from wherever the
    /// series is anchored; the rows differ along the value and the unit.
    #[test]
    fn a_rate_expression_names_one_occurrence_per_period() {
        // (case, expression, the period in seconds)
        let rows: [(&str, &str, u64); 6] = [
            ("one minute", "rate(1 minute)", 60),
            ("five minutes", "rate(5 minutes)", 300),
            ("a plural unit with one", "rate(1 minutes)", 60),
            ("a singular unit with twelve", "rate(12 hour)", 12 * 3_600),
            ("two hours", "rate(2 hours)", 7_200),
            ("three days", "rate(3 days)", 3 * 86_400),
        ];
        for (case, expression, period) in rows {
            let occurrences = Occurrences::parse(expression, None)
                .unwrap_or_else(|| panic!("case {case:?}: the expression names occurrences"));
            assert!(
                !occurrences.is_one_time(),
                "case {case:?}: a rate expression names a series, got {occurrences:?}"
            );
            let anchor = epoch(1_800_000_000);
            let first = occurrences.next_after(anchor);
            assert_eq!(
                first,
                Some(epoch(1_800_000_000 + period)),
                "case {case:?}: expected the first occurrence one period on, got {first:?}"
            );
            let second = first.and_then(|first| occurrences.next_after(first));
            assert_eq!(
                second,
                Some(epoch(1_800_000_000 + 2 * period)),
                "case {case:?}: expected the second occurrence two periods on, got {second:?}"
            );
        }
    }

    /// A `cron(...)` expression names the instants its fields do, read in the
    /// schedule's zone: the rows differ along the fields and along the zone the
    /// same fields are read in.
    #[test]
    fn a_cron_expression_names_the_instants_its_fields_do() {
        // 2030-01-01T00:00:00Z, a Tuesday.
        let from = epoch(1_893_456_000);
        // (case, expression, zone, the next occurrence as seconds since the epoch)
        let rows: [(&str, &str, Option<&str>, u64); 4] = [
            ("every minute", "cron(* * * * ? *)", None, 1_893_456_060),
            ("one hour of the day", "cron(0 8 * * ? *)", None, 1_893_456_000 + 8 * 3_600),
            // 08:00 in UTC-05:00 is 13:00 UTC on that date.
            (
                "one hour of the day in a western zone",
                "cron(0 8 * * ? *)",
                Some("America/New_York"),
                1_893_456_000 + 13 * 3_600,
            ),
            // The first day of the next month, 2030-02-01T00:00:00Z.
            ("one day of the month", "cron(0 0 1 * ? *)", None, 1_896_134_400),
        ];
        for (case, expression, zone, expected) in rows {
            let occurrences = Occurrences::parse(expression, zone)
                .unwrap_or_else(|| panic!("case {case:?}: the expression names occurrences"));
            let next = occurrences.next_after(from);
            assert_eq!(
                next,
                Some(epoch(expected)),
                "case {case:?}: expected the occurrence at {expected}, got {next:?}"
            );
        }
    }

    /// Text that states no computable series names no occurrence at all, whichever
    /// way it is malformed — and a well-formed expression read in a zone nothing
    /// names does not either.
    #[test]
    fn text_that_states_no_series_names_no_occurrence() {
        // (case, expression, zone)
        let rows: [(&str, &str, Option<&str>); 7] = [
            ("no form at all", "every 5 minutes", None),
            ("a rate of zero", "rate(0 minutes)", None),
            ("a rate in a unit nothing measures", "rate(5 fortnights)", None),
            ("a cron of five fields", "cron(0 8 1 * ?)", None),
            // The service's own cron dialect, which the shared grammar does not read.
            ("a cron on the last day of the month", "cron(0 8 L * ? *)", None),
            ("an at instant that is not one", "at(not-an-instant)", None),
            ("a zone nothing names", "rate(5 minutes)", Some("Not/AZone")),
        ];
        for (case, expression, zone) in rows {
            let occurrences = Occurrences::parse(expression, zone);
            assert!(
                occurrences.is_none(),
                "case {case:?}: expected no occurrences, got {occurrences:?}"
            );
        }
    }
}
