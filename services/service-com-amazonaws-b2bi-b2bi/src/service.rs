// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-b2bi-b2bi` service: how it claims requests and how it words the
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
//! - `override_protocol` — the codec the modelled sequence runs through. This
//!   service returns its own [`wire::B2biJsonProtocol`]: the model's `awsJson1_0`
//!   with the two corrections that belong to the wire rather than to any one
//!   operation (the `date-time` form of a timestamp, and the `__type` a nested
//!   structure carries).
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

pub mod wire;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, which the model declares service-wide with
/// `@httpError 400` (`smithy_query(query="shape",shape="ValidationException")`). It is
/// also the code the framework would have chosen, restated here because
/// `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Render one violation the way b2bi states it.
///
/// Two classes are stated differently from the way the framework states them, and every
/// other class keeps the framework's own sentence (`Violation::message`, value and all):
/// a class this service has no recorded wording for is better left as the framework
/// states it than reworded on a guess.
///
/// * a violated numeric `@range` bound names the member path and the bound it failed, and
///   NOTHING of the value that was rejected;
/// * a violated `@pattern` states the regular expression WITHOUT the `^`/`$` anchors the
///   model writes around the service's own expression, which is the tree's shared
///   rendering (`::service_common::validation::unanchored`).
///
/// The requirement sentences and the envelope below mirror the framework's own
/// (`vendor/smithy-schema-server/src/constraints.rs`: `Violation::requirement` and
/// `envelope`) and must stay in step with them. They are restated here rather than
/// reused because that crate renders a clause only through `Violation::message`, which
/// always prefixes the rejected value and always prints a pattern as the model writes it.
///
/// MIGRATION NOTE (the one `services/common/src/validation.rs` asks every new inliner to
/// leave): this is the same value-free clause rendering that
/// `services/service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs`
/// already inlines, over the same `N validation error(s) detected: …` envelope. It is
/// inlined again here because `Violation::requirement` is private to the vendored
/// framework and `vendor/` is read-only from inside the emulator, so there is no public
/// seam to call; the fix that owns the migration declares that shared module (or a public
/// `Violation::message_without_value()`) and replaces both copies with a call to it.
fn clause(violation: &Violation) -> String {
    let (subject, requirement) = match &violation.kind {
        // A numeric bound is stated with no value at all, whether or not the member is
        // `@sensitive`.
        ViolationKind::MinValue(min) => (
            "Value".to_string(),
            format!("Member must have value greater than or equal to {min}"),
        ),
        ViolationKind::MaxValue(max) => (
            "Value".to_string(),
            format!("Member must have value less than or equal to {max}"),
        ),
        // A pattern keeps the framework's subject — which already withholds a
        // `@sensitive` member's value — and restates only the expression.
        ViolationKind::Pattern(pattern) => (
            subject(violation),
            format!(
                "Member must satisfy regular expression pattern: {}",
                ::service_common::validation::unanchored(pattern)
            ),
        ),
        _ => return violation.message(),
    };
    format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    )
}

/// How a clause names the value that was rejected, exactly as the framework names it: the
/// value itself, `null` for a member that was absent, and NOTHING AT ALL for a
/// `@sensitive` member, whose value never reaches a response.
fn subject(violation: &Violation) -> String {
    if violation.sensitive {
        return "Value".to_string();
    }
    match &violation.value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value null".to_string(),
    }
}

/// Whether `violation` is one of the classes this service rewords: a violated numeric
/// `@range` bound, or a violated `@pattern`.
fn is_reworded(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_) | ViolationKind::Pattern(_)
    )
}

/// The `ValidationException` message for a refused request: the AWS validation envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the refusal, leaving the framework's wording, for a request carrying no
/// violation of a class this service rewords — there is nothing to reword there. A request
/// that mixes one with another class IS reworded, so the reworded clause reads as this
/// service states it while every other clause reads exactly as the framework would have
/// stated it.
fn reworded_envelope(violations: &[Violation]) -> Option<String> {
    if !violations.iter().any(is_reworded) {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-b2bi-b2bi` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's wire protocol, built once here because `override_protocol` hands
    /// back a borrow of it. See [`wire`].
    protocol: wire::B2biJsonProtocol,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            protocol: wire::B2biJsonProtocol::new(),
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
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // b2bi states a violated numeric `@range` bound WITHOUT the value that violated
        // it (the message names the member path and the bound only), and a violated
        // `@pattern` with the expression unanchored. The code stays the modelled
        // `ValidationException` (HTTP 400) — what the framework would have answered — so
        // only the wording changes, and only for those classes: every other refusal,
        // authentication included, keeps the framework's wording because
        // `reworded_envelope` declines it.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: reworded_envelope(violations)?,
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model-derived protocol is `awsJson1_0`, and so is this one: it delegates
        // routing, request deserialization and the error envelope to the framework's own
        // `awsJson1_0`, and differs only in the success body — where a timestamp goes out
        // in the `date-time` form its model declares, and a nested structure of the
        // service's shared internal model carries the `__type` discriminator the
        // recordings of this API show on it. See [`wire`] for both rules and the shapes
        // they cover.
        Some(&self.protocol)
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
