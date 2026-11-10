// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-eks-awswesleyfrontend` service: how it claims requests and how it words the
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
//!   correctly. EDITED HERE: it claims a tag request the model's own tag contract
//!   refuses, so that refusal is stated in the error code the called operation
//!   declares (see [`tag_contract`]), an add-on request whose `resolveConflicts`
//!   is outside the modelled enum, so that refusal names the operation it was sent
//!   to (see [`resolve_conflicts_contract`]), and a request whose body the
//!   framework's walk refuses but whose operation answers the pass-role
//!   authorization, a capability type this API does not define, or the missing
//!   cluster first (see [`precedence`], and [`off_enum_member_contract`] for the
//!   rule the undefined-value half of it implements). Every other
//!   request resolves through the generated routing table.
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
//! A sixth seam, `override_protocol`, is the one this service also takes: it serves
//! the model's own `restJson1` with the input deserializer wrapped, because this
//! service reads a body leniently — a scalar supplied in another of JSON's scalar
//! types than the modelled one, and the one member it reads without regard to case
//! (see [`lenient_json`]).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

pub mod lenient_json;
pub mod lenient_scalars;
pub mod off_enum_member_contract;
pub mod parameter_contract;
pub mod precedence;
pub mod resolve_conflicts_contract;
pub mod tag_contract;

use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, ServerHttpProtocol, Service,
};
use std::sync::Arc;

/// The codec a layer that runs BEFORE dispatch reads a request body with.
///
/// It has to be the codec the modelled sequence will read that body with — this
/// service's own (see [`lenient_json`]) — because a layer that reads a member
/// differently from the handler it precedes can refuse a request that handler
/// would have served: this service reads one add-on member without regard to case
/// (see [`lenient_scalars`]), and a pre-dispatch contract reading the strict codec
/// would judge such a value as naming nothing. The model's own codec stands in
/// only for a service that overrides none.
pub fn body_codec<'svc>(
    svc: &'svc dyn Service,
    definition: &'svc dyn ErasedServiceDefinition,
) -> &'svc dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| definition.protocol())
}

