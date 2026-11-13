// SPDX-License-Identifier: CC0-1.0

//! What follows ONE delivery attempt of an accepted asynchronous invocation: whether the
//! event is handed to the function again, and how long after.
//!
//! "When an error occurs, your function may be invoked multiple times. Retry behavior varies
//! by error type, client, event source, and invocation type. For example, if you invoke a
//! function asynchronously and it returns an error, Lambda executes the function up to two
//! more times" (`Invoke`). So a delivery attempt whose handler ERRORED is not the end of the
//! event: Lambda makes further attempts, up to [`MAX_ASYNC_DELIVERY_ATTEMPTS`] in all, and
//! only then does the entry keep what the last of them came to
//! ([`super::AsyncInvocationOutcome`]).
//!
//! What does NOT get another attempt is an event the function could not be RUN for at all —
//! a package this emulator embeds no engine for, one it cannot read, a configuration naming
//! no handler. The retry above is for a function that ran and errored; a run that never
//! started fails the same way however often it is attempted, so the entry records that
//! instead of spending attempts on it.
//!
//! An attempt that found NO CAPACITY to run the function is neither of those: nothing about
//! the event stopped it, so it is attempted AGAIN rather than recorded as what became of the
//! event — "If your function is throttled, Lambda returns the event to the queue and attempts
//! to run the function again for up to six hours. The retry interval increases exponentially
//! from 1 second after the first attempt to a maximum of 5 minutes." That is
//! [`next_throttled_attempt_in`], bounded by how long the event is RETAINED
//! ([`event_retention`]) rather than by the attempts an errored handler has: how busy the host
//! running this emulator happens to be decides when the function runs the event, never
//! whether it ever does.
//!
//! This module is the policy and nothing else: the attempt itself, and the entry the outcome
//! lands on, are [`super::async_invocation`]'s.

use std::time::Duration;

/// How many delivery attempts one accepted asynchronous event has.
///
/// "If you invoke a function asynchronously and it returns an error, Lambda executes the
/// function up to two more times" (`Invoke`): the first attempt plus those two.
pub const MAX_ASYNC_DELIVERY_ATTEMPTS: u32 = 3;

/// How long after a delivery attempt whose handler errored the next attempt is made.
///
/// An EMULATOR-CHOSEN span, inside the range this crate chooses for every span of its own
/// ([`super::ACTIVATION_DELAY`]). AWS publishes a minute-scale wait between the attempts it
/// makes on an asynchronous invocation; a wait of that order would outlast any caller's
/// observation of its own event, so what is kept faithful is the part a client can observe —
/// how many attempts the event has ([`MAX_ASYNC_DELIVERY_ATTEMPTS`]), and that they are
/// separated in time rather than run back to back.
pub const ASYNC_RETRY_DELAY: Duration = Duration::from_secs(2);

/// How long until the next delivery attempt of an event whose attempts that RAN the handler
/// so far number `runs_made`, or `None` when the event has no further attempt.
///
/// `function_error` is the `FunctionError` the attempt's run reported — "If present,
/// indicates that an error occurred during function execution" — and `None` for a handler
/// that returned, which is an event Lambda is done with. A run that never started is not an
/// errored handler and reaches here as `ran: false`: no attempt can change what stopped it,
/// so the event has no further attempt either.
///
/// The count is of the attempts whose handler RAN, because that is what the bound is about:
/// "The maximum number of times to retry when the function returns an error"
/// (`FunctionEventInvokeConfig$MaximumRetryAttempts`). An attempt that found no capacity ran
/// nothing, so it spends none of them and is decided by [`next_throttled_attempt_in`]
/// instead.
#[must_use]
pub fn next_attempt_in(
    ran: bool,
    function_error: Option<&str>,
    runs_made: u32,
) -> Option<Duration> {
    let errored = ran && function_error.is_some();
    (errored && runs_made < MAX_ASYNC_DELIVERY_ATTEMPTS).then_some(ASYNC_RETRY_DELAY)
}

/// How long Lambda keeps an accepted asynchronous event while it tries to get the function
/// run for it: "It retains events in a queue for up to six hours"
/// (`PutFunctionEventInvokeConfig`), which is also the largest age the model lets a caller
/// ask for (`smithy_query(query="shape", shape="MaximumEventAgeInSeconds")`: 60 to 21600).
///
/// A duration AWS DOCUMENTS, so it keeps its published value rather than being compressed
/// like the spans this crate chooses for itself ([`super::ACTIVATION_DELAY`]).
pub const DEFAULT_ASYNC_EVENT_RETENTION: Duration = Duration::from_secs(21_600);

