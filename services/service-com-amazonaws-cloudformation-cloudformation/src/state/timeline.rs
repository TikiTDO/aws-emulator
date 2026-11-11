// SPDX-License-Identifier: CC0-1.0

//! The TIMELINE of one stack operation's walk over a stack's resources: when each resource
//! was SCHEDULED and when it FINISHED.
//!
//! A stack operation is not one instant but a walk over the dependency graph the template
//! describes, and CloudFormation reports that walk through `DescribeStackEvents`: a
//! resource is recorded `*_IN_PROGRESS` when the operation schedules it and
//! `*_COMPLETE`/`*_FAILED` when it finishes, each `StackEvent` carrying its own `Timestamp`
//! ("Time the status was updated"). One operation's event log is therefore a timeline, and
//! its shape is the shape of the graph:
//!
//! * two resources with no dependency between them are scheduled together, so their
//!   `[scheduled, finished]` intervals OVERLAP;
//! * a resource that depends on another is scheduled only once that one has finished;
//! * a join is scheduled only once the last of its predecessors has finished;
//! * a resource that genuinely takes time — an `AWS::CloudFormation::WaitCondition` held
//!   unsignalled — has a correspondingly long interval and holds everything behind it;
//! * a DELETION walks the same graph backwards, so a dependent is gone before its
//!   dependency's deletion starts and independent branches come down together.
//!
//! What a client does with that: a deployment dashboard reports what a deploy is doing, how
//! long each resource took and which resource is holding the stack open; timing tooling
//! measures a deploy. A log in which every resource was scheduled and finished at one
//! instant answers none of those questions.
//!
//! This module is the plan of that walk and nothing else. [`Timeline`] holds the steps one
//! operation owes events for, [`Timeline::times`] computes when each step is scheduled and
//! finishes, and [`crate::state::StoredStack::settle`] is what records the events whose
//! instant the clock has reached — so a client polling mid-operation reads the walk as far
//! as it has got and never an event stamped in the future.
//!
//! ## Where the instants come from
//!
//! The operation's own span is the one the stack's clock already owes
//! ([`crate::state::STATUS_TRANSITION_DELAY`]): the walk is laid out INSIDE that span, one
//! [`Timeline::unit`] per LEVEL of the graph, so a stack completes exactly when it did
//! before and no resource finishes after the operation that acted on it. Nothing sleeps —
//! each step's instants are computed from the instant the operation started — and the one
//! thing that stretches a walk is a wait condition, whose completion is owed by the signal
//! that satisfies it rather than by the clock, so the steps behind it are scheduled from
//! that signal's arrival.
//!
//! Inside one step, the instant a record carries is the instant the thing it reports
//! HAPPENED: the step's last event is the resource finishing, and every record before it
//! reports something the owning service was already asked for — the create CloudFormation
//! records as initiated once the request for it has gone to that service — so those fall at
//! the START of the step ([`StepTimes::event_at`]) and are in the log for the whole of the
//! span the resource then takes, however that span ends.
//!
//! ## Bounds
//!
//! One step per resource transition the walk recorded, which a stack operation records at
//! most twice per resource it acts on — bounded by the resources a stack may hold
//! ([`crate::state::MAX_CONCURRENT_RESOURCES`]) — and the steps are dropped as soon as the
//! last event they owe has been recorded. [`Timeline::times`] is one pass over them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::provisioning::ResourceTransition;

/// The suffix of every `ResourceStatus` that OPENS a resource's transition
/// ([`super::resource_status`]), which is what tells the two halves of a step apart. The
/// same suffix a STACK's own in-progress statuses carry, so there is one spelling of it.
const IN_PROGRESS: &str = super::IN_PROGRESS_SUFFIX;

/// The prefix of every `ResourceStatus` of a resource being DELETED, which is the half of
/// the walk that runs against the dependency relation rather than along it.
const DELETE: &str = "DELETE_";