/// The `com-amazonaws-eks-awswesleyfrontend` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Answers a `TagResource` / `UntagResource` request the model's own tag
    /// contract refuses, in the error channel those two operations declare (see
    /// [`tag_contract`]). Held on the service because
    /// [`Service::override_operation`] hands back a borrow of one of its own
    /// operations.
    refused_tag_request: tag_contract::RefusedTagRequest,
    /// Answers a `CreateAddon` / `UpdateAddon` request whose `resolveConflicts`
    /// is outside the modelled enum, in this service's own sentence for that
    /// value (see [`resolve_conflicts_contract`]). Held on the service for the
    /// same reason the refused tag request is.
    refused_addon_request: resolve_conflicts_contract::RefusedAddonRequest,
    /// Answers a request whose body the framework's validating walk refuses but
    /// whose operation answers something else FIRST — the pass-role
    /// authorization of a role it hands Amazon EKS, a capability type this API
    /// does not define, or the cluster it is addressed to (see [`precedence`]).
    /// Held on the service for the same reason the two above are, and holding the
    /// shared engine because the missing-cluster half is answered from this
    /// service's store.
    precedence_first: precedence::PrecedenceFirst<Eng>,
    /// The codec the modelled sequence reads and writes this service's requests
    /// with: the model's own `restJson1`, reading a body the way this service
    /// reads one (see [`lenient_json`]). Held on the service because
    /// [`Service::override_protocol`] hands back a borrow of it and must never
    /// build one per request.
    protocol: lenient_json::LenientRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            refused_tag_request: tag_contract::RefusedTagRequest::new(),
            refused_addon_request: resolve_conflicts_contract::RefusedAddonRequest::new(),
            precedence_first: precedence::PrecedenceFirst::new(engine),
            protocol: lenient_json::LenientRestJson::new(),
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
        //
        // EDITED HERE: a tag request the model's own tag contract refuses is claimed
        // so that the refusal is stated in the error code the CALLED operation
        // declares. `TagResource` and `UntagResource` declare `BadRequestException`
        // and nothing else, where the rest of this service declares
        // `InvalidParameterException` — the code `override_refusal` below picks. That
        // seam is per SERVICE and cannot tell the two apart, so the choice is made
        // here, before dispatch, which is the only place the operation is known.
        // Every other request keeps the generated routing table as its only source.
        // See [`tag_contract`] for the whole argument and for the refusal this
        // answers instead.
        //
        // ALSO EDITED HERE, for the same reason: an add-on request whose
        // `resolveConflicts` is outside the modelled enum is refused in a sentence
        // that names the OPERATION it was sent to, which a per-service refusal seam
        // cannot word because no operation is named by the time it is reached. See
        // [`resolve_conflicts_contract`], and [`off_enum_member_contract`] for the
        // rule both it and the capability-type half of [`precedence`] implement.
        // The ROUTING KEY is resolved by the model's own codec, which is the one
        // that carries this service's route table; the BODY is read by the codec
        // dispatch will read it with (see [`body_codec`]).
        let protocol = self.entry.protocol();
        let key = protocol.routing_key(req).ok()?;
        let body = body_codec(self, &self.entry);
        if tag_contract::refusal_for(key, body, req).is_some() {
            return Some(&self.refused_tag_request as &dyn ErasedOperation);
        }
        if resolve_conflicts_contract::refusal_for(key, body, req).is_some() {
            return Some(&self.refused_addon_request as &dyn ErasedOperation);
        }
        // ALSO EDITED HERE, and for the same reason — the decision needs the
        // operation, which only this seam knows: a request whose body the
        // framework's validating walk would refuse, sent to an operation that
        // answers the pass-role authorization of a role it hands Amazon EKS, a
        // capability type this API does not define, or the cluster it is addressed
        // to, BEFORE it judges the body. The walk runs before any handler does, so
        // without this seam the body refusal pre-empts all three. See [`precedence`],
        // which claims nothing else.
        if precedence::has_a_question(key, body, req) {
            return Some(&self.precedence_first as &dyn ErasedOperation);
        }
        None
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // This service reads a request body leniently: a scalar supplied in
        // another of JSON's scalar types than the modelled one, and the one member
        // it reads without regard to case. The codec that applies it is otherwise
        // the model's own `restJson1`, and it writes every response unchanged. See
        // [`lenient_json`].
        Some(&self.protocol)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // EKS states that class of refusal as `InvalidParameterException` — "The
        // specified parameter is invalid. Review the available parameters for the
        // API request." — which nearly every operation that takes input declares,
        // rather than as the framework's generic `ValidationException`. So a
        // `CreateCluster` whose `name` breaks the modelled
        // `^[0-9A-Za-z][A-Za-z0-9\-_]*$` pattern or its 1..100 length answers
        // 400 `InvalidParameterException`, as the real service does.
        //
        // The MESSAGE is this service's own wherever it states one for the rule that
        // was broken (see [`parameter_contract`]): a parameter Amazon EKS publishes
        // its own refusal sentence for is answered in that sentence rather than in
        // the framework's constrained-leaf report, which is a different error
        // contract from the one this API states. A rule it states no sentence for
        // keeps the framework's report, which already names the member and the
        // constraint it broke.
        //
        // The seam is per SERVICE, not per operation, so it cannot word the two tag
        // operations — the only ones here that model `BadRequestException` instead —
        // in their own channel. Those two are claimed BEFORE dispatch by
        // `override_operation` above, so a refusal of theirs never reaches this seam;
        // everything that does reach it declares `InvalidParameterException`.
        //
        // Wording the refusal per OPERATION is not reachable from here:
        // `Refusal` (vendor/smithy-server-runtime/src/pre_dispatch.rs) carries
        // only the violations, no operation identity, and a refusal is raised
        // before any operation is named. So this picks the code the majority of
        // this service's operations declare, and the minority that declares another
        // one is answered before dispatch instead (see [`tag_contract`]) rather than
        // guessed at from the member paths in the violations.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidParameterException".to_string(),
                message: parameter_contract::refusal_message(violations)
                    .or_else(|| aws_smithy_schema_server::envelope(violations))
                    .unwrap_or_else(|| "invalid request".to_string()),
            }),
            // Authentication refusals belong to the framework: the emulator, not a
            // service, verifies signatures.
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
