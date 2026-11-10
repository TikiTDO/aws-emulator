// SPDX-License-Identifier: CC0-1.0

//! The TARGET HEALTH state machine of one target group: what `DescribeTargetHealth`
//! reports for a registration, and the two spans the clock owes it.
//!
//! A registration is not born serving traffic and does not stop serving it the instant a
//! caller asks: `TargetHealthStateEnum`
//! (`smithy_query(query="shape",shape="TargetHealthStateEnum")`) names
//! `initial`/`healthy`/`unhealthy`/`unused`/`draining`/`unavailable`, and
//! `TargetHealth$State`'s own documentation gives each of them a reason
//! (`TargetHealth$Reason`, the `TargetHealthReasonEnum` values quoted below). Two of
//! those states are TRANSIENT — the clock, not a caller, is what ends them — and this
//! module is the one place both are decided, so the read that REPORTS a target's health
//! and the data plane that decides whether a target may be sent a request cannot drift:
//!
//! * `initial` — "the load balancer is still sending the target the minimum number of
//!   health checks required to determine its health status" — which begins when health
//!   checking on the registration begins: when it is registered, or, for a target group
//!   no load balancer forwarded to yet, when the group first becomes one a load balancer
//!   forwards to ([`TargetGroupRecord::begin_health_checking`]). A group nothing forwards
//!   to has nothing checking its targets at all, which is the `unused` state below.
//! * `draining` — "the load balancer is draining connections to the target", the state
//!   `DeregisterTargets` puts a registration in: it stops receiving NEW requests
//!   immediately and leaves the target group only once the deregistration delay has
//!   passed ([`deregistration_delay`]).
//!
//! Past the registration span the state is the one the CHECKS reached, never a constant:
//! `unhealthy` once the group's `UnhealthyThresholdCount` consecutive checks failed, and the
//! second `initial` (`Elb.InitialHealthChecking`) while a check has failed and that threshold
//! is not reached yet — so a registration whose checks are failing is never reported healthy
//! on the way to its verdict. What one check observes is decided in [`super::host_counts`],
//! against the effective health-check port and the ports the workload behind the registration
//! serves ([`ServedPorts`]). A registration whose checks pass, and one no check observed
//! anything about, keep the `healthy` the clock owes them.
//!
//! Both spans are stored as the instant they come due, never slept for; a read
//! materialises what the clock already owes ([`TargetGroupRecord::settle_draining`]) and
//! writes nothing else.

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::ops::types::{TargetDescription, TargetHealth, TargetHealthDescription};

use super::{
    target_group_attribute, targets_per_target_group, RegisteredTarget, TargetGroupRecord,
    TargetKey, SETTLE_AFTER,
};

/// The target group attribute that holds how long a deregistering target drains, from
/// `TargetGroupAttribute$Key`'s own catalog: "`deregistration_delay.timeout_seconds` -
/// The amount of time, in seconds, for Elastic Load Balancing to wait before changing
/// the state of a deregistering target from `draining` to `unused`."
pub const DEREGISTRATION_DELAY_ATTRIBUTE: &str = "deregistration_delay.timeout_seconds";

/// How long a deregistering registration of `group` drains before it leaves the group:
/// the value [`DEREGISTRATION_DELAY_ATTRIBUTE`] carries for it — what a caller wrote,
/// else the documented default the attribute is carried at.
///
/// A DURATION the caller configures, so it keeps the value the attribute reports rather
/// than being shortened to the span this emulator chooses for a transition of its own:
/// the whole point of the attribute is that a client can ask for the drain it needs and
/// then observe exactly it. A target group the attribute does not apply to — "if the
/// target is a Lambda function, this attribute is not supported" — drains for no time at
/// all, so such a deregistration takes effect at once.
///
/// BOUNDED by the attribute's own documented range
/// (`super::AttributeDomain::integer_ceiling`), which is the range a write of it is refused
/// outside: the span is arithmetic on the clock, so it is held to what the collection
/// could legitimately carry rather than to whatever a number parses to. A domain stating
/// no range bounds nothing and the configured value stands as it is — the arithmetic on it
/// is checked either way ([`due_at`]).
pub fn deregistration_delay(group: &TargetGroupRecord) -> Duration {
    let Some(spec) = target_group_attribute(DEREGISTRATION_DELAY_ATTRIBUTE) else {
        return Duration::ZERO;
    };
    if !spec.applies_to(group) {
        return Duration::ZERO;
    }
    let ceiling = spec
        .domain
        .integer_ceiling()
        .and_then(|ceiling| u64::try_from(ceiling).ok());
    group
        .attributes
        .get(DEREGISTRATION_DELAY_ATTRIBUTE)
        .cloned()
        .or_else(|| spec.default_for(group))
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(Duration::ZERO, |seconds| {
            Duration::from_secs(ceiling.map_or(seconds, |ceiling| seconds.min(ceiling)))
        })
}

