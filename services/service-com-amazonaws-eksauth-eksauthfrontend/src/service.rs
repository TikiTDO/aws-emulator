// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-eksauth-eksauthfrontend` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. Here it
//!   resolves the SAME operation the generated table holds, behind the IAM
//!   authorization decision (`identity_authorization`), which AWS takes before the
//!   request's own input is read.
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

/// Whether IAM lets the caller perform the action it asks for — the decision taken in front
/// of this service's one operation, through the [`identity_authorization::IamAuthorized`]
/// wrapper `override_operation` below resolves.
pub mod identity_authorization;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Operation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The error shape every input-constraint refusal of this service is stated as.
///
/// From the model (`smithy_query(query="operation",
/// operation="AssumeRoleForPodIdentity")`): the operation's errors are
/// `AccessDeniedException`, `ExpiredTokenException`, `InternalServerException`,
/// `InvalidParameterException` (400), `InvalidRequestException`, `InvalidTokenException`,
/// `ResourceNotFoundException`, `ServiceUnavailableException` and `ThrottlingException`.
/// There is no `ValidationException`, so the framework's default code — and
/// `service_common::validation_message::aws_validation_refusal`, which words a refusal for
/// the services that DO model it — names a shape no client of this service can
/// deserialize. That is why this file words the refusal itself rather than delegating the
/// whole body to that helper.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The input members this service's model marks `@sensitive`, as dotted validation
/// paths.
///
/// From `smithy_query(query="shape", shape="AssumeRoleForPodIdentityRequest")`: `token`
/// targets `JwtToken`, which carries `@sensitive`; no other input member or target does.
/// A refusal naming one of these states the constraint it broke WITHOUT the value, the
/// way the service does — an error body is logged by whoever reads it, and the value
/// here is a credential.
const SENSITIVE_INPUT_PATHS: &[&str] = &["token"];

/// How this service states one constraint violation.
///
/// The clause is the framework's own ([`Violation::message`]), so the envelope's member
/// paths and every requirement have one source and cannot drift per service. Two things
/// are chosen here, and both are what `services/common/src/validation.rs` names as the
/// per-service halves of this rendering:
///
/// * a `@pattern` requirement states the expression UNANCHORED ([`unanchored`]). A
///   constraint pattern applies to the whole value by definition, so the `^`/`$` the
///   model wraps it in state nothing the clause does not already say, and this service's
///   recorded answer for a member modelled
///   `^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$` is the expression between
///   them;
/// * a `@sensitive` member's value is WITHHELD: the clause is rendered from a violation
///   carrying no value at all, so no part of the value can reach the message whatever the
///   value contains, and the framework's "Value null at '<member>'" — which would state a
///   null the request did not send — becomes "Value at '<member>'". An absent member has
///   no value to withhold and keeps the framework's own wording.
fn stated(violation: &Violation) -> String {
    // The walk reports a `@sensitive` member with its value already withheld, and this
    // service withholds the value of every member it classes as one, so either side of
    // that is the same clause: the value half is dropped and the subject names the member
    // alone.
    let withheld =
        violation.sensitive || (violation.value.is_some() && is_sensitive(&violation.path));
    let restated = Violation {
        path: violation.path.clone(),
        kind: match &violation.kind {
            ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(pattern)),
            other => other.clone(),
        },
        value: if withheld {
            None
        } else {
            violation.value.clone()
        },
        sensitive: violation.sensitive,
    };
    let clause = restated.message();
    if !withheld {
        return clause;
    }
    match clause.strip_prefix(VALUE_NULL_AT) {
        Some(rest) => format!("{VALUE_AT}{rest}"),
        // Already the withheld subject, which is what the framework words a withheld
        // violation with; it carries no value (none was attached), so it stands as it is.
        None => clause,
    }
}

/// How the framework opens a clause whose violation carries no value, and how this
/// service opens one whose value it withheld.
const VALUE_NULL_AT: &str = "Value null at '";
const VALUE_AT: &str = "Value at '";

