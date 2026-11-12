// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch alarm gate a deployment in flight runs under.
//!
//! A deployment or deployment group carries an `AlarmConfiguration` —
//! "information about alarms associated with a deployment or deployment group"
//! (`smithy_query(query="shape",shape="AlarmConfiguration")`) — and CodeDeploy
//! POLLS those alarms while the deployment is under way: a deployment one of them
//! is sounding for is not allowed to reach traffic, so it is stopped (the
//! `Stopped` status, with the `ALARM_ACTIVE` error code naming the alarms) rather
//! than completed. That is the whole point of the configuration, so this module
//! is the one place the stored configuration is EVALUATED, for every compute
//! platform and every deployment style alike.
//!
//! Which configuration a deployment runs under: the one the deployment itself
//! carries, else its deployment group's — "the alarm configuration that you
//! specify here will override the alarm configuration at the deployment group
//! level"
//! (`smithy_query(query="doc",shape="CreateDeploymentInput",member="overrideAlarmConfiguration")`).
//! The automatic rollback a stop then performs is read the same way: the
//! deployment's own configuration, else its group's, and a rollback happens only
//! when that configuration is enabled for the `DEPLOYMENT_STOP_ON_ALARM` event
//! (`smithy_query(query="shape",shape="AutoRollbackEvent")`).
//!
//! The alarms belong to CloudWatch, so their state is read out of the CloudWatch
//! emulator's OWN alarm store through the shared engine — the same store
//! `PutMetricAlarm` writes, `SetAlarmState` forces and `DescribeAlarms` reads.
//! Nothing about an alarm is copied into this service's store, so one alarm has
//! one home and a deployment is never gated on a state the alarm no longer holds.
//! An alarm whose state cannot be read there at all — one the account does not
//! hold, or a CloudWatch that is not mounted beside this service — is the
//! "status information can't be retrieved" case the configuration's own
//! `ignorePollAlarmFailure` decides:
//! "<code>false</code>: The deployment stops if alarm status information can't be
//! retrieved from Amazon CloudWatch"
//! (`smithy_query(query="shape",shape="AlarmConfiguration")`).
//!
//! WHEN the gate is applied: [`poll_alarms`] runs before every handler reads the
//! store ([`super::with_state`]) and before the clock settles a deployment, so a
//! deployment the alarms had already stopped can never be observed having
//! succeeded instead. The stop is a transition the CLOCK owed — the alarm was
//! sounding at that instant whether or not anybody called — so applying it from a
//! reporting operation cannot show a caller anything a later caller would not
//! also see. A deployment is gated only from the instant it STARTS
//! ([`super::deployment_start_due`]), which is why `CreateDeployment` is answered
//! with the acceptance it would be answered with anyway and the stop follows out
//! of band.

use std::collections::{BTreeSet, HashMap};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use crate::ops::types::{AlarmConfiguration, ErrorInformation};

use super::{
    deployment_is_terminal, now_ms, rollback, service_state,
    ComAmazonawsCodedeployCodedeploy20141006State, Deployment, DeploymentGroup,
    DEPLOYMENT_CREATOR_ROLLBACK, DEPLOYMENT_STATUS_IN_PROGRESS, DEPLOYMENT_STATUS_STOPPED,
};

/// The `ErrorCode` CodeDeploy reports on a deployment its alarms stopped
/// (`smithy_query(query="shape",shape="ErrorCode")` carries it).
pub const ERROR_CODE_ALARM_ACTIVE: &str = "ALARM_ACTIVE";

/// The `AutoRollbackEvent` that asks for a rollback of a deployment an alarm
/// stopped (`smithy_query(query="shape",shape="AutoRollbackEvent")`:
/// DEPLOYMENT_FAILURE | DEPLOYMENT_STOP_ON_ALARM | DEPLOYMENT_STOP_ON_REQUEST).
pub const AUTO_ROLLBACK_EVENT_STOP_ON_ALARM: &str = "DEPLOYMENT_STOP_ON_ALARM";

/// What one in-flight deployment's gate polls, resolved from the store once so
/// the CloudWatch read needs nothing of this service's own state.
#[derive(Debug, Clone)]
struct Gate {
    /// The deployment the gate stops.
    deployment_id: String,
    /// The alarms it polls, in the order its configuration lists them, with
    /// duplicates and blank names dropped.
    alarms: Vec<String>,
    /// Whether an alarm whose state cannot be retrieved leaves the deployment
    /// running; see this module's documentation.
    ignore_poll_alarm_failure: bool,
    /// Whether a stop also creates the rollback deployment, which is what the
    /// `DEPLOYMENT_STOP_ON_ALARM` event asks for.
    rolls_back: bool,
}

