// SPDX-License-Identifier: CC0-1.0

//! The STABILISATION of one stack operation: the resources whose owning service has
//! answered the call the operation made about them without having carried it out yet — a
//! create that does not report the resource ready, and a delete that does not report the
//! resource gone.
//!
//! ## The rule this module implements
//!
//! A create is not finished when the owning service's create call returns. Several AWS
//! APIs answer a create while the resource is still coming up — an ACM certificate is
//! `PENDING_VALIDATION` "upon being requested" and is not usable until it is issued — and
//! CloudFormation does not report such a resource `CREATE_COMPLETE` on the strength of the
//! accepted call: it holds the resource (and the stack operation behind it) at
//! `*_IN_PROGRESS` until that service reports the resource in the state it is ready in,
//! while already reporting the `PhysicalResourceId` that service assigned. A client that
//! reads a resource as created therefore reads a resource that can serve traffic, and a
//! stack is not reported complete while one of its resources cannot.
//!
//! A DELETE is the same relation read backwards, and the same services answer it the same
//! way: an accepted delete leaves the resource in a state it is still being removed from, so
//! CloudFormation does not report the operation that deleted it complete on the strength of
//! the accepted call either — the stack stays `*_IN_PROGRESS` until the service that owns
//! the resource no longer holds it. A client that reads a stack operation as complete
//! therefore finds every resource that operation deleted gone in the service that owned it.
//!
//! What ENDS either wait is the owning service's own answer, read through that service's own
//! API ([`super::provisioner::Provisioner::readiness`] for a create,
//! [`super::provisioner::Provisioner::removal`] for a delete) — never a status this crate
//! guesses. The wait is bounded ([`CREATE_STABILISATION_WINDOW`],
//! [`REMOVAL_STABILISATION_WINDOW`]): a resource the owning service has not brought up, or
//! has not let go of, inside it fails the resource, and the stack rolls back, exactly as a
//! call that service refused outright does. So no template can leave a stack in progress for
//! ever.
//!
//! This module is the model of those waits, and nothing else. [`StabilisationBarrier`] is
//! one resource's: which answer it waits for ([`Awaited`]), what the call left, the instant
//! the wait began, and the instant the owning service was first seen to give that answer.
//! [`super::StoredStack::settle`] will not complete an operation that still holds an
//! unreleased barrier, and what releases one is
//! [`super::provisioning::advance_stabilisations`].
//!
//! What it gates is what the STACK reports: the resources a deployment declares are created
//! in one pass ([`super::provisioning::reconcile`]), so a resource declared after one that
//! is still coming up is created in its own owning service either way — what no client is
//! told is that the operation, or the resource holding it, finished.
//!
//! ## Bounds
//!
//! One barrier per resource of the operation in flight whose create answered "not ready
//! yet" and one per resource whose delete answered "not gone yet", so the set is bounded by
//! twice the resources a stack may hold ([`super::MAX_CONCURRENT_RESOURCES`]), and every
//! barrier is dropped when the operation that opened it concludes.

use std::time::{Duration, SystemTime};

use super::provisioning::custom_resource::{ProviderRequest, ProviderResponse};

/// The longest CloudFormation holds ONE resource's creation open waiting for the service
/// that owns it to report the resource ready.
///
/// This emulator's ceiling on an open wait ([`super::MAX_HELD_WAIT`]), which is what the
/// waits for an answer from ANOTHER SERVICE of this emulator are bounded by: nothing
/// published says how long one of them takes, unlike the signal wait a template's own
/// declaration asks for ([`super::wait_conditions::WaitConditionBarrier::deadline`]), which
/// runs for the span that declaration stated. Long enough that every coming-up span the
/// services of this emulator owe a new resource falls due inside it many times over, measured
/// from the call it waits for ([`StabilisationBarrier::asked_at`]), and short enough that a
/// resource nothing will ever bring up fails the stack inside a client's poll loop instead of
/// outliving it.
pub const CREATE_STABILISATION_WINDOW: Duration = super::MAX_HELD_WAIT;

/// The longest it holds ONE resource's deletion open waiting for the service that owns it to
/// report the resource gone.
///
/// The same ceiling, for the same reason read backwards: every removal span the services of
/// this emulator owe a deleted resource falls due inside it many times over, and a resource
/// nothing is ever going to remove fails the stack inside a client's poll loop instead of
/// holding the operation for ever.
pub const REMOVAL_STABILISATION_WINDOW: Duration = super::MAX_HELD_WAIT;

