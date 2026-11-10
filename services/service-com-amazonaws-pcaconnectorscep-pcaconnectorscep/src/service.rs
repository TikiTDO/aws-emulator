// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-pcaconnectorscep-pcaconnectorscep` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. EDITED HERE: it
//!   also claims a request addressed to a connector's SCEP data plane (see "The SCEP
//!   data plane" below).
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE: a request to a published `Connector$Endpoint` is named as an
//!   operation, so this service answers it; every other request resolves through the
//!   generated routing table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The SCEP data plane
//!
//! This service publishes two kinds of address. The CONTROL plane is
//! `pca-connector-scep.<region>.amazonaws.com`, whose first label names the service, and
//! that is what the framework's route key resolves. Beside it, every connector is given
//! its own SCEP URL (`Connector$Endpoint`, "the connector's HTTPS public SCEP URL"), which
//! is the address a SCEP-enabled device or an MDM system is configured with. A request to
//! it has to reach THIS service rather than the control-plane resolver, which can name no
//! service from a connector's own label — or from the emulator's own endpoint, where the
//! same plane is published with the connector in the PATH for a client that resolves no
//! `amazonaws.com` name ([`crate::state::announced_connector_endpoint`]).
//!
//! Two seams carry it, and nothing else changes:
//!
//! - [`Service::can_handle`] also claims a request addressed to that plane
//!   ([`scep::claims_parts`]), which is what routes it here at all.
//! - [`Service::override_operation`] serves it, through [`scep::ScepDataPlane`]. A SCEP
//!   device presents no AWS credential, so that plane declares no auth scheme and the
//!   router serves the request however it arrived.
//!
//! What is served on it — the private CA's own certificate for `GetCACert`, and an error
//! for every SCEP operation this emulator does not implement — is documented in [`scep`].

pub mod scep;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-pcaconnectorscep-pcaconnectorscep` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves the SCEP data plane a connector publishes — the wire its `Endpoint`
    /// advertises. Held on the service because [`Service::override_operation`] hands the
    /// router a BORROW of one of its own operations, and it owns the shared engine so every
    /// answer is computed from the same store the control-plane handlers write.
    scep: scep::ScepDataPlane<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            scep: scep::ScepDataPlane::new(Arc::clone(&engine)),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // A request whose resolved route key is this service's signing name is the control
        // plane, exactly as the framework's default claims it.
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to a connector's SCEP data plane — the URL this service
        // PUBLISHES in every connector's `Endpoint`, whose host is a connector's own rather
        // than this service's, or the reserved path that plane is published at on the
        // emulator's OWN endpoint. Neither can be named by a route key. See the module docs:
        // without this the router answers `404 UnknownService` for a URL this service handed
        // out.
        scep::claims_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A request to a published connector `Endpoint` names no modelled operation — SCEP
        // is the device's own protocol — so it is named here instead, and served by the
        // plane that holds this service's state. Everything else resolves through the
        // generated routing table.
        if self.scep.claims(req) {
            return Some(&self.scep);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
