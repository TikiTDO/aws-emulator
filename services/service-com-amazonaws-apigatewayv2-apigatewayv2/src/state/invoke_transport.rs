// SPDX-License-Identifier: CC0-1.0

//! The IN-PROCESS TRANSPORT this service performs an HTTP api's integrations over when the
//! request that reached it carries none of its own.
//!
//! An `AWS_PROXY` route's answer is the answer of a function AWS Lambda holds, and a
//! first-class service action's is the answer of the service it names, so serving such a
//! route is a call into another service of this same emulator. The handle that call is made
//! over is normally the one the framework hands the request being served
//! (`OperationContext::callback`) — but one class of invoke never gets one: a request whose
//! browser-set headers read as cross-site is left unnamed by
//! [`crate::service`](crate::service), so it is answered from the routing seam, which is
//! handed the request and no context at all. That is exactly the request an api's
//! `CorsConfiguration` exists for, and API Gateway serves it like any other.
//!
//! So the handle is REMEMBERED: the operation that creates an api keeps the one it was
//! handed, and the data plane performs that api's integrations over it. This is the rule the
//! WebSocket data plane already follows for the same reason — a frame arriving on a socket
//! the core owns carries no request either, so the socket is opened with the callback the
//! create was handed ([`crate::state::websocket_transport`]).
//!
//! The handle is the emulator this service runs within, so remembering it claims nothing and
//! grants nothing: every call made over it is served by this same process, against this same
//! state, signed with the credentials the framework already handed this service. Which of
//! them is remembered therefore cannot change what a call over it reaches.
//!
//! ONE thing a handle does carry that is the handing-over call's rather than the served
//! request's: the re-entrancy count the framework stamps on every request sent over it, which
//! is how the router bounds a chain of in-process calls that comes back round on itself. A
//! remembered handle's count is frozen at the call that handed it over, and the framework
//! exposes no way to re-stamp one, so this transport cannot advance that chain — which is why
//! the data plane bounds its own laps instead of relying on it
//! (`crate::service::invoke`'s `MAX_INTEGRATIONS_IN_FLIGHT`).
//!
//! All of this exists because the seam that serves such a request is handed no context. The
//! day the framework hands one there — a routing-error seam with a context, or a way for an
//! operation to declare that it serves browser traffic, so the request stays the named
//! operation it already is — this module, that bound and the wait in
//! `crate::service::invoke` all go, and the integration is performed over the request's own
//! transport like every other invoke's.

use ::smithy_server_runtime::EmbeddedEmulator;

/// The in-process transport this service keeps for the invokes it has to perform a route's
/// integration for with no request context of its own.
///
/// A field of [`crate::state::ComAmazonawsApigatewayv2Apigatewayv2State`] rather than a store
/// of its own, for the reason [`crate::state::websocket_transport`]'s is: it is read under
/// the same lock as the api records whose routes it serves.
#[derive(Debug, Default, Clone)]
pub struct InvokeTransport {
    /// The handle, once this service has been handed one. `None` before the first call
    /// reaches this service, which is before any api it could serve exists.
    held: Option<EmbeddedEmulator>,
}

impl InvokeTransport {
    /// Keep `callback` as the transport this service's data plane performs integrations over.
    ///
    /// The LATEST handle wins, and the choice is immaterial: every one of them serves this
    /// same emulator and this same state, so one api's routes are performed over the same
    /// services whichever call handed the handle over (see this module's documentation for
    /// the one thing a handle does carry of its own, and what bounds it).
    pub fn remember(&mut self, callback: EmbeddedEmulator) {
        self.held = Some(callback);
    }

    /// The transport to perform an integration over, or `None` when this service has never
    /// been handed one.
    #[must_use]
    pub fn held(&self) -> Option<EmbeddedEmulator> {
        self.held.clone()
    }
}
