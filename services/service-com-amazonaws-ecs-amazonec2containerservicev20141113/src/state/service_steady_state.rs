// SPDX-License-Identifier: CC0-1.0

//! The ECS service scheduler's STEADY STATE: the task a service replaces when one of its
//! tasks stops, and the task nobody replaces.
//!
//! # The rule this module implements
//!
//! A service "runs and maintains your desired number of tasks from a specified task
//! definition" (`CreateService`), and that maintenance is CONTINUOUS: "if any of your tasks
//! fail or stop, the Amazon ECS service scheduler launches another instance of your task
//! definition to replace it in order to maintain the desired number of tasks in the
//! service". So whenever a service holds fewer tasks of its current deployment than its
//! `desiredCount`, the scheduler places the shortfall — however the shortfall arose: a task
//! a caller stopped with `StopTask`, a task that failed, or a task that ended on its own.
//!
//! A task that belongs to NO service has no scheduler maintaining a count, so a stopped
//! `RunTask`/`StartTask` task is never relaunched. That contrast is the whole of this
//! module: it needs no code of its own, because a replacement is only ever placed for a
//! count a SERVICE wants.
//!
//! # Why the clock owes it
//!
//! The reconcile is the work the scheduler was already doing when the task stopped: the
//! desired count is the caller's own standing instruction, so bringing the task set back up
//! to it was owed at the instant the task stopped whether or not anybody called. It is
//! therefore materialised by the clock pass
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]) — the same
//! pass that materialises a task's own transitions, and the same pass the clock pump drives
//! between calls — and it places nothing a service does not want: a service that is not
//! `ACTIVE`, one whose deployment controller is not the ECS one (its tasks come from a task
//! set), one whose rollout the deployment circuit breaker failed (`Deployment$rolloutState`:
//! "A deployment in `FAILED` state doesn't launch any new tasks") and one that already holds
//! the tasks it wants are all left exactly as they are.
//!
//! The placement itself is the scheduler's ordinary one
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service`]): a
//! replacement is a task of the service's current deployment, placed under the service's own
//! placement, `PENDING` with its start owed by the clock — so a replacement is
//! indistinguishable from the task it replaces except for being a new task, which is what
//! Amazon ECS does.

use super::*;

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Place the tasks every ACTIVE service is short of, as of `now`.
    ///
    /// The scheduler's steady-state pass: for each service that places its own tasks, the
    /// task set is brought back in line with the `desiredCount` the caller asked for
    /// ([`Self::scale_service`], the same reconcile the call that changes that count runs).
    /// A service whose cluster has nowhere to put the task places nothing and records why
    /// on its event stream, exactly as that call does.
    ///
    /// A placement this emulator has no room for ([`MAX_TASKS`]) is left for later rather
    /// than failing the pass: the clock pass has no caller to answer.
    pub(super) fn maintain_service_task_counts(&mut self, now: SystemTime) {
        let keys: Vec<(String, String)> = self.services.keys().cloned().collect();
        for key in keys {
            let Some(service) = self.services.get(&key) else {
                continue;
            };
            if !service.is_active() || !service.places_tasks() {
                continue;
            }
            // `Deployment$rolloutState`: "A deployment in FAILED state doesn't launch any new
            // tasks" — so a rollout the deployment circuit breaker failed
            // ([`super::circuit_breaker`]) is the one service the scheduler places nothing
            // for, however short of its desired count it is.
            if service.deployment_rollout_failure.is_some() {
                continue;
            }
            // The revision the service's CURRENT deployment places: a replacement runs what
            // the service is deploying now, not what an earlier deployment ran.
            let definition = service
                .task_definition
                .clone()
                .and_then(|revision| self.task_definitions.get(&revision).cloned());
            let Some(definition) = definition else {
                continue;
            };
            // The Region the service's own ARNs carry, so a replacement's ARN names the
            // Region the service lives in rather than whichever Region last called.
            let region = super::request_arn::RequestArn::parse(&service.cluster_arn)
                .map(|arn| arn.region.to_string())
                .unwrap_or_else(|| DEFAULT_REGION.to_string());
            let _ = self.scale_service(&key, &definition, &region, now);
        }
    }
}