/// The instant a span that starts `now` comes due.
///
/// The addition is CHECKED: a span the clock cannot represent an end for is due at once,
/// so no arithmetic on a caller-configured duration can end a request in a panic.
fn due_at(now: SystemTime, span: Duration) -> SystemTime {
    now.checked_add(span).unwrap_or(now)
}

/// The `TargetHealthStateEnum` values this machine reports, and the
/// `TargetHealthReasonEnum` reason and description each is reported with — the model's
/// own words for the condition, so one state cannot be worded two ways.
const INITIAL: (&str, &str, &str) = (
    "initial",
    "Elb.RegistrationInProgress",
    "Target registration is in progress",
);
const DRAINING: (&str, &str, &str) = (
    "draining",
    "Target.DeregistrationInProgress",
    "Target deregistration is in progress",
);
const UNUSED_NOT_IN_USE: (&str, &str, &str) = (
    "unused",
    "Target.NotInUse",
    "Target group is not configured to receive traffic from the load balancer",
);
const UNUSED_NOT_REGISTERED: (&str, &str, &str) = (
    "unused",
    "Target.NotRegistered",
    "Target is not registered to the target group",
);
const UNAVAILABLE: (&str, &str, &str) = (
    "unavailable",
    "Target.HealthCheckDisabled",
    "Health checks are not enabled for this target group",
);
const UNHEALTHY_TIMED_OUT: (&str, &str, &str) =
    (UNHEALTHY_STATE, "Target.Timeout", "Request timed out");
/// The other `initial` the model names, for a registration whose checks have STARTED and
/// have not decided yet: "`Elb.InitialHealthChecking` - The load balancer is still sending
/// the target the minimum number of health checks required to determine its health status"
/// (`TargetHealth$Reason`).
const INITIAL_HEALTH_CHECKING: (&str, &str, &str) = (
    "initial",
    "Elb.InitialHealthChecking",
    "Health checks in progress",
);

/// The `TargetHealthStateEnum` value of a registration the load balancer can route to.
///
/// `TargetHealth$State`'s own documentation gives it no reason code ("a reason code is not
/// provided") and no description, so a healthy registration reports the state alone.
pub const HEALTHY_STATE: &str = "healthy";

/// The `TargetHealthStateEnum` value of a registration whose health checks the load balancer
/// could not get an answer to.
pub const UNHEALTHY_STATE: &str = "unhealthy";

/// Where the periodic health check of ONE registration has got to: what the checks have
/// observed in a row, and the verdict those runs have reached.
///
/// `UnhealthyThresholdCount` is "the number of consecutive health check failures required
/// before considering a target unhealthy" and `HealthyThresholdCount` "the number of
/// consecutive health check successes required before considering an unhealthy target
/// healthy", so the verdict only MOVES once that many consecutive checks disagree with it —
/// which is what [`TargetProbe::observed`] folds an observation into. A registration no check
/// has observed anything about carries the verdict it started with, which is the one the
/// clock owes it ([`TargetGroupRecord::health_of`]).
#[derive(Debug, Clone, Default)]
pub struct TargetProbe {
    /// Consecutive failing checks since the last passing one.
    failures: u32,
    /// Consecutive passing checks since the last failing one.
    passes: u32,
    /// What the runs have DECIDED: `Some(true)` once as many consecutive checks passed as
    /// the group's `HealthyThresholdCount`, `Some(false)` once as many failed as its
    /// `UnhealthyThresholdCount`, and `None` while neither threshold has been reached.
    verdict: Option<bool>,
}

impl TargetProbe {
    /// Fold one check's observation into the verdict: `observed` is whether the check
    /// passed, and the two thresholds are the target group's own.
    ///
    /// A threshold the group reports as less than one still takes one check, so a verdict is
    /// never reached by no check at all.
    pub fn observed(&mut self, observed: bool, healthy_threshold: i32, unhealthy_threshold: i32) {
        let threshold = |configured: i32| configured.max(1).unsigned_abs();
        if observed {
            self.failures = 0;
            self.passes = self.passes.saturating_add(1);
            if self.passes >= threshold(healthy_threshold) {
                self.verdict = Some(true);
            }
        } else {
            self.passes = 0;
            self.failures = self.failures.saturating_add(1);
            if self.failures >= threshold(unhealthy_threshold) {
                self.verdict = Some(false);
            }
        }
    }