/// `regex` with the pair of whole-value anchors the model wraps a `@pattern` in removed.
///
/// The pair is dropped POSITIONALLY and only when BOTH ends carry one: a `^` alone, or a
/// `$` alone, is an expression anchored at one end — a different expression, not an
/// anchored spelling of this one — and a `$` preceded by an odd run of backslashes is an
/// escaped dollar rather than an anchor. The CHECK always runs the model's pattern as
/// written; only this rendering drops the pair.
///
/// TODO: duplicate of `services/common/src/validation.rs::unanchored`; delete when that
/// module's migration lands.
fn unanchored(regex: &str) -> String {
    let Some(body) = regex.strip_prefix('^') else {
        return regex.to_string();
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => inner.to_string(),
        _ => regex.to_string(),
    }
}

/// Whether `text` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// Whether `path` names a `@sensitive` member, or anything inside one.
fn is_sensitive(path: &str) -> bool {
    SENSITIVE_INPUT_PATHS.iter().any(|sensitive| {
        path == *sensitive
            || path
                .strip_prefix(*sensitive)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// Wrap stated clauses in the envelope this service's refusals carry — `None` when there
/// is nothing to state, so the caller keeps the framework's own wording.
///
/// `aws_smithy_schema_server::envelope` renders this same envelope and is the source of
/// the wording, but it cannot render these clauses: it takes `Violation`s, and a clause
/// whose `@sensitive` value is withheld has no `Violation` that produces it (a violation
/// with no value states "Value null at"). `services/common/src/validation.rs` names the
/// envelope a service dialect for that reason.
fn validation_envelope(clauses: &[String]) -> Option<String> {
    match clauses {
        [] => None,
        [only] => Some(format!("1 validation error detected: {only}")),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        )),
    }
}

/// The `com-amazonaws-eksauth-eksauthfrontend` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// This service's one modelled operation behind the IAM decision
    /// ([`identity_authorization::IamAuthorized`]), built once here and never per request: it
    /// carries the operation's TYPE and a handle on the one shared engine, so it is a name
    /// rather than a second instance of the operation.
    authorized: Box<dyn ErasedOperation>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            authorized: Box::new(identity_authorization::IamAuthorized::<
                crate::ops::assume_role_for_pod_identity::AssumeRoleForPodIdentity<Eng>,
                Eng,
            >::new(Arc::clone(&engine))),
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

/// The routing key of the one operation this service binds, which is also the key the
/// generated routing table registers it under.
fn modeled_operation<Eng: Send + Sync + 'static>() -> &'static str {
    <crate::ops::assume_role_for_pod_identity::AssumeRoleForPodIdentity<Eng> as Operation>::NAME
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves, and
        // the operation it names is the one served — wrapped in
        // [`identity_authorization::IamAuthorized`] so IAM's decision about the action is
        // taken BEFORE the request's input is read, as AWS takes it (the refusal of an
        // unauthorized caller comes ahead of body validation). A request the key cannot be
        // read from, or one naming any other key, resolves to `None` exactly as before, so the
        // generated table and the router's routing-error path are untouched.
        let key = self.entry.protocol().routing_key(req).ok()?;
        (key == modeled_operation::<Eng>()).then_some(&*self.authorized)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // EVERY input-constraint refusal of this service is its own modeled
        // `InvalidParameterException` (400), whose shape the model documents as "The
        // specified parameter is invalid. Review the available parameters for the API
        // request." The framework's default names `ValidationException`, which this
        // service's model does not declare at all, so a client cannot catch it.
        //
        // The MESSAGE is not that documentation sentence: a constraint refusal of this
        // service is recorded answering the clause envelope built from the request —
        // `1 validation error detected: Value at 'token' failed to satisfy constraint:
        // Member must satisfy regular expression pattern: …` — which names the member
        // and the constraint so a caller can fix the request from it. The shape's doc
        // text is what the shape MEANS; the envelope is what this operation answers with.
        // Both the code and the message are replaced together, because a client branches
        // on the code.
        match refusal {
            Refusal::Invalid(violations) => {
                let clauses: Vec<String> = violations.iter().map(stated).collect();
                // Nothing to state: keep the framework's wording rather than
                // answering with an empty envelope.
                Some(Refused {
                    code: INVALID_PARAMETER.to_string(),
                    message: validation_envelope(&clauses)?,
                })
            }
            // Authentication is the emulator's own concern, and its 403 is not one of
            // this service's modeled parameter errors.
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
