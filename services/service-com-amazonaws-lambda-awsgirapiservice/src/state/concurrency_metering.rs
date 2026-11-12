// SPDX-License-Identifier: CC0-1.0

//! The CONCURRENCY metering Lambda publishes about the capacity of the account it is
//! running in: how much of that capacity is claimed, and how much of it is being used by
//! the functions that have no per-function limit.
//!
//! A reservation and a provisioned allocation are capacity the account has COMMITTED to one
//! function ([`super::ComAmazonawsLambdaAwsgirapiserviceState::allocated_concurrency`]), and
//! an execution is capacity being USED at this instant. Which of the two a running execution
//! counts against is the whole substance of this metering: an execution of a function that
//! reserves concurrency is charged to that reservation — "Concurrency settings apply to the
//! function as a whole" and the reservation is the whole of what the account commits to it —
//! so it is never charged to the account's unreserved pool a second time, while an execution
//! of a function that reserves nothing comes out of that pool and is what
//! [`UNRESERVED_CONCURRENT_EXECUTIONS_METRIC`] measures. What the account has claimed
//! altogether ([`CLAIMED_ACCOUNT_CONCURRENCY_METRIC`]) is therefore the pool in use plus
//! every per-function allocation, which is why the difference between the two series at one
//! instant is exactly the allocated concurrency Lambda's own control plane reports.
//!
//! Three properties of this metering follow from that, and each is observable:
//!
//! * IT IS PUBLISHED OFF THE RESPONSE PATH. No client call produces these datapoints — they
//!   are what an operator reads to alarm on the Region's remaining headroom, so they exist
//!   whether or not anybody is calling. [`drive_concurrency_metering`] runs the sampler for
//!   as long as the account holds a function, and [`publish_account_concurrency`] is one
//!   sample, which a caller with no async runtime to sample on can take directly.
//! * IT IS A GAUGE, SAMPLED. Each of these series measures a quantity that HAS a value at
//!   every instant rather than counting events, so one sample is one observation of it,
//!   published at the regular one-minute storage resolution
//!   ([`super::metrics::STANDARD_STORAGE_RESOLUTION_SECONDS`]) — a reader laying one-minute
//!   periods over the series reads `Maximum` as the highest the quantity was seen to reach
//!   in that minute, which is what makes a burst shorter than a minute observable at all.
//!   [`SAMPLE_INTERVAL`] is how often a sample is taken, and the per-function gauge is
//!   observed at the instants it RISES besides ([`publish_function_concurrency`]), so the
//!   peak of a window is the peak the function really reached in it rather than the highest
//!   the cadence happened to catch.
//! * NOTHING IS METERED FOR WORK NOTHING DID. An account-level series is published every
//!   sample because the quantity it measures is defined at every instant (an account whose
//!   functions are idle is claiming exactly its allocations), while
//!   [`CONCURRENT_EXECUTIONS_METRIC`] is published for a function that IS processing
//!   executions and for no other: a function nobody invoked has no datapoint in the window
//!   rather than a zero. The executions these samples charge are the ones the store holds
//!   ([`super::FunctionRecord`]'s `concurrent_executions`), never a burst nothing
//!   performed.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::metrics::{
    append, cloudwatch_state, function_dimensions, observation, series, CloudWatchState,
    COUNT_UNIT,
};
use super::{service_state, ComAmazonawsLambdaAwsgirapiserviceState};

/// How many executions functions WITHOUT reserved concurrency are processing in this Region
/// at this instant: the account's unreserved pool in use.
pub const UNRESERVED_CONCURRENT_EXECUTIONS_METRIC: &str = "UnreservedConcurrentExecutions";

/// How much of this Region's concurrency the account has claimed at this instant: the
/// unreserved pool in use plus every allocation committed to an individual function.
pub const CLAIMED_ACCOUNT_CONCURRENCY_METRIC: &str = "ClaimedAccountConcurrency";

/// How many executions ONE function is processing at this instant, reported per function.
pub const CONCURRENT_EXECUTIONS_METRIC: &str = "ConcurrentExecutions";

/// How often the sampler takes one sample of the series above.
///
/// An EMULATOR-CHOSEN cadence — Lambda publishes no sampling frequency, only the series — and
/// deliberately shorter than the one-minute resolution the samples are stored at: a burst
/// that is over inside a minute is observable only if a sample was taken while it was in
/// flight, and `Maximum` over the minute is then the highest sample of that minute. It is
/// non-zero, so the sampler hands the runtime back between samples instead of spinning.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);