/// Why a gate stopped its deployment, carrying the alarms it stopped it for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    /// One or more of the polled alarms are in the `ALARM` state.
    Activated(Vec<String>),
    /// No polled alarm is sounding, but the state of one or more of them could
    /// not be retrieved and the configuration did not ask for that to be
    /// ignored.
    Unpollable(Vec<String>),
}

impl Stop {
    /// The sentence `errorInformation.message` carries for this stop, naming the
    /// alarms the deployment was stopped for.
    fn message(&self) -> String {
        match self {
            Self::Activated(alarms) => format!(
                "One or more alarms have been activated according to the Amazon CloudWatch \
                 metrics you selected, and the affected deployments have been stopped. \
                 Activated alarms: {alarms}",
                alarms = alarms.join(", ")
            ),
            Self::Unpollable(alarms) => format!(
                "The deployment was stopped because the state of the following alarms could \
                 not be retrieved from Amazon CloudWatch: {alarms}",
                alarms = alarms.join(", ")
            ),
        }
    }
}

/// Stop every deployment in flight that the alarms it polls have stopped.
///
/// Runs with THIS service's store unlocked between phases: the deployments to
/// poll for are read under this store's lock, the alarm states under CloudWatch's,
/// and the stops written under this one again — the two stores are never held at
/// once. A store holding no deployment with an armed gate costs one read and no
/// CloudWatch lock at all.
pub fn poll_alarms<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = now_ms();
    let armed = {
        let store = state.lock();
        armed_gates(&store, now)
    };
    if armed.is_empty() {
        return;
    }
    let polled = alarm_states(engine, &armed);
    let stopped: Vec<(Gate, Stop)> = armed
        .into_iter()
        .filter_map(|gate| gate.verdict(&polled).map(|stop| (gate, stop)))
        .collect();
    if stopped.is_empty() {
        return;
    }
    let mut store = state.lock();
    for (gate, stop) in &stopped {
        stop_deployment(&mut store, gate, stop, now);
    }
}

/// The gate of every deployment that is in flight at `now_ms` and polls at least
/// one alarm.
fn armed_gates(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    now_ms: i64,
) -> Vec<Gate> {
    state
        .pending_deployments
        .iter()
        .filter_map(|id| state.deployments.get(id))
        .filter(|deployment| gate_is_armed(deployment, now_ms))
        .filter_map(|deployment| gate_of(state, deployment))
        .collect()
}

/// Whether `deployment` is one the gate applies to at `now_ms`.
///
/// A deployment that has reached an outcome is owed nothing further, and one the
/// clock has not yet started has not reached any traffic to be stopped from
/// reaching. A deployment the SERVICE created to roll another one back is
/// CodeDeploy's own remediation for the stop that created it — gating it on the
/// same alarms would leave the group stuck on the revision the alarms are
/// sounding about — so it runs ungated, and a rollback therefore never rolls
/// back a rollback.
fn gate_is_armed(deployment: &Deployment, now_ms: i64) -> bool {
    deployment.complete_time_ms.is_none()
        && !deployment_is_terminal(&deployment.status)
        && deployment.starts_at_ms <= now_ms
        && deployment.creator != DEPLOYMENT_CREATOR_ROLLBACK
}

/// The gate `deployment` runs under, or `None` when it polls no alarm: a
/// deployment whose effective configuration is absent, is not enabled, or names
/// no alarm has nothing to poll, and nothing can stop it.
fn gate_of(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
) -> Option<Gate> {
    let configuration = effective_alarm_configuration(state, deployment)?;
    if !configuration.enabled {
        return None;
    }
    let mut seen = BTreeSet::new();
    let alarms: Vec<String> = configuration
        .alarms
        .iter()
        .flatten()
        .filter_map(|alarm| alarm.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| seen.insert(name.to_string()))
        .map(str::to_string)
        .collect();
    if alarms.is_empty() {
        return None;
    }
    Some(Gate {
        deployment_id: deployment.deployment_id.clone(),
        alarms,
        ignore_poll_alarm_failure: configuration.ignore_poll_alarm_failure,
        rolls_back: rolls_back_on_alarm(state, deployment),
    })
}

/// The deployment group `deployment` was created against, when the store still
/// holds it.
fn group_of<'a>(
    state: &'a ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
) -> Option<&'a DeploymentGroup> {
    rollback::group_of(state, deployment)
}

