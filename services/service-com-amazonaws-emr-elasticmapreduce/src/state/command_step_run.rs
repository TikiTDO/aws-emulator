// SPDX-License-Identifier: CC0-1.0

//! WHEN a step's command is answered for: the queue side of a `command-runner.jar` step.
//!
//! The status such a step ends in, and the session that answers it — a parse and a fixed
//! allow-list of session-local utilities, with NO host command, process or shell started
//! anywhere — are [`super::step_command_status`]'s. This module is only about when that
//! answer is given and what one call may spend giving it: a step's command is owed an answer
//! from the instant its cluster STARTED the step, and the answer is given by the next
//! operation that reports that step or its cluster, with no store guard held — exactly as a
//! submitted Spark application's run is ([`super::spark_step_run`]).
//!
//! A step whose command has no answer yet does not END ([`StepRecord::awaits_command`]): the
//! state it ends in is the status its command answered, so it stays `RUNNING` until there is
//! an answer rather than being reported COMPLETED or FAILED for a command nothing has
//! answered for.

use super::*;

/// How many commands ONE call answers for, leaving the rest to the next call that reports the
/// cluster. CHOSEN: the ceiling on what one REQUEST costs, beside the session's own ceiling
/// on what one command costs. It is above the steps a cluster runs at once by a wide margin,
/// so a realistic queue settles in the call that reports it.
pub const MAX_COMMANDS_RESOLVED_PER_CALL: usize = 32;

impl ClusterRecord {
    /// Every command of this cluster that has no answer yet, in queue order: which step
    /// submitted it and what it is.
    ///
    /// A step is owed an answer once the cluster has STARTED it and nothing has answered for
    /// it yet ([`StepRecord::awaits_command`]); a step that has not started, has already been
    /// answered for, or is stopping is owed none.
    fn unanswered_step_commands(&self) -> Vec<(String, StepCommand)> {
        self.steps
            .iter()
            .filter(|step| step.awaits_command() && step.start_date_time.is_some())
            .filter_map(|step| Some((step.id.clone(), step.command.clone()?)))
            .collect()
    }
}

/// Answer for every command of the cluster `cluster_id` whose step has started, and record
/// what each answered.
///
/// WHAT A COMMAND REACHES: a parse and a fixed allow-list of session-local utilities, in
/// process ([`super::step_command_status`], whose header states it in full). No host command,
/// process or shell is started, here or anywhere this calls. A command outside that list is
/// refused with the status a shell gives a command it could not find, and its step then
/// reports that its work was not run.
///
/// Called by the operations that report a step or its cluster, BEFORE they take this
/// service's store, and never with it held: the bounds that confine a command stand between
/// one caller's program and every other caller's latency rather than inside this service's
/// lock. The store is taken only to read the unanswered commands and to write the outcomes,
/// and this answers nothing at all for a cluster id this account holds no cluster under.
///
/// Every transition this leads to was owed whether or not anybody called — the command was
/// owed from the instant the cluster STARTED the step — so a reporting operation may apply
/// it: what it adds is the step's own answer, which is what the step then ends in
/// ([`StepRecord::run_between`]). What ONE call answers for is bounded
/// ([`MAX_COMMANDS_RESOLVED_PER_CALL`]); a step whose turn did not come stays owed an answer
/// and the next call gives it.
pub fn resolve_due_step_commands<Eng: Send + Sync + 'static>(
    engine: &Eng,
    cluster_id: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let unanswered: Vec<(String, StepCommand)> = {
        let mut store = state.lock();
        // The queue is run up to this instant first, so a step whose turn has come is
        // STARTED and its command is among the unanswered ones.
        run_due_steps(engine, &mut store, cluster_id, now);
        store
            .clusters
            .get(cluster_id)
            .map(ClusterRecord::unanswered_step_commands)
            .unwrap_or_default()
    };
    if unanswered.is_empty() {
        return;
    }
    let answered: Vec<(String, StepOutcome)> = unanswered
        .into_iter()
        .take(MAX_COMMANDS_RESOLVED_PER_CALL)
        .map(|(step_id, command)| {
            // A command the session answered for nothing of ends the step the way every
            // unaccounted-for submission of this service ends it: in the modelled failure
            // state saying its work was not run, never a status nothing produced.
            let outcome = command_outcome(&command).unwrap_or_else(not_executed);
            (step_id, outcome)
        })
        .collect();
    let mut store = state.lock();
    for (step_id, outcome) in answered {
        if let Some(step) = store
            .clusters
            .get_mut(cluster_id)
            .and_then(|cluster| cluster.step_mut(&step_id))
        {
            // Only a step that STILL awaits an answer takes this one: in the window the guard
            // was released another call may have answered the same command, or the caller may
            // have cancelled the step or its cluster, and neither is overwritten by an answer
            // computed before that happened.
            if step.awaits_command() {
                step.outcome = Some(outcome);
            }
        }
    }
    // The steps can now reach the state their commands answered, if their spans have already
    // passed.
    run_due_steps(engine, &mut store, cluster_id, now);
}
