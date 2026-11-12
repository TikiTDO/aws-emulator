// SPDX-License-Identifier: CC0-1.0

//! The practice run configuration ARC holds for a managed resource, and the rules every
//! operation that writes one applies to it.
//!
//! A configuration is ARC's own record ABOUT a resource, not part of the resource: ARC
//! models no operation that registers a managed resource, so a resource either was seeded
//! into this store or was published by the service that owns it, and a configuration
//! written for either is kept in one map keyed by `resourceIdentifier`
//! ([`ZonalShiftState::practice_run_configurations`]). The accessors that answer about a
//! managed resource merge it in for the length of a call, so `practiceRunConfiguration`
//! and the `practiceRunStatus` derived from it read the same whichever way the resource
//! reached ARC.
//!
//! The rules here are the model's and the documentation's, and they hold for every
//! operation that writes a configuration rather than for one of them. Each is a rule the
//! model does NOT express as a constraint, so each is the handler's to apply — a modelled
//! constraint (a list's length, a member's pattern, a `type`'s enumerated set) is already
//! refused at the boundary before a handler runs:
//!
//! * an allowed or blocked window is one window of a SINGLE day — "The `blockedWindows`
//!   have to start and end on the same day. Windows that span multiple days aren't
//!   supported" (`PracticeRunConfiguration$blockedWindows`, and the same sentence on
//!   `allowedWindows`) — named as a time of day, so it starts and ends at real clock
//!   times and ends after it starts;
//! * a blocked date is a calendar date in UTC — "Specify blocked dates, in UTC, in the
//!   format `YYYY-MM-DD`" (`PracticeRunConfiguration$blockedDates`) — so a value of that
//!   shape that names no day of a month is not one.

use chrono::{NaiveDate, NaiveTime};

use super::ComAmazonawsArczonalshiftPercdataplaneState as ZonalShiftState;
use crate::ops::types::{ControlCondition, PracticeRunConfiguration};

/// The message ARC answers when a resource is asked for a practice run configuration it
/// already has.
///
/// Like the other refusals of this service it names no identifier: the conflict is a
/// property of the resource the request named, which the caller already named. The
/// configuration of a resource that has one is changed through
/// `UpdatePracticeRunConfiguration`.
pub const PRACTICE_CONFIGURATION_EXISTS_MESSAGE: &str =
    "There is already a practice run configuration for the requested resource.";

/// The message ARC answers when a call needs a practice run configuration the resource
/// it names does not have.
///
/// The sentence is ARC's own for this condition — it is the first of the two the service
/// answers when zonal autoshift is asked to be enabled for a resource that has none
/// ([`super::PRACTICE_CONFIGURATION_REQUIRED_MESSAGE`]) — without the clause about
/// enabling autoshift, which is that call's and not this one's. Like the other refusals
/// of this service it names no identifier and has no variable part: the conflict is a
/// property of the resource the caller already named, and a resource either has a
/// configuration or does not.
pub const PRACTICE_CONFIGURATION_MISSING_MESSAGE: &str =
    "Resource does not have a practice run configuration.";

/// The `ConflictExceptionReason` ARC names for every conflict whose cause is that a
/// resource has no practice run configuration
/// (`smithy_query(query="shape", shape="ConflictExceptionReason")`). Declared once here,
/// where the configuration is kept, so the operations that need one cannot drift apart on
/// the reason they report.
pub const PRACTICE_CONFIGURATION_MISSING_REASON: &str = "PracticeConfigurationDoesNotExist";

/// Why a write of a practice run configuration is refused: the
/// `ValidationExceptionReason` and the message, which the calling operation lifts into
/// its own modelled `ValidationException`.
#[derive(Debug)]
pub struct PracticeRunConfigurationRefused {
    pub reason: &'static str,
    pub message: String,
}

/// The format a window of days and times is written in, as the documentation of
/// `allowedWindows` and `blockedWindows` gives it: `DAY:HH:MM-DAY:HH:MM`.
const WINDOW_FORMAT: &str = "DAY:HH:MM-DAY:HH:MM";

/// The format a blocked date is written in (`blockedDates`: "in the format
/// `YYYY-MM-DD`"), as both the message and [`NaiveDate::parse_from_str`] read it.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// The time of day half of a window, as [`NaiveTime::parse_from_str`] reads it.
const WINDOW_TIME_FORMAT: &str = "%H:%M";

