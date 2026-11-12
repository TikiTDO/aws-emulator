// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-controltower-awscontroltowerapis` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. This service answers a failed input validation with
//!   Control Tower's front-door bad-request refusal; see that method.
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether IAM lets a request perform the action it names — the decision every operation
/// of this service whose model declares `AccessDeniedException` takes before it reads or
/// writes anything.
pub mod authorization;

/// The error code AWS Control Tower answers a request that failed input validation
/// with.
///
/// Every Control Tower operation is a restJson1 REST call, and a request whose
/// members do not satisfy the model's constraints is refused by the REST front door
/// before the modeled service error is reached: the answer is a generic bad-request
/// refusal, not the per-member `ValidationException` the operations declare. The
/// distinction is client-visible — an SDK unmarshals `ValidationException` into a
/// typed, modeled error and this code into a generic service error — so the code a
/// client branches on has to be this one. `ValidationException` remains what a
/// HANDLER returns when a request that satisfied every modeled constraint is still
/// not acceptable to the service; only the pre-handler refusal is reworded here.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The message that front-door refusal carries.
///
/// It is constant: the front door refuses the request before it is dispatched to an
/// operation, so it names neither the member nor the constraint that failed. No model
/// trait or AWS document states this text, so it is this emulator's rendering of that
/// one refusal rather than a value a client may rely on — the status (400) and
/// [`BAD_REQUEST_EXCEPTION`] are the contract.
const INVALID_REQUEST_BODY_MESSAGE: &str = "Invalid request body";

/// The `com-amazonaws-controltower-awscontroltowerapis` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Control Tower refuses such a request at its REST front door, so the
        // answer is the generic bad-request code and message above rather than the
        // framework's per-member `ValidationException` envelope: the front door has
        // not dispatched the request to an operation, so it states neither the
        // member nor the constraint. One decision for every operation and every
        // constraint kind — a request refused before a handler ran is refused the
        // same way whichever member failed.
        //
        // The status stays with the protocol (400), and both the code and the
        // message are replaced together: a message without its matching code is
        // worse than leaving both alone.
        //
        // An authentication refusal is NOT an input-validation failure, so it keeps
        // the framework's 403 wording; `Refusal` is `#[non_exhaustive]`, and a
        // refusal class this crate has not seen keeps the default wording too.
        match refusal {
            Refusal::Invalid(_) => Some(Refused {
                code: BAD_REQUEST_EXCEPTION.to_string(),
                message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
            }),
            _ => None,
        }
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
