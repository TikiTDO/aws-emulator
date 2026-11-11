// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesisvideo-kinesisvideo-20170930` service: how it claims requests and how it words the
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
//!
//! Two of those seams are taken here. `override_refusal` states a refused input value in
//! this service's own error code and AWS's own wording ([`invalid_argument_refusal`]), and
//! `override_operation` serves every routable request through
//! [`error_message_member::ErrorMessageMemberOperation`], so a failure reaches the caller in
//! the envelope this service answers that operation's refusals in — see
//! [`refusal_envelope`] for which envelope that is and why the refusal seam cannot state it,
//! and [`error_message_member`] for the body member a modelled failure carries its text in.
//! That same seam SCOPES this service's routes to the endpoint a control call arrives on: a
//! call that reached an address this service announced for a data plane is answered as the
//! unrouted path it is ([`data_plane_address`]).
//!
//! A third seam is taken too: `override_routing_error` answers a request this service
//! claimed and could name no modelled operation for in this family's FRONT-DOOR envelope
//! ([`unknown_operation`]), which is the one wording every endpoint of it states an
//! unroutable request in — the framework's `rest` default (`501 NotImplemented`) is a
//! status this service never answers.

pub mod data_plane_address;
pub mod error_message_member;
pub mod refusal_envelope;
pub mod unknown_operation;

use aws_smithy_schema_server::Violation;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

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
/// prefix — this control plane, Kinesis Video Media, Archived Media, Signaling
/// Channels and WebRTC Storage — so the routing key alone matches all five and the
/// router, which refuses rather than guesses, answers `500 AmbiguousService` for ALL
/// of them: a code no AWS model carries. Their modelled path spaces are DISJOINT at
/// the first segment (`createStream`, `listTagsForStream`, `tagStream`, … here;
/// `getMedia` in Media; `getClip`/`getHLSStreamingSessionURL`/`listFragments` in
/// Archived Media; `v1` in Signaling Channels; `joinStorageSession` in WebRTC
/// Storage), which is exactly the case this framework discriminator exists for, so
/// each of the five claims only its own traffic and a path none of them binds is
/// claimed by nobody.
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