/// What ONE sample of the account's concurrency measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountConcurrency {
    /// The executions the account's UNRESERVED pool is processing: those of the functions
    /// that reserve no concurrency of their own.
    pub unreserved_concurrent_executions: i32,
    /// Everything the account has claimed: the pool in use above, plus every allocation
    /// committed to an individual function.
    pub claimed_account_concurrency: i32,
}

impl ComAmazonawsLambdaAwsgirapiserviceState {
    /// The executions the account's unreserved pool is processing at this instant.
    ///
    /// Only the functions that reserve nothing draw on that pool: a reserving function's
    /// executions come out of its own reservation, which the account has already committed
    /// to it in full.
    #[must_use]
    pub fn unreserved_concurrent_executions(&self) -> i32 {
        self.functions
            .values()
            .filter(|function| function.reserved_concurrent_executions.is_none())
            .map(|function| function.concurrent_executions)
            .fold(0i32, i32::saturating_add)
    }

    /// ONE sample of this account's concurrency: the unreserved pool in use, and everything
    /// the account has claimed — that pool plus [`Self::allocated_concurrency`].
    #[must_use]
    pub fn account_concurrency(&self) -> AccountConcurrency {
        let in_use = self.unreserved_concurrent_executions();
        AccountConcurrency {
            unreserved_concurrent_executions: in_use,
            claimed_account_concurrency: in_use.saturating_add(self.allocated_concurrency()),
        }
    }

    /// Every function that is processing at least one execution at this instant, with how
    /// many — in the store's own stable order.
    ///
    /// A function processing nothing is not in it: it has no observation to publish for the
    /// window rather than a zero.
    #[must_use]
    pub fn busy_functions(&self) -> Vec<(String, i32)> {
        self.functions
            .iter()
            .filter(|(_, function)| function.concurrent_executions > 0)
            .map(|(name, function)| (name.clone(), function.concurrent_executions))
            .collect()
    }
}

/// Take ONE sample of this engine's Lambda concurrency at `now` and publish it.
///
/// The account-level series are published every sample; a per-function
/// [`CONCURRENT_EXECUTIONS_METRIC`] observation is published for each function that is
/// processing executions. The Lambda store is only READ here — the sample changes nothing a
/// later caller sees — and the datapoints go into the CloudWatch store of the same engine
/// ([`super::metrics`]), which is the store `GetMetricData` answers from. An engine carrying
/// no CloudWatch has nowhere to publish to, so nothing is.
pub fn publish_account_concurrency<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    let Some(functions) = service_state(engine) else {
        return;
    };
    let Some(store) = cloudwatch_state(engine) else {
        return;
    };
    let (account, busy) = {
        let held = functions.lock();
        (held.account_concurrency(), held.busy_functions())
    };
    let at_secs = epoch_seconds(now);
    publish(
        &store,
        UNRESERVED_CONCURRENT_EXECUTIONS_METRIC,
        None,
        account.unreserved_concurrent_executions,
        at_secs,
    );
    publish(
        &store,
        CLAIMED_ACCOUNT_CONCURRENCY_METRIC,
        None,
        account.claimed_account_concurrency,
        at_secs,
    );
    for (function_name, running) in busy {
        publish(
            &store,
            CONCURRENT_EXECUTIONS_METRIC,
            Some(&function_name),
            running,
            at_secs,
        );
    }
}