/// The configuration a request asks ARC to hold for a resource, or the refusal the
/// caller gets instead.
///
/// One reading of the request for every operation that writes a configuration, so none
/// of them can accept a configuration another would refuse. Each list is kept as the
/// caller gave it — an absent optional list stays absent — except `blockedDates`, whose
/// model default is the empty list.
pub fn requested_practice_run_configuration(
    outcome_alarms: Vec<ControlCondition>,
    blocking_alarms: Option<Vec<ControlCondition>>,
    blocked_windows: Option<Vec<String>>,
    allowed_windows: Option<Vec<String>>,
    blocked_dates: Option<Vec<String>>,
) -> Result<PracticeRunConfiguration, PracticeRunConfigurationRefused> {
    // The two window members carry the same rule and are refused with the reason named
    // for each (`ValidationExceptionReason`: `InvalidPracticeAllowedWindow` for the
    // allowed windows, `InvalidPracticeWindows` for the blocked ones).
    if let Some(windows) = blocked_windows.as_deref() {
        check_windows("blockedWindows", "InvalidPracticeWindows", windows)?;
    }
    if let Some(windows) = allowed_windows.as_deref() {
        check_windows("allowedWindows", "InvalidPracticeAllowedWindow", windows)?;
    }
    let blocked_dates = blocked_dates.unwrap_or_default();
    for date in &blocked_dates {
        if NaiveDate::parse_from_str(date, DATE_FORMAT).is_err() {
            return Err(PracticeRunConfigurationRefused {
                // A blocked date is one of the practice-run blockers, which is the
                // reason the enum names for one it cannot read.
                reason: "InvalidPracticeBlocker",
                message: format!(
                    "blockedDates value {date} is not a date in the format YYYY-MM-DD"
                ),
            });
        }
    }

    Ok(PracticeRunConfiguration {
        blocking_alarms,
        outcome_alarms,
        blocked_windows,
        allowed_windows,
        blocked_dates,
    })
}

/// The configuration ARC holds for a resource after a request CHANGES the one it already
/// holds, or the refusal the caller gets instead.
///
/// A change is applied member by member on top of `held`: a member the request names
/// replaces the held one — a member named as the empty list therefore removes what was
/// held, leaving the member empty — and a member the request does not name is left
/// exactly as held. That is the whole of what `UpdatePracticeRunConfiguration` documents
/// it does ("change one or more of the following: add, change, or remove the blocking
/// alarm; change the outcome alarm; or add, change, or remove blocking dates or time
/// windows"): removal is a list named empty, which every member but `outcomeAlarms`
/// admits (`outcomeAlarms` is length 1..10, so it can only be changed).
///
/// The merged configuration is then read the ONE way every write of a configuration
/// reads one ([`requested_practice_run_configuration`]), so a change cannot leave a
/// resource holding a configuration a create would have refused.
pub fn updated_practice_run_configuration(
    held: PracticeRunConfiguration,
    outcome_alarms: Option<Vec<ControlCondition>>,
    blocking_alarms: Option<Vec<ControlCondition>>,
    blocked_windows: Option<Vec<String>>,
    allowed_windows: Option<Vec<String>>,
    blocked_dates: Option<Vec<String>>,
) -> Result<PracticeRunConfiguration, PracticeRunConfigurationRefused> {
    requested_practice_run_configuration(
        outcome_alarms.unwrap_or(held.outcome_alarms),
        blocking_alarms.or(held.blocking_alarms),
        blocked_windows.or(held.blocked_windows),
        allowed_windows.or(held.allowed_windows),
        // `blockedDates` is the one list the model defaults to the empty list rather than
        // leaving absent, so what is held for it is always a list.
        Some(blocked_dates.unwrap_or(held.blocked_dates)),
    )
}

/// Refuse a window that is not one window of a single day.
fn check_windows(
    member: &str,
    reason: &'static str,
    windows: &[String],
) -> Result<(), PracticeRunConfigurationRefused> {
    for window in windows {
        if !names_one_day_window(window) {
            return Err(PracticeRunConfigurationRefused {
                reason,
                message: format!(
                    "{member} value {window} is not a window of a single day in the \
                     format {WINDOW_FORMAT}"
                ),
            });
        }
    }
    Ok(())
}

/// Whether `window` names one window that starts and ends on the same day, at real
/// times of day, and ends after it starts.
///
/// A window that ends when it starts is not one the documentation leaves room for
/// either: a window is a span of days and times practice runs are allowed or blocked
/// in, and a span of no length allows and blocks nothing.
#[must_use]
fn names_one_day_window(window: &str) -> bool {
    let Some((start, end)) = window.split_once('-') else {
        return false;
    };
    let (Some((start_day, start_time)), Some((end_day, end_time))) =
        (start.split_once(':'), end.split_once(':'))
    else {
        return false;
    };
    if start_day != end_day {
        return false;
    }
    let (Ok(start_time), Ok(end_time)) = (
        NaiveTime::parse_from_str(start_time, WINDOW_TIME_FORMAT),
        NaiveTime::parse_from_str(end_time, WINDOW_TIME_FORMAT),
    ) else {
        return false;
    };
    end_time > start_time
}

impl ZonalShiftState {
    /// The practice run configuration ARC holds for a resource, or `None` when none has
    /// been written for it.
    #[must_use]
    pub fn practice_run_configuration(
        &self,
        resource_identifier: &str,
    ) -> Option<PracticeRunConfiguration> {
        self.practice_run_configurations
            .get(resource_identifier)
            .cloned()
    }

    /// Hold `configuration` for a resource, replacing any configuration it had.
    pub fn set_practice_run_configuration(
        &mut self,
        resource_identifier: &str,
        configuration: PracticeRunConfiguration,
    ) {
        self.practice_run_configurations
            .insert(resource_identifier.to_string(), configuration);
    }
}
