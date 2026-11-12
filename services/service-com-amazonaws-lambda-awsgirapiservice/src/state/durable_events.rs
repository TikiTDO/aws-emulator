// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as a DURABLE EXECUTION's status moves: one
//! event per status an execution enters.
//!
//! # The rule this module implements
//!
//! A durable execution is not only a handle a client polls: every status it MOVES INTO is
//! published onto the account's default EventBridge bus ([`DEFAULT_EVENT_BUS`]) under the
//! `source` this service owns ([`EVENT_SOURCE`]) and the `detail-type` of a durable
//! execution's status change ([`DURABLE_EXECUTION_STATUS_CHANGE`]), carrying the execution's
//! identity, the function it runs on, its status and its instants in the event's `detail`
//! ([`StatusChange::detail`]). A rule on that bus whose `EventPattern` matches then delivers
//! it to that rule's targets, so a client that reacts to a durable execution through
//! EventBridge — alerting on one that timed out, reconciling long-running work — observes
//! the run without calling this service again.
//!
//! # How a change becomes an event
//!
//! Nothing here decides WHEN anything moves: the call that starts an execution, the run that
//! settles it ([`super::durable_run`]), the stop a caller makes
//! ([`super::DurableExecutionRecord::stop`]) and the `ExecutionTimeout` the clock owes
//! ([`super::DurableExecutionRecord::materialize`]) do, and the status they write is the fact
//! an event reports. So this module is a DIFF rather than a second state machine:
//! [`record_status_changes`] compares every stored execution against the status last
//! published for it and queues one change per execution whose status differs — whichever
//! call, run or clock tick materialised it, and without any transition site having to
//! remember to announce itself. Nothing is invented: an execution's status, its instants and
//! its error are read off the record a `GetDurableExecution` of the same instant reports.
//!
//! A queued change goes out with NO lock held and off the response path, on the one clock
//! [`drive`] runs per store: an execution's terminal status falls due after the call that
//! started it has answered — an asynchronous invocation's run settles the execution on a task
//! of its own, and the `ExecutionTimeout` falls due with nobody calling at all — so a change
//! nobody will ask for is published by the same clock as one a call produced. EventBridge is
//! reached the way every emulated service reaches another, through the framework's in-process
//! callback (`service_common::eventbridge_client`). Publication is best-effort — an emulator
//! with no EventBridge mounted has nowhere to publish — so a batch the bus did not take is
//! logged, put back on the queue and tried again, and the clock stands down once a few
//! attempts in a row have failed, leaving the queue for the next call to arm a clock for. The
//! clock wakes, applies whatever the clock has made due, publishes what that produced, and
//! stands down once no execution is still in flight and nothing is left unpublished.
//!
//! The queue is BOUNDED ([`MAX_PENDING_EVENTS`], oldest dropped first) because every
//! transition appends to it, and the record of what has been published is pruned to the
//! executions the store still holds, so neither grows with the number of calls served.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::serde_json::{Map, Value};

use super::ComAmazonawsLambdaAwsgirapiserviceState;

/// The `source` of every event Lambda publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry ([`super::ARN_NAMESPACE`]).
pub const EVENT_SOURCE: &str = "aws.lambda";

/// The `detail-type` of the event published when a DURABLE EXECUTION changes status.
pub const DURABLE_EXECUTION_STATUS_CHANGE: &str = "Lambda Durable Execution Status Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a batch of more changes than that goes out in several
/// requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many status changes wait to be published at once.
///
/// An EMULATOR-CHOSEN ceiling, like the one on the executions themselves
/// ([`super::MAX_DURABLE_EXECUTIONS`]): the queue is in memory and every transition appends
/// to it, so past this point the OLDEST entry makes room for the newest rather than the store
/// growing without bound. It is above the executions one function can hold, so a burst of
/// transitions is published whole.
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many changes ONE drain takes out of the queue before publishing them; [`drain`]
/// repeats until the queue is empty, so this bounds one batch rather than what a call
/// publishes.
const MAX_PUBLISHED_PER_DRAIN: usize = 100;

/// How long the clock waits between two wake-ups while an execution is still in flight.
///
/// An EMULATOR-CHOSEN span, like the other spans this crate owns: Lambda publishes no latency
/// for a status-change event. It is deliberately non-zero, so the clock hands the runtime
/// back between wake-ups instead of spinning, and short enough that a client watching only
/// its queue sees a status change promptly.
const CLOCK_INTERVAL: Duration = Duration::from_millis(250);

