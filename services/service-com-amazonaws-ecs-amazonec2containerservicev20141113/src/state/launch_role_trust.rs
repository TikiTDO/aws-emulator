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
//! The decision is what the START of a task waits for, not the other way round: until this
//! pass has run for a task, no clock pass reports it `RUNNING`
//! ([`TaskRecord::launch_steps_taken`]). A task whose containers started before anything
//! decided whether Amazon ECS may run them would be a task launched under a role it was
//! refused, so the start is withheld instead. The withholding is never a stall: every call
//! that places a task takes the steps before it answers, and the clock pump takes them on
//! every wake-up ahead of the clock pass it drives ([`advance_task_clock`]) — so the start of
//! a task the CLOCK placed, which no call was there to take them for, follows within one pump
//! tick of the instant it was owed at.
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
///
/// A task this pass has run for is one the clock may then start
/// ([`TaskRecord::launch_steps_taken`]).
pub fn settle_task_launches<Eng: Send + Sync + 'static>(engine: &Eng) {
    evaluate_launch_role_assumptions(engine);
    evaluate_image_pulls(engine);
}

/// Materialise everything the clock owes, as of `now`, with the launch steps outstanding
/// taken around it — the whole pass, for a caller that is not one of this service's own
/// operations: the clock pump, which drives the lifecycle between calls, and another service
/// asking this one whether the capacity a movement set is in effect.
///
/// The order is the one Amazon ECS takes the steps in:
///
/// 1. the steps outstanding are taken, so the pass below starts no task whose launch nothing
///    has decided ([`TaskRecord::launch_steps_taken`]) — the roles a launch resolved are put
///    to the account's IAM records before any container of it runs, not after;
/// 2. the clock pass materialises what was owed at `now`, which is also where the service
///    scheduler places the tasks a service is short of ([`super::service_steady_state`]);
/// 3. and the steps of the tasks THAT placement produced are taken, so a task the clock
///    placed is launched by the pass that placed it, exactly as a task a call places is.
///
/// What ONE call costs is bounded by what the store HOLDS, not by the request: each step is a
/// pass over the task map, which is capped at [`MAX_TASKS`] however many calls arrive, and
/// nothing here takes a size, a count or a wait from a request. Nothing is dialled, resolved
/// or opened either: the facts the steps are decided against are read out of the emulated IAM,
/// Amazon EC2 and Amazon ECR services' OWN in-memory stores through the shared engine handle
/// ([`settle_task_launches`], [`evaluate_image_pulls`]), so the registry an image reference
/// names is answered by the emulated registry rather than reached over a network.
///
/// Called with NO guard of this service's store held, which is also what keeps the lock order
/// one-way: each pass takes this store's lock itself and releases it before the launch steps
/// take another store's, so no two of them are ever held at once (see the module's LOCK ORDER
/// note).
pub fn advance_task_clock<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    settle_task_launches(engine);
    if let Some(state) = service_state(engine) {
        state.lock().advance_clock(now);
    }
    settle_task_launches(engine);
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

impl TaskRecord {
    /// Whether the launch steps Amazon ECS takes before this task's containers can start have
    /// been taken for it: the roles the launch resolved put to the account's IAM records
    /// ([`evaluate_launch_role_assumptions`]) and the images its containers run fetched
    /// ([`evaluate_image_pulls`]).
    ///
    /// Read off what those steps RECORDED rather than off a flag beside them, so the two
    /// cannot drift. The fetch is the LAST of them and it stamps the instants it took on
    /// every task it was made for, whether or not it succeeded
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::record_image_pulls`]); a
    /// launch the pass refused carries the reason instead ([`TaskRecord::launch_failure`]),
    /// which is why the fetch was not attempted for it. So either one is the pass having run
    /// for this task.
    ///
    /// A task that has only just been PLACED carries neither, because the pass that places a
    /// task holds none of the facts these steps are decided against: they are read out of the
    /// other services' own stores, outside this store's lock ([`settle_task_launches`]). The
    /// clock therefore WITHHOLDS the start of such a task — a task that reports `RUNNING` is
    /// one whose containers have started, which cannot have happened before the steps that
    /// decide whether they may.
    pub fn launch_steps_taken(&self) -> bool {
        self.launch_failure.is_some() || self.pull_started_at.is_some()
    }
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
