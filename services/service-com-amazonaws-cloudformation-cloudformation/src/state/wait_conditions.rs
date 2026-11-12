// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE-SIGNAL WAITS of one stack operation: the barrier a resource whose
//! declaration asks for signals puts in a create or an update, and the signals that release
//! it.
//!
//! CloudFormation "doesn't proceed with a stack creation or update until resources receive
//! the required number of signals or the timeout period is exceeded"
//! (`SignalResource`). TWO declarations ask for that wait, and this module models both,
//! because the wait is the same thing either way — the resource stays in its
//! `*_IN_PROGRESS` status until the signals arrive, and nothing that depends on it is
//! started before that:
//!
//! * an `AWS::CloudFormation::WaitCondition`, whose `Count` and `Timeout` properties say
//!   how many signals it waits for and for how long; and
//! * a `CreationPolicy`'s `ResourceSignal` on a resource of ANY type — "when
//!   CloudFormation creates the associated resource, it waits for a success signal" — whose
//!   `Count` and `Timeout` members say the same two things, the timeout as an ISO 8601
//!   duration.
//!
//! A template that declares either therefore has an operation whose completion is not owed
//! by the clock alone: it is owed once every such resource has the success signals its
//! `Count` asks for, and the resources that depend on one are not started before that.
//!
//! This module is the model of that barrier, and nothing else:
//!
//! * [`DeclaredWaitCondition`] is what a DEPLOYMENT computes — one entry per resource of the
//!   template whose declaration waits for signals, with the number of signals it waits for,
//!   how long it waits (bounded by the documented maximum, so no template leaves a stack in
//!   progress for ever), which declaration opened it, and the resources of the same template
//!   whose creation waits on it. It is
//!   computed where the template is read ([`crate::state::StoredStack::deploy_template`]),
//!   outside the store lock, because reading a caller-sized body is work the request pays
//!   for.
//! * [`WaitConditionBarrier`] is one of those, OPENED by the operation now in flight: the
//!   plan plus the instant that operation started.
//!   [`crate::state::StoredStack::settle`] will not complete an operation that still holds
//!   an unsatisfied barrier, and whether one is satisfied is read off the signals
//!   `SignalResource` recorded on the stack.
//!
//! ## Bounds
//!
//! Nothing here grows with what a caller sends: a barrier is the plan plus one instant,
//! and the signals themselves are the stack's own bounded log
//! ([`crate::state::MAX_SIGNALS_PER_STACK`]). The `Timeout` a template may declare is
//! bounded by its own documented maximum ([`MAX_TIMEOUT`]), and a declaration that states
//! none is held to the documented default ([`DEFAULT_TIMEOUT`]) rather than to no deadline,
//! so every wait has an instant it gives up at and no declaration holds a stack in progress
//! for longer than the published ceiling of the property that opened it. Reading the waits
//! out of a caller-supplied body is ONE pass over it whatever it declares ([`declared`]),
//! charged to that deployment's evaluation budget.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::template;
use super::StoredResource;

/// The resource type whose creation waits for signals by TYPE, as this crate's template
/// reader names it ([`template::WAIT_CONDITION`]).
pub use super::template::WAIT_CONDITION as WAIT_CONDITION_TYPE;

/// How many success signals a declaration that states no `Count` waits for.
///
/// `AWS::CloudFormation::WaitCondition`'s `Count` — "the number of success signals that
/// CloudFormation must receive before it continues the stack creation process" — is not a
/// required property, and a wait condition that states no count continues on one signal.
/// `CreationPolicy`'s `ResourceSignal.Count` documents the same default ("Default: 1").
pub const DEFAULT_SIGNAL_COUNT: usize = 1;

/// The longest a wait runs: "the maximum time that can be specified for this property is 12
/// hours (43200 seconds)" (`AWS::CloudFormation::WaitCondition`'s `Timeout`), which is the
/// same ceiling `CreationPolicy`'s `ResourceSignal.Timeout` documents ("Maximum: PT12H").
///
/// The documented ceiling of a duration the TEMPLATE asks for, so a declaration past it
/// waits this long rather than as long as it asked.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(43_200);