/// How many consecutive wake-ups that published nothing the clock makes before standing
/// down.
///
/// An execution left `RUNNING` by a run this emulator never completed would otherwise keep
/// the clock awake for the life of the process. Past this many idle wake-ups it stands down
/// and says so; the next call that can move an execution arms a new one.
const MAX_IDLE_WAKE_UPS: u32 = 1_200;

/// How many consecutive publications that did not reach EventBridge the clock makes before
/// standing down.
///
/// An emulator with no EventBridge mounted has nowhere to publish, and every attempt answers
/// the same failure, so retrying at [`CLOCK_INTERVAL`] for the whole idle cap would be a
/// thousand attempts nothing can change. The queued changes stand either way: the next call
/// that can move an execution arms a new clock, which tries them again.
const MAX_PUBLICATION_FAILURES: u32 = 8;

/// One status a durable execution moved into, waiting to be published.
///
/// Every field is read off the stored execution, so the event reports the execution a read of
/// the same instant reports and never a second, invented copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusChange {
    /// The Region the function the execution belongs to was created in, which is the Region
    /// whose default bus the event is published onto: a bus belongs to a Region.
    pub region: String,
    /// The ARN the execution is addressed by.
    pub durable_execution_arn: String,
    /// The execution's own name, as the call that started it named it.
    pub durable_execution_name: String,
    /// The ARN of the function version the execution runs on.
    pub function_arn: String,
    /// The name of the function the execution belongs to.
    pub function_name: String,
    /// The `ExecutionStatus` the execution moved into.
    pub status: String,
    /// When the execution started.
    pub started: SystemTime,
    /// When it ended, once it reached a terminal status.
    pub ended: Option<SystemTime>,
    /// WHEN this change happened — the instant the execution ended for a terminal status, and
    /// the instant it started for the first one — which is what the event's own `time`
    /// reports rather than the instant the queue happened to be drained.
    pub at: SystemTime,
}

impl StatusChange {
    /// The event `detail` this change is reported in.
    ///
    /// Public because it IS the contract: the members a rule's `EventPattern` filters on,
    /// which is what this crate's own suite holds the published event to. A member the
    /// execution does not carry — the end instant of one still running — is left out rather
    /// than reported as an instant nothing happened at.
    #[must_use]
    pub fn detail(&self) -> Value {
        let mut detail = Map::new();
        detail.insert(
            "durableExecutionArn".to_string(),
            Value::String(self.durable_execution_arn.clone()),
        );
        detail.insert(
            "durableExecutionName".to_string(),
            Value::String(self.durable_execution_name.clone()),
        );
        detail.insert(
            "functionArn".to_string(),
            Value::String(self.function_arn.clone()),
        );
        detail.insert(
            "functionName".to_string(),
            Value::String(self.function_name.clone()),
        );
        detail.insert("status".to_string(), Value::String(self.status.clone()));
        detail.insert(
            "startTimestamp".to_string(),
            Value::from(epoch_millis(self.started)),
        );
        if let Some(ended) = self.ended {
            detail.insert(
                "endTimestamp".to_string(),
                Value::from(epoch_millis(ended)),
            );
        }
        Value::Object(detail)
    }
}

/// Queue every status change this store has not published yet, and keep the clock that
/// publishes it running.
///
/// The entry point for every operation that can move a durable execution's status, called
/// with NO lock held: it takes the lock, records what the diff finds, releases it and arms
/// the one clock of this store. The `PutEvents` itself is made by that clock rather than by
/// the request — Lambda's answer to an invocation says nothing about a bus, and a target the
/// event reaches may call back into this service — so nothing a caller is told waits for, or
/// depends on, the publication. With no async runtime to arm a clock on nothing is published:
/// the changes stay queued for the next call that can arm one.
pub fn publish<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    record_status_changes(&mut state.lock(), SystemTime::now());
    drive(&state, callback);
}

/// What one drain of the queue came to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Drained {
    /// How many changes the bus took.
    published: usize,
    /// Whether a batch did not reach the bus at all, which is the signal the clock stands
    /// down on: an emulator with no EventBridge mounted answers this to every attempt.
    failed: bool,
}

