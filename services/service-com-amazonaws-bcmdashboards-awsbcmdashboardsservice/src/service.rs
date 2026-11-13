// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bcmdashboards-awsbcmdashboardsservice` service: how it claims requests and how it words the
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
//!   generated routing table; this service returns `Some` for the operations its
//!   dispatcher fronts (see below).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …).
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering.
//! - `override_protocol` — the codec this service's own responses are serialized in.
//!   The default returns `None`, keeping the model-derived protocol.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! TWO seams are taken here.
//!
//! - `override_operation` is taken for the reason named in [`response_media_type`]: this
//!   service's recorded wire has two announcers, so which of them answered decides the
//!   media type a response announces. The operations the dashboard and scheduled-report
//!   dispatcher fronts are served through the shared
//!   [`ResponseMediaTypeOperation`], which serves the modelled operation the generated
//!   routing table names and then announces the dispatcher's media type on the responses
//!   that dispatcher produced. Everything else keeps the media type the model's
//!   `awsJson1_0` protocol trait implies and the framework already stamps: a refusal that
//!   reports the request as invalid, every response of a resource-tagging operation, an
//!   unroutable request, a transport-level failure, the authentication refusal. Routing
//!   stays the model's throughout — a request the routing table cannot serve is declined
//!   here, so the framework answers it as the unroutable request it is.
//! - `override_refusal` is taken for a different reason: a boundary refusal of a
//!   `@pattern`-constrained member quotes the expression the service publishes rather than
//!   the anchored spelling the model wraps it in, and a numeric value outside its member's
//!   declared range is stated with the member and the bound alone. Routing, status, codes,
//!   the envelope, the member paths and every other requirement stay with the framework.
//!   Both choices live in [`validation`].

pub mod response_media_type;
pub mod validation;

use response_media_type::{
    fronted_by_the_dispatcher, produced_by_the_dispatcher, DISPATCHER_RESPONSE_MEDIA_TYPE,
};
use service_common::response_media_type::ResponseMediaTypeOperation;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-bcmdashboards-awsbcmdashboardsservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper the operations of the dashboard and scheduled-report dispatcher are
    /// served through, which announces that dispatcher's media type on the responses it
    /// produced (see [`response_media_type`]). Held in a field, built once, because
    /// `override_operation` hands back a reference to it.
    dispatcher: ResponseMediaTypeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            dispatcher: ResponseMediaTypeOperation::announcing(
                DISPATCHER_RESPONSE_MEDIA_TYPE,
                produced_by_the_dispatcher,
            ),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
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
        // A request for an operation the dispatcher fronts is served through the shared
        // wrapper, which delegates to that same routing-table entry and then announces the
        // dispatcher's media type on what it produced (see [`response_media_type`]). A
        // resource-tagging request is declined here and resolves through the generated
        // routing table unchanged, so it keeps the media type the framework stamps. Either
        // way the routing is the model's: a request the table cannot serve is declined too,
        // so the framework answers it as the unroutable request it is.
        let definition: &dyn ErasedServiceDefinition = &self.entry;
        let routing_key = definition.protocol().routing_key(req).ok()?;
        if !fronted_by_the_dispatcher(routing_key) {
            return None;
        }
        service_common::response_media_type::routes(definition, req)
            .then_some(&self.dispatcher as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service state it its own way. What this service states, and all it states, is
        // in [`validation`]: the `ValidationException` envelope the framework already
        // renders, with a `@pattern` requirement quoted as the expression a value has to
        // satisfy and a numeric range stated without the number the request carried. The
        // code, the envelope, the member paths and every other requirement stay the
        // framework's, and the sentences stay in `service-common`.
        validation::refused(refusal)
    }
}
