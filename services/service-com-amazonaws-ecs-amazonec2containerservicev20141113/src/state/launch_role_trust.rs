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
//! records no pull instants. Ahead of it FOR EACH TASK, not merely earlier in the pass: the
//! fetch is attempted only for a task this step has already been taken for
//! ([`TaskRecord::launch_roles_assumed`]), so a task placed while this step was deciding
//! another launch — by the scheduler, on another caller's clock pass — waits for the next
//! pass instead of being fetched for and started with its roles never put to IAM.
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
pub async fn settle_task_launches<Eng: Send + Sync + 'static>(engine: &Eng) {
    evaluate_launch_role_assumptions(engine);
    evaluate_image_pulls(engine).await;
}

/// Materialise everything the clock owes, as of `now`, with the launch steps outstanding
/// taken around it — the whole pass, for a caller that is not one of this service's own
/// operations: the clock pump, which drives the lifecycle between calls, and another service
/// asking this one whether the capacity a movement set is in effect.
///
/// The order is the one Amazon ECS takes the steps in:
///
/// 1. the capacity the account's instances owe their clusters is registered, so the pass
///    below has somewhere to place a task whose capacity has arrived
///    ([`super::settle_ec2_capacity`]);
/// 2. the launch steps outstanding are taken, so the pass below starts no task whose launch
///    nothing has decided ([`TaskRecord::launch_steps_taken`]) — the roles a launch resolved
///    are put to the account's IAM records before any container of it runs, not after;
/// 3. the clock pass materialises what was owed at `now`, which is also where the service
///    scheduler places the tasks a service is short of ([`super::service_steady_state`]);
/// 4. and the steps of the tasks THAT placement produced are taken, so a task the clock
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
pub async fn advance_task_clock<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    // The capacity the account's Amazon EC2 instances owe their clusters, which the clock
    // owes just as it owes a task's own moves: an instance launched into a cluster was
    // registered by its agent whether or not anybody called
    // ([`super::settle_ec2_capacity`]). Taken before the pass below, so the scheduler places
    // what the new capacity lets it place in this same pass.
    super::settle_ec2_capacity(engine, now);
    settle_task_launches(engine).await;
    if let Some(state) = service_state(engine) {
        state.lock().advance_clock(now);
    }
    settle_task_launches(engine).await;
}

/// Attempt the role assumes of every task that has just been launched, and record the stop a
/// task whose role Amazon ECS cannot assume earns.
///
/// Every task the pass was MADE for is recorded as assumed
/// ([`TaskRecord::launch_roles_assumed`]), not only the ones it refused: that is what the
/// step that follows it waits for, so a task placed after this pass collected its launches
/// is left to the next pass rather than fetched for on the strength of carrying no refusal
/// yet.
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
    let verdicts: Vec<(String, Option<String>)> =
        match ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(
            engine,
        ) {
            // Under the IAM store's lock alone, and in one pass: the lock is taken once
            // however many tasks were launched, and only owned sentences leave it. The FIRST
            // role the launch resolved that withholds the assume is the one it stops with,
            // which is why the roles are judged in the order they are listed in.
            Some(iam) => {
                let store = iam.lock();
                launches
                    .iter()
                    .map(|launch| {
                        let refusal = launch
                            .role_arns
                            .iter()
                            .find(|role_arn| role_withholds_assumption(&store, role_arn))
                            .map(|role_arn| unable_to_assume_role_message(role_arn));
                        (launch.task_arn.clone(), refusal)
                    })
                    .collect()
            }
            // An engine carrying no IAM store says nothing about any role, so it refuses
            // nothing — and the step has still been taken for these launches, which is what
            // lets them go on to their image fetch.
            None => launches
                .iter()
                .map(|launch| (launch.task_arn.clone(), None))
                .collect(),
        };
    state.lock().record_launch_role_assumptions(&verdicts);
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
    /// launch either step refused carries the reason instead ([`TaskRecord::launch_failure`]),
    /// which is why the fetch was not attempted for it. So either one is the pass having run
    /// for this task — the fetch only together with the assume it follows
    /// ([`TaskRecord::launch_roles_assumed`]), since an agent may report pull instants of its
    /// own (`SubmitTaskStateChange`) and a report about a fetch is not an assume.
    ///
    /// A task that has only just been PLACED carries neither, because the pass that places a
    /// task holds none of the facts these steps are decided against: they are read out of the
    /// other services' own stores, outside this store's lock ([`settle_task_launches`]). The
    /// clock therefore WITHHOLDS the start of such a task — a task that reports `RUNNING` is
    /// one whose containers have started, which cannot have happened before the steps that
    /// decide whether they may.
    pub fn launch_steps_taken(&self) -> bool {
        self.launch_failure.is_some()
            || (self.launch_roles_assumed && self.pull_started_at.is_some())
    }
}