/// Publish what is queued onto the account's event bus, and answer what that came to.
///
/// A batch the bus did not take is put BACK on the queue ([`requeue`]) and the drain ends
/// there: the clock's next wake-up tries it again, so a publication that failed because
/// nothing was listening yet is retried rather than lost, and a bus that cannot be reached at
/// all cannot hold one wake-up in a loop.
async fn drain(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) -> Drained {
    let mut drained = Drained::default();
    loop {
        // The guard is taken and dropped inside this statement: the diff is what records the
        // transitions the clock has made due, and the batch is taken in the same critical
        // section so no change is published twice.
        let changes = {
            let mut store = state.lock();
            record_status_changes(&mut store, SystemTime::now());
            take_pending(&mut store)
        };
        if changes.is_empty() {
            return drained;
        }
        let failed = publish_batch(callback, &changes).await;
        drained.published += changes.len() - failed.len();
        if !failed.is_empty() {
            requeue(&mut state.lock(), failed);
            drained.failed = true;
            return drained;
        }
    }
}

/// Bring every durable execution up to whatever the clock has already made due, then queue
/// one change per execution whose status differs from the one last published for it.
///
/// The ONLY transition applied here is the one the clock already owed
/// ([`super::DurableExecutionRecord::materialize`]): it would have been owed at the same
/// instant even if nobody had called. Every other status was written by the call or the run
/// that produced it, and this only NOTICES it.
fn record_status_changes(store: &mut ComAmazonawsLambdaAwsgirapiserviceState, now: SystemTime) {
    for function in store.functions.values_mut() {
        function.materialize_durable_executions(now);
    }
    let mut changes: Vec<StatusChange> = Vec::new();
    let mut held: BTreeSet<String> = BTreeSet::new();
    for function in store.functions.values() {
        for execution in function.durable_executions.values() {
            let arn = function.durable_execution_arn(execution);
            held.insert(arn.clone());
            if store.noticed_durable_status.get(&arn) == Some(&execution.status) {
                continue;
            }
            changes.push(StatusChange {
                region: function.region.clone(),
                durable_execution_arn: arn,
                durable_execution_name: execution.name.clone(),
                function_arn: function.versioned_arn(&execution.version),
                function_name: function.config.name.clone(),
                status: execution.status.clone(),
                started: execution.started,
                ended: execution.ended,
                // The instant the change happened: the end of a terminal one — a timeout is
                // dated at the instant it fell due rather than at the wake-up that noticed
                // it — and the start of the first one.
                at: execution.ended.unwrap_or(execution.started),
            });
        }
    }
    for change in changes {
        // The status this change reports is recorded as the one NOTICED for the execution
        // before anything goes out, which is what makes the diff idempotent: a later pass
        // over the same status queues nothing, so one move produces one event however often
        // the diff runs while the queue is still being drained.
        store
            .noticed_durable_status
            .insert(change.durable_execution_arn.clone(), change.status.clone());
        enqueue(store, change);
    }
    // What has been published is remembered only for the executions the store still holds, so
    // the record cannot outgrow them.
    store
        .noticed_durable_status
        .retain(|arn, _| held.contains(arn));
}

/// Queue `change` for publication, keeping the queue inside [`MAX_PENDING_EVENTS`] by making
/// room with the OLDEST change.
///
/// The one place a change enters the queue — the diff that notices it, and the re-queue of a
/// batch whose publication did not reach EventBridge ([`requeue`]) — so the bound holds
/// however a change arrives, exactly as the asynchronous invocation queue's does
/// (`FunctionRecord::hold_async_invocation`).
fn enqueue(store: &mut ComAmazonawsLambdaAwsgirapiserviceState, change: StatusChange) {
    store.pending_durable_events.push_back(change);
    while store.pending_durable_events.len() > MAX_PENDING_EVENTS {
        store.pending_durable_events.pop_front();
    }
}

/// Put a batch whose publication did not reach EventBridge back at the HEAD of the queue,
/// oldest first, so the next drain tries it again rather than losing it.
///
/// It is bounded like every other way into the queue: a store already holding
/// [`MAX_PENDING_EVENTS`] changes makes room with its oldest, so a bus that cannot be reached
/// at all cannot grow the store.
fn requeue(store: &mut ComAmazonawsLambdaAwsgirapiserviceState, failed: Vec<StatusChange>) {
    for change in failed.into_iter().rev() {
        store.pending_durable_events.push_front(change);
    }
    while store.pending_durable_events.len() > MAX_PENDING_EVENTS {
        store.pending_durable_events.pop_back();
    }
}

