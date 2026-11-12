// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader` service: how it claims requests and how it words the
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

/// Whether the CALLER may perform the action a request asks for, decided against the
/// identity-based IAM policies in force on the principal its credentials name.
///
/// IAM decides a signed call ahead of the service, so every operation of this read plane
/// asks this gate before it resolves the stream the request addresses; it lives beside
/// this file rather than in one handler because the decision is the same rule for all six.
pub mod authorization;

/// Every INPUT shape of this service, as the generated schema statics.
///
/// What they are read for: a constraint broken by the ELEMENTS of a collection member is
/// stated at that member with the element shape's WHOLE declared constraint set, and that
/// set is a property of the model rather than of the request — the refusal carries only
/// the constraints a value actually broke. Codegen attaches a collection member's element
/// constraints to that member's schema, so the sets are read off these shapes.
///
/// ALL of them, not the ones one refusal happens to name: the rule holds for every
/// constrained collection of every operation. `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`
/// names exactly these statics; one missing from this list is not a wrong clause, only a
/// clause stating the constraints that failed rather than the element shape's whole set.
const INPUT_SCHEMAS: &[&::aws_smithy_schema::Schema] = &[
    crate::ops::types::GET_CLIP_INPUT_SCHEMA,
    crate::ops::types::GET_DASH_STREAMING_SESSION_URL_INPUT_SCHEMA,
    crate::ops::types::GET_HLS_STREAMING_SESSION_URL_INPUT_SCHEMA,
    crate::ops::types::GET_IMAGES_INPUT_SCHEMA,
    crate::ops::types::GET_MEDIA_FOR_FRAGMENT_LIST_INPUT_SCHEMA,
    crate::ops::types::LIST_FRAGMENTS_INPUT_SCHEMA,
];

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// routing key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality.
///
/// That path conjunct is what separates services sharing one SigV4 signing name. FIVE
/// emulated services sign as `kinesisvideo` and are addressed by that one endpoint
/// prefix — this archived-media endpoint, the Kinesis Video control plane, Media,
/// Signaling Channels and WebRTC Storage — so the routing key alone matches all five
/// and the router, which refuses rather than guesses, answers `500 AmbiguousService`
/// for ALL of them: a code no AWS model carries. Their modelled path spaces are
/// DISJOINT at the first segment (`getClip`, `getHLSStreamingSessionURL`,
/// `listFragments`, … here; `createStream`/`listTagsForStream`/… in the control plane;
/// `getMedia` in Media; `v1` in Signaling Channels; `joinStorageSession` in WebRTC
/// Storage), which is exactly the case this framework discriminator exists for.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader` emulated service: the generated [`ServiceDefinition`] plus
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]: this
        // service's signing name (`kinesisvideo`) is shared with the Kinesis Video
        // control plane, Media, Signaling Channels and WebRTC Storage, so the claim is
        // the routing key AND the model-derived first URI path segment. Matching on
        // the signing name alone would have five services claim every `kinesisvideo`
        // request, which the router answers with 500 `AmbiguousService` instead of
        // serving any of them.
        can_handle_parts(parts)
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
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // This endpoint reports a constraint its MODEL declares on the ELEMENTS of a
        // collection member against that MEMBER: no element position, no `member` step
        // and no element value, with every requirement the element shape declares nested
        // inside `Member must satisfy constraint: [ … ]` — so a caller is told what every
        // element of that member has to satisfy, rather than which one element offended.
        // The framework reports it at the offending element instead, which is the only
        // difference; every other clause, the envelope, the member paths and the error
        // code stay the framework's, which is also the code this `restJson1` service
        // already answers a constraint refusal with.
        //
        // An authentication refusal is a different failure with its own code and status,
        // and nothing about it is this service's to word.
        service_common::validation_message::aws_validation_refusal_over_elements(
            refusal,
            INPUT_SCHEMAS,
            // This endpoint quotes a `@pattern` requirement as the MODEL declares it,
            // anchors and all — the framework's own rendering of the expression.
            str::to_string,
        )
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. The framework's `rest` default is `501
        // NotImplemented`, which no endpoint of the Kinesis Video Streams family
        // answers: a request whose method and URI name no operation this endpoint hosts
        // is rejected by the front door ahead of any handler, in the one envelope that
        // family states an unroutable request in — the control plane's
        // `service::unknown_operation`, which owns that wording for the whole family.
        let _ = req;
        Some(
            ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::service::unknown_operation
                ::unknown_operation_response(),
        )
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
