// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ecs-amazonec2containerservicev20141113` service: how it claims requests and how it words the
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

pub mod authorization;
pub mod request_pipeline;

use aws_smithy_schema::Schema;
use smithy_server_runtime::{
    http, DeserializableStruct, ErasedOperation, Refusal, Refused, ServerHttpProtocol, Service,
    Violation, ViolationKind,
};
use std::sync::Arc;

/// Read a request into `T` through the protocol and schema the dispatch sequence uses, so a
/// layer that runs before dispatch reads exactly the value the handler would be given, or
/// `None` for a body it cannot read at all — which is the framework's refusal, not that
/// layer's question.
pub fn read_input<T: DeserializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    schema: &'static Schema,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<T> {
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    T::deserialize(&mut *deserializer).ok()
}

/// The error this service answers a request refused before any handler ran with: "The
/// specified parameter isn't valid. Review the available parameters for the API
/// request", the one such failure this service's model declares. Every ECS operation
/// that takes parameters declares it, and none declares a validation error shape
/// besides it.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The member a describe names the optional fields it wants back in.
const INCLUDE_MEMBER: &str = "include";

/// What Amazon ECS says when a CLUSTER read asks for a field that is not one of the
/// fields a cluster can be described with.
const INVALID_CLUSTER_IDENTIFIER: &str = "The cluster identifier is invalid";

/// Whether `violation` is a value of a cluster read's `include` list that is not one of
/// the fields a cluster can be described with.
///
/// The refused element names the `include` member it came from, and the values it was
/// checked against are the cluster fields ([`crate::state::CLUSTER_FIELDS`]) — which is
/// what distinguishes it from the `include` of a read of some other resource, whose
/// member accepts that resource's own fields.
fn is_unknown_cluster_field(violation: &Violation) -> bool {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return false;
    };
    violation
        .path
        .split('.')
        .next()
        .is_some_and(|member| member == INCLUDE_MEMBER)
        && allowed.len() == crate::state::CLUSTER_FIELDS.len()
        && allowed
            .iter()
            .all(|value| crate::state::CLUSTER_FIELDS.contains(&value.as_str()))
}

/// What the modelled refusal says when the walk reported no clause to state: the
/// `InvalidParameterException` doc's own sentence. A refusal of this service carries a
/// modelled code whatever the walk had to say about it, so this is the message of last
/// resort rather than a reason to leave the refusal unworded.
const INVALID_PARAMETER_MESSAGE: &str =
    "The specified parameter isn't valid. Review the available parameters for the API \
     request.";

/// Whether `violation` is a `tags` entry whose key or value carries a character the tag
/// character set does not admit.
///
/// The offending value sits under the `tags` member the request carried it in, and what it
/// failed is that member's `@pattern` — the one constraint on a tag that is about its
/// CHARACTERS, which is what Amazon ECS answers its own tag sentence for. A tag that is
/// too long fails a length bound instead and keeps the framework's clause.
fn is_invalid_tag_character(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::Pattern(_))
        && violation
            .path
            .split('.')
            .next()
            .is_some_and(|member| member == TAGS_MEMBER)
}

/// The member a request carries its caller-supplied tag set in.
const TAGS_MEMBER: &str = "tags";

/// The sentence a capacity-provider-strategy entry's `base` or `weight` outside its
/// modelled range earns, or `None` when `violation` is about something else.
///
/// Amazon ECS words these two in the strategy's own terms
/// ([`crate::state::capacity_provider_strategy_value_message`]), so the refusal is keyed
/// on the position the value sits in: an entry of a member whose name ENDS in the strategy
/// shape's own name — `capacityProviderStrategy` on the operations that place tasks, and
/// `defaultCapacityProviderStrategy` on the ones that set a cluster's default — and the
/// member of that entry that crossed a numeric bound.
fn capacity_provider_strategy_bound(violation: &Violation) -> Option<String> {
    if !matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)
    ) {
        return None;
    }
    let list = violation.path.split('.').next()?;
    if !list
        .to_ascii_lowercase()
        .ends_with(CAPACITY_PROVIDER_STRATEGY_SUFFIX)
    {
        return None;
    }
    let member = violation.path.rsplit('.').next()?;
    let value = violation.value.as_deref()?;
    crate::state::capacity_provider_strategy_value_message(member, value)
}