/// Take at most [`MAX_PUBLISHED_PER_DRAIN`] queued changes out of the store, oldest first —
/// one batch of the drain [`drain`] repeats until the queue is empty.
fn take_pending(store: &mut ComAmazonawsLambdaAwsgirapiserviceState) -> Vec<StatusChange> {
    let taking = MAX_PUBLISHED_PER_DRAIN.min(store.pending_durable_events.len());
    store.pending_durable_events.drain(..taking).collect()
}

/// Publish one batch of changes, in `PutEvents` requests of at most
/// [`MAX_PUT_EVENTS_ENTRIES`] entries, each made in the Region the executions it reports run
/// in, and answer the changes the bus did NOT take.
///
/// Publication is best-effort — an emulator with no EventBridge mounted has nowhere to
/// publish — so a failure is logged rather than raised; the changes it answers are the ones
/// a later drain tries again ([`requeue`]).
async fn publish_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    changes: &[StatusChange],
) -> Vec<StatusChange> {
    let mut failed: Vec<StatusChange> = Vec::new();
    // One request per Region: a bus belongs to a Region, and an execution is published onto
    // the bus of the Region its function runs in.
    let mut by_region: BTreeMap<&str, Vec<&StatusChange>> = BTreeMap::new();
    for change in changes {
        by_region
            .entry(change.region.as_str())
            .or_default()
            .push(change);
    }
    for (region, entries) in by_region {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
        for window in entries.chunks(MAX_PUT_EVENTS_ENTRIES) {
            let mut request = client.put_events();
            for change in window {
                let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS)
                    .source(EVENT_SOURCE)
                    .detail_type(DURABLE_EXECUTION_STATUS_CHANGE)
                    // The event's `time` is WHEN the execution moved:
                    // `PutEventsRequestEntry$Time` is "the time stamp of the event, per
                    // RFC3339. If no time stamp is provided, the time stamp of the PutEvents
                    // call is used."
                    .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                        epoch_millis(change.at),
                    ))
                    // The resource the event is about, which is how EventBridge carries the
                    // identity of what moved beside the detail.
                    .resources(change.durable_execution_arn.clone())
                    .detail(change.detail().to_string())
                    .build();
                request = request.entries(entry);
            }
            if let Err(error) = request.send().await {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a Lambda durable-execution status change could not be published to \
                     EventBridge"
                );
                failed.extend(window.iter().map(|change| (*change).clone()));
            }
        }
    }
    failed
}

/// Start the one in-process task that wakes while an execution of this store is still in
/// flight, applies whatever the clock has made due and publishes it.
///
/// Called by [`publish`], so every operation that can move an execution keeps the clock
/// running; a second call while one runs starts nothing. With no async runtime to spawn onto
/// there is no clock: the changes are then recorded and published by the next operation
/// instead.
fn drive(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim(&mut state.lock()) else {
        return;
    };
    let driven = ::engine::ServiceState::clone(state);
    // The same handle into this same emulator the request itself would have published
    // through, owned by the task rather than borrowed from the request that started it.
    let callback = callback.clone();
    let handle = runtime.spawn(async move {
        // `true` only if a cap is what ended the loop; a clock that ran out of work has
        // already released its claim inside the one critical section that observed it.
        let mut capped = true;
        let mut idle_wake_ups = 0;
        let mut failures = 0;
        while idle_wake_ups < MAX_IDLE_WAKE_UPS && failures < MAX_PUBLICATION_FAILURES {
            if !in_flight_or_release(&mut driven.lock(), generation) {
                capped = false;
                break;
            }
            // Every wake-up waits, which is what hands the runtime back between them. No lock
            // is held across it.
            ::tokio::time::sleep(CLOCK_INTERVAL).await;
            let drained = drain(&driven, &callback).await;
            // A wake-up that published leaves the clock as fresh as a new one, so only the
            // ones that found nothing, and only the attempts that did not reach the bus,
            // count against their caps.
            idle_wake_ups = match drained.published {
                0 => idle_wake_ups + 1,
                _ => 0,
            };
            failures = if drained.failed { failures + 1 } else { 0 };
        }
        if capped {
            ::tracing::warn!(
                "the durable-execution event clock stood down after {idle_wake_ups} wake-ups \
                 that published nothing and {failures} publication(s) that did not reach \
                 EventBridge; the queued status changes stand, and the next call to this \
                 service publishes what it owes"
            );
            // Only THIS clock's claim, so a clock started since is left holding its own.
            release(&mut driven.lock(), generation);
        }
    });
    set_clock(&mut state.lock(), generation, handle);
}