/// How far apart the records a step makes at its START are stamped.
///
/// Every event of a step but the last reports work the service that owns the resource has
/// already been ASKED for, so they all happened as the walk reached the resource rather than
/// across the span it then spends on it ([`StepTimes::event_at`]). They are stamped this far
/// apart so each still carries an instant of its own and a read that orders the log by
/// `StackEvent$Timestamp` reports them in the order the walk made them. This short, because
/// what it separates is two records of one answer and not a span anything waits for: the
/// record that a resource's work has STARTED is in the log by the time any client can read
/// the log at all, however long the resource then takes.
const RECORD_TICK: Duration = Duration::from_micros(1);

/// One `StackEvent` a step of the walk owes about one resource.
#[derive(Debug, Clone)]
pub struct PlannedEvent {
    /// `StackEvent$PhysicalResourceId`, which a resource the walk has only STARTED on does
    /// not have yet.
    pub physical_resource_id: Option<String>,
    /// `StackEvent$ResourceStatus`.
    pub status: String,
    /// `StackEvent$ResourceStatusReason`.
    pub reason: Option<String>,
    /// `StackEvent$ResourceProperties`.
    pub resource_properties: Option<String>,
    /// Whether this event has been recorded on the stack's log already: the walk's events
    /// are recorded as the clock reaches them, so each one is written exactly once.
    pub recorded: bool,
}

impl PlannedEvent {
    /// The event one recorded transition owes.
    fn of(transition: &ResourceTransition) -> Self {
        Self {
            physical_resource_id: transition.physical_resource_id.clone(),
            status: transition.status.clone(),
            reason: transition.reason.clone(),
            resource_properties: transition.resource_properties.clone(),
            recorded: false,
        }
    }

    /// Whether this event OPENS a transition rather than ending it.
    pub fn in_progress(&self) -> bool {
        self.status.ends_with(IN_PROGRESS)
    }
}

/// One resource the walk ACTED ON: the events of its transition, in the order the walk
/// recorded them, and what it had to wait for.
#[derive(Debug, Clone)]
pub struct Step {
    /// `StackEvent$LogicalResourceId`.
    pub logical_id: String,
    /// `StackEvent$ResourceType`.
    pub resource_type: String,
    /// The events this step owes: the first OPENS the transition (`*_IN_PROGRESS`) and the
    /// last ENDS it (`*_COMPLETE`/`*_FAILED`), with whatever the walk recorded in between —
    /// the second `CREATE_IN_PROGRESS` that says the create was initiated in the owning
    /// service. The one that ends it carries the instant the resource finished; the records
    /// before it carry the instant the walk ASKED, at the start of the span it spends on
    /// this resource ([`StepTimes::event_at`]).
    pub events: Vec<PlannedEvent>,
    /// The logical ids of the OTHER resources of this walk this one waits for.
    pub after: BTreeSet<String>,
    /// Those same resources as the STEPS of this walk they name ([`resolve_order`]), which
    /// is what the instants are computed from: a step is scheduled once every one of these
    /// has finished.
    pub waits_for: Vec<usize>,
    /// Whether this step DELETES its resource, which is what decides the direction it reads
    /// the dependency relation in.
    pub deleting: bool,
    /// Whether what ENDS this step is owed by a resource signal rather than by the clock:
    /// a wait condition the operation is waiting at finishes when its signals arrive.
    pub gated: bool,
    /// Which pass of the walk this step belongs to: a deployment creates what the template
    /// declares and only then deletes what it replaced, so those removals are a second
    /// pass, which starts once the first has finished.
    pub pass: usize,
}

impl Step {
    /// Whether the walk has recorded no END for this step yet, so a transition it records
    /// next for the same resource belongs to this step rather than opening another.
    fn is_open(&self) -> bool {
        self.events
            .last()
            .is_some_and(PlannedEvent::in_progress)
    }
}

/// When one step is scheduled and when it finishes.
///
/// `None` for an instant the walk cannot have reached: a step waiting behind a wait
/// condition that is still short of the signals it waits for has neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepTimes {
    pub scheduled_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
}