/// The tail every member carrying a capacity provider strategy shares, lowercased so the
/// comparison is about the SHAPE the member holds and not about how the member spells its
/// own prefix.
const CAPACITY_PROVIDER_STRATEGY_SUFFIX: &str = "capacityproviderstrategy";

/// How this service words the refusal of a request whose values failed the rules the
/// model declares.
///
/// The cases, in the order they are decided. A cluster read that asks for an unknown
/// `include` field is the one such refusal Amazon ECS states in its own sentence
/// ([`INVALID_CLUSTER_IDENTIFIER`]). A tag whose characters are outside the set a tag
/// admits, and a capacity-provider-strategy `base` or `weight` outside its range, are
/// conditions Amazon ECS words in DOMAIN terms rather than in the model's — one fixed
/// sentence about the tag set, and the strategy member with its whole range — so neither
/// keeps the generic clause. A value outside an ENUM's value set is worded from the
/// request — the member in Amazon ECS's own words and the values it accepts
/// ([`crate::state::enum_value_message`]), the sentence the corpus records for
/// `assignPublicIp`; when more than one value is outside its set, the sentence names the
/// FIRST of them, which is the one member the caller has to change first, as every other
/// refusal this service derives from a request does. Every other one keeps the
/// framework's clause, which states the value, the position and the requirement it
/// failed, falling back to [`INVALID_PARAMETER_MESSAGE`] when there is no clause to
/// state.
fn refusal_message(violations: &[Violation]) -> String {
    if violations.iter().any(is_unknown_cluster_field) {
        return INVALID_CLUSTER_IDENTIFIER.to_string();
    }
    if violations.iter().any(is_invalid_tag_character) {
        return crate::state::INVALID_TAG_CHARACTERS_MESSAGE.to_string();
    }
    if let Some(message) = violations.iter().find_map(capacity_provider_strategy_bound) {
        return message;
    }
    let outside_an_enum = violations.iter().find_map(|violation| match &violation.kind {
        ViolationKind::EnumValue(accepted) => Some((&violation.path, accepted)),
        _ => None,
    });
    if let Some((path, accepted)) = outside_an_enum {
        return crate::state::enum_value_message(path, accepted);
    }
    ::aws_smithy_schema_server::envelope(violations)
        .unwrap_or_else(|| INVALID_PARAMETER_MESSAGE.to_string())
}

/// The `com-amazonaws-ecs-amazonec2containerservicev20141113` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// What this service does with a request before a handler runs: the IAM authorization
    /// decision, then the modelled operation the routing key names (see
    /// [`request_pipeline`]). Held on the service because [`Service::override_operation`]
    /// hands back a borrow of one of its own operations, and holding the shared engine
    /// because that decision is answered from this emulator's IAM and STS stores.
    pipeline: request_pipeline::RequestPipeline<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            pipeline: request_pipeline::RequestPipeline::new(engine),
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
        // EDITED HERE: every request this service serves is handed to
        // [`request_pipeline`], which takes the IAM authorization decision this service
        // makes BEFORE any handler runs ([`authorization`]) and then hands the request to
        // the modelled operation the routing key names. That decision needs both the
        // operation, which only this seam knows, and the request context, which only a
        // claimed operation is handed — which is why it lives behind this one claim.
        //
        // Claimed only for a routing key the generated table actually serves, so a request
        // naming no operation of this service still reaches `override_routing_error` rather
        // than a layer with nothing to run.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.entry.routing_table().get(key)?;
        Some(&self.pipeline as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Amazon ECS answers such a refusal with an error it MODELS: a client
        // branches on the error type, and this service declares no validation
        // error shape at all, so the code is the modeled
        // `InvalidParameterException` (HTTP 400) rather than a shape no operation
        // of this service can return. The wording comes from [`refusal_message`].
        //
        // An authentication refusal keeps the framework's wording: it is not a
        // request-parameter failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: INVALID_PARAMETER_EXCEPTION.to_string(),
                message: refusal_message(violations),
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
