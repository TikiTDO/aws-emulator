// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-connectcontactlens-amazonconnectcontactlens` service: how it claims requests and how it words the
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
//!   correctly, or serve a modelled operation WRAPPED. This service wraps its operation in
//!   [`front_door::FrontDoor`], so the decisions AWS takes before dispatch are taken before
//!   the handler runs (see the section below).
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
//! # How this service answers a request it never ran
//!
//! Three decisions are taken before the operation a request names is dispatched — the
//! `InstanceId` is resolved to an Amazon Connect instance, IAM is asked about the action on
//! the contact the request addresses, and the body is judged against the model's own
//! constraints — and each has its own code, status and wording. The framework's pre-dispatch
//! renderer can state none of them: its only seam, `override_refusal`, carries a code and a
//! sentence, takes its status from the protocol, and is handed the violations alone, so it
//! can neither answer the front door's 401 nor see which members the request carried.
//!
//! Every operation is therefore served through [`front_door::FrontDoor`], which takes those
//! three decisions in AWS's own order. [`instance_id`], [`authorization`] and
//! [`invalid_request`] own one decision each, and `override_refusal` below words anything the
//! framework still renders from the same helpers, so the two paths cannot disagree.

use smithy_server_runtime::{
    http, ErasedOperation, Operation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether IAM lets the caller perform the action the request names.
mod authorization;
/// The stages every request passes before its operation is dispatched.
mod front_door;
/// How a request refused for a modelled input constraint is stated.
mod invalid_request;
/// Resolving the request's `InstanceId` to an Amazon Connect instance.
mod instance_id;

/// The operation this service serves, as the generated routing table names it.
type ListRealtimeContactAnalysisSegments<Eng> =
    crate::ops::list_realtime_contact_analysis_segments::ListRealtimeContactAnalysisSegments<Eng>;

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first
/// URI path segments this service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means the model published no usable discriminator and
/// is "no constraint", and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a
/// service that must answer `POST /` claims the root path without claiming every path.
/// Matching is EXACT segment equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The `com-amazonaws-connectcontactlens-amazonconnectcontactlens` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The layer every request is served behind; see [`front_door::FrontDoor`]. Built once
    /// here because [`Service::override_operation`] hands back a borrow of it.
    front_door: front_door::FrontDoor<ListRealtimeContactAnalysisSegments<Eng>, Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`. The front-door
    /// layer is handed the SAME engine, so the identity it judges and the state the handler
    /// reads come from one store.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            front_door: front_door::FrontDoor::new(Arc::clone(&engine)),
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
        // This service signs as `connect` — a signing name it SHARES with Amazon
        // Connect (`services/service-com-amazonaws-connect-amazonconnectservice`),
        // whose own endpoint is `connect` while this one's is `contact-lens`. The
        // router's routing key comes from the SigV4 credential scope first, so both
        // services resolve to `connect` and matching on the signing name ALONE makes
        // both claim every request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed the way the framework narrows it for the other
        // signing-name collisions: the signing name must match AND the request must
        // address this service's own modeled URI path space (see `claims_modeled_path`).
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                claims_modeled_path(parts, crate::ROUTE_PATH_SEGMENTS)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: serve the modelled operation through its front-door layer, so the
        // three decisions AWS takes before dispatch are taken — and worded — the way AWS
        // takes them (see the module docs and [`front_door::FrontDoor`]).
        //
        // The key comes from the protocol's own routing resolution, so a request naming no
        // modelled operation resolves to nothing here and is left to the generated routing
        // table (and then to the framework's routing error).
        let routing_key = self.entry.protocol().routing_key(req).ok()?;
        (routing_key
            == <ListRealtimeContactAnalysisSegments<Eng> as Operation>::NAME)
            .then_some(&self.front_door as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service state it the way
        // Contact Lens states it: the modelled `InvalidRequestException`, never the synthetic
        // `ValidationException`, which this model does not declare. [`front_door`] answers
        // every refusal it can state in full, so what reaches here is what the framework
        // still renders — a clause class this service has no wording for, and the fallback
        // when the modelled error cannot be serialized. Worded from the same helpers, so the
        // two paths cannot disagree, and both halves are replaced together because a client
        // branches on the code.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => invalid_request::refused_for(
                violations,
                <ListRealtimeContactAnalysisSegments<Eng> as Operation>::INPUT_SCHEMA,
            ),
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
