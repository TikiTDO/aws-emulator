// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplacecatalog-awsmpseymour` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service, ViolationKind,
};
use std::sync::Arc;

/// The violation-path segment the boundary validator writes for a LIST element,
/// which names no member of the request.
const LIST_ELEMENT_SEGMENT: &str = "member";

/// The `com-amazonaws-marketplacecatalog-awsmpseymour` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Agreement, Deployment,
        // Discovery, Entitlement, Metering, Reporting), each of which serves its own
        // endpoint (`catalog.marketplace` here, `reporting-marketplace`,
        // `entitlement.marketplace`, …). The router's routing key comes from the SigV4
        // credential scope first, so all of them resolve to `aws-marketplace` and
        // matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`, leaving this
        // service unreachable.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (`/CancelChangeSet`, `/StartChangeSet`, `/TagResource`,
        // `/GetResourcePolicy`, `/ListEntities`, …), disjoint from every sibling's,
        // which is what `claims_request` matches against the generated
        // `ROUTE_PATH_SEGMENTS` when a service declares no RPC protocol.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
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
        // A member this service will not accept is stated the way this service
        // states it: ONE bracketed fault naming the member, the value it
        // rejected and the syntax it required — never the framework's generic
        // aggregate, which addresses members by lowercase wire path and lists a
        // sentence per violation where this service reports exactly one.
        //
        // The boundary validator refuses a member that fails its modelled
        // `@pattern` before any handler runs, so this seam is the only place
        // that wording can be chosen — and the one place it is chosen for EVERY
        // operation and every member, rather than per operation. Every member
        // this service judges against a rule of its OWN — a syntax, a set or a
        // range it states in words — is off the boundary instead
        // (`skip_validation` in `src/ops/types/`) and is checked by its handler,
        // which answers the modelled `ValidationException` and its 422: a
        // pre-dispatch refusal cannot carry that status, since it takes the
        // protocol's own. What reaches here is a member held to exactly the
        // model's own constraint and for which this service states no sentence
        // of its own — a tag key or value, a filter name, a policy document, the
        // details of a change.
        //
        // The code stays `ValidationException`, which is both the framework's own
        // code for a constraint failure and the modelled client error a handler
        // answers for one, so a client branches on one code however the value was
        // refused. The HTTP status of a boundary refusal stays with the protocol
        // (400); only a handler-returned `ValidationException` carries the
        // modelled 422.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        // The FIRST refused member, which is the one the service names: it
        // reports one fault however many members were malformed.
        let violation = violations.first()?;
        let ViolationKind::Pattern(pattern) = &violation.kind else {
            // Every other kind of violation (a missing member, a length or a
            // range) has no such published syntax to quote, so the framework's
            // own wording stands.
            return None;
        };
        // The path is rooted at the top-level input member and descends from
        // there (`changeSet.1.member.entity.type`); the member a fault addresses
        // is its last NAMED segment, since the list indices and element segments
        // in between name no member.
        let member = violation.path.split('.').rfind(|segment| {
            *segment != LIST_ELEMENT_SEGMENT && segment.parse::<usize>().is_err()
        })?;
        Some(Refused {
            code: "ValidationException".to_string(),
            message: crate::state::invalid_member_message(
                member,
                violation.value.as_deref().unwrap_or_default(),
                pattern,
            ),
        })
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