/// How long a wait runs for when the declaration that opened it states no timeout at all.
///
/// `CreationPolicy`'s `ResourceSignal.Timeout` — "the amount of time that CloudFormation
/// waits for the number of signals that the `Count` property specifies" — documents
/// "Default: PT5M", which is this span.
/// `AWS::CloudFormation::WaitCondition`'s own `Timeout` is a REQUIRED property, so a
/// declaration that omits it has asked for no particular wait and is held to that same
/// documented default rather than left without a deadline: no template leaves a stack in
/// progress for ever.
///
/// A declaration that DOES state a timeout waits exactly as long as it asked, bounded by
/// [`MAX_TIMEOUT`] ([`WaitConditionBarrier::deadline`]).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Which declaration opened a wait, which is what decides the operations it applies to and
/// how its expiry is worded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredBy {
    /// An `AWS::CloudFormation::WaitCondition`: a resource that exists to wait for signals,
    /// so it waits whichever operation deploys it.
    WaitCondition,
    /// A `CreationPolicy`'s `ResourceSignal` on a resource of another type: the attribute
    /// "is invoked during resource creation", so only a CREATE waits at it.
    CreationPolicy,
}

/// One resource-signal wait a deployment found in the template it is deploying.
///
/// Computed once, where the template is read, from the resource record the deployment
/// materialised for it and the declaration that record came from: the values here are the
/// declaration's own with every intrinsic function already evaluated, so nothing re-reads
/// the body later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredWaitCondition {
    /// The logical id the template declares the resource under, which is the
    /// `LogicalResourceId` a signal names.
    pub logical_id: String,
    /// `Count`: how many success signals release it.
    pub count: usize,
    /// `Timeout`: how long it waits for them, bounded by [`MAX_TIMEOUT`] and defaulted to
    /// [`DEFAULT_TIMEOUT`].
    pub timeout: Duration,
    /// The logical ids of the resources of the same template whose creation waits on this
    /// one: every resource that depends on it, directly or through another resource.
    /// CloudFormation creates a resource only once the resources it needs exist, so these
    /// are the ones the operation has not started while the barrier holds.
    pub blocks: BTreeSet<String>,
    /// Which declaration asked for the wait.
    pub declared_by: DeclaredBy,
}

impl DeclaredWaitCondition {
    /// Whether only a CREATE waits here, so an update that re-deploys the same resource is
    /// not held at it: a `CreationPolicy` governs the create alone, while a wait condition
    /// waits whichever operation deploys it.
    pub fn only_on_create(&self) -> bool {
        self.declared_by == DeclaredBy::CreationPolicy
    }
}

/// What one declaration asks of a wait, before the resources it blocks are known.
struct SignalPlan {
    count: usize,
    timeout: Duration,
    declared_by: DeclaredBy,
}

/// The wait `resource`'s declaration asks for, or `None` for one that asks for none.
///
/// `declaration` is the `Resources` entry the record was materialised from, which is where
/// the `CreationPolicy` resource attribute lives — a stored resource carries the evaluated
/// `Properties` alone. A `CreationPolicy`'s `ResourceSignal` is read in preference to the
/// properties, because that is the attribute AWS documents the signalling of a create on
/// ("we recommend that you use the `CreationPolicy` attribute"), and a wait condition that
/// carries one is a wait condition whose count and timeout the attribute states.
fn signal_plan(
    declaration: Option<&template::DeclaredResource>,
    resource: &StoredResource,
) -> Option<SignalPlan> {
    let wait_condition = resource.resource_type == WAIT_CONDITION_TYPE;
    let declared_by = if wait_condition {
        DeclaredBy::WaitCondition
    } else {
        DeclaredBy::CreationPolicy
    };
    if let Some(resource_signal) = declaration.and_then(creation_policy_resource_signal) {
        return Some(SignalPlan {
            count: signal_count(resource_signal.get("Count")),
            timeout: policy_timeout(resource_signal.get("Timeout")),
            declared_by,
        });
    }
    if !wait_condition {
        return None;
    }
    Some(SignalPlan {
        count: signal_count(resource.properties.get("Count")),
        timeout: declared_timeout(resource.properties.get("Timeout")),
        declared_by,
    })
}

/// The `ResourceSignal` member of a declaration's `CreationPolicy`, where it carries one.
fn creation_policy_resource_signal(
    declared: &template::DeclaredResource,
) -> Option<&::serde_json::Value> {
    declared
        .declaration
        .get("CreationPolicy")?
        .get("ResourceSignal")
}