/// Claim the single event clock of this store: the claim's GENERATION when the caller may
/// start one, `None` when one already runs.
fn claim(store: &mut ComAmazonawsLambdaAwsgirapiserviceState) -> Option<u64> {
    if store.durable_event_clock_running {
        return None;
    }
    store.durable_event_clock_running = true;
    store.durable_event_clock_generation = store.durable_event_clock_generation.wrapping_add(1);
    Some(store.durable_event_clock_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock standing down cannot
/// clear a claim a clock started since is holding.
fn release(store: &mut ComAmazonawsLambdaAwsgirapiserviceState, generation: u64) {
    if store.durable_event_clock_generation == generation {
        store.durable_event_clock_running = false;
    }
}

/// Keep the running clock's task handle, aborting the one it replaces.
///
/// Only when `generation` is still the CURRENT claim: a clock can end before its starter gets
/// here, and a call that claimed the clock since then has stored a handle of its own —
/// replacing that one would abort a live clock and leave its claim held for ever.
fn set_clock(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.durable_event_clock_generation != generation {
        return;
    }
    if let Some(previous) = store.durable_event_clock.replace(handle) {
        previous.abort();
    }
}

/// Whether this store still owes an event — and, when it does not, RELEASE the claim
/// `generation` holds.
///
/// Work is outstanding while an execution is still RUNNING (its terminal status is still to
/// come), while a status differs from the one last published for it, or while the queue still
/// holds a change. Observing "nothing is owed" and standing down happen in ONE critical
/// section, which is what makes the hand-over race-free: a call holds this same lock to move
/// an execution BEFORE it tries to claim the clock, so either this call sees that execution
/// and the running clock keeps it, or the call's claim finds the flag cleared and starts a
/// clock of its own.
fn in_flight_or_release(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
) -> bool {
    let owed = !store.pending_durable_events.is_empty()
        || store.functions.values().any(|function| {
            function.durable_executions.values().any(|execution| {
                execution.status == super::EXECUTION_STATUS_RUNNING
                    || store
                        .noticed_durable_status
                        .get(&function.durable_execution_arn(execution))
                        != Some(&execution.status)
            })
        });
    if !owed {
        release(store, generation);
    }
    owed
}

/// `at` as the epoch milliseconds an event's `time` and a detail's instants carry.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(at.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the queue is bounded, so a burst of transitions cannot grow the store without
    /// bound — the OLDEST change makes room for the newest, exactly as the asynchronous
    /// invocation queue does — whichever way a change enters it: the diff that notices one,
    /// and the re-queue of a batch the bus did not take.
    #[test]
    fn the_queue_of_pending_changes_is_bounded() {
        let mut store = ComAmazonawsLambdaAwsgirapiserviceState::default();
        // (case, how many changes enter the queue, how many the store then holds)
        let rows: [(&str, usize, usize); 3] = [
            ("fewer changes than the bound", 10, 10),
            ("as many changes as the bound", MAX_PENDING_EVENTS, MAX_PENDING_EVENTS),
            ("more changes than the bound", MAX_PENDING_EVENTS + 25, MAX_PENDING_EVENTS),
        ];
        for (case, queued, expected) in rows {
            store.pending_durable_events.clear();
            for index in 0..queued {
                enqueue(&mut store, change_numbered(index));
            }
            let held = store.pending_durable_events.len();
            assert_eq!(
                held, expected,
                "case {case:?}: expected the store to hold {expected} change(s), got {held}"
            );
            // The ones kept are the NEWEST, so a drain publishes what happened most recently
            // rather than a prefix that was dropped.
            let newest = store
                .pending_durable_events
                .back()
                .map(|change| change.durable_execution_name.clone());
            assert_eq!(
                newest.as_deref(),
                Some(format!("exec-{}", queued - 1).as_str()),
                "case {case:?}: expected the newest change to be kept, got {newest:?}"
            );
        }
    }

    /// RULE: a batch the bus did not take goes back to the HEAD of the queue, in its own
    /// order, so the next drain tries the oldest unpublished change first — under the same
    /// bound, which it makes room inside with the NEWEST change rather than the one being
    /// retried.
    #[test]
    fn a_batch_the_bus_did_not_take_is_tried_again_first() {
        // (case, how many changes are already queued, how many failed, the names the queue
        // then holds at its head and at its tail)
        let rows: [(&str, usize, usize); 3] = [
            ("an empty queue", 0, 3),
            ("a queue with room", 5, 2),
            ("a queue already at the bound", MAX_PENDING_EVENTS, 2),
        ];
        for (case, waiting, failing) in rows {
            let mut store = ComAmazonawsLambdaAwsgirapiserviceState::default();
            for index in 0..waiting {
                enqueue(&mut store, change_numbered(1_000 + index));
            }
            let failed: Vec<StatusChange> =
                (0..failing).map(change_numbered).collect();
            requeue(&mut store, failed);
            let held = store.pending_durable_events.len();
            assert_eq!(
                held,
                (waiting + failing).min(MAX_PENDING_EVENTS),
                "case {case:?}: expected the bound to hold after a re-queue, got {held}"
            );
            let head: Vec<String> = store
                .pending_durable_events
                .iter()
                .take(failing)
                .map(|change| change.durable_execution_name.clone())
                .collect();
            let expected: Vec<String> = (0..failing).map(|index| format!("exec-{index}")).collect();
            assert_eq!(
                head, expected,
                "case {case:?}: expected the re-queued batch at the head in its own order, got \
                 {head:?}"
            );
        }
    }

    /// One change standing for the execution numbered `index`.
    fn change_numbered(index: usize) -> StatusChange {
        StatusChange {
            region: "us-east-1".to_string(),
            durable_execution_arn: format!(
                "arn:aws:lambda:us-east-1:000000000000:function:f:$LATEST/durable-execution/\
                 exec-{index}/abc"
            ),
            durable_execution_name: format!("exec-{index}"),
            function_arn: "arn:aws:lambda:us-east-1:000000000000:function:f".to_string(),
            function_name: "f".to_string(),
            status: super::super::EXECUTION_STATUS_RUNNING.to_string(),
            started: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            ended: None,
            at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        }
    }

    /// RULE: the detail reports the execution's identity, its status and its instants, and
    /// leaves out an instant the execution does not carry.
    #[test]
    fn the_detail_reports_what_the_execution_carries() {
        let started = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ended = UNIX_EPOCH + Duration::from_secs(1_800_000_030);
        // (case, the status the execution carries, the end instant it carries, what
        // endTimestamp must be)
        let rows: [(&str, &str, Option<SystemTime>, Option<i64>); 2] = [
            (
                "an execution still running",
                super::super::EXECUTION_STATUS_RUNNING,
                None,
                None,
            ),
            (
                "an execution that ended",
                super::super::EXECUTION_STATUS_SUCCEEDED,
                Some(ended),
                Some(1_800_000_030_000),
            ),
        ];
        for (case, status, end, expected) in rows {
            let change = StatusChange {
                region: "us-east-1".to_string(),
                durable_execution_arn: "arn:aws:lambda:us-east-1:000000000000:function:f:\
                                        $LATEST/durable-execution/one/abc"
                    .to_string(),
                durable_execution_name: "one".to_string(),
                function_arn: "arn:aws:lambda:us-east-1:000000000000:function:f".to_string(),
                function_name: "f".to_string(),
                status: status.to_string(),
                started,
                ended: end,
                at: end.unwrap_or(started),
            };
            let detail = change.detail();
            assert_eq!(
                detail.get("status").and_then(Value::as_str),
                Some(status),
                "case {case:?}: expected the status in the detail, got {detail}"
            );
            assert_eq!(
                detail.get("startTimestamp").and_then(Value::as_i64),
                Some(1_800_000_000_000),
                "case {case:?}: expected the start instant in epoch milliseconds, got {detail}"
            );
            let reported = detail.get("endTimestamp").and_then(Value::as_i64);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected endTimestamp {expected:?}, got {reported:?}"
            );
            for member in ["durableExecutionArn", "durableExecutionName", "functionArn", "functionName"] {
                assert!(
                    detail.get(member).and_then(Value::as_str).is_some(),
                    "case {case:?}: expected {member:?} in the detail, got {detail}"
                );
            }
        }
    }
}