    /// Whether the checks have decided this registration is unhealthy.
    pub fn is_unhealthy(&self) -> bool {
        self.verdict == Some(false)
    }

    /// Whether the checks have FAILED without reaching a verdict yet: at least one check
    /// since the last passing one found nothing answering, and not as many consecutive ones
    /// as the group's `UnhealthyThresholdCount` takes.
    ///
    /// Which is the `initial` the model words as "the load balancer is still sending the
    /// target the minimum number of health checks required to determine its health status":
    /// nothing has found this registration healthy, so it is not reported healthy on its way
    /// to the unhealthy verdict. A registration whose checks are passing, and one no check
    /// observed anything about, are not this: neither is contradicted by a failing check, so
    /// both keep the health the clock owes them. A registration that already HAS a verdict
    /// keeps it until as many consecutive checks disagree with it as its threshold takes,
    /// which is what [`Self::observed`] folds.
    pub fn is_failing(&self) -> bool {
        self.verdict.is_none() && self.failures > 0
    }
}

/// The ports the workload behind a registration SERVES, as inclusive ranges of port
/// numbers — one `start..=start` per single port.
///
/// What a health check of that registration can get an answer on: a check CONNECTS to the
/// effective health-check port ([`TargetGroupRecord::health_check_port_of`]), so a port the
/// workload does not serve is a port nothing answers on, however healthy the workload
/// itself is. An EMPTY list declares nothing: the registration of a bare address is one this
/// service knows no ports for, and a check of it observes what it observes about the address
/// alone.
///
/// SHARED, not copied: one workload's declared ports are the same list for every
/// registration that stands for it, so registering it in several target groups costs one
/// allocation and cloning a registration costs none.
pub type ServedPorts = Arc<[RangeInclusive<i32>]>;

/// One reported health, from a `(state, reason, description)` triple above.
fn reported(state: (&str, &str, &str)) -> TargetHealth {
    TargetHealth {
        state: Some(state.0.to_string()),
        reason: Some(state.1.to_string()),
        description: Some(state.2.to_string()),
    }
}

impl RegisteredTarget {
    /// Whether the workload behind this registration answers on `port` \u2014 `None` when
    /// nothing declares which ports it serves ([`ServedPorts`]), which is no answer either
    /// way rather than a port it does not serve.
    ///
    /// The one reading of [`RegisteredTarget::serves_ports`], so the health check and any
    /// later reader of a registration's ports agree on what a declared set means.
    pub fn serves(&self, port: i32) -> Option<bool> {
        if self.serves_ports.is_empty() {
            return None;
        }
        Some(self.serves_ports.iter().any(|range| range.contains(&port)))
    }
}

impl TargetGroupRecord {
    /// Health checking on this group's registrations BEGINS now: every registration it
    /// already holds owes its move out of `initial` a settling span from this instant.
    ///
    /// Called where a target group becomes one a load balancer forwards to, which is what
    /// starts the health checks: before that nothing is checking these targets, so their
    /// reported state is `unused` rather than a health, and the `initial` their first
    /// check runs under has not started. A registration that is DRAINING is not checked
    /// again — it is leaving the group — so its span is left alone.
    pub fn begin_health_checking(&mut self, now: SystemTime) {
        for target in self.targets.values_mut() {
            if target.draining_until.is_none() {
                target.healthy_at = due_at(now, SETTLE_AFTER);
            }
        }
    }

    /// Deregister the registration `key` names: the state transition
    /// `DeregisterTargets` performs.
    ///
    /// A registration a load balancer is sending traffic to enters `draining` and stays in
    /// the group until [`deregistration_delay`] has passed — "the amount of time ... to
    /// wait before changing the state of a deregistering target from `draining` to
    /// `unused`" — which is the contract a client that drains connections before tearing
    /// a target down waits on. One nothing is sending traffic to (a target group no load
    /// balancer forwards to, or a drain configured to take no time) has no connections to
    /// drain, so it leaves the group at once. A deregistration asked for twice keeps the
    /// instant the first one set: the drain does not restart.
    pub fn deregister(&mut self, key: &TargetKey, now: SystemTime) {
        let drains_for = if self.load_balancer_arns.is_empty() {
            Duration::ZERO
        } else {
            deregistration_delay(self)
        };
        if drains_for.is_zero() {
            self.targets.remove(key);
            return;
        }
        if let Some(target) = self.targets.get_mut(key) {
            if target.draining_until.is_none() {
                target.draining_until = Some(due_at(now, drains_for));
            }
        }
    }

