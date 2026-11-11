// SPDX-License-Identifier: CC0-1.0

//! The DEPLOYMENT CIRCUIT BREAKER: how a rollout whose tasks cannot be launched reaches a
//! TERMINAL rollout state instead of waiting for a steady state that can never arrive.
//!
//! `Deployment$rolloutState` states the whole state machine: "When a service deployment is
//! started, it begins in an `IN_PROGRESS` state. When the service reaches a steady state,
//! the deployment transitions to a `COMPLETED` state. If the service fails to reach a steady
//! state and circuit breaker is turned on, the deployment transitions to a `FAILED` state. A
//! deployment in `FAILED` state doesn't launch any new tasks." So the breaker is what gives
//! a rollout that cannot succeed an END: without one the rollout stays `IN_PROGRESS` and a
//! client polling for a terminal state never converges.
//!
//! What the breaker COUNTS is launches that failed. `Deployment$failedTasks`: "The number of
//! consecutively failed tasks in the deployment. A task is considered a failure if the
//! service scheduler can't launch the task, the task doesn't transition to a `RUNNING`
//! state" — and `DeploymentCircuitBreaker$resetOnHealthyTask`: "a task that reaches a
//! healthy state resets the failure count to `0`", whose "default is `true`". A launch this
//! emulator cannot make is decided against the placement it was made into
//! ([`evaluate_image_pulls`]) and the clock then stops the task it produced
//! ([`TaskRecord::fail_launch`]), so the count is READ OFF the rollout's own task records
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::deployment_launch_failures`])
//! and cannot drift from the tasks that exist.
//!
//! How many failures it tolerates is [`circuit_breaker_threshold`], which is at least
//! [`MIN_CIRCUIT_BREAKER_THRESHOLD`] for the bounded percentage type — more failures than a
//! small service has tasks. The scheduler is what produces the rest: it keeps the service at
//! its desired count, so a task it loses to a failed launch is replaced
//! ([`super::service_steady_state`]) and each replacement is another attempt for the breaker
//! to count. Once the breaker HAS failed the rollout that replacing stops, as the same doc
//! requires: a deployment in `FAILED` state launches no new task.
//!
//! Where the failure leaves the service is the breaker's `rollback`: "If rollback is on,
//! when a service deployment fails, the service is rolled back to the last deployment that
//! completed successfully" — which is a REDEPLOYMENT of that revision, recorded exactly like
//! the rollback a lifecycle-hook action or a `StopServiceDeployment` asks for
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::roll_back_service_deployment_to`]).
//!
//! Every transition here is one the CLOCK owes: the failure falls due at the instant the
//! launch it counted failed, and the rollback one [`TRANSITION_SPAN`] after the failure that
//! triggered it — stored and compared, never slept on, so a caller reading in between sees
//! the rollout `FAILED` before the rollback takes the service back.

use super::*;

/// `Deployment$rolloutStateReason` ("A description of the rollout state of a deployment") of
/// a rollout the deployment circuit breaker failed.
///
/// The whole sentence is fixed text: the breaker fails a rollout for ONE reason — the tasks
/// it launched did not start — so the wording carries no value of the request's.
pub const CIRCUIT_BREAKER_FAILURE_REASON: &str =
    "ECS deployment circuit breaker: tasks failed to start.";

/// `ServiceDeploymentBrief$statusReason` ("Information about why the service deployment is in
/// the current status. For example, the circuit breaker detected a failure") of a rollout the
/// breaker ROLLED BACK, and the `Rollback$reason` that rollback carries ("The reason the
/// rollback happened. For example, the circuit breaker initiated the rollback operation").
/// Fixed text, naming no value of the request's.
pub const CIRCUIT_BREAKER_ROLLED_BACK_REASON: &str =
    "Service deployment rolled back because the circuit breaker threshold was exceeded.";

/// The same for a rollout the breaker failed with no rollback configured, which ends where it
/// stands.
pub const CIRCUIT_BREAKER_STOPPED_REASON: &str =
    "Service deployment stopped because the circuit breaker threshold was exceeded.";