/// `Count` as a declaration states it, or [`DEFAULT_SIGNAL_COUNT`].
///
/// A template may spell a numeric value as a number or as a string (both forms deploy), so
/// both are read; a value that is neither is no count at all and the default stands. Zero is
/// not a number of signals to wait for, so it is read as the default too.
fn signal_count(declared: Option<&::serde_json::Value>) -> usize {
    let declared = whole_number(declared).unwrap_or(DEFAULT_SIGNAL_COUNT as u64);
    let declared = usize::try_from(declared).unwrap_or(usize::MAX);
    declared.max(DEFAULT_SIGNAL_COUNT)
}

/// An `AWS::CloudFormation::WaitCondition`'s `Timeout` — "the number of seconds to wait" —
/// as the declaration states it, bounded by [`MAX_TIMEOUT`] and defaulted to
/// [`DEFAULT_TIMEOUT`].
fn declared_timeout(declared: Option<&::serde_json::Value>) -> Duration {
    whole_number(declared).map_or(DEFAULT_TIMEOUT, |seconds| {
        Duration::from_secs(seconds).min(MAX_TIMEOUT)
    })
}

/// A `CreationPolicy`'s `ResourceSignal.Timeout` as the declaration states it, bounded by
/// [`MAX_TIMEOUT`] and defaulted to [`DEFAULT_TIMEOUT`].
///
/// This one is "in ISO8601 duration format" — `PT10M` is ten minutes — so it is read through
/// the one ISO 8601 duration reading this workspace has rather than as a number of seconds:
/// a value the grammar does not admit states no wait and the default stands.
fn policy_timeout(declared: Option<&::serde_json::Value>) -> Duration {
    declared
        .and_then(::serde_json::Value::as_str)
        .and_then(::service_common::iso8601_span::iso8601_span)
        .map_or(DEFAULT_TIMEOUT, |timeout| timeout.min(MAX_TIMEOUT))
}

