// SPDX-License-Identifier: CC0-1.0

//! Whether a target group's health-check SCHEDULE can be honored: the two members that
//! time the probe have to fit together.
//!
//! `HealthCheckIntervalSeconds` is "the approximate amount of time, in seconds, between
//! health checks of an individual target" and `HealthCheckTimeoutSeconds` is "the amount
//! of time, in seconds, during which no response from a target means a failed health
//! check". A check whose timeout OUTLASTS the interval between checks is still waiting
//! for an answer after the next check was already due, so the pair describes a probe
//! schedule no load balancer can run: the timeout may be as long as the interval, and no
//! longer.
//!
//! The model bounds each member on its own (`range 5..300` and `range 2..120`) and
//! declares nothing about the pair, so the boundary validator passes every combination
//! and this is the only gate. It is read here ONCE, by every operation that writes a
//! health check — `CreateTargetGroup`, which settles the whole check from the request and
//! the documented defaults, and `ModifyTargetGroup`, which changes the members the
//! request names on a check the target group already stores — because both decide it on
//! the same two EFFECTIVE values: what the request carries, else what the operation would
//! otherwise leave in place. Both answer the refusal under this service's own
//! `ValidationError` code (`crate::scaffolded`), as every other rule its model states in
//! prose rather than as a constraint does.

/// The wording Elastic Load Balancing refuses an unhonorable health-check schedule with.
///
/// Fixed text with no variable part: neither member is identified by a value the caller
/// sent — what cannot hold together is the pair.
pub const HEALTH_CHECK_SCHEDULE_MESSAGE: &str =
    "Health check interval must be greater than the timeout.";

/// Whether the health check a call would leave behind can be run, and, when it cannot,
/// the rule it broke.
///
/// `interval_seconds` and `timeout_seconds` are the EFFECTIVE values — the ones the
/// target group ends up with — so a caller passes what the request carries, else the
/// value it leaves in place (the stored one on a modify, the documented default on a
/// create).
pub fn check_health_check_schedule(
    interval_seconds: i32,
    timeout_seconds: i32,
) -> Result<(), String> {
    if timeout_seconds <= interval_seconds {
        return Ok(());
    }
    Err(HEALTH_CHECK_SCHEDULE_MESSAGE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_may_last_as_long_as_the_interval_and_no_longer() {
        for (interval, timeout, honorable) in [
            (6, 5, true),
            (300, 120, true),
            (5, 2, true),
            (5, 5, true),
            (120, 120, true),
            (10, 11, false),
            (5, 120, false),
            (30, 31, false),
        ] {
            assert_eq!(
                check_health_check_schedule(interval, timeout).is_ok(),
                honorable,
                "case interval {interval} timeout {timeout}: expected honorable={honorable}, got {:?}",
                check_health_check_schedule(interval, timeout)
            );
        }
    }
}