    /// The registrations of this group that another emulated service's SCHEDULER owns,
    /// each as the key it is stored under and the workload it stands for
    /// ([`RegisteredTarget::registered_for`]).
    ///
    /// What a scheduler reconciling its workload's registrations may act on: a
    /// registration a `RegisterTargets` caller made is absent from this, so a reconcile
    /// cannot take a caller's own target out of the group. It answers EVERY scheduler's,
    /// because this service cannot know whose a workload is — the caller filters the answer
    /// to the workloads it owns, so two schedulers sharing one group leave each other's
    /// registrations alone.
    pub fn scheduler_registrations(&self) -> Vec<(TargetKey, String)> {
        self.targets
            .iter()
            .filter_map(|(key, target)| {
                target
                    .registered_for
                    .as_ref()
                    .map(|workload| (key.clone(), workload.clone()))
            })
            .collect()
    }

    /// Register the target `id` (on `port`, else this group's own) for `workload`, on
    /// behalf of the emulated service whose scheduler owns that workload — the Amazon ECS
    /// service registering a task its `loadBalancers` bind.
    ///
    /// The registration is this group's own in every respect a reader sees: it is keyed and
    /// health-checked exactly as `RegisterTargets` keys and checks one, so it begins
    /// `initial` and is reported whatever its checks then determine
    /// ([`TargetGroupRecord::health_of`]). It carries the workload it stands for, which is
    /// what lets the scheduler that added it take it out again.
    ///
    /// `serves_ports` is the ports that workload LISTENS on, as the scheduler that runs it
    /// declares them ([`ServedPorts`]): a check of this registration gets an answer only on
    /// a port among them, so a health check the target group points at any other port is
    /// answered by nothing. A scheduler that declares none leaves the ports unknown.
    ///
    /// Nothing is replaced: a key this group already holds — whether a caller registered it
    /// or the same workload did — is left exactly as it is, so a reconcile does not restart
    /// a drain or a health check. A group already holding its quota of targets
    /// ([`targets_per_target_group`]) takes no further registration, the same bound
    /// `RegisterTargets` refuses past.
    ///
    /// Answers whether this call added a registration.
    pub fn register_for(
        &mut self,
        id: &str,
        port: Option<i32>,
        availability_zone: Option<String>,
        workload: &str,
        serves_ports: ServedPorts,
        now: SystemTime,
    ) -> bool {
        let key: TargetKey = (id.to_string(), port.or(self.port));
        if self.targets.contains_key(&key) {
            return false;
        }
        if self.targets.len() >= targets_per_target_group(self) {
            return false;
        }
        self.targets.insert(
            key.clone(),
            RegisteredTarget {
                id: key.0,
                port: key.1,
                availability_zone,
                registered_at: now,
                healthy_at: now + SETTLE_AFTER,
                draining_until: None,
                probe: TargetProbe::default(),
                registered_for: Some(workload.to_string()),
                serves_ports,
            },
        );
        true
    }

    /// Materialise what the clock already owes this group: a registration whose draining
    /// span is over has LEFT the target group, whether or not anything called in the
    /// meantime.
    ///
    /// Called by the operations that read or write this group's registrations, so the
    /// removal a deregistration already made due is what a later caller sees. It owes the
    /// same removal at the same instant to every caller, so this moves no state a call of
    /// its own would not have moved.
    pub fn settle_draining(&mut self, now: SystemTime) {
        self.targets
            .retain(|_, target| target.draining_until.is_none_or(|due| now < due));
    }

    /// The health of one registered target, derived from what is stored plus the clock:
    /// a deregistering target is draining, a target group no load balancer forwards to
    /// has nothing checking its targets, a group whose health checks are off reports them
    /// unavailable, and a registration whose checking has not settled yet is still coming
    /// up.
    pub fn health_of(&self, target: &RegisteredTarget, now: SystemTime) -> TargetHealth {
        if let Some(due) = target.draining_until {
            // The drain the caller asked for, until the instant it is over — past which
            // the registration has left the group ([`Self::settle_draining`]) and is no
            // registration of this group at all.
            return reported(if now < due { DRAINING } else { UNUSED_NOT_REGISTERED });
        }
        if self.load_balancer_arns.is_empty() {
            return reported(UNUSED_NOT_IN_USE);
        }
        if !self.health_check.enabled {
            return reported(UNAVAILABLE);
        }
        if now < target.healthy_at {
            return reported(INITIAL);
        }
        if target.probe.is_unhealthy() {
            // The checks have run and this registration answered none of them: the load
            // balancer considers a target unhealthy once the group's threshold of
            // consecutive failures is reached, and `TargetHealth$Reason` words that for a
            // check nothing answered.
            return reported(UNHEALTHY_TIMED_OUT);
        }
        if target.probe.is_failing() {
            // A check has failed and the group's threshold of consecutive failures has not
            // been reached yet, which is the state `TargetHealth$Reason` words as the
            // minimum number of checks still being sent: nothing has found this
            // registration healthy, so it is not reported healthy on the way to its verdict.
            return reported(INITIAL_HEALTH_CHECKING);
        }
        TargetHealth {
            state: Some(HEALTHY_STATE.to_string()),
            reason: None,
            description: None,
        }
    }