/// The walk one stack operation performed over a stack's resources, and the events it owes
/// for it.
#[derive(Debug, Clone)]
pub struct Timeline {
    /// The instant the operation that owns this walk started — what the whole timeline is
    /// measured from.
    pub started_at: SystemTime,
    /// How long ONE level of the dependency graph takes, so that the deepest chain of the
    /// walk still finishes inside the operation's own span.
    pub unit: Duration,
    /// The steps, in the order the walk recorded them.
    pub steps: Vec<Step>,
    /// The order the steps RESOLVE in ([`resolution_order`]): indices into `steps`, each
    /// after the ones it waits for, which is what lets the instants be computed in one pass.
    pub order: Vec<usize>,
    /// The operation every event of this walk belongs to: "All events triggered by a given
    /// stack operation are assigned the same client request token".
    pub operation_id: Option<String>,
    pub client_request_token: Option<String>,
}

/// Plan the walk `transitions` recorded, or `None` when the operation acted on nothing.
///
/// `dependencies` is the direct dependency relation of the template the operation deploys
/// ([`super::template::direct_dependencies`]); `gated` names the resources whose completion
/// is owed by a signal rather than by the clock (the wait conditions the operation is
/// waiting at); and `window` is the span the operation's own clock owes, which the walk is
/// laid out inside.
pub fn plan(
    started_at: SystemTime,
    window: Duration,
    transitions: &[ResourceTransition],
    dependencies: &BTreeMap<String, BTreeSet<String>>,
    gated: &BTreeSet<String>,
    operation_id: Option<String>,
    client_request_token: Option<String>,
) -> Option<Timeline> {
    let mut steps = steps_of(transitions, gated);
    if steps.is_empty() {
        return None;
    }
    resolve_order(&mut steps, dependencies);
    let order = resolution_order(&steps);
    let unit = level_span(&steps, &order, window);
    Some(Timeline {
        started_at,
        unit,
        steps,
        order,
        operation_id,
        client_request_token,
    })
}

/// The steps the recorded transitions make up: one per transition of one resource, carrying
/// every event the walk recorded for it.
///
/// A transition the walk records for a resource whose step is still OPEN belongs to that
/// step — the second `CREATE_IN_PROGRESS` of a create, and the terminal that ends it. One it
/// records for a resource whose step has already ended opens another, which is what a
/// deployment that replaces a resource does (it creates one and deletes the one it
/// replaced), so no record the walk made is dropped or merged into the wrong transition.
fn steps_of(transitions: &[ResourceTransition], gated: &BTreeSet<String>) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for transition in transitions {
        let event = PlannedEvent::of(transition);
        let open = steps
            .iter_mut()
            .rev()
            .find(|step| step.logical_id == transition.logical_id && step.is_open());
        if let Some(step) = open {
            if !event.in_progress() {
                // What ENDS the step: a wait condition the operation is waiting at is not
                // finished by the clock but by the signals it waits for.
                step.gated = gated.contains(&transition.logical_id);
            }
            step.events.push(event);
            continue;
        }
        steps.push(Step {
            logical_id: transition.logical_id.clone(),
            resource_type: transition.resource_type.clone(),
            gated: !event.in_progress() && gated.contains(&transition.logical_id),
            events: vec![event],
            after: BTreeSet::new(),
            waits_for: Vec::new(),
            deleting: transition.status.starts_with(DELETE),
            pass: 0,
        });
    }
    // A removal the deployment made because the new template replaced the resource happens
    // after everything that deployment CREATED: a second pass of the same walk.
    let creates = steps.iter().any(|step| !step.deleting);
    for step in &mut steps {
        if step.deleting && creates {
            step.pass = 1;
        }
    }
    steps
}

