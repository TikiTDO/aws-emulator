// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-route53domains-route53domains-v20140515` service: how it claims requests and how it words the
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
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The routing key an UNSIGNED awsJson request to this service resolves to.
///
/// The framework reads the SigV4 credential scope first — `route53domains`, this
/// service's signing name — and falls back to the `X-Amz-Target` prefix with a trailing
/// `_<digits>` API version stripped. This service's prefix is
/// `Route53Domains_v20140515` (OPERATIONS.md), whose version suffix starts with a letter
/// and so survives that stripping, which leaves the key below. It names this service and
/// no other, so claiming it costs nothing and lets an anonymous client reach the service.
const TARGET_ROUTE_KEY: &str = "route53domains_v20140515";

/// The `com-amazonaws-route53domains-route53domains-v20140515` emulated service: the generated [`ServiceDefinition`] plus
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
        // Claim a request whose resolved route key is this service's signing name, or
        // the `X-Amz-Target` spelling an unsigned awsJson request resolves to — see
        // [`TARGET_ROUTE_KEY`] for why the two differ for this service.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name() || key == TARGET_ROUTE_KEY,
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
        // Route 53 Domains has ONE client-input error, and every operation in the
        // model lists it: `InvalidInput`, HTTP 400 — it is this service's
        // `common_errors` entry (smithy_query(query="service_info")). So a request
        // the framework refused against the model's own rules — an absent
        // `AdminContact`, a `DurationInYears` outside the modelled 1..10 range, a
        // `DomainName` past 255 characters — must come back under that code rather
        // than the framework's generic `ValidationException`, because that is the
        // code a Route 53 Domains client branches on.
        //
        // The message is the framework's clause for each violated member, wrapped in
        // the envelope AWS reports a constraint refusal in: the number of violations
        // found, then the clauses. The count is what tells a caller whether one
        // member or several are wrong, so it is the violation count of THIS request,
        // never a fixed number.
        match refusal {
            Refusal::Invalid(violations) => {
                let clauses: Vec<String> = violations
                    .iter()
                    .map(smithy_server_runtime::Violation::message)
                    .collect();
                let found = clauses.len();
                let noun = if found == 1 { "error" } else { "errors" };
                Some(Refused {
                    code: "InvalidInput".to_string(),
                    message: format!(
                        "{found} validation {noun} detected: {}",
                        clauses.join("; ")
                    ),
                })
            }
            // An unauthenticated caller is not an input problem: leave the
            // framework's 403 wording alone.
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