/// The smallest and largest `MaximumEventAgeInSeconds` the model admits
/// (`smithy_query(query="shape", shape="MaximumEventAgeInSeconds")`: `@range(min: 60, max:
/// 21600)`), which a stored value is read inside however it got there.
const MIN_EVENT_AGE_SECONDS: i32 = 60;
/// The upper end of that range; see [`MIN_EVENT_AGE_SECONDS`].
const MAX_EVENT_AGE_SECONDS: i32 = 21_600;

/// How long an event accepted for a qualifier whose asynchronous-invocation configuration
/// states `maximum_event_age_in_seconds` is retained.
///
/// "The maximum age of a request that Lambda sends to a function for processing"
/// (`FunctionEventInvokeConfig$MaximumEventAgeInSeconds`) is the window when a caller has
/// stated one, and the documented default ([`DEFAULT_ASYNC_EVENT_RETENTION`]) is the window
/// for every qualifier nobody has configured — which is what
/// [`super::EventInvokeConfigRecord`] holds no value of its own for.
#[must_use]
pub fn event_retention(maximum_event_age_in_seconds: Option<i32>) -> Duration {
    maximum_event_age_in_seconds.map_or(DEFAULT_ASYNC_EVENT_RETENTION, |seconds| {
        let clamped = seconds.clamp(MIN_EVENT_AGE_SECONDS, MAX_EVENT_AGE_SECONDS);
        Duration::from_secs(u64::try_from(clamped).unwrap_or_default())
    })
}

/// The wait before the next attempt of an event whose attempt found no capacity, and the
/// ceiling those waits grow to: "The retry interval increases exponentially from 1 second
/// after the first attempt to a maximum of 5 minutes."
pub const THROTTLED_FIRST_RETRY: Duration = Duration::from_secs(1);
/// The ceiling that growth reaches; see [`THROTTLED_FIRST_RETRY`].
pub const THROTTLED_MAX_RETRY: Duration = Duration::from_secs(300);

/// How many doublings reach that ceiling: one second doubled nine times is past five
/// minutes, so no later throttle needs a larger shift to be answered the ceiling.
const MAX_THROTTLE_DOUBLINGS: u32 = 9;