/// Which answer from the owning service a barrier is waiting for.
///
/// The answers are asked of different APIs of that service and mean opposite things, so the
/// barrier carries which one it is rather than leaving it to be guessed from the operation:
/// one stack operation can hold both about the same logical resource at once, when it
/// REPLACES one — the new physical resource is coming up while the old one is being removed.
///
/// It also carries WHICH ACTION on the resource the wait was opened by, because that is what
/// a wait that does not come through says did not go through: a resource that was being
/// brought up failed its CREATE, one whose declared configuration was being changed failed
/// its UPDATE, and one that was being removed failed its DELETE
/// ([`super::StoredStack::fail_stabilisation`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awaited {
    /// The resource the operation CREATED is reported ready
    /// ([`super::provisioner::Provisioner::readiness`]).
    Ready,
    /// The resource the operation CHANGED is reported ready: the same answer [`Self::Ready`]
    /// waits for, about a resource the stack ALREADY HAD whose declaration this deployment
    /// changed — a change the owning service, or the provider of a custom resource, accepted
    /// without having carried it out yet.
    Updated,
    /// The resource the operation DELETED is no longer held by that service
    /// ([`super::provisioner::Provisioner::removal`]).
    Gone,
    /// Nothing has been asked of anybody about this resource yet: the resources it DEPENDS
    /// ON are still waiting for their own answers, so what this barrier waits for is those
    /// waits releasing — which is when its own creation can START.
    ///
    /// It is held because the declaration that would be sent reads a value of a resource
    /// whose OWN wait is still open — an attribute the owning service publishes only once it
    /// reports the resource ready, or, for a custom resource, the `Data` its provider answers
    /// with — so the resource is created once that wait releases, by the same walk that would
    /// have created it ([`super::provisioning::deferred`]). Not a question for any owning
    /// service, which is why it is not one of the readiness answers above.
    Deferred,
}

impl Awaited {
    /// Whether this wait is for the resource to be REPORTED READY, which is the one question
    /// a create's wait and a change's wait both ask of whoever owns the resource.
    ///
    /// What they differ in is the action each would report as failed, not the answer that
    /// ends them, so every reader of that answer asks this rather than naming the variants.
    pub fn awaits_readiness(self) -> bool {
        matches!(self, Self::Ready | Self::Updated)
    }
}

/// One resource of the operation in flight that is still open: the owning service accepted
/// the create and does not report the resource ready yet, or accepted the delete and still
/// holds the resource.
#[derive(Debug, Clone)]
pub struct StabilisationBarrier {
    /// Which of the owning service's answers ends this wait.
    pub awaited: Awaited,
    /// The logical id the template declares the resource under, which is what the stack's
    /// events and per-resource reads name it by.
    pub logical_id: String,
    /// `StackResource$ResourceType`, which is what says WHICH service's answer ends this
    /// wait.
    pub resource_type: String,
    /// The identifier the owning service assigned, which the read that ends this wait
    /// addresses the resource by — and which CloudFormation already reports for the resource
    /// while a creation's wait holds.
    pub physical_resource_id: String,
    /// The state that service reported the resource in when the call answered, as its own
    /// API spells it. Carried so the failure a wait that runs out reports says what the
    /// resource was still in.
    pub reported_state: String,
    /// The instant the operation that acted on the resource started, which the wait's own
    /// records are stamped from — and which the resource's own `*_IN_PROGRESS` event and
    /// reported timestamp carry, so one instant is the whole of what the stack says about
    /// this resource.
    pub opened_at: SystemTime,
    /// The instant the CALL this wait is for was made, which is the instant the wait RUNS
    /// FROM ([`Self::deadline`]).
    ///
    /// Not the same instant as [`Self::opened_at`]: a stack operation walks its resources one
    /// after another, and each call is made when the walk reaches that resource, so a
    /// resource an operation reaches late in a long walk is asked about long after the
    /// operation started. The window is the span the service that owns the resource — or the
    /// provider of a custom resource — is given to carry out THAT call, so it runs from the
    /// call: measuring it from the operation's start would give such a resource only what was
    /// left of the operation's window, and a resource reached past it none at all.
    pub asked_at: SystemTime,
    /// The instant the owning service was first seen to give the answer this barrier waits
    /// for, or `None` while it does not.
    pub released_at: Option<SystemTime>,
    /// The PROVIDER REQUEST this wait is for, where the resource is a CUSTOM RESOURCE
    /// ([`super::provisioning::custom_resource`]): such a resource is backed by no API of
    /// this emulator's, so what ends its wait is not a service's own read but the answer
    /// the provider PUTs to the request's `ResponseURL`. `None` for every other resource,
    /// whose wait is that service's answer about the resource.
    pub provider_request: Option<ProviderRequest>,
    /// The answer the provider PUT to that request's `ResponseURL`, once it has
    /// ([`super::ComAmazonawsCloudformationCloudformationState::record_provider_response`]).
    pub provider_response: Option<ProviderResponse>,
    /// The instant a pass CLAIMED the creation this barrier is WITHHOLDING
    /// ([`Awaited::Deferred`]), or `None` while none has.
    ///
    /// What keeps a withheld resource from being created twice: the pass that resumes it is
    /// driven by every operation that reports a stack's progress, and by the task a landed
    /// provider answer starts ([`super::provider_driver`]), so two of them can read the same
    /// released wait. Creating the resource is not something a second pass may add to — it
    /// would send the provider a second request, and the stack would then hold a wait no
    /// answer releases — so the pass marks the wait it is acting on while it holds the store's
    /// lock, and a pass that finds it marked leaves it alone
    /// ([`super::provisioning::deferred`]).
    pub resumed_at: Option<SystemTime>,
}