/// A whole number a template value states, spelled as a number or as a string.
fn whole_number(declared: Option<&::serde_json::Value>) -> Option<u64> {
    match declared? {
        ::serde_json::Value::Number(number) => number.as_u64(),
        ::serde_json::Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

/// Every resource-signal wait `resources` carries, with the resources each one blocks.
///
/// `template` is the body those records were materialised from, which is both where each
/// declaration's `CreationPolicy` lives and what says what depends on what: a resource that
/// names a waiting resource in a `DependsOn`, a `Ref` or an `Fn::GetAtt` — or that names
/// something which does — is one the operation cannot start until that wait is satisfied.
///
/// ONE pass over the template for the whole set ([`template::waits_on_each`]), charged to
/// the deployment's budget, however many waits the body declares: the relation is computed
/// once and inverted, rather than walked per wait.
pub fn declared(
    template: &template::Template,
    resources: &[StoredResource],
    budget: &mut template::Budget,
) -> Vec<DeclaredWaitCondition> {
    let declarations: BTreeMap<&str, &template::DeclaredResource> = template
        .resources
        .iter()
        .map(|declared| (declared.logical_id.as_str(), declared))
        .collect();
    let waiting: Vec<(&StoredResource, SignalPlan)> = resources
        .iter()
        .filter_map(|resource| {
            let declaration = declarations.get(resource.logical_id.as_str()).copied();
            signal_plan(declaration, resource).map(|plan| (resource, plan))
        })
        .collect();
    if waiting.is_empty() {
        return Vec::new();
    }
    let roots: BTreeSet<String> = waiting
        .iter()
        .map(|(resource, _)| resource.logical_id.clone())
        .collect();
    // The relation the other way round: per waiting resource, the resources that wait on it.
    let mut blocked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (logical_id, waits_on) in template::waits_on_each(template, &roots, budget) {
        for root in waits_on {
            blocked.entry(root).or_default().insert(logical_id.clone());
        }
    }
    waiting
        .into_iter()
        .map(|(resource, plan)| DeclaredWaitCondition {
            logical_id: resource.logical_id.clone(),
            count: plan.count,
            timeout: plan.timeout,
            blocks: blocked.remove(&resource.logical_id).unwrap_or_default(),
            declared_by: plan.declared_by,
        })
        .collect()
}

/// One resource-signal wait the operation in flight is waiting at: what the template
/// declared, and when the wait started.
///
/// The SIGNALS it waits for are not held here: `SignalResource` records every signal on
/// the stack itself ([`crate::state::StoredStack::record_signal`], keyed by the signalled
/// logical id and the signal's `UniqueId` and bounded there), so the one record of what
/// has arrived is that log, and whether this barrier is satisfied is a question about it
/// ([`crate::state::StoredStack::barrier_satisfied`]).
#[derive(Debug, Clone)]
pub struct WaitConditionBarrier {
    /// What the deployment computed for this wait.
    pub declared: DeclaredWaitCondition,
    /// The instant the operation that opened the barrier started — which is when the wait
    /// began, and therefore what its `Timeout` runs from.
    pub opened_at: SystemTime,
}

impl WaitConditionBarrier {
    /// Open the barrier of an operation starting at `now`.
    pub fn open(declared: DeclaredWaitCondition, now: SystemTime) -> Self {
        Self {
            declared,
            opened_at: now,
        }
    }

    /// The logical id a signal names this wait by.
    pub fn logical_id(&self) -> &str {
        &self.declared.logical_id
    }

    /// How many success signals release it (`Count`).
    pub fn count(&self) -> usize {
        self.declared.count
    }

    /// Whether this resource's creation waits on this one.
    pub fn blocks(&self, logical_id: &str) -> bool {
        self.declared.blocks.contains(logical_id)
    }

    /// The instant the wait gives up.
    ///
    /// The wait is HELD for the span the declaration asked for, which is the span the
    /// resource it gates stays `*_IN_PROGRESS` — and reportable — for: nothing but a client's
    /// own `SignalResource` call ends it before then, and that is the point of it.
    /// "CloudFormation doesn't proceed with a stack creation or update until resources receive
    /// the required number of signals or the timeout period is exceeded" (`SignalResource`),
    /// so the span a client has to poll the gate in and signal it is the span its own
    /// declaration stated. That value is already held to the maximum its property publishes
    /// ([`MAX_TIMEOUT`]) and to the documented default where it states none
    /// ([`DEFAULT_TIMEOUT`]) where the declaration is read ([`declared`]), so the wait runs
    /// for exactly what the caller asked for and still has an instant it gives up at, after
    /// which the stack reaches a terminal status.
    ///
    /// Added without unwinding: `SystemTime`'s own addition panics past the clock's range,
    /// which no bounded span can reach from an instant the operation really started at, so
    /// an instant that could not be computed is read as a wait already over rather than as a
    /// failure of the read.
    pub fn deadline(&self) -> SystemTime {
        self.opened_at
            .checked_add(self.declared.timeout)
            .unwrap_or(self.opened_at)
    }

    /// Whether the wait has run out at `now` — which fails the operation for a wait that is
    /// not satisfied by then: "if the wait condition doesn't receive the specified number of
    /// success signals before the `Timeout` period expires, CloudFormation assumes that the
    /// wait condition has failed".
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.deadline() <= now
    }

    /// Why the operation failed, for a wait that ran out: as CloudFormation words it for the
    /// declaration that opened the wait — a wait condition reports what it was waiting for
    /// against what it had `received`, while a creation policy reports the signals it never
    /// got.
    pub fn timed_out_reason(&self, received: usize) -> String {
        match self.declared.declared_by {
            DeclaredBy::WaitCondition => format!(
                "WaitCondition timed out. Received {received} conditions when expecting {}",
                self.declared.count
            ),
            DeclaredBy::CreationPolicy => format!(
                "Failed to receive {} resource signal(s) within the specified duration",
                self.declared.count.saturating_sub(received)
            ),
        }
    }
}

/// The reason CloudFormation records on the event for a signal it RECEIVED, naming the
/// signal's status and the unique id it arrived under — the two things one signal carries.
///
/// The one wording for every signal this service records, whoever sent it: the client's own
/// `SignalResource` call ([`crate::ops::signal_resource`]) and the instances of a signalled
/// resource ([`super::provisioning::instance_signals`]) report a signal the same way,
/// because what the event reports is the signal rather than the caller.
pub fn signal_received(status: &str, unique_id: &str) -> String {
    format!("Received {status} signal with UniqueId {unique_id}")
}

/// The status an operation an expired wait FAILED leaves the stack in, by the
/// `*_IN_PROGRESS` status that operation was in.
///
/// A rollback has a failed status of its own — `UPDATE_ROLLBACK_FAILED` is the state
/// `ContinueUpdateRollback` resumes from — and a create's and an update's are the ones every
/// other failure of those operations reports ([`super::failed_status_of`]).
fn failed_status(in_progress: &str) -> &'static str {
    match in_progress {
        super::status::UPDATE_ROLLBACK_IN_PROGRESS => super::status::UPDATE_ROLLBACK_FAILED,
        super::status::ROLLBACK_IN_PROGRESS => super::status::ROLLBACK_FAILED,
        other => super::failed_status_of(other),
    }
}