/// The alarm configuration `deployment` runs under: its own override, else its
/// deployment group's.
fn effective_alarm_configuration<'a>(
    state: &'a ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &'a Deployment,
) -> Option<&'a AlarmConfiguration> {
    deployment.override_alarm_configuration.as_ref().or_else(|| {
        group_of(state, deployment).and_then(|group| group.alarm_configuration.as_ref())
    })
}

/// Whether stopping `deployment` for an alarm also rolls it back: only an enabled
/// configuration that names the `DEPLOYMENT_STOP_ON_ALARM` event asks for that,
/// read through the one place that question is answered ([`rollback::rolls_back_on`]).
fn rolls_back_on_alarm(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
) -> bool {
    rollback::rolls_back_on(state, deployment, AUTO_ROLLBACK_EVENT_STOP_ON_ALARM)
}

/// The state CloudWatch holds for each alarm the armed gates poll; `None` for an
/// alarm whose state could not be retrieved.
///
/// ONE CloudWatch read for every gate, so a request costs one lock of that store
/// rather than one per deployment. Call it with this service's own store guard
/// already released: the two stores are never locked at once.
fn alarm_states<Eng: Send + Sync + 'static>(
    engine: &Eng,
    armed: &[Gate],
) -> HashMap<String, Option<String>> {
    let names: BTreeSet<&str> = armed
        .iter()
        .flat_map(|gate| gate.alarms.iter().map(String::as_str))
        .collect();
    let Some(state) = cloudwatch::state::service_state(engine) else {
        // No CloudWatch beside this service is the case where alarm status
        // information cannot be retrieved at all.
        return names
            .into_iter()
            .map(|name| (name.to_string(), None))
            .collect();
    };
    let store = state.lock();
    names
        .into_iter()
        .map(|name| (name.to_string(), store.alarm_state_value(name)))
        .collect()
}

impl Gate {
    /// Whether the polled states stop this deployment, and what for.
    ///
    /// A sounding alarm stops it whatever `ignorePollAlarmFailure` says — that
    /// member is about state the poll could not RETRIEVE, not about the state it
    /// retrieved — so the activated alarms are looked for first.
    fn verdict(&self, polled: &HashMap<String, Option<String>>) -> Option<Stop> {
        let state_of = |name: &String| polled.get(name).cloned().flatten();
        let named = |predicate: &dyn Fn(&String) -> bool| -> Vec<String> {
            self.alarms
                .iter()
                .filter(|name| predicate(name))
                .cloned()
                .collect()
        };
        let activated = named(&|name| {
            state_of(name).as_deref() == Some(cloudwatch::state::ALARM_STATE_ALARM)
        });
        if !activated.is_empty() {
            return Some(Stop::Activated(activated));
        }
        if self.ignore_poll_alarm_failure {
            return None;
        }
        let unpollable = named(&|name| state_of(name).is_none());
        (!unpollable.is_empty()).then_some(Stop::Unpollable(unpollable))
    }
}

/// Stop the deployment `gate` guards at `now_ms`, with the error information
/// CodeDeploy reports for an alarm, and roll it back when its configuration asks
/// for that.
fn stop_deployment(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    gate: &Gate,
    stop: &Stop,
    now_ms: i64,
) {
    // The store was unlocked while the alarms were read, so what the gate was
    // resolved from is checked again here: a deployment another caller stopped,
    // or the clock completed, in between keeps the outcome it already has.
    if !state
        .deployments
        .get(&gate.deployment_id)
        .is_some_and(|deployment| gate_is_armed(deployment, now_ms))
    {
        return;
    }
    // The start the clock had already owed this deployment, so a deployment
    // stopped before anybody read it still reports the instant it began rather
    // than the instant the stop was materialised.
    if let Some(deployment) = state.deployments.get_mut(&gate.deployment_id) {
        if deployment.start_time_ms.is_none() {
            deployment.start_time_ms = Some(deployment.starts_at_ms);
            deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
        }
    }
    let settled = state.settle_deployment(
        &gate.deployment_id,
        DEPLOYMENT_STATUS_STOPPED,
        Some(ErrorInformation {
            code: Some(ERROR_CODE_ALARM_ACTIVE.to_string()),
            message: Some(stop.message()),
        }),
        now_ms,
    );
    if settled && gate.rolls_back {
        rollback::record_rollback(state, &gate.deployment_id, now_ms);
    }
}