/// The roles ONE launch has Amazon ECS assume, in the order it assumes them.
///
/// Collected from the stored tasks under this service's lock
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::launches_awaiting_role_assumption`])
/// and answered against the IAM store under that store's own, so no two locks are ever held
/// at once. A launch that resolved NO role carries an empty list and is still one the pass was
/// made for: there was nothing to refuse it by.
#[derive(Debug, Clone)]
pub struct LaunchRoles {
    /// The task the verdict belongs to.
    pub task_arn: String,
    /// The roles the launch resolved: its execution role, then its task role.
    pub role_arns: Vec<String>,
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// Every task on its way UP whose launch roles have not been put to the account's IAM
    /// records yet: one whose `desiredStatus` is `RUNNING` that has not started, that carries
    /// no launch failure already and that this step has not been taken for
    /// ([`TaskRecord::launch_roles_assumed`]), with the roles its launch resolved.
    ///
    /// The execution role comes first because Amazon ECS acts as it before the containers are
    /// given the task role, so a launch that names two roles it cannot assume stops with the
    /// first of them. A launch that names neither has no role to assume, so the step decides
    /// nothing for it — and is still listed, because what the step RECORDS is that it has been
    /// taken.
    pub fn launches_awaiting_role_assumption(&self) -> Vec<LaunchRoles> {
        self.tasks
            .values()
            .filter(|task| {
                task.launch_failure.is_none()
                    && !task.launch_roles_assumed
                    && task.desired_status == STATUS_RUNNING
                    && task.is_pending()
            })
            .map(|task| LaunchRoles {
                task_arn: task.task_arn.clone(),
                role_arns: [
                    task.execution_role_arn.clone(),
                    self.launched_task_role_arn(task),
                ]
                .into_iter()
                .flatten()
                .collect(),
            })
            .collect()
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

    /// Record that the roles each named launch resolved have been put to the account's IAM
    /// records, and why Amazon ECS could not launch the ones they refuse: the `stoppedReason`
    /// the stop the clock then owes carries ([`TaskRecord::fail_launch`]).
    ///
    /// The FIRST refusal a task carries is the one it stops with, which is why nothing
    /// overwrites a failure already recorded. No image is fetched for a refused task — the
    /// fetch is made as the role that could not be assumed — so no pull instants are recorded
    /// with it, which is what the fetch's own pass withholds it by
    /// ([`Self::launches_awaiting_image_pull`]). Only a task that is still on its way UP takes
    /// the verdict: one that has started was launched after all, and one that is stopping is on
    /// its way out either way.
    pub fn record_launch_role_assumptions(&mut self, verdicts: &[(String, Option<String>)]) {
        for (task_arn, refusal) in verdicts {
            if let Some(task) = self.tasks.get_mut(task_arn) {
                if task.desired_status != STATUS_RUNNING || !task.is_pending() {
                    continue;
                }
                task.launch_roles_assumed = true;
                if task.launch_failure.is_none() {
                    task.launch_failure = refusal.clone();
                }
            }
        }
    }
}