/// What the failure of an expired wait OWES the services that own the stack's resources —
/// work no read of the store can do, since each item of it is a call to another service.
///
/// Answered by [`super::StoredStack::fail_expired_wait`] to the caller that is in a position
/// to make those calls ([`super::provisioning::advance_stabilisations`]). It is what ANY
/// clock-driven failure of the same operation owes, so the rollback a stack's own triggers
/// force answers with it too ([`super::StoredStack::fail_on_rollback_triggers`]).
#[derive(Debug, Clone)]
pub enum ExpiredWaitOwes {
    /// A failed CREATE: "ROLLBACK — Roll back the stack creation. This is the default", so
    /// the resources the create had already made are REMOVED again in the services that own
    /// them, with the records the stack holds and the policies that rollback reads
    /// ([`super::provisioning::unwind`]).
    Removal {
        resources: Vec<StoredResource>,
        held: BTreeSet<String>,
        needs: super::provisioning::Dependencies,
        policies: super::provisioning::Policies,
    },
    /// A failed UPDATE: the previous stack configuration goes back in the services that own
    /// the resources the update changed, which is the one path every rollback this service
    /// owes goes through ([`super::settle_due_rollbacks`]).
    Revert,
}

impl super::StoredStack {
    /// Materialise the failure an EXPIRED, unsatisfied wait of the operation in flight owes
    /// at `now`, and answer the work that failure leaves for the services that own the
    /// stack's resources.
    ///
    /// "If the wait condition doesn't receive the specified number of success signals before
    /// the `Timeout` period expires, CloudFormation assumes that the wait condition has
    /// failed": the operation takes its failed status carrying that reason — owed at that
    /// instant even if nobody had called — and a failed operation does not stop there. Which
    /// operation expired decides what it owes:
    ///
    /// * a CREATE is rolled back ("ROLLBACK … This is the default"), so the stack goes
    ///   through `ROLLBACK_IN_PROGRESS` to `ROLLBACK_COMPLETE` and the resources it had made
    ///   are removed again ([`ExpiredWaitOwes::Removal`]);
    /// * an UPDATE is rolled back to the previous stack configuration, which the stack
    ///   records as owed ([`super::StoredStack::begin_owed_update_rollback`]) and the caller
    ///   performs ([`ExpiredWaitOwes::Revert`]) — and the rollback itself WAITS at the wait
    ///   conditions the configuration it puts back declares and that it has to re-create,
    ///   since such a wait condition is created again and waits for its own signals;
    /// * a ROLLBACK that expires at one of those cannot be carried through, so the stack is
    ///   left in the failed rollback status — `UPDATE_ROLLBACK_FAILED`, the state
    ///   `ContinueUpdateRollback` resumes from — with what the owning services are still
    ///   holding kept for that resumption.
    ///
    /// Nothing is rolled back for a caller that asked for the failed state to be kept
    /// (`DisableRollback`, `OnFailure=DO_NOTHING`), which is the one operation that parks in
    /// its failed status.
    ///
    /// Called with the store locked and the result acted on with it RELEASED. `None` is a
    /// stack with no wait that has run out, and one whose failure owes those services
    /// nothing.
    pub fn fail_expired_wait(&mut self, now: SystemTime) -> Option<ExpiredWaitOwes> {
        // A barrier left behind by an operation that has already concluded belongs to no
        // operation: there is nothing to fail.
        self.pending.as_ref()?;
        let reason = self
            .wait_conditions
            .iter()
            .find(|barrier| barrier.has_expired(now) && !self.barrier_satisfied(barrier))
            .map(|barrier| {
                barrier.timed_out_reason(self.successful_signals(barrier.logical_id()))
            })?;
        let creating = self.status == super::status::CREATE_IN_PROGRESS;
        let updating = self.status == super::status::UPDATE_IN_PROGRESS;
        // A ROLLBACK is the third operation a wait can run out in: it deploys the previous
        // configuration, so a wait condition it puts back waits as any deployment's does.
        let rolling_back = !creating && !updating;
        let failed_status = failed_status(&self.status);
        // The RESOURCE the wait was for did not come up: its own terminal event becomes that
        // failure rather than the completion the walk had planned for it, so the event stream
        // says which resource the operation failed at and why — the same rewrite a resource
        // its owning service never got ready gets
        // ([`super::StoredStack::fail_stabilisation`]).
        let expired: Vec<String> = self
            .wait_conditions
            .iter()
            .filter(|barrier| barrier.has_expired(now) && !self.barrier_satisfied(barrier))
            .map(|barrier| barrier.logical_id().to_string())
            .collect();
        for logical_id in &expired {
            let deployed_before = self
                .resources
                .iter()
                .any(|deployed| &deployed.resource.logical_id == logical_id);
            let resource_status = if rolling_back || (deployed_before && updating) {
                super::resource_status::UPDATE_FAILED
            } else {
                super::resource_status::CREATE_FAILED
            };
            self.fail_planned_resource_event(logical_id, resource_status, &reason);
            if rolling_back {
                // "You can specify only resources that are in the UPDATE_FAILED state
                // because a rollback failed" (`ContinueUpdateRollback$ResourcesToSkip`): the
                // resource the rollback could not get through is the one in that state, so
                // that is what it reports — with the event that says so.
                self.fail_deployed_resource(logical_id, resource_status, &reason, now);
            }
        }
        // What a rollback of this update would have to put back, and so which of the
        // previous configuration's wait conditions it re-creates — read BEFORE the failure
        // below retires the plan that says so.
        let rollback_waits = if updating {
            self.waits_the_rollback_deploys()
        } else {
            Vec::new()
        };
        // Whether the stack RESTS in the failed status decides whether an event of its own
        // reports it: a create this failure rolls back, and an update it reverts, move into
        // that rollback at this very instant and the rollback's own event carries the
        // failure ([`super::StoredStack::fail_transition_superseded`]).
        if !self.disable_rollback && (updating || creating) {
            self.fail_transition_superseded(now, failed_status, reason.clone());
        } else {
            self.fail_transition(now, failed_status, reason.clone());
        }
        if self.disable_rollback {
            return None;
        }
        if updating {
            self.begin_owed_update_rollback(now, reason);
            // The rollback deploys the previous configuration, so a wait condition it
            // re-creates waits for signals exactly as the deployment that first created it
            // did: the rollback is not complete while one of them is unsatisfied, and a wait
            // that runs out leaves it UPDATE_ROLLBACK_FAILED.
            self.wait_conditions = rollback_waits
                .into_iter()
                .map(|declared| WaitConditionBarrier::open(declared, now))
                .collect();
            self.retire_signals_of_open_waits();
            return Some(ExpiredWaitOwes::Revert);
        }
        if !creating {
            // A ROLLBACK that could not be carried through: the stack keeps what the owning
            // services are still holding, because that is what a resumed rollback has to put
            // back ([`crate::ops::continue_update_rollback`]).
            return None;
        }
        self.begin_rollback(now, reason);
        Some(ExpiredWaitOwes::Removal {
            resources: self.current_resources(),
            held: self.held_resource_ids(),
            needs: super::provisioning::dependencies_in_flight(self),
            policies: self.rolled_back_resource_policies(),
        })
    }

