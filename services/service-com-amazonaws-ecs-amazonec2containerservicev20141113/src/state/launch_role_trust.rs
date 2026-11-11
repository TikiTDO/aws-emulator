// SPDX-License-Identifier: CC0-1.0

//! The roles a LAUNCH has Amazon ECS assume, and the stop a task whose role it cannot assume
//! earns.
//!
//! # The rule this module implements
//!
//! A launch names up to two roles Amazon ECS has to act under before the task's containers can
//! run: the task EXECUTION role, which Amazon ECS itself acts as, and the TASK role, which it
//! hands the task's containers. Either way it reaches the role by assuming it, and whether a
//! role admits that assume is the question [`super::execution_role_trust`] answers against the
//! account's own IAM records.
//!
//! A launch Amazon ECS cannot assume a role of launches nothing: no container starts, no image
//! is fetched, and the task is stopped at the instant it would otherwise have started, with
//! the stop code of a task that never ran ([`STOP_CODE_TASK_FAILED_TO_START`]) and the
//! `stoppedReason` [`unable_to_assume_role_message`] words, naming the role that was not
//! assumed. Each role the launch RESOLVED is judged — the one its own `overrides` name, else
//! the one the stored revision declares ([`resolved_execution_role_arn`],
//! [`resolved_task_role_arn`]) — and the execution role first, because Amazon ECS acts as it
//! before the containers are given the task role.
//!
//! A launch that names no role, and one about which IAM answered nothing, is launched: this
//! service refuses only what IAM showed it ([`role_withholds_assumption`]), and an emulator
//! with no IAM mounted at all shows it nothing.
//!
//! The assume is decided at LAUNCH, by the same pass the image fetch is decided in and ahead
//! of it ([`settle_task_launches`]): Amazon ECS pulls an image AS the execution role, so a
//! task whose role it could not assume never reaches the registry, which is why such a task
//! records no pull instants.
//!
//! LOCK ORDER: the IAM store's lock is taken with no guard of this service's store held, and
//! hands back owned values — the same contract [`evaluate_image_pulls`] has, which is what
//! keeps a lock cycle between the stores unreachable.

use super::*;

/// Settle everything ONE pass over the just-launched tasks decides, in the order Amazon ECS
/// takes the steps: the roles the launch names are assumed first, and only a task whose roles
/// were assumed goes on to fetch its images.
///
/// This is the entry point every operation that LAUNCHES a task calls, and the one the clock
/// pump calls for a task the CLOCK placed; it is called with NO guard of this service's store
/// held, because each half reads another service's store under that store's own lock
/// ([`evaluate_launch_role_assumptions`], [`evaluate_image_pulls`]).
pub fn settle_task_launches<Eng: Send + Sync + 'static>(engine: &Eng) {
    evaluate_launch_role_assumptions(engine);
    evaluate_image_pulls(engine);
}

/// Attempt the role assumes of every task that has just been launched, and record the stop a
/// task whose role Amazon ECS cannot assume earns.
///
/// Nothing is recorded for a launch nothing can be shown about (see the module docs), and
/// nothing is decided twice: a task that already carries a launch failure is past the
/// question, and one that has started has already been launched.
pub fn evaluate_launch_role_assumptions<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let launches = state.lock().launches_awaiting_role_assumption();
    if launches.is_empty() {
        return;
    }
    // An engine carrying no IAM store says nothing about any role, so it refuses nothing.
    let Some(iam) =
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
    else {
        return;
    };
    // Under the IAM store's lock alone, and in one pass: the lock is taken once however many
    // tasks were launched, and only owned sentences leave it.
    let refusals: Vec<(String, String)> = {
        let store = iam.lock();
        launches
            .into_iter()
            .filter(|(_, role_arn)| role_withholds_assumption(&store, role_arn))
            .map(|(task_arn, role_arn)| (task_arn, unable_to_assume_role_message(&role_arn)))
            .collect()
    };
    if refusals.is_empty() {
        return;
    }
    state.lock().record_unassumable_role_refusals(&refusals);
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Every role the launch of a task that is still coming up has Amazon ECS assume, as
    /// `(task ARN, role ARN)` pairs: for each task on its way UP (its `desiredStatus` is
    /// `RUNNING` and it has not started) that carries no launch failure already, its
    /// execution role and then its task role.
    ///
    /// The execution role comes first because Amazon ECS acts as it before the containers are
    /// given the task role, so a launch that names two roles it cannot assume stops with the
    /// first of them. A launch that names neither has no role to assume, so the step decides
    /// nothing for it.
    pub fn launches_awaiting_role_assumption(&self) -> Vec<(String, String)> {
        let mut launches = Vec::new();
        for task in self.tasks.values() {
            if task.launch_failure.is_some()
                || task.desired_status != STATUS_RUNNING
                || !task.is_pending()
            {
                continue;
            }
            let roles = [
                task.execution_role_arn.clone(),
                self.launched_task_role_arn(task),
            ];
            for role_arn in roles.into_iter().flatten() {
                launches.push((task.task_arn.clone(), role_arn));
            }
        }
        launches
    }

    /// The TASK role the launch of `task` resolved: the one its own overrides name, and the
    /// one the revision it runs declares when they name none ([`resolved_task_role_arn`]).
    ///
    /// `None` for a launch that resolved none, and for a task whose revision this store no
    /// longer holds — a revision that is gone says nothing about the role the task runs
    /// under. The execution role needs no such lookup: it is kept on the task itself
    /// ([`TaskRecord::execution_role_arn`]), because the image pull is made as it.
    fn launched_task_role_arn(&self, task: &TaskRecord) -> Option<String> {
        let (family, revision) = parse_task_definition_reference(&task.task_definition_arn)?;
        let definition = self.task_definitions.get(&(family, revision?))?;
        resolved_task_role_arn(task.overrides.as_ref(), definition).map(ToString::to_string)
    }

    /// Record why Amazon ECS could not launch each named task: the `stoppedReason` the stop
    /// the clock then owes carries ([`TaskRecord::fail_launch`]).
    ///
    /// The FIRST refusal a task carries is the one it stops with, which is why nothing
    /// overwrites a failure already recorded. No image was fetched for such a task — the
    /// fetch is made as the role that could not be assumed — so no pull instants are recorded
    /// with it. Only a task that is still on its way UP takes the failure: one that has
    /// started was launched after all, and one that is stopping is on its way out either way.
    pub fn record_unassumable_role_refusals(&mut self, refusals: &[(String, String)]) {
        for (task_arn, reason) in refusals {
            if let Some(task) = self.tasks.get_mut(task_arn) {
                if task.launch_failure.is_none()
                    && task.desired_status == STATUS_RUNNING
                    && task.is_pending()
                {
                    task.launch_failure = Some(reason.clone());
                }
            }
        }
    }
}