impl StabilisationBarrier {
    /// Open the wait a call the operation just made left behind
    /// ([`super::provisioning::Stabilising`]).
    ///
    /// `opened_at` is the instant the operation that acted on the resource started
    /// ([`Self::opened_at`]) and `asked_at` the instant its own call was made
    /// ([`Self::asked_at`]), which is what the window below is measured from.
    pub fn open(
        pending: &super::provisioning::Stabilising,
        opened_at: SystemTime,
        asked_at: SystemTime,
    ) -> Self {
        Self {
            awaited: pending.awaited,
            logical_id: pending.logical_id.clone(),
            resource_type: pending.resource_type.clone(),
            physical_resource_id: pending.physical_resource_id.clone(),
            reported_state: pending.reported_state.clone(),
            opened_at,
            asked_at,
            released_at: None,
            provider_request: pending.provider_request.clone(),
            provider_response: None,
            resumed_at: None,
        }
    }

    /// Whether this barrier and `other` are the same wait: the same answer awaited about the
    /// same physical resource of the same logical one.
    ///
    /// What identifies a wait, because a logical id alone does not: an operation that
    /// REPLACES a resource holds one wait for the new physical resource coming up and one
    /// for the old one being removed, both under that logical id, and each is released by
    /// its own answer.
    pub fn is_the_same_wait(&self, other: &Self) -> bool {
        self.awaited == other.awaited
            && self.logical_id == other.logical_id
            && self.physical_resource_id == other.physical_resource_id
            // A custom resource's wait is identified by the REQUEST it is waiting for as
            // well: one resource can be asked twice under the same physical id, and only the
            // answer to the request a barrier was opened for releases that barrier.
            && self.provider_request.as_ref().map(|request| &request.request_id)
                == other.provider_request.as_ref().map(|request| &request.request_id)
    }

    /// Whether the owning service has given the answer this barrier waits for, so the
    /// operation may proceed.
    pub fn released(&self) -> bool {
        self.released_at.is_some()
    }

    /// The instant the wait gives up, which is the window of the answer it waits for, run
    /// from the instant the call it is waiting for was made ([`Self::asked_at`]).
    pub fn deadline(&self) -> SystemTime {
        let window = match self.awaited {
            // A change the owning service has accepted is waited out for as long as a
            // creation is: both wait for that service to report the resource in the state it
            // serves traffic in.
            Awaited::Ready | Awaited::Updated => CREATE_STABILISATION_WINDOW,
            // A creation that has not started yet is held for the same window a started one
            // is: what it waits for is the wait of the resource it depends on, which runs
            // for no longer than that itself.
            Awaited::Deferred => CREATE_STABILISATION_WINDOW,
            Awaited::Gone => REMOVAL_STABILISATION_WINDOW,
        };
        self.asked_at + window
    }

    /// Whether the wait has run out at `now`, which fails the resource it is holding.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.deadline() <= now
    }

    /// Why the resource failed, for a wait that ran out: which resource did not reach what
    /// was waited for, and the state the service that owns it was still reporting.
    ///
    /// `state` is that service's latest answer, which is not always the one the call left
    /// ([`Self::reported_state`]).
    pub fn not_stabilised_reason(&self, state: &str) -> String {
        let state = if state.is_empty() {
            self.reported_state.as_str()
        } else {
            state
        };
        let awaited = match self.awaited {
            Awaited::Ready => "created",
            Awaited::Deferred => "created",
            Awaited::Updated => "updated",
            Awaited::Gone => "deleted",
        };
        format!(
            "{} {} was still {state} when the wait for it to be {awaited} ran out",
            self.resource_type, self.physical_resource_id
        )
    }
}