    /// Record that the resource `logical_id` names is the one the operation in flight could
    /// not get through, with the event that says so.
    ///
    /// The record a client reads back for the resource takes the failed status and the
    /// reason, and the stack's event log carries that transition: a rollback that could not
    /// be carried through has to say WHICH resource it stopped at, because that is the
    /// resource a resumed rollback may be told to skip.
    fn fail_deployed_resource(
        &mut self,
        logical_id: &str,
        resource_status: &str,
        reason: &str,
        now: SystemTime,
    ) {
        let Some(deployed) = self
            .resources
            .iter_mut()
            .find(|deployed| deployed.resource.logical_id == logical_id)
        else {
            return;
        };
        deployed.status = resource_status.to_string();
        deployed.status_timestamp = now;
        deployed.status_reason = Some(reason.to_string());
        let physical_resource_id = deployed.resource.physical_resource_id.clone();
        let resource_type = deployed.resource.resource_type.clone();
        let operation_id = self
            .last_operation()
            .map(|operation| operation.operation_id.clone());
        let client_request_token = self.client_request_token.clone();
        self.record_resource_event(
            now,
            logical_id,
            Some(physical_resource_id),
            &resource_type,
            resource_status,
            Some(reason.to_string()),
            None,
            operation_id,
            client_request_token,
        );
    }