/// Publish ONE reading of the per-function concurrency gauge
/// ([`CONCURRENT_EXECUTIONS_METRIC`]): `running` is how many executions the function
/// `function_name` names is processing at `now`.
///
/// This is what makes a gauge a client reads `Maximum` off report the function's real peak.
/// The quantity it measures HAS a value at every instant, so a reader laying periods over the
/// series reads the highest the quantity was SEEN to reach in each period — a burst nobody
/// sampled is a burst nobody can observe, and [`SAMPLE_INTERVAL`] alone would miss every
/// execution shorter than it. So the gauge is observed at the instants it RISES as well,
/// which are the admissions of the invocations that raise it
/// ([`super::execution_slot::admit_invocation`]): the greatest reading of a window is then the
/// greatest concurrency the function held in it, however short the executions were.
///
/// Nothing is published for a reading of no executions: the per-function series is published
/// for a function that IS processing executions and for no other, so a function nobody invoked
/// has no datapoint in the window rather than a zero.
///
/// It goes into the CloudWatch store of the SAME engine ([`super::metrics`]). Call it with no
/// Lambda store lock held: it takes the CloudWatch store's own.
pub fn publish_function_concurrency<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
    running: i32,
    now: SystemTime,
) {
    if running <= 0 {
        return;
    }
    let Some(store) = cloudwatch_state(engine) else {
        return;
    };
    publish(
        &store,
        CONCURRENT_EXECUTIONS_METRIC,
        Some(function_name),
        running,
        epoch_seconds(now),
    );
}

/// Publish one observation of `metric`: of the series of the function `function_name` names,
/// or of the account-level series (no dimension at all) when it names none.
fn publish(
    store: &::engine::ServiceState<CloudWatchState>,
    metric: &str,
    function_name: Option<&str>,
    value: i32,
    at_secs: f64,
) {
    let dimensions = function_name.map(function_dimensions).unwrap_or_default();
    let datum = observation(f64::from(value), COUNT_UNIT, at_secs);
    if !append(store, series(metric, dimensions), datum) {
        ::tracing::warn!(
            metric = %metric,
            "the CloudWatch store is at capacity; a Lambda concurrency sample could not be \
             published"
        );
    }
}

/// `at` as the epoch seconds every stored datapoint is stamped with.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Start the background sampler for this engine's account, if one is not already running.
///
/// Called by the operation that gives the account a function to meter. At most one sampler
/// runs per store (the store carries the claim), it samples for as long as the account holds
/// a function and stands down when it holds none, and with no async runtime to spawn onto
/// there is no sampler — a caller can then take the same sample through
/// [`publish_account_concurrency`].
pub fn drive_concurrency_metering<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_sampler(&mut state.lock()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // The sampler ends when the account has nothing left to meter, which it observes —
        // and releases its claim in — inside one critical section.
        while metering_wanted_or_release(&mut state.lock(), generation) {
            // Every sample waits first, which is what hands the runtime back between
            // samples. No lock is held across it.
            ::tokio::time::sleep(SAMPLE_INTERVAL).await;
            publish_account_concurrency(&*engine, SystemTime::now());
        }
    });
    // The handle is kept so the sampler it replaces is aborted rather than merely forgotten.
    set_sampler(&mut claimed.lock(), generation, handle);
}

/// Claim the single sampler of this store: the claim's GENERATION when the caller may start
/// one, `None` when one already runs.
fn claim_sampler(store: &mut ComAmazonawsLambdaAwsgirapiserviceState) -> Option<u64> {
    if store.concurrency_metering_running {
        return None;
    }
    store.concurrency_metering_running = true;
    store.concurrency_metering_generation = store.concurrency_metering_generation.wrapping_add(1);
    Some(store.concurrency_metering_generation)
}

/// Keep the running sampler's task handle, aborting the one it replaces — and only while
/// `generation` is still the current claim, so a starter whose own sampler has already stood
/// down installs nothing and aborts nothing (see [`super::sqs_poller::drive_sqs_pollers`],
/// which hands its loop over the same way).
fn set_sampler(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.concurrency_metering_generation != generation {
        return;
    }
    if let Some(previous) = store.concurrency_metering.replace(handle) {
        previous.abort();
    }
}

/// Whether this account still has concurrency to meter — and, when it has not, RELEASE the
/// claim `generation` holds.
///
/// An account holds concurrency to meter while it holds a function: a function claims the
/// capacity it reserves and the capacity it provisions, and it is the thing an execution can
/// be running on. Observing "nothing to meter" and standing down happen in ONE critical
/// section, which is what makes the hand-over race-free: a create holds this same lock to
/// store its function BEFORE it tries to claim the sampler, so either this call sees that
/// function and the running sampler keeps it, or the create's claim finds the flag cleared
/// and starts a sampler of its own.
fn metering_wanted_or_release(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
) -> bool {
    let wanted = !store.functions.is_empty();
    if !wanted && store.concurrency_metering_generation == generation {
        store.concurrency_metering_running = false;
    }
    wanted
}
