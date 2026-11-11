// SPDX-License-Identifier: CC0-1.0

//! Receiving a custom resource provider's answer: the one write the published `ResponseURL`
//! performs on this store.
//!
//! ## The rule this module implements
//!
//! A custom resource is held at its `*_IN_PROGRESS` status until the provider its
//! `ServiceToken` names answers the request CloudFormation sent it
//! ([`super::provisioning::custom_resource`]). That answer arrives at the request's
//! `ResponseURL`, which this emulator publishes and serves
//! ([`crate::service::custom_resource_plane`]) — so the one thing the plane does to this
//! store is record the answer against the WAIT it answers.
//!
//! The wait is the resource's own stabilisation barrier
//! ([`super::stabilising::StabilisationBarrier`]), which carries the request it was opened
//! for: an answer is RECORDED only against a barrier still waiting for THAT request, so a
//! write naming a request this emulator never sent, one whose answer has already been
//! acted on, or one belonging to an operation that has concluded, changes nothing. The
//! write itself is still accepted — the published URL is the target a pre-signed `PUT`
//! goes to, which is [`crate::service::custom_resource_plane`]'s own rule; what this
//! module decides is whether the answer CONCLUDES anything. What a recorded answer then
//! does to the resource —
//! the `PhysicalResourceId` it is known by, the `Data` `Fn::GetAtt` resolves against, and
//! the `Delete` a replacement owes for the resource it replaced — is
//! [`super::provisioning::advance_stabilisations`]'s, which is the one place that can make
//! the calls a conclusion owes.

use super::provisioning::custom_resource::{ProviderResponse, ResponseTarget};
use super::ComAmazonawsCloudformationCloudformationState;

/// How a write to a published `ResponseURL` was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseOutcome {
    /// The answer is recorded against the request it answers, and the resource's wait is
    /// released by the next caller that asks.
    Recorded,
    /// The URL names no request this emulator is waiting for an answer to: it was never
    /// sent, it has already been answered, or the stack operation that sent it has
    /// concluded.
    NoSuchRequest,
}

impl ComAmazonawsCloudformationCloudformationState {
    /// The `ResponseURL` this emulator published for the request `target` names, or `None`
    /// when it is waiting for no such answer.
    ///
    /// What the plane holds an arriving write to: the published URL is the ONE address that
    /// request's answer was asked for at, so a write presenting another signature — or none —
    /// is a write to an address this service did not hand out. A target no barrier awaits has
    /// no published URL, and such a write is accepted and recorded against nothing, exactly
    /// as a pre-signed `PUT` to a concluded operation's URL is.
    pub fn awaited_response_url(&self, region: &str, target: &ResponseTarget) -> Option<String> {
        let stack = self.resolve(region, &target.stack_id)?;
        stack.stabilisations.iter().find_map(|barrier| {
            let request = barrier.provider_request.as_ref()?;
            (!barrier.released()
                && barrier.logical_id == target.logical_resource_id
                && request.request_id == target.request_id)
                .then(|| request.response_url.clone())
        })
    }

    /// Record one custom resource provider's answer against the request it answers.
    ///
    /// Addressed exactly as the published URL addresses it: the stack, the logical resource
    /// and the request id must all name one barrier of that stack that is still waiting —
    /// a wait already released has had its answer acted on, and recording a second one
    /// would change a resource the operation has finished with.
    pub fn record_provider_response(
        &mut self,
        region: &str,
        target: &ResponseTarget,
        response: ProviderResponse,
    ) -> ResponseOutcome {
        let Some(stack) = self.resolve_mut(region, &target.stack_id) else {
            return ResponseOutcome::NoSuchRequest;
        };
        let Some(barrier) = stack.stabilisations.iter_mut().find(|barrier| {
            !barrier.released()
                // A request that already has an answer is not awaiting one: the FIRST answer
                // is the one the resource is concluded on, so a second write to the same URL
                // cannot change a resource the operation has already been told about.
                && barrier.provider_response.is_none()
                && barrier.logical_id == target.logical_resource_id
                && barrier
                    .provider_request
                    .as_ref()
                    .is_some_and(|request| request.request_id == target.request_id)
        }) else {
            return ResponseOutcome::NoSuchRequest;
        };
        barrier.provider_response = Some(response);
        ResponseOutcome::Recorded
    }
}