    /// The wait conditions of the configuration a rollback of the update in flight puts back
    /// that the rollback DEPLOYS again.
    /// A rollback deploys the previous configuration of the resources the failed update
    /// changed — the ones the plan it holds has under `previous` differently from what that
    /// update `applied` ([`super::OwedRollback`]) — and a wait condition it deploys again is
    /// not the satisfied one the stack had: "you can't update a wait condition that has
    /// already been satisfied unless you also change the handle", which is the same reading
    /// of a declaration that decides whether the UPDATE waits at one
    /// ([`super::StoredStack::opening_barriers`]). A wait condition the update left exactly
    /// as it was is not one the rollback deploys, and a `CreationPolicy`'s wait governs a
    /// create alone.
    fn waits_the_rollback_deploys(&self) -> Vec<DeclaredWaitCondition> {
        let Some(plan) = self.rollback_plan.as_ref() else {
            return Vec::new();
        };
        let deployed_again: BTreeSet<&str> = plan
            .previous
            .iter()
            .filter(|previous| {
                plan.applied
                    .iter()
                    .find(|applied| applied.logical_id == previous.logical_id)
                    .is_none_or(|applied| {
                        applied.physical_resource_id != previous.physical_resource_id
                            || applied.properties != previous.properties
                    })
            })
            .map(|previous| previous.logical_id.as_str())
            .collect();
        if deployed_again.is_empty() {
            return Vec::new();
        }
        // The template that is going back in force is the one the previous configuration
        // carries, which is also where those resources were declared.
        let Some(body) = self
            .previous_configuration
            .as_ref()
            .and_then(|configuration| {
                configuration
                    .processed_template_body
                    .as_deref()
                    .or(configuration.template_body.as_deref())
            })
        else {
            return Vec::new();
        };
        let Some(declared_template) = template::parse(body) else {
            return Vec::new();
        };
        let mut budget = template::Budget::new();
        declared(&declared_template, &plan.previous, &mut budget)
            .into_iter()
            .filter(|wait| {
                !wait.only_on_create() && deployed_again.contains(wait.logical_id.as_str())
            })
            .collect()
    }

    /// Drop the signals recorded for a wait condition the operation in flight is waiting at
    /// that arrived BEFORE that wait was opened.
    ///
    /// "If you send multiple signals to a single resource (such as signaling a wait
    /// condition), each signal requires a different unique ID" — and the signals a wait
    /// condition receives are the ones sent to the handle it was created with. A deployment
    /// that CREATES a wait condition again (the template changed its handle, or a rollback
    /// put the previous one back) is therefore waiting on a wait condition that has received
    /// nothing: the signals that satisfied the earlier one satisfied THAT deployment, and
    /// cannot stand in for the signals this one is waiting for. A wait condition the stack
    /// already has and the deployment declares unchanged opens no barrier at all
    /// ([`super::StoredStack::opening_barriers`]), so its signals are left exactly as they
    /// are.
    ///
    /// Called by the operation that opened the barriers, right after it did.
    pub fn retire_signals_of_open_waits(&mut self) {
        if self.wait_conditions.is_empty() || self.signals.is_empty() {
            return;
        }
        let opened: Vec<(String, SystemTime)> = self
            .wait_conditions
            .iter()
            .map(|barrier| (barrier.logical_id().to_string(), barrier.opened_at))
            .collect();
        self.signals.retain(|signal| {
            !opened.iter().any(|(logical_id, opened_at)| {
                *logical_id == signal.logical_resource_id && signal.received_at < *opened_at
            })
        });
    }
}