/// What the circuit breaker has counted for ONE rollout, read off the tasks that rollout
/// placed.
#[derive(Debug, Clone, Default)]
pub struct LaunchFailures {
    /// `ServiceDeploymentCircuitBreaker$failureCount`: "The number of times the circuit
    /// breaker detected a service deployment failure", which is also the deployment's own
    /// `failedTasks`.
    pub count: i32,
    /// The instant the LAST of them failed — the instant everything the breaker then owes
    /// falls due at, so nothing is dated by whoever happened to call.
    pub latest: Option<SystemTime>,
}

/// Whether the breaker `configuration` turns on resets its failure count when a task of the
/// rollout becomes healthy — `DeploymentCircuitBreaker$resetOnHealthyTask`: "When set to
/// `true`, a task that reaches a healthy state resets the failure count to `0`. ... The
/// default is `true`."
pub fn circuit_breaker_resets_on_healthy_task(configuration: &DeploymentConfiguration) -> bool {
    configuration
        .deployment_circuit_breaker
        .as_ref()
        .and_then(|breaker| breaker.reset_on_healthy_task)
        .unwrap_or(CIRCUIT_BREAKER_RESETS_ON_HEALTHY_TASK)
}

/// The service event the scheduler writes when the circuit breaker fails a rollout, in the
/// wording the Amazon ECS service event messages use for a deployment whose tasks did not
/// start — the counterpart of the "deployment completed." entry a rollout that reached its
/// steady state earns.
fn deployment_failed_event(service_name: &str, deployment_id: &str) -> String {
    format!(
        "(service {service_name}) (deployment {deployment_id}) deployment failed: tasks \
         failed to start."
    )
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Materialise what the DEPLOYMENT CIRCUIT BREAKER owes, as of `now`: the `FAILED` a
    /// watched rollout earns once the launches it lost reach the breaker's threshold, the
    /// replacement launches the scheduler owes it before then, and the rollback that failure
    /// starts.
    ///
    /// Each is owed at the instant a launch of the rollout failed, whether or not anybody
    /// called, so a reporting operation may run them — and each happens at most once,
    /// because the rollout leaves the state that owed it.
    pub(super) fn advance_deployment_circuit_breakers(&mut self, now: SystemTime) {
        let keys: Vec<(String, String)> = self.services.keys().cloned().collect();
        for key in keys {
            self.advance_service_circuit_breaker(&key, now);
        }
    }

    /// The same, for ONE service.
    fn advance_service_circuit_breaker(&mut self, key: &(String, String), now: SystemTime) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        // Only the ECS scheduler's own rolling update has a deployment the breaker can fail
        // ("The deployment circuit breaker can only be used for services using the rolling
        // update (ECS) deployment type"), and a service that is being deleted is on its way
        // out rather than rolling anything out.
        if !service.is_active() || !service.places_tasks() {
            return;
        }
        // The rollback the breaker started on a rollout it already failed: it falls due one
        // span after that failure, so the failed rollout is observable before the service is
        // taken back to the revision it is rolled back to.
        if let Some(due) = service
            .deployment_rollback_due_at
            .filter(|due| *due <= now)
        {
            self.roll_back_failed_rollout(key, due);
            return;
        }
        // "A deployment in FAILED state doesn't launch any new tasks": a rollout the breaker
        // has already failed is counted no further and replaces nothing.
        if service.deployment_rollout_failure.is_some() {
            return;
        }
        // And a rollout that REACHED its steady state is past the breaker altogether: the
        // breaker is what decides a service that "fails to reach a steady state", and
        // `Deployment$rolloutState` makes the state it reached instead terminal ("When the
        // service reaches a steady state, the deployment transitions to a COMPLETED state"),
        // so a launch this service loses afterwards is not that rollout's failure.
        if service.deployment_rollout_completed {
            return;
        }
        let Some(breaker) = service
            .deployment_configuration
            .deployment_circuit_breaker
            .clone()
            .filter(|breaker| breaker.enable)
        else {
            return;
        };
        let threshold = circuit_breaker_threshold(Some(&breaker), service.desired_count);
        let failures = self.deployment_launch_failures(
            service,
            &service.deployment_id,
            circuit_breaker_resets_on_healthy_task(&service.deployment_configuration),
        );
        // Nothing is owed on a rollout that has lost no launch: the breaker watches, and
        // there is nothing yet to watch. The launches it tolerates are replaced by the
        // scheduler's own steady-state pass ([`super::service_steady_state`]), and each
        // replacement is another launch for the breaker to count.
        let Some(latest) = failures.latest else {
            return;
        };
        if failures.count >= threshold {
            self.fail_rollout(key, latest, breaker.rollback);
        }
    }

    /// The launches of the rollout `deployment_id` that FAILED, as the circuit breaker counts
    /// them: the tasks it placed that Amazon ECS could not launch at all
    /// ([`TaskRecord::failed_to_launch`]), counted since the last task of that rollout
    /// reached `RUNNING` when the breaker resets on a healthy task.
    ///
    /// Derived from the task records, so the count a response reports is the count the
    /// breaker acts on.
    pub(super) fn deployment_launch_failures(
        &self,
        service: &ServiceRecord,
        deployment_id: &str,
        resets_on_healthy_task: bool,
    ) -> LaunchFailures {
        Self::launch_failures_of(
            &self.service_tasks(service),
            deployment_id,
            resets_on_healthy_task,
        )
    }

    /// The same, over a service's tasks that have ALREADY been collected — the form every
    /// reported deployment of one service shares, so describing a service costs one pass over
    /// the task map rather than one per deployment.
    pub(super) fn launch_failures_of(
        tasks: &[&TaskRecord],
        deployment_id: &str,
        resets_on_healthy_task: bool,
    ) -> LaunchFailures {
        let placed: Vec<&TaskRecord> = tasks
            .iter()
            .copied()
            .filter(|task| task.started_by.as_deref() == Some(deployment_id))
            .collect();
        let healthy_at = resets_on_healthy_task
            .then(|| {
                placed
                    .iter()
                    .filter_map(|task| task.started_at)
                    .filter_map(|at| SystemTime::try_from(at).ok())
                    .max()
            })
            .flatten();
        let mut failures: Vec<SystemTime> = placed
            .iter()
            .filter(|task| task.failed_to_launch())
            .filter_map(|task| task.launch_failed_at())
            .filter(|at| healthy_at.is_none_or(|healthy| *at > healthy))
            .collect();
        failures.sort_unstable();
        LaunchFailures {
            count: i32::try_from(failures.len()).unwrap_or(i32::MAX),
            latest: failures.last().copied(),
        }
    }

    /// Fail the rollout `key`'s service is on, as of `at`: the breaker counted as many failed
    /// launches as it tolerates, so "the deployment transitions to a `FAILED` state" with the
    /// reason that names the breaker, and the rollback it configured is owed one span later.
    ///
    /// The rollout reaches no steady state and no stability confirmation after this: the
    /// instants both were owed at are dropped, so nothing can settle a rollout that has
    /// failed.
    fn fail_rollout(&mut self, key: &(String, String), at: SystemTime, rollback: bool) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        let service_name = service.name.clone();
        let deployment_id = service.deployment_id.clone();
        let deployment_arn = self
            .current_service_deployment(service)
            .map(|deployment| deployment.arn.clone());
        let event =
            self.mint_service_event(at, deployment_failed_event(&service_name, &deployment_id));
        let Some(service) = self.services.get_mut(key) else {
            return;
        };
        service.deployment_rollout_failure = Some(CIRCUIT_BREAKER_FAILURE_REASON.to_string());
        service.deployment_steady_due_at = None;
        service.deployment_stable_due_at = None;
        service.deployment_rollback_due_at = rollback.then(|| at + TRANSITION_SPAN);
        service.record_events([event]);
        let Some(record) = deployment_arn.and_then(|arn| self.service_deployments.get_mut(&arn))
        else {
            return;
        };
        // A rollout a caller already asked to stop ends where that stop takes it: the
        // operator asked first, so the breaker records the failed rollout state and leaves
        // the rollout's own record to the stop.
        if record.is_finished() || record.stop_requested() {
            return;
        }
        if rollback {
            // The rollback has been asked for — by the breaker — and has not begun yet, so
            // the status reason is what the breaker detected.
            record.status = SERVICE_DEPLOYMENT_ROLLBACK_REQUESTED.to_string();
            record.status_reason = CIRCUIT_BREAKER_FAILURE_REASON.to_string();
            record.circuit_breaker_triggered = true;
        } else {
            // "If it is turned on, a service deployment will transition to a failed state and
            // stop launching new tasks": with no rollback configured the rollout ends where
            // it stands, which is the end a rollout that stopped without reaching its steady
            // state is recorded under.
            record.circuit_breaker_triggered = true;
            finish_deployment(
                record,
                DateTime::from(at),
                SERVICE_DEPLOYMENT_STOPPED,
                CIRCUIT_BREAKER_STOPPED_REASON,
            );
        }
    }

    /// Carry out the rollback the breaker started on the rollout `key`'s service failed, as
    /// of `at`: "the service is rolled back to the last deployment that completed
    /// successfully".
    ///
    /// Where there is nothing to put back — a service whose first rollout failed, or one
    /// whose completed revision is no longer there to run again — nothing is rolled back and
    /// nobody is left to refuse it to, so the rollout ends
    /// [`SERVICE_DEPLOYMENT_ROLLBACK_FAILED`] and the service keeps the failed rollout,
    /// exactly as a rollback a caller asked for on a first rollout does.
    fn roll_back_failed_rollout(&mut self, key: &(String, String), at: SystemTime) {
        let Some(service) = self.services.get(key) else {
            return;
        };
        let record = self.current_service_deployment(service).cloned();
        if let Some(record) = record.filter(|record| {
            // The stop a caller asked for takes the rollout where IT leaves the service, so
            // a rollout that is already ending under one is not rolled back twice.
            !record.is_finished() && !record.stop_requested() && !record.is_stopping()
        }) {
            let source = self.last_completed_service_deployment(&record).cloned();
            // A service with no completed rollout to put back, and one whose revision is no
            // longer there to run again, are the same case: there is nothing to roll back to.
            let rolled_back = match source {
                Some(source) => self.roll_back_service_deployment_to(
                    &record,
                    &source,
                    at,
                    CIRCUIT_BREAKER_ROLLED_BACK_REASON,
                ),
                None => Err(LifecycleHookRefusal::NoPreviousRevision),
            };
            if let Some(stored) = self.service_deployments.get_mut(&record.arn) {
                match rolled_back {
                    // The rollback the BREAKER performed is reported as the breaker's: the
                    // sentence the rolled-back rollout carries is the one its `rollback`
                    // carries, and the breaker that fired is the one that watched it.
                    Ok(()) => {
                        stored.status_reason = CIRCUIT_BREAKER_ROLLED_BACK_REASON.to_string();
                        stored.circuit_breaker_triggered = true;
                    }
                    // Nothing was put back, and nobody is left to refuse the rollback to, so
                    // the rollout ends where it stands rather than reporting a rollback that
                    // did not happen.
                    Err(_) => {
                        let reason = no_previous_revision_reason(&stored.service_arn);
                        stored.circuit_breaker_triggered = true;
                        finish_deployment(
                            stored,
                            DateTime::from(at),
                            SERVICE_DEPLOYMENT_ROLLBACK_FAILED,
                            &reason,
                        );
                    }
                }
            }
        }
        // The rollback has been carried out, so nothing more is owed for it — including on a
        // service whose rollback put a revision back, where the redeployment has already left
        // the record on the NEW rollout.
        if let Some(service) = self.services.get_mut(key) {
            service.deployment_rollback_due_at = None;
        }
    }

    /// The rollout of the same service BEFORE `record` that completed successfully — "the
    /// last deployment that completed successfully", which is the one a circuit-breaker
    /// rollback puts back — and `None` for a service that never had one.
    pub(super) fn last_completed_service_deployment(
        &self,
        record: &ServiceDeploymentRecord,
    ) -> Option<&ServiceDeploymentRecord> {
        self.service_deployments
            .values()
            .filter(|other| {
                other.service_arn == record.service_arn
                    && other.order_key() < record.order_key()
                    && other.status == SERVICE_DEPLOYMENT_SUCCESSFUL
            })
            .max_by(|left, right| left.order_key().cmp(&right.order_key()))
    }
}