    /// The description of a target that is not registered with this group.
    ///
    /// It carries the same `HealthCheckPort` a registered target's does
    /// (`TargetGroupRecord::health_check_port_for`): the port is the group's health-check
    /// configuration read against the target the request named, which an unregistered
    /// target has as much as a registered one.
    pub fn unregistered_health_description(
        &self,
        target: &TargetDescription,
    ) -> TargetHealthDescription {
        TargetHealthDescription {
            target: Some(target.clone()),
            health_check_port: self.health_check_port_for(target.port),
            target_health: Some(reported(UNUSED_NOT_REGISTERED)),
            anomaly_detection: None,
            administrative_override: None,
        }
    }

    /// The health description of one registered target.
    pub fn health_description_of(
        &self,
        target: &RegisteredTarget,
        now: SystemTime,
    ) -> TargetHealthDescription {
        TargetHealthDescription {
            target: Some(TargetDescription {
                id: target.id.clone(),
                port: target.port,
                availability_zone: target.availability_zone.clone(),
                quic_server_id: None,
            }),
            health_check_port: self.health_check_port_of(target),
            target_health: Some(self.health_of(target, now)),
            anomaly_detection: None,
            administrative_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target group carrying `configured` as its deregistration delay.
    fn group_with(configured: Option<&str>) -> TargetGroupRecord {
        let mut group = TargetGroupRecord {
            protocol: Some("HTTP".to_string()),
            target_type: "instance".to_string(),
            load_balancer_arns: vec!["an-attached-load-balancer".to_string()],
            ..TargetGroupRecord::default()
        };
        if let Some(configured) = configured {
            group.attributes.insert(
                DEREGISTRATION_DELAY_ATTRIBUTE.to_string(),
                configured.to_string(),
            );
        }
        group
    }

    /// The drain is the configured value, bounded by the attribute's documented range —
    /// so a figure no write of the attribute could have stored cannot become a span, and
    /// no value makes the clock arithmetic leave what it can represent.
    #[test]
    fn the_drain_is_the_configured_value_bounded_by_the_attributes_own_range() {
        let ceiling = u64::try_from(
            target_group_attribute(DEREGISTRATION_DELAY_ATTRIBUTE)
                .and_then(|spec| spec.domain.integer_ceiling())
                .unwrap_or_default(),
        )
        .unwrap_or_default();

        for (case, configured, expected) in [
            ("nothing configured", None, Some(Duration::from_secs(300))),
            ("no delay", Some("0"), Some(Duration::ZERO)),
            ("one second", Some("1"), Some(Duration::from_secs(1))),
            ("the range's ceiling", Some("3600"), None),
            ("one past the ceiling", Some("3601"), None),
            ("the largest figure a u64 carries", Some("18446744073709551615"), None),
            ("a figure no number parses", Some("not-a-number"), Some(Duration::ZERO)),
        ] {
            let expected = expected.unwrap_or(Duration::from_secs(ceiling));
            let group = group_with(configured);
            let delay = deregistration_delay(&group);
            assert_eq!(
                delay, expected,
                "case {case:?}: expected {expected:?}, got {delay:?}"
            );
            // Whatever the figure, the instant the drain comes due is representable.
            let now = SystemTime::now();
            assert!(
                due_at(now, delay) >= now,
                "case {case:?}: the due instant must be representable, got {:?}",
                due_at(now, delay)
            );
        }
    }

    /// A target group the attribute does not apply to drains for no time at all.
    #[test]
    fn a_lambda_target_group_drains_for_no_time() {
        let mut group = group_with(Some("3600"));
        group.target_type = "lambda".to_string();
        group.protocol = None;
        let delay = deregistration_delay(&group);
        assert_eq!(
            delay,
            Duration::ZERO,
            "case a lambda target group: the attribute is not supported for it, got {delay:?}"
        );
    }
}