/// Fill in what each step waits for.
///
/// A step that CREATES or UPDATES a resource waits for the steps of the resources that
/// resource depends on; a step that DELETES one waits for the steps of the resources that
/// depend on IT — "a resource is deleted before whatever it depends on"
/// ([`super::provisioning::delete`]) — which is the same relation read backwards. Only
/// resources this walk itself acted on are waited for: one the operation left alone is
/// already there.
///
/// Each of those is then resolved to the STEP it names ([`Step::waits_for`]): the step of
/// that resource in this step's own pass, or the latest one in an earlier pass. The
/// relation is therefore read off the dependencies alone — never off the order the walk
/// happened to record its transitions in — and it cannot point from one pass back into a
/// later one, so the steps a step waits for always precede it.
fn resolve_order(steps: &mut [Step], dependencies: &BTreeMap<String, BTreeSet<String>>) {
    let acted_on: BTreeSet<String> = steps
        .iter()
        .map(|step| step.logical_id.clone())
        .collect();
    // The relation the other way round, built once: per resource, the resources that
    // depend on it.
    let mut dependents: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (dependent, needs) in dependencies {
        if !acted_on.contains(dependent) {
            continue;
        }
        for need in needs {
            dependents
                .entry(need.as_str())
                .or_default()
                .insert(dependent.clone());
        }
    }
    // The last step of each resource per pass, which is what a step of that pass or a later
    // one waits for.
    let mut by_resource: BTreeMap<(&str, usize), usize> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate() {
        by_resource.insert((step.logical_id.as_str(), step.pass), index);
    }
    // The step of the SAME resource the walk recorded BEFORE this one, where there is one.
    // A walk that acted on one resource twice did the second thing AFTER the first had ended
    // — a resource whose failed change was then put back, a resource created and then
    // deleted again — so the later step is scheduled from the earlier one's end rather than
    // beside it, and the two transitions read in the order they happened.
    let previous_of_resource: Vec<Option<usize>> = {
        let mut previous: Vec<Option<usize>> = vec![None; steps.len()];
        let mut last_seen: BTreeMap<String, usize> = BTreeMap::new();
        for (index, step) in steps.iter().enumerate() {
            if let Some(earlier) = last_seen.insert(step.logical_id.clone(), index) {
                previous[index] = Some(earlier);
            }
        }
        previous
    };
    let resolved: Vec<(BTreeSet<String>, Vec<usize>)> = steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let mut after = if step.deleting {
                dependents
                    .get(step.logical_id.as_str())
                    .cloned()
                    .unwrap_or_default()
            } else {
                dependencies
                    .get(&step.logical_id)
                    .map(|needs| {
                        needs
                            .iter()
                            .filter(|need| acted_on.contains(need.as_str()))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            };
            after.remove(&step.logical_id);
            let mut waits_for: Vec<usize> = after
                .iter()
                .filter_map(|waited_for| {
                    (0..=step.pass).rev().find_map(|pass| {
                        by_resource
                            .get(&(waited_for.as_str(), pass))
                            .copied()
                            .filter(|waited_for| *waited_for != index)
                    })
                })
                .collect();
            if let Some(earlier) = previous_of_resource.get(index).copied().flatten() {
                waits_for.push(earlier);
            }
            (after, waits_for)
        })
        .collect();
    for (step, (after, waits_for)) in steps.iter_mut().zip(resolved) {
        step.after = after;
        step.waits_for = waits_for;
    }
}

