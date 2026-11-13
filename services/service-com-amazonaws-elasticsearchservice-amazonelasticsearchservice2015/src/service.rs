// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015` service: how it claims requests and how it words the
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
use std::collections::BTreeSet;
use std::sync::Arc;

/// The members of `refusal` whose clauses state NO submitted value: the ones the request
/// broke SEVERAL constraints of.
///
/// This front door echoes the value a member submitted while it has ONE thing to say
/// about that member, and states the member and its requirements alone once it has
/// several: what distinguishes the clauses of such a report is the requirement each
/// names, and the value is the same in all of them. A member with one violation keeps the
/// echo, whatever it carried and wherever it was bound — a URI label, a query parameter,
/// a body member nested in a structure.
///
/// Empty for a refusal that is not a constraint report, which keeps every clause the
/// framework's.
fn members_stated_without_their_value(refusal: &Refusal) -> BTreeSet<String> {
    let Refusal::Invalid(violations) = refusal else {
        return BTreeSet::new();
    };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut several: BTreeSet<String> = BTreeSet::new();
    for violation in violations {
        if !seen.insert(violation.path.as_str()) {
            several.insert(violation.path.clone());
        }
    }
    several
}

/// `refusal` with every enumerated value set it reports listed in the order this front
/// door states one.
///
/// The ORDER is the order a hash table over the admitted values is walked
/// ([`::service_common::validation_message::in_hashed_enum_order`], the fleet's one home
/// for it), which is the order a recorded refusal of this surface lists a set in — the
/// model declares `EngineType` `[OpenSearch, Elasticsearch]`
/// (`smithy_query(query="shape",shape="EngineType")`) and codegen hands the walk the
/// values sorted, so neither of those orders is what a clause states. MEMBERSHIP is
/// untouched, and so is every other part of the violation: only the order inside the
/// bracketed set changes.
///
/// Anything that is not a constraint report is handed back as it came, so the entry point
/// above still answers `None` for it.
fn in_hashed_value_set_order(refusal: &Refusal) -> Refusal {
    let Refusal::Invalid(violations) = refusal else {
        return refusal.clone();
    };
    Refusal::Invalid(
        violations
            .iter()
            .map(::service_common::validation_message::in_hashed_enum_order)
            .collect(),
    )
}

/// Whether the request these `parts` came from resolves to this service's signing name
/// AND addresses a first URI path segment this model publishes
/// (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `services/common`'s `claims_request` is the fleet's owner of that rule and is CALLED
/// here rather than re-implemented: it states it over `Parts`, which is exactly what
/// [`Service::can_handle`] is handed. `None` for the RPC service-shape id because this
/// model is `restJson1`, so the discriminator is the model-derived path space alone —
/// the same conjunct the framework's own predicate applies.
fn claims_own_path_space(parts: &http::request::Parts, signing_name: &str) -> bool {
    ::service_common::claims_request(parts, signing_name, crate::ROUTE_PATH_SEGMENTS, None)
}

/// The `com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015` emulated service: the generated [`ServiceDefinition`] plus
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
        // This service signs as `es` — a signing name, endpointPrefix AND arnNamespace it
        // SHARES with the modern Amazon OpenSearch Service model
        // (`services/service-com-amazonaws-opensearch-amazonopensearchservice`;
        // `smithy_query(query="service_info")` on either gives sigv4Name `es`,
        // endpointPrefix `es`, arnNamespace `es`, protocol `restJson1`, differing only in
        // sdkId and in the API version they declare — `2015-01-01` here, `2021-01-01`
        // there). AWS serves both APIs on the one `es.<region>.amazonaws.com` endpoint and
        // every SDK for either signs that one name, so the router — which resolves a
        // request's routing key from the SigV4 credential scope first — handed both
        // services the key `es`. Claiming on the signing name ALONE therefore made each
        // claim the other's traffic, the router found two claimants and answered `500
        // AmbiguousService` for EVERY request to the endpoint: a code no AWS model
        // declares, and one botocore retries, so a client burnt its retry budget before
        // failing.
        //
        // The claim is narrowed to this service's own modelled wire surface — the rule
        // every service under a shared signing name claims by, delegated to the
        // framework's own owner of it (see [`claims_own_path_space`]). This model is
        // `restJson1`, which binds every operation to a URI, so the discriminator is the
        // model-derived path space the generated `ROUTE_PATH_SEGMENTS` publishes — and it
        // is exactly how AWS separates the two APIs behind that one endpoint: every
        // operation of this model is bound under `/2015-01-01/…` and every operation of the
        // modern model under `/2021-01-01/…`, so the two first segments are disjoint and
        // each request has exactly one claimant.
        claims_own_path_space(parts, self.entry.identity().signing_name())
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
        // This API states a constraint refusal in the AWS envelope the framework
        // already renders, with ONE rendering of its own: a `@pattern` requirement
        // quotes the expression a value has to satisfy, not the whole-value pair the
        // model wraps that expression in — a model's `^…$` is how it demands a total
        // match, and the service publishes the expression inside it. An expression
        // that anchors its own BRANCHES is a whole-value expression already (the two
        // ends of its text are the first and last branch's anchors), so it is quoted
        // as the model declares it: dropping those ends would state a requirement of
        // no branch. Both renderings are recorded on this surface — see
        // `crate::state::quoted_pattern`, the one home for the choice, which the
        // handler-stated clauses reach too. The CHECK is unchanged (the boundary still
        // runs the model's pattern as written); only the quoting differs, and the code
        // travels with the message as it must.
        //
        // The other thing that is this service's own is the VALUE half of a clause: a
        // member the request broke SEVERAL constraints of is stated without the value it
        // submitted ([`members_stated_without_their_value`]).
        //
        // And one ORDER: an enumerated type's value set is listed the way this front door
        // walks its own hash table over the admitted values
        // ([`in_hashed_enum_order`]) — neither the order the model declares them in nor
        // lexicographic. The order belongs to the TYPE and is COMPUTED from the values, so
        // a type the model gains is listed in its own place with no table naming it, and
        // the set a caller is refused on by the boundary reads like the one a HANDLER
        // states for a map key it cannot reach (`crate::state::request_constraints`).
        //
        // The words are the fleet's one home for them rather than a copy here, so this
        // service's refusals cannot drift from another's.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        let without_value = members_stated_without_their_value(refusal);
        ::service_common::validation_message::aws_validation_refusal_withholding_with(
            &in_hashed_value_set_order(refusal),
            crate::state::quoted_pattern,
            |path| without_value.contains(path),
        )
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
