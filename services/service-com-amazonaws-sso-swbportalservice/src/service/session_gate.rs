// SPDX-License-Identifier: CC0-1.0

//! The portal's authentication gate: the sign-in session a request's bearer token names,
//! resolved BEFORE anything else the request carries is read.
//!
//! The portal has exactly one caller identity — the sign-in session the presented bearer
//! token names — so a request presenting no usable session is an unauthenticated request
//! and is refused as one, whatever else it did or did not carry. The session itself is
//! resolved through [`crate::state::ComAmazonawsSsoSwbportalserviceState::signed_in`],
//! the one place this service decides what a token names, so the gate and the handler
//! behind it cannot disagree.
//!
//! A handler's own gate cannot state that rule by itself. The framework reads and
//! validates a request's modelled members BEFORE it calls any handler, so a value outside
//! a declared bound (a `maxResults` outside the modelled range) ends the request in the
//! framework's refusal tail, where the handler — and therefore the handler's gate — never
//! runs. Answering that refusal as an invalid request would tell a caller the service
//! could not authenticate that its parameter was read and judged, and would give two
//! equally unreadable parameters of one request opposite precedences.
//!
//! So this module owns both halves of the rule for the whole service:
//!
//! - [`SessionGate::presents_a_usable_session`] — the question, asked of a request that
//!   has reached no handler. [`crate::service`] asks it in both seams that see such a
//!   request.
//! - [`SessionGate`] as an [`ErasedOperation`] — the answer for a request the model DOES
//!   name an operation for, served through
//!   [`Service::override_operation`](smithy_server_runtime::Service::override_operation),
//!   which the router consults before it deserializes or validates anything.

use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};
use std::sync::Arc;
use std::time::SystemTime;

/// Resolves the sign-in session a request presents, and serves the portal's one
/// authentication refusal to a request that presents none.
///
/// Generic over the engine type for the same reason the rest of this crate is: naming a
/// concrete engine here would let the gate drift from the service the router mounts.
pub(super) struct SessionGate<Eng> {
    /// The SAME shared engine the generated entry threaded into every operation, so the
    /// gate resolves a presented token against the one store the handlers read. Never a
    /// store of its own: it is the `Arc` the router handed to `EmulatedService::new`.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> SessionGate<Eng> {
    /// Build the gate on the service's shared engine.
    pub(super) fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Whether `req` presents a sign-in session this service still serves at the moment
    /// it is asked.
    ///
    /// `false` covers every way a request can fail to present one — no access-token
    /// header at all, or one naming a session that is unknown, logged out or expired —
    /// because the portal authenticates a caller by that session and distinguishes none
    /// of them. It is the same question
    /// [`crate::state::ComAmazonawsSsoSwbportalserviceState::signed_in`] answers for a
    /// handler, asked of a request that has not reached one — resolved against the same
    /// two sources, so the gate cannot refuse a token a handler would have served.
    ///
    /// A pure READ: it writes nothing, so asking it cannot change what a later caller
    /// sees, and two identical requests get the identical answer.
    pub(super) fn presents_a_usable_session(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> bool {
        let Some(token) = super::presented_access_token(req) else {
            return false;
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // UNREACHABLE with the engine the router mounts (see the note in
            // `ops/list_accounts.rs`): with no store to resolve the token against there
            // is nothing to authenticate it with, so the gate claims nothing here and
            // the framework's own answer stands.
            return true;
        };
        let now = SystemTime::now();
        // Resolved before this store is locked, exactly as a handler does it.
        let federated = crate::state::federated_sign_in(&*self.engine, token, now);
        let state = state.lock();
        state.signed_in(token, federated.as_ref(), now).is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for SessionGate<Eng> {
    /// Answer the portal's one authentication refusal — `UnauthorizedException`, with the
    /// status and `__type` its model carries and the gate's one message — in the envelope
    /// of the protocol this request is served in, so it is the same response a handler's
    /// own gate renders.
    ///
    /// Reached only for a request [`crate::service`] has already found to present no
    /// usable session, so there is nothing left of the request to read: the refusal
    /// derives nothing from it.
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The one write every portal request makes, and only what the clock already
            // owed: a minted role credential past its advertised expiration is spent at
            // that instant, whether or not the request that happens to arrive next is one
            // this service will serve, so it is ended here exactly as each handler ends
            // it. Nothing about the refusal below changes because this ran.
            if let Some(state) = crate::state::service_state(&*self.engine) {
                let mut state = state.lock();
                crate::state::end_expired_credentials(
                    &mut state,
                    ctx.credential_store(),
                    SystemTime::now(),
                );
            }
            crate::state::unauthorized_response(svc.definition().wire_protocol(req).protocol())
                // Only if the protocol cannot serialize the error, which it cannot fail
                // to do for a shape of this service's own model; the framework's own
                // answer stands rather than an invented one.
                .unwrap_or_else(|| svc.definition().routing_error_response(req))
        })
    }
}
