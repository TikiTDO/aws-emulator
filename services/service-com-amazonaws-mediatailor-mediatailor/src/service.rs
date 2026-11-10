// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediatailor-mediatailor` service: how it claims requests and how it words the
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
//! A sixth seam, `override_protocol`, is one this service DOES take, for the one framing
//! correction a handler cannot make: see [`wire`].
//!
//! Three of those seams are EDITED here:
//!
//! - `override_operation` serves every modelled operation wrapped in the two rules every
//!   request body and every response of this service passes through: the body read under
//!   both spellings of each member the model renames with `@jsonName` (see
//!   [`body_member_names`] for the rule and why the request is the only place it can be
//!   applied), and the request id reported on whatever the operation answered (below).
//! - `override_refusal` states a request refused before any handler ran under the one
//!   client-input error this service's model declares.
//! - `override_protocol` frames an answer that carries no content as one.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## Naming the request a response answers
//!
//! Every error response MediaTailor sends names the request it answered, in the
//! `requestId` member of its JSON error body beside the `__type` and the `message`. No
//! error shape of this model declares that member — it belongs to the response, so no
//! handler can set it, and the framework's `restJson1` error renderers write the `__type`
//! and the `message` alone.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for every
//! emulated service, and this file only chooses the report this service's envelope takes
//! (the body member) and wires it at the two layers every response of this service passes
//! through:
//!
//! * `service_common::query_error_envelope::RequestIdReportingOperation::error_member`,
//!   composed around each operation this service serves wrapped (see
//!   [`body_member_names::served_operations`]) and handed back from `override_operation`
//!   for a request the generated routing table can serve, covers everything an operation
//!   answers — a success (untouched), a modelled error, this service's own not-found and
//!   not-implemented envelopes, and a constraint refusal raised while it was being served;
//! * [`RequestIdEnvelopeDefinition::error_member`], the definition wrapper `definition`
//!   hands back, covers a response NO operation produced: a routing failure, a transport
//!   failure detected up front, and a pre-dispatch authentication refusal.

pub mod body_member_names;
pub mod wire;

use service_common::query_error_envelope::RequestIdEnvelopeDefinition;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, OperationRoutingTable, Refusal, Refused,
    ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The error code this service names for a request refused before any handler ran.
///
/// MediaTailor's model declares ONE client-input error — `BadRequestException`, HTTP 400 —
/// and it is the error the operations that declare an error declare: see the `TagResource`
/// operation's `errors` list in this crate's `smithy-model/` document.
/// A refusal of a value the model itself rejects is that error, so it travels under that
/// code rather than under a shape this service's model does not carry.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The error class's own words, from the shape's documentation: "A request contains
/// unexpected data."
const BAD_REQUEST_EXCEPTION_MESSAGE: &str = "A request contains unexpected data.";

/// The `com-amazonaws-mediatailor-mediatailor` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// The same modelled operations the generated table holds, each served with its body
    /// read under both spellings of every member the model renames
    /// ([`body_member_names`]) and with the request id reported on whatever it answered.
    /// Keyed by the routing key the protocol resolves — the key the generated table uses —
    /// and built from the SAME shared engine, so a wrapped operation reads and writes the
    /// one store.
    served: OperationRoutingTable,
    /// This service's wire protocol: the model's own `restJson1`, with an answer that
    /// carries no content framed as one ([`wire`]). Built once here, never per request,
    /// because [`Service::override_protocol`] hands back a borrow of it.
    protocol: wire::NoContentFramedRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let served = body_member_names::served_operations(&engine);
        Self {
            entry: RequestIdEnvelopeDefinition::error_member(crate::ServiceDefinition::new(engine)),
            served,
            protocol: wire::NoContentFramedRestJson::new(),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what is
        // handed back is the shared `RequestIdEnvelopeDefinition` over that entry, which
        // reads identity, protocol and the routing table straight off it and names the
        // request in the error body of the three framework-level renderers.
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
        // Not a routing decision: the operation served is the one the generated routing
        // table names for this request, wrapped in the two rules every response and every
        // request body of this service passes through — the body read under both spellings
        // of each member the model renames (`@jsonName`), and the request id reported on
        // whatever the operation answered. The re-keying cannot be done anywhere behind
        // this seam: the codec binds a body key to a member by its wire name and skips a
        // key it binds to nothing, so a member spelled the other way is reported absent
        // before any handler runs. A request the table cannot serve is left to the
        // framework's routing failure, which the definition wrapper above reports on.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.served.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        match refusal {
            // A request the framework refused before any handler ran — an absent required
            // member, or a value outside a declared bound — is a request that "contains
            // unexpected data", which is the one client-input error MediaTailor declares:
            // `BadRequestException`, HTTP 400. The framework's default names
            // `ValidationException`, a shape this service's model does not declare at all,
            // so a client branching on the code cannot recognise the failure. The code is
            // this service's; the CLAUSE stays the framework's, because it is the sentence
            // AWS's own boundary validator states for a refused value (which member was
            // null, which bound it broke) and the model fixes the code, not the words.
            Refusal::Invalid(violations) => Some(Refused {
                code: BAD_REQUEST_EXCEPTION.to_string(),
                message: aws_smithy_schema_server::constraints::envelope(violations)
                    // Unreachable: a refusal carries at least one violation. A refusal
                    // with no clause to state still names what it is, in the error
                    // shape's own words.
                    .unwrap_or_else(|| BAD_REQUEST_EXCEPTION_MESSAGE.to_string()),
            }),
            // Everything else keeps the framework's wording: an unauthenticated caller is
            // the emulator's refusal, not this service's. `Refusal` is `#[non_exhaustive]`,
            // so the catch-all is what covers it.
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

    fn override_protocol(&self) -> Option<&dyn ServerHttpProtocol> {
        // The model's own `restJson1` codec, with one framing correction: an answer at the
        // no-content status carries no body and states the length it has. Held in a field
        // (built once in `new`), as this seam requires — never constructed per request.
        // Everything a request READS and every answer that carries content is the generated
        // protocol's, unchanged.
        Some(&self.protocol)
    }
}
