// SPDX-License-Identifier: CC0-1.0

//! The in-process clock that drives a deployment while nobody is calling
//! CodeDeploy.
//!
//! A deployment's progression is a series of transitions the clock owes it: its
//! start, each traffic increment it shifts ([`super::traffic_shift`]), the stop an
//! alarm it polls asks for ([`super::alarm_gate`]) and its terminal status. Every
//! one of them is materialised by [`super::with_state`] before a handler reads the
//! store, so a CodeDeploy caller always sees what was already due.
//!
//! But a client that deploys a Lambda revision then watches the ALIAS — that is
//! the documented way to see the first increment of a canary — and makes no
//! further CodeDeploy call, so an increment that only the next CodeDeploy call
//! could write would never be written at all. This is the task that performs it:
//! one per store, started by the operation that creates work for it, ticking until
//! no deployment is in flight. It changes WHAT happens to nothing — each tick
//! applies exactly what [`super::with_state`] would have applied at that instant —
//! only WHETHER it happens without a caller.
//!
//! With no async runtime to spawn onto there is no clock, which is the case in a
//! unit test that drives a handler directly; there nothing is waiting for an
//! increment either, and the next call through the store still materialises what
//! the clock owed.
//!
//! One thing the tick does beyond that pass: it PUBLISHES the trigger
//! notifications the pass came to owe ([`super::trigger_notifications`]), because
//! a deployment group's topic is told about an outcome whether or not anybody
//! called CodeDeploy to observe it.

use std::sync::Arc;
use std::time::Duration;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{hook_invocation, now_ms, service_state, with_state};

/// How often the clock wakes up to ask what is due.
///
/// Emulator-chosen: fine enough that an increment is written close to the instant
/// it fell due, and coarse enough that an idle emulator spends nothing noticeable
/// on it.
const CLOCK_TICK: Duration = Duration::from_millis(100);

/// How many ticks one clock may take before it stands down, so a task a request
/// started cannot outlive that request by much: at [`CLOCK_TICK`] a tick, ten
/// minutes. A later call that creates a deployment starts a fresh one.
const MAX_CLOCK_TICKS: usize = 6_000;

/// After how long with no tick recorded another caller may take the clock over.
///
/// The claim is held by whichever task last recorded a tick; a task that ENDED
/// without releasing it (a runtime shut down under it) would otherwise keep a
/// store from ever being driven again, so the claim lapses after this much silence.
const CLOCK_LAPSE_MILLIS: i64 = 5_000;

/// Which task, if any, is driving one store's deployments.
#[derive(Debug, Default)]
pub struct DeploymentClock {
    /// The generation of the task currently driving the store, and when it last
    /// recorded a tick.
    running: Option<(u64, i64)>,
    /// The generation the next claim is handed, so a task that was taken over can
    /// tell that it was.
    next_generation: u64,
    /// The task itself, so at most one is alive per store and it can always be
    /// STOPPED: a handle dropped on the floor could not be.
    task: Option<::tokio::task::JoinHandle<()>>,
}

impl DeploymentClock {
    /// Claim the clock for a new task, answering its generation, or `None` when
    /// another task is already driving this store.
    fn claim(&mut self, now_ms: i64) -> Option<u64> {
        if let Some((_, ticked_ms)) = self.running {
            if now_ms.saturating_sub(ticked_ms) < CLOCK_LAPSE_MILLIS {
                return None;
            }
        }
        self.next_generation += 1;
        let generation = self.next_generation;
        self.running = Some((generation, now_ms));
        Some(generation)
    }

    /// Record a tick of the task `generation` names, answering whether it is still
    /// the task driving this store — `false` once another one has taken over, which
    /// is the generation's cue to stand down.
    fn ticked(&mut self, generation: u64, now_ms: i64) -> bool {
        match self.running {
            Some((running, _)) if running == generation => {
                self.running = Some((generation, now_ms));
                true
            }
            _ => false,
        }
    }

    /// Release the claim the task `generation` holds, if it still holds it.
    fn release(&mut self, generation: u64) {
        if matches!(self.running, Some((running, _)) if running == generation) {
            self.running = None;
            self.task = None;
        }
    }

    /// Hold the task just started, aborting whichever task the claim it took was
    /// held by: at most one clock is alive per store, and it can always be stopped.
    fn set_task(&mut self, task: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.task.replace(task) {
            previous.abort();
        }
    }
}

/// Start the one task that drives this store's deployments, if one is not already
/// running.
///
/// Call it from an operation that leaves a deployment in flight — creating one, or
/// stopping one in a way that creates its rollback — with that handler's own
/// engine handle and its own clone of the in-process callback.
///
/// The callback is what this clock reaches the rest of the emulator through: the
/// lifecycle event hooks of a deployment in flight are INVOKED through it
/// ([`super::hook_invocation`]), and the trigger notifications its own transitions
/// owe are PUBLISHED through it ([`super::trigger_notifications`]) — so both reach
/// the same handlers a client's own calls reach.
pub fn drive_deployment_clock<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().deployment_clock.claim(now_ms()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let callback = callback.clone();
    let claimed = ::engine::ServiceState::clone(&state);
    let task = runtime.spawn(async move {
        for _ in 0..MAX_CLOCK_TICKS {
            ::tokio::time::sleep(CLOCK_TICK).await;
            let Some(state) = service_state(&*engine) else {
                return;
            };
            if !state.lock().deployment_clock.ticked(generation, now_ms()) {
                return;
            }
            // The pass every handler performs: the alarm gate, the traffic
            // increments the clock owes, and the status transitions.
            let in_flight = with_state(&*engine, |store| store.deployments_in_flight());
            // And the one pass no handler can perform: dispatching the lifecycle
            // event hook a deployment in flight is due, which INVOKES a Lambda
            // validation function and so needs both a runtime to dispatch on and
            // the callback to dispatch through.
            hook_invocation::dispatch_due_hooks(&engine, &callback);
            // The trigger notifications that pass came to owe, published before
            // the loop can end: a deployment's LAST transition is the one that
            // takes it out of flight, so a drain only on the next tick would
            // never happen for it.
            super::trigger_notifications::deliver(&*engine, &callback).await;
            if in_flight != Some(true) {
                break;
            }
        }
        if let Some(state) = service_state(&*engine) {
            state.lock().deployment_clock.release(generation);
        }
    });
    claimed.lock().deployment_clock.set_task(task);
}