/// How long until the next delivery attempt of an event whose attempt found NO CAPACITY to
/// run the function, or `None` when the event has been held for as long as it is retained.
///
/// A throttle is not what became of the event: "If your function is throttled, Lambda
/// returns the event to the queue and attempts to run the function again for up to six
/// hours. The retry interval increases exponentially from 1 second after the first attempt
/// to a maximum of 5 minutes." So the event is attempted again for as long as `retention`
/// ([`event_retention`]) keeps it, `age` being how long it has been held so far, and the
/// wait grows with the throttles it has met (`throttles_so_far`, counting the one just met)
/// up to [`THROTTLED_MAX_RETRY`]. The last wait is cut to what is left of the window, so no
/// attempt is made past it.
#[must_use]
pub fn next_throttled_attempt_in(
    throttles_so_far: u32,
    age: Duration,
    retention: Duration,
) -> Option<Duration> {
    let left = retention.checked_sub(age).filter(|left| !left.is_zero())?;
    let doublings = throttles_so_far
        .saturating_sub(1)
        .min(MAX_THROTTLE_DOUBLINGS);
    let wait = THROTTLED_FIRST_RETRY
        .saturating_mul(2u32.saturating_pow(doublings))
        .min(THROTTLED_MAX_RETRY);
    Some(wait.min(left))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the table below: the case, whether the run happened, the `FunctionError`
    /// it reported, how many attempts the event has had, and when the next one is due.
    type Row = (&'static str, bool, Option<&'static str>, u32, Option<Duration>);

    /// One row per combination the policy decides over: whether the run happened at all,
    /// whether its handler errored, and how many attempts the event has already had —
    /// including at the published bound and one past it.
    #[test]
    fn only_an_errored_handler_with_an_attempt_left_is_delivered_again() {
        let rows: &[Row] = &[
            ("the handler returned on its first attempt", true, None, 1, None),
            (
                "the handler errored on its first attempt",
                true,
                Some("Unhandled"),
                1,
                Some(ASYNC_RETRY_DELAY),
            ),
            (
                "the handler errored on its second attempt",
                true,
                Some("Unhandled"),
                2,
                Some(ASYNC_RETRY_DELAY),
            ),
            (
                "the handler errored on the last attempt the event has",
                true,
                Some("Unhandled"),
                MAX_ASYNC_DELIVERY_ATTEMPTS,
                None,
            ),
            (
                "the handler errored past the attempts the event has",
                true,
                Some("Unhandled"),
                MAX_ASYNC_DELIVERY_ATTEMPTS + 1,
                None,
            ),
            (
                "the handler errored with another function error",
                true,
                Some("Unhandled.OutOfMemory"),
                1,
                Some(ASYNC_RETRY_DELAY),
            ),
            ("the run never started", false, None, 1, None),
            (
                "the run never started, with an error reported for it",
                false,
                Some("Unhandled"),
                1,
                None,
            ),
            (
                "the handler returned on its last attempt",
                true,
                None,
                MAX_ASYNC_DELIVERY_ATTEMPTS,
                None,
            ),
        ];
        for (case, ran, function_error, attempts_made, expected) in rows {
            let answered = next_attempt_in(*ran, *function_error, *attempts_made);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected the next attempt in {expected:?}, got {answered:?}"
            );
        }
    }

    /// One row of the throttle table: the case, how many throttles the event has met, how
    /// long it has been held, how long it is retained, and when the next attempt is due.
    type ThrottleRow = (&'static str, u32, Duration, Duration, Option<Duration>);

    /// One row per point the published rule decides over: the wait after the first throttle,
    /// its growth, the ceiling that growth stops at, a window with time left in it and one
    /// spent, and the last wait being cut to what is left.
    #[test]
    fn a_throttled_event_is_attempted_again_until_its_window_is_spent() {
        let rows: &[ThrottleRow] = &[
            (
                "the first throttle",
                1,
                Duration::ZERO,
                DEFAULT_ASYNC_EVENT_RETENTION,
                Some(THROTTLED_FIRST_RETRY),
            ),
            (
                "the second throttle doubles the wait",
                2,
                Duration::from_secs(1),
                DEFAULT_ASYNC_EVENT_RETENTION,
                Some(THROTTLED_FIRST_RETRY * 2),
            ),
            (
                "the fourth throttle doubles it again",
                4,
                Duration::from_secs(7),
                DEFAULT_ASYNC_EVENT_RETENTION,
                Some(THROTTLED_FIRST_RETRY * 8),
            ),
            (
                "a throttle past the published ceiling waits the ceiling",
                20,
                Duration::from_secs(600),
                DEFAULT_ASYNC_EVENT_RETENTION,
                Some(THROTTLED_MAX_RETRY),
            ),
            (
                "an event held for as long as it is retained has no further attempt",
                1,
                DEFAULT_ASYNC_EVENT_RETENTION,
                DEFAULT_ASYNC_EVENT_RETENTION,
                None,
            ),
            (
                "an event held past its window has no further attempt",
                1,
                DEFAULT_ASYNC_EVENT_RETENTION + Duration::from_secs(1),
                DEFAULT_ASYNC_EVENT_RETENTION,
                None,
            ),
            (
                "the last wait is cut to what is left of the window",
                20,
                Duration::from_secs(55),
                Duration::from_secs(60),
                Some(Duration::from_secs(5)),
            ),
            (
                "a window a caller shortened is the one in force",
                1,
                Duration::from_secs(59),
                Duration::from_secs(60),
                Some(Duration::from_secs(1)),
            ),
        ];
        for (case, throttles, age, retention, expected) in rows {
            let answered = next_throttled_attempt_in(*throttles, *age, *retention);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected the next attempt in {expected:?}, got {answered:?}"
            );
        }
    }

    /// The window an event is retained for: what a caller stated, read inside the range the
    /// model admits, and the documented six hours for a qualifier nobody configured — at
    /// each published bound and one past it.
    #[test]
    fn the_retention_window_is_what_the_configuration_states() {
        // (case, the configured `MaximumEventAgeInSeconds`, the window in force)
        let rows: &[(&str, Option<i32>, Duration)] = &[
            ("no configuration at all", None, DEFAULT_ASYNC_EVENT_RETENTION),
            (
                "the smallest age the model admits",
                Some(MIN_EVENT_AGE_SECONDS),
                Duration::from_secs(60),
            ),
            (
                "an age inside the range",
                Some(900),
                Duration::from_secs(900),
            ),
            (
                "the largest age the model admits",
                Some(MAX_EVENT_AGE_SECONDS),
                DEFAULT_ASYNC_EVENT_RETENTION,
            ),
            (
                "an age below the range",
                Some(MIN_EVENT_AGE_SECONDS - 1),
                Duration::from_secs(60),
            ),
            (
                "an age above the range",
                Some(MAX_EVENT_AGE_SECONDS + 1),
                DEFAULT_ASYNC_EVENT_RETENTION,
            ),
        ];
        for (case, configured, expected) in rows {
            let answered = event_retention(*configured);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected the window {expected:?}, got {answered:?}"
            );
        }
    }
}
