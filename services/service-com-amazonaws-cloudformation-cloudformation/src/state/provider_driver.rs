// SPDX-License-Identifier: CC0-1.0

//! What a LANDED provider answer owes, carried out without waiting for the next client call.
//!
//! ## The rule this module implements
//!
//! A custom resource's provider answers CloudFormation out of band, at the request's
//! `ResponseURL` ([`super::provisioning::custom_resource`]). What that answer makes due is
//! not only a status: a `FAILED` answer to an `Update` owes the ROLLBACK's own requests, and
//! a replacement owes the cleanup `Delete` of the resource it replaced — in-process calls to
//! the services that own them, which CloudFormation makes itself. On AWS nothing of that
//! waits for the client to call again: the provider answers, and the next request the
//! operation owes is published.
//!
//! The operations that REPORT a stack's progress drive exactly this work
//! ([`super::provisioning::advance_stabilisations`] and [`super::settle_due_rollbacks`]),
//! because they are handlers and a handler is the one place an in-process call can be made
//! from. The plane that receives a provider's answer is not a handler
//! ([`crate::service::custom_resource_plane`]), so this module is how the answer still
//! drives them: it starts a task that runs those same two functions under a context of its
//! own, for a bounded window, so the requests an answer owes are published whether or not a
//! client reads the stack next.
//!
//! A pass of this task can run BESIDE a client's own read of the stack, which drives the
//! same two functions, so each piece of work either is taken from the store under its lock
//! (a due rollback,
//! [`super::ComAmazonawsCloudformationCloudformationState::take_due_rollbacks`]) or is
//! identified by what it is owed FOR, so the second pass to reach the same conclusion adds
//! nothing to it ([`super::StoredStack::record_stabilisations`] holds one barrier per wait).
//!
//! Nothing here decides an outcome. It materialises what the CLOCK had already made due at
//! the instant it runs — the same thing a reporting operation materialises, and nothing a
//! reporting operation would not — so a client reading the stack afterwards sees exactly
//! what it would have seen had it done the reading itself.
//!
//! ## What the task runs under
//!
//! An in-process call needs a handler's [`::smithy_server_runtime::OperationContext`], which
//! carries the callback into this emulator every provisioning call rides. The plane has none,
//! so the context is built from what a CLIENT call left behind: the callback handle and the
//! service identity of the last operation to drive a stack's progress ([`ProvisioningReach`],
//! remembered by [`super::provisioning::advance_stabilisations_in`] and
//! [`super::settle_due_rollbacks`] — the same two functions this task runs). The REGION is
//! passed to them, not read off a request: the one the stack's own ARN names, which is what a
//! client's call would have carried in its credential scope.
//!
//! ## Bounds
//!
//! One task per landed answer, each running at most [`DRIVE_ATTEMPTS`] passes spaced
//! [`super::STATUS_TRANSITION_DELAY`] apart, so a landed answer costs a bounded amount of
//! work and the window is over before the wait any resource is held at could run out.

use std::sync::Arc;

use ::smithy_server_runtime::{OperationContext, ServiceIdentity};

/// How many passes one landed answer's task makes.
///
/// A pass publishes what is due NOW. What an answer owes can become due one transition later
/// — a failure's rollback is owed once the failure itself is due
/// ([`super::STATUS_TRANSITION_DELAY`]) — so the task looks again across a few transitions
/// rather than once, and then stops: a stack still in flight after that is one a client is
/// reading anyway, and every such read drives the same two functions.
const DRIVE_ATTEMPTS: usize = 4;

/// The handle this service's own in-process calls are made through, kept so the work a
/// landed provider answer owes can be carried out with no further client call.
///
/// Both halves come from a handler's context and are cheap, cloneable handles to THIS
/// emulator: the callback ([`::smithy_server_runtime::OperationContext::callback`]) is the
/// in-process transport every provisioning call already rides, and the identity is the
/// service's own. Neither carries anything of the request it was taken from — no caller, no
/// credential of a caller's and no body — so what is remembered is the emulator, not a
/// client.
#[derive(Debug, Clone)]
pub struct ProvisioningReach {
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    identity: ServiceIdentity,
}

impl ProvisioningReach {
    /// The reach of the handler running `ctx`.
    pub fn of(ctx: &OperationContext<'_>) -> Self {
        Self {
            callback: ctx.callback().clone(),
            identity: ctx.identity().clone(),
        }
    }
}

impl super::ComAmazonawsCloudformationCloudformationState {
    /// Remember how this emulator is reached, so a landed provider answer can drive the work
    /// it owes ([`drive_landed_answer`]).
    ///
    /// Called by the provisioning entry points every stack operation that reports a stack's
    /// progress goes through: by the time a provider can answer anything, a request of its
    /// own has been published from a client call that went through one of them.
    pub fn remember_provisioning_reach(&mut self, ctx: &OperationContext<'_>) {
        self.provisioning_reach = Some(ProvisioningReach::of(ctx));
    }
}

/// Carry out what a provider's landed answer owes, in a task of its own.
///
/// Started by the plane that received the answer, for the Region the stack the answer was
/// about belongs to. It does nothing at all until a client call has provisioned something —
/// there is no reach to make a call through before that, and no answer can have landed
/// either.
pub fn drive_landed_answer<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, region: &str) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Some(reach) = state.lock().provisioning_reach.clone() else {
        return;
    };
    let engine = Arc::clone(engine);
    let region = region.to_string();
    // DETACHED on purpose: the work outlives the write that made it due, exactly as it
    // outlives the client call that would otherwise have driven it. The task holds only
    // cloneable handles, so it borrows nothing of the request that started it.
    ::tokio::spawn(async move {
        for pass in 0..DRIVE_ATTEMPTS {
            if pass > 0 {
                ::tokio::time::sleep(super::STATUS_TRANSITION_DELAY).await;
            }
            // The parts a context is built over name nothing: this task serves no request,
            // so there is no caller, no credential and no body — the Region the work runs for
            // is passed to it, and the calls it makes are signed by the callback with the
            // emulator's own credentials exactly as a handler's are.
            let parts = ::http::Request::new(()).into_parts().0;
            let ctx = OperationContext::new(&parts, &reach.identity, reach.callback.clone());
            let Some(state) = super::service_state(&*engine) else {
                return;
            };
            super::provisioning::advance_stabilisations_in(&ctx, &state, &region).await;
            super::settle_due_rollbacks(&ctx, &*engine).await;
        }
    });
}