/// The `com-amazonaws-kinesisvideo-kinesisvideo-20170930` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The shared engine, held beside the entry so the seams below can read this store —
    /// which is where the addresses this service announced for its data planes live.
    engine: Arc<Eng>,
    /// The wrapper `override_operation` hands back. Stateless, and held here rather than
    /// built per request, because the seam answers a reference borrowed from `&self`.
    error_message_member: error_message_member::ErrorMessageMemberOperation,
    /// What a control-plane call that reached a data plane's address is answered with.
    /// Stateless, and held for the same reason.
    unrouted_on_data_plane_address: data_plane_address::UnroutedOnDataPlaneAddress,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            error_message_member: error_message_member::ErrorMessageMemberOperation,
            unrouted_on_data_plane_address: data_plane_address::UnroutedOnDataPlaneAddress,
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
        // service's signing name (`kinesisvideo`) is shared with Kinesis Video Media,
        // Archived Media, Signaling Channels and WebRTC Storage, so the claim is the
        // routing key AND the model-derived first URI path segment. Matching on the
        // signing name alone would have five services claim every `kinesisvideo`
        // request, which the router answers with 500 `AmbiguousService` instead of
        // serving any of them.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every routable request is served through the wrapper, which delegates to the
        // operation the generated routing table names and then reports the text of whatever
        // it answered in the body member this service's error shapes declare (see
        // [`error_message_member`]). A request the table cannot serve is declined here, so
        // the router answers it as the unroutable request it is.
        //
        // Before that: a control call that reached an address this service announced for a
        // DATA plane reached an endpoint that binds no control route, so it is answered as
        // the unrouted path it is and no operation of this service runs for it (see
        // [`data_plane_address`]).
        if data_plane_address::addressed_a_data_plane(&*self.engine, req) {
            return Some(&self.unrouted_on_data_plane_address as &dyn ErasedOperation);
        }
        error_message_member::routes(&self.entry, req)
            .then_some(&self.error_message_member as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // See [`invalid_argument_refusal`]: this service's code for a bad input value,
        // over the shared AWS wording of the constraint envelope. Every other refusal
        // class keeps the framework's own wording.
        invalid_argument_refusal(refusal)
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. The framework's `rest` default is `501
        // NotImplemented`, which this family's endpoints never answer: a request whose
        // method and URI name no operation this endpoint hosts is rejected by the front
        // door ahead of any handler, in the envelope [`unknown_operation`] states.
        let _ = req;
        Some(unknown_operation::unknown_operation_response())
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

/// The MODELLED error Kinesis Video Streams answers a refused input value with on the
/// operations its control plane fronts: `InvalidArgumentException` — "The value for this
/// input parameter is invalid." — which `smithy_query(query="service_info")` lists among
/// this service's common errors, so every operation declares it. WHICH refusals carry it,
/// rather than the generic code the framework's renderer writes, is
/// [`refusal_envelope`]'s: the seam below cannot see which operation a refusal belongs to.
pub(crate) const INVALID_ARGUMENT_EXCEPTION: &str = "InvalidArgumentException";

/// What this service says when the refusal carries no violation to state at all — the
/// modeled shape's own sentence, since there is no member to name.
const INVALID_ARGUMENT_MESSAGE: &str = "The value for this input parameter is invalid.";

/// How this service WORDS a request the boundary validator refused before any handler
/// ran, or `None` to keep the framework's wording for a refusal that is not about a
/// value.
///
/// The message is the shared AWS wording of the constraint envelope — the framework's own
/// envelope, member paths and requirements, with the two renderings AWS differs from the
/// framework in: a `@pattern` requirement is quoted without the whole-value anchors the
/// model wraps it in, and a clause states NO rejected value. Nothing about the envelope,
/// the paths or the requirements is spelled here, so this service cannot drift from the
/// others that state the same sentence; what it chooses, and all it chooses, is which
/// clauses state no value ([`states_no_value`]) and the ORDER an enumerated type's value
/// set is listed in.
///
/// The CODE stays the one the framework's renderer picks for a constraint refusal, because
/// this seam is handed the violations alone and the code is a property of the STACK that
/// fronts the refused operation: [`refusal_envelope`] states it, at the one seam that sees
/// which operation answered.
fn invalid_argument_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // An enumerated type's admissible values are listed in the order this service's front
    // door walks its own TABLE of them, which is neither the order the model declares them
    // in nor lexicographic, so each violation is re-ordered before the envelope is rendered
    // (`service_common::validation_message::in_hashed_enum_order`). The derivation is
    // computed from the declared values themselves, so it belongs to the enumerated TYPE
    // rather than to a member, an operation or a depth, and a type the model gains needs no
    // row anywhere. Only the values move: the member path, the value half and the
    // requirement's wording stay with the one place that writes them, and every other
    // violation class comes back untouched.
    let stated = Refusal::Invalid(
        violations
            .iter()
            .map(service_common::validation_message::in_hashed_enum_order)
            .collect(),
    );
    let message = service_common::validation_message::aws_validation_refusal_withholding_violations(
        &stated,
        states_no_value,
    )
    .map(|stated| stated.message)
    .unwrap_or_else(|| INVALID_ARGUMENT_MESSAGE.to_string());
    Some(Refused {
        code: refusal_envelope::GENERIC_VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// Whether the clause for `violation` states no value — true for EVERY violation, whatever
/// constraint class it broke and whether or not the request carried a value at all.
///
/// This service's refusals state the REQUIREMENT a member's value had to satisfy and never
/// the value that missed it: the sentence is therefore the same for every request that
/// breaks one constraint of one member, so no caller-supplied bytes reach the message. A
/// member the request did not carry has no value to state either — not even the `null` the
/// framework writes for an absent one — because the requirement (`Member must not be null`)
/// is already the whole of what such a clause reports.
///
/// It is asked about the VIOLATION rather than about a member path because the answer is a
/// property of this service's whole envelope rather than of one member
/// (`service_common::validation_message::aws_validation_refusal_withholding_violations`).
fn states_no_value(_violation: &Violation) -> bool {
    true
}