/// The order the walk's steps RESOLVE in: every step after the steps it waits for and after
/// every step of an earlier pass.
///
/// What this is for: the instants are computed in one pass over the steps
/// ([`Timeline::times`]), so each step has to come after everything it is computed from —
/// which the order the walk RECORDED its transitions in is, but is not guaranteed to be,
/// since that order is the provisioning walk's rather than this relation's. A relation no
/// order can satisfy (a cycle, which no deployable template has) leaves the steps it holds
/// at the end, in the order they were recorded, so every step is still resolved exactly
/// once.
fn resolution_order(steps: &[Step]) -> Vec<usize> {
    let mut resolved = vec![false; steps.len()];
    let mut order: Vec<usize> = Vec::with_capacity(steps.len());
    // How many steps of each pass are still to resolve, so "after every step of an earlier
    // pass" costs a lookup rather than a scan.
    let mut outstanding: BTreeMap<usize, usize> = BTreeMap::new();
    for step in steps {
        *outstanding.entry(step.pass).or_insert(0) += 1;
    }
    // One round per step at most: each round resolves every step whose own are out.
    for _ in 0..steps.len() {
        let mut progressed = false;
        for (index, step) in steps.iter().enumerate() {
            if resolved[index] {
                continue;
            }
            let earlier_pass_open = outstanding
                .iter()
                .any(|(pass, left)| *pass < step.pass && *left > 0);
            let waiting = step
                .waits_for
                .iter()
                .any(|waited_for| !resolved.get(*waited_for).copied().unwrap_or(true));
            if earlier_pass_open || waiting {
                continue;
            }
            resolved[index] = true;
            if let Some(left) = outstanding.get_mut(&step.pass) {
                *left = left.saturating_sub(1);
            }
            order.push(index);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    for (index, settled) in resolved.iter().enumerate() {
        if !settled {
            order.push(index);
        }
    }
    order
}

/// How long one LEVEL of this walk takes, so the deepest chain of it still finishes inside
/// `window`.
///
/// The walk's depth is the longest chain of steps that wait for one another, counting a
/// later pass as starting after every earlier one. The span is one level short of dividing
/// the window exactly, so the last step of the deepest chain finishes BEFORE the operation
/// that made it — which holds for a chain of any depth, since the span shrinks with the
/// depth rather than being floored at some length of its own. It is never ZERO, though, so a
/// level is always a span and not an instant.
fn level_span(steps: &[Step], order: &[usize], window: Duration) -> Duration {
    let mut level = vec![0usize; steps.len()];
    let mut pass_depth: BTreeMap<usize, usize> = BTreeMap::new();
    for index in order.iter().copied() {
        let Some(step) = steps.get(index) else {
            continue;
        };
        let mut at = 0usize;
        for (pass, depth) in &pass_depth {
            if *pass < step.pass {
                at = at.max(depth + 1);
            }
        }
        for waited_for in &step.waits_for {
            if let Some(reached) = level.get(*waited_for) {
                at = at.max(reached + 1);
            }
        }
        if let Some(reached) = level.get_mut(index) {
            *reached = at;
        }
        pass_depth
            .entry(step.pass)
            .and_modify(|depth| *depth = (*depth).max(at))
            .or_insert(at);
    }
    let levels = u32::try_from(level.iter().copied().max().unwrap_or(0))
        .unwrap_or(u32::MAX)
        .saturating_add(2);
    (window / levels).max(Duration::from_nanos(1))
}

impl Timeline {
    /// When each step of the walk is scheduled and when it finishes.
    ///
    /// `released` is the instant each gated step's signals arrived, by logical id: a gated
    /// step absent from it has not been released, so it has not finished and neither has
    /// anything waiting behind it.
    ///
    /// ONE pass, in the order the steps resolve in ([`resolution_order`]), so every step is
    /// computed from steps already computed — whatever order the walk recorded them in.
    pub fn times(&self, released: &BTreeMap<String, SystemTime>) -> Vec<StepTimes> {
        let mut times: Vec<StepTimes> = vec![StepTimes::default(); self.steps.len()];
        // When every step of each pass had finished, as the walk reaches each later pass.
        let mut pass_end: BTreeMap<usize, Option<SystemTime>> = BTreeMap::new();
        for index in self.order.iter().copied() {
            let Some(step) = self.steps.get(index) else {
                continue;
            };
            let mut scheduled_at = Some(self.started_at);
            for (pass, end) in &pass_end {
                if *pass < step.pass {
                    scheduled_at = later(scheduled_at, *end);
                }
            }
            for waited_for in &step.waits_for {
                if let Some(waited_for) = times.get(*waited_for) {
                    scheduled_at = later(scheduled_at, waited_for.finished_at);
                }
            }
            let finished_at = scheduled_at.and_then(|scheduled_at| {
                let owed = scheduled_at + self.unit;
                if step.gated {
                    // Its signals are what end it, and it is not finished before the walk
                    // reached it however early they arrived.
                    released
                        .get(step.logical_id.as_str())
                        .map(|at| (*at).max(owed))
                } else {
                    Some(owed)
                }
            });
            if let Some(settled) = times.get_mut(index) {
                *settled = StepTimes {
                    scheduled_at,
                    finished_at,
                };
            }
            let end = pass_end.entry(step.pass).or_insert(scheduled_at);
            *end = later(*end, finished_at);
        }
        times
    }

    /// The instant the LAST step of the walk finished, or `None` while no step has an
    /// instant at all.
    ///
    /// This is what an operation's own conclusion is held to: the operation cannot be
    /// reported as complete before the resources it acted on were.
    pub fn ends_at(&self, released: &BTreeMap<String, SystemTime>) -> Option<SystemTime> {
        self.times(released)
            .into_iter()
            .filter_map(|times| times.finished_at)
            .max()
    }

    /// Compress the walk into `[started_at, at]`, where the operation that made it CONCLUDED
    /// sooner than the span the walk was laid out across.
    ///
    /// A failure is the case: an owning service refuses a resource and the operation is over
    /// at that instant, with the walk's later steps still ahead of it. The walk did happen,
    /// and in that order, so its steps are SCALED into the span the operation really took
    /// rather than all stamped at its end — which would be the degenerate log this timeline
    /// exists to replace. The span the operation really took is what it is: a failure
    /// answered microseconds after the call leaves the steps microseconds apart.
    ///
    /// A walk some of whose events have already been recorded is left alone: those instants
    /// are in the log, and scaling what is left could place a later step before them.
    pub fn compress_into(&mut self, at: SystemTime, released: &BTreeMap<String, SystemTime>) {
        if self
            .steps
            .iter()
            .any(|step| step.events.iter().any(|event| event.recorded))
        {
            return;
        }
        let Some(ends_at) = self.ends_at(released) else {
            return;
        };
        let (Ok(concluded_in), Ok(walked_in)) = (
            at.duration_since(self.started_at),
            ends_at.duration_since(self.started_at),
        ) else {
            return;
        };
        if ends_at <= at || walked_in.is_zero() {
            return;
        }
        // The same walk at the same proportions, in the span the operation really took.
        self.unit = self
            .unit
            .mul_f64(concluded_in.as_secs_f64() / walked_in.as_secs_f64())
            .max(Duration::from_nanos(1));
    }

    /// The instant each resource this walk acted on finished, by logical id — what a
    /// resource's own `StackResource$Timestamp` ("Time the status was updated") reports for
    /// the operation that left it in its status.
    pub fn finished_instants(
        &self,
        released: &BTreeMap<String, SystemTime>,
    ) -> BTreeMap<String, SystemTime> {
        let mut instants: BTreeMap<String, SystemTime> = BTreeMap::new();
        for (step, times) in self.steps.iter().zip(self.times(released)) {
            if let Some(finished_at) = times.finished_at {
                instants.insert(step.logical_id.clone(), finished_at);
            }
        }
        instants
    }
}

impl StepTimes {
    /// The instant the `index`th of a step's `count` events happened at.
    ///
    /// The first is when the walk SCHEDULED the resource and the last when it FINISHED.
    /// Whatever the walk recorded in BETWEEN them reports something the owning service has
    /// already been asked for — the create CloudFormation records as initiated once the
    /// request for it has gone to that service — so it happened at the START of the span the
    /// walk spends on the resource and not at some point measured back from the end of it:
    /// what takes the span is the resource coming up, which the LAST event reports. Those
    /// records are therefore stamped one [`RECORD_TICK`] apart from the instant the step
    /// opened, which is what keeps each one's own instant distinct and in the order the walk
    /// made them, and never at or past the instant the step ended.
    ///
    /// `None` while the walk has not reached that event: for the record that ENDS a step
    /// that is what the step not having finished means, and an earlier record of a step the
    /// walk has not scheduled at all has no instant either.
    pub fn event_at(&self, index: usize, count: usize) -> Option<SystemTime> {
        let scheduled_at = self.scheduled_at?;
        if index == 0 {
            return Some(scheduled_at);
        }
        if index + 1 >= count {
            return self.finished_at;
        }
        let at = scheduled_at + RECORD_TICK * u32::try_from(index).unwrap_or(u32::MAX);
        Some(match self.finished_at {
            // A step the walk spent less than that on — one an owning service refused
            // microseconds after the call, whose walk is reported in the span it really
            // took — keeps the order of its records rather than stamping one past its own
            // end.
            Some(finished_at) => at.min(finished_at),
            None => at,
        })
    }
}

/// The later of two instants, where an instant that is not known at all keeps the answer
/// unknown: a step that waits for something unfinished has not been reached.
fn later(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        _ => None,
    }
}
