// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice` service: how it claims requests and how it words the
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
//! Three of those seams are taken here. `override_refusal` words a framework refusal the
//! way this service words one (below), and `override_operation` plus `definition` are
//! taken for the reasons named in [`query_error_envelope`]: an awsQuery error response
//! carries a `<RequestId>` element in its `<ErrorResponse>` envelope, which the
//! framework's renderers omit, and an environment's events are PUBLISHED to the Amazon SNS
//! topic its configuration names, which no single operation owns.
//! `override_operation` wraps the operation the routing table
//! names, so the element reaches a handler-returned error, the scaffold's 501 and a
//! refusal raised while serving it, and the [`ElasticBeanstalkDefinition`] this file hands
//! back from `definition` adds it to a routing failure, a transport failure and a
//! pre-dispatch refusal. Everything else about those paths — status, `<Type>`, `<Code>`,
//! `<Message>` — stays with the framework.

pub mod authorization;
pub mod environment_plane;
pub mod query_error_envelope;

use query_error_envelope::{ElasticBeanstalkDefinition, RequestIdEnvelopeOperation};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: ElasticBeanstalkDefinition<Eng>,
    /// The envelope wrapper every routable request is served through. Held in a field,
    /// built once, because `override_operation` hands back a reference to it.
    envelope: RequestIdEnvelopeOperation<Eng>,
    /// The ENVIRONMENT data plane: the traffic a client sends to the address an
    /// environment publishes, which names no modelled operation. Held in a field
    /// for the same reason.
    environment_plane: environment_plane::EnvironmentPlane<Eng>,
    /// The one shared engine, which the data plane's CLAIM reads the store
    /// through: `can_handle` is answered from the records this service holds.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: ElasticBeanstalkDefinition::new(Arc::clone(&engine)),
            envelope: RequestIdEnvelopeOperation::new(Arc::clone(&engine)),
            environment_plane: environment_plane::EnvironmentPlane::new(Arc::clone(&engine)),
            engine,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
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
            // And an ENVIRONMENT's own traffic: a request addressed to the
            // address an environment of this store publishes
            // ([`environment_plane::claims_parts`]). Such a request names no
            // modelled operation and carries no credential scope naming this
            // service — a client of a published address has no AWS credential to
            // sign with — so the front door answered a routing failure for the
            // one address this service told the caller to use.
            None => environment_plane::claims_parts(&*self.engine, parts),
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // An ENVIRONMENT's own traffic is served by the data plane, which is the
        // one request this service answers that names no modelled operation by
        // construction (no modelled operation is bound at an environment's
        // address). It gets first refusal, so the claim `can_handle` made and the
        // serving are decided by the one rule.
        if self.environment_plane.claims(req) {
            return Some(&self.environment_plane as _);
        }
        // Every request the generated routing table can serve is served through the
        // envelope wrapper, which delegates to that same table entry and then adds the
        // `<RequestId>` the awsQuery error envelope carries (see
        // [`query_error_envelope`]). The routing stays the model's: a request whose
        // `Action` names no modelled operation is declined here, so the framework answers
        // it as the unroutable request it is — through the definition above, which adds
        // the element to that answer too.
        self.entry.routes(req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a rule the model declares). Two
        // DIFFERENT failures arrive here and Elastic Beanstalk names them with two
        // different codes:
        //
        // * an absent required parameter -> `MissingRequiredParameter`
        //   (`TerminateEnvironment$EnvironmentId`: "If you do not specify
        //   either, Elastic Beanstalk returns MissingRequiredParameter error.")
        // * a value outside a modelled constraint (an enumerated value set, a
        //   `@length` or `@range` bound, a `@pattern`) -> `ValidationError`, the
        //   awsQuery request-validation code. This refusal is raised BEFORE any
        //   handler runs, so it is a statement about the SHAPE of the request and
        //   not about the account's state — which is why it must not borrow
        //   `InvalidParameterValue`, the code this service documents for a
        //   well-formed parameter that cannot be used (`CreateEnvironment`
        //   `$EnvironmentName`: "If the specified name already exists in the
        //   region, Elastic Beanstalk returns an InvalidParameterValue error").
        //   Handlers raise that one themselves, once identity is resolved.
        //
        // Both the code and the message are replaced together, so a client that
        // branches on the code reads one that matches the prose. An
        // authentication refusal keeps the framework's wording.
        match refusal {
            Refusal::Invalid(violations) if !violations.is_empty() => {
                let all_absent = violations.iter().all(|violation| {
                    violation.kind == smithy_server_runtime::ViolationKind::MissingRequiredMember
                });
                if all_absent {
                    return Some(Refused {
                        code: crate::scaffolded::MISSING_REQUIRED_PARAMETER.to_string(),
                        message: violations
                            .iter()
                            .map(smithy_server_runtime::Violation::message)
                            .collect::<Vec<String>>()
                            .join(" "),
                    });
                }
                // Reaching here means at least one value was read and failed a
                // constraint. A refusal that mixes the two classes does not arrive
                // from the wire: absence is found while READING the request, which
                // stops before the constraint walk over what was read, so an
                // absent required member is always refused on its own.
                Some(Refused {
                    code: crate::scaffolded::VALIDATION_ERROR.to_string(),
                    message: validation_error_message(violations),
                })
            }
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

/// The marker the framework's own clause is built
/// around: [`smithy_server_runtime::Violation::message`] renders
/// `Value '<v>' at '<path>' failed to satisfy constraint: <requirement>`, and the
/// requirement itself is what AWS nests when the offending value is a COLLECTION
/// ELEMENT. `Violation::requirement` is private to the framework, so the
/// requirement is recovered from the one rendering that states it rather than
/// spelled a second way here (which would let the two drift apart).
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// AWS's standard request-validation message, built from the request:
///
/// ```text
/// <N> validation error(s) detected: Value '<value>' at '<member path>' failed to
/// satisfy constraint: <requirement>[; …]
/// ```
///
/// `N` counts the MEMBERS that failed, not the values: several bad elements of
/// one list are one violation of that list member, reported with the offending
/// elements bracketed and the element requirement nested, which is the form AWS
/// answers a list-of-enum member with
/// (`Value '[<bad>]' at 'attributeNames' failed to satisfy constraint: Member must
/// satisfy constraint: [Member must satisfy enum value set: [...]]`).
///
/// Every violation yields exactly one clause, so for the non-empty violation list
/// this is only ever called with (see `override_refusal`'s guard) the count is at
/// least one and the envelope's noun is never read off an empty list.
fn validation_error_message(violations: &[smithy_server_runtime::Violation]) -> String {
    let clauses = violation_clauses(violations);
    let noun = if clauses.len() == 1 {
        "validation error"
    } else {
        "validation errors"
    };
    format!(
        "{} {noun} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// One clause per failing MEMBER, in the order the walk found them.
///
/// A violation whose path ends at a collection ELEMENT (`<member>.<index>.member`)
/// is attributed to the collection member itself, so two bad elements of one list
/// collapse into a single clause listing both values.
fn violation_clauses(violations: &[smithy_server_runtime::Violation]) -> Vec<String> {
    /// One collection member's violations: where its clause belongs in the
    /// output, which member and requirement identify the group, and the elements
    /// that failed. The slot lives on the group, so the clause can only ever be
    /// written where the group was opened.
    struct Collapsed {
        slot: usize,
        member: String,
        requirement: String,
        values: Vec<String>,
    }

    let mut collapsed: Vec<Collapsed> = Vec::new();
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let Some(member) = collection_member_path(&violation.path) else {
            clauses.push(violation.message());
            continue;
        };
        let requirement = requirement_of(violation);
        let value = violation.value.clone().unwrap_or_else(|| "null".to_string());
        if let Some(group) = collapsed
            .iter_mut()
            .find(|group| group.member == member && group.requirement == requirement)
        {
            group.values.push(value);
            continue;
        }
        collapsed.push(Collapsed {
            // The clause is rendered once the group is complete; its place in the
            // output is taken now, so the order the violations arrived in holds.
            slot: clauses.len(),
            member: member.to_string(),
            requirement,
            values: vec![value],
        });
        clauses.push(String::new());
    }
    for group in collapsed {
        clauses[group.slot] = format!(
            "Value '[{}]' at '{}' failed to satisfy constraint: \
             Member must satisfy constraint: [{}]",
            group.values.join(", "),
            group.member,
            group.requirement
        );
    }
    clauses
}

/// The requirement the framework states for `violation` — the text after the LAST
/// [`CONSTRAINT_MARKER`] in a clause rendered with NO value attached.
///
/// Both halves of that matter: a clause rendered WITH the value starts with the
/// caller's own bytes (`Value '<value>' at '<path>' …`), and a member path can
/// carry a caller-chosen map key, so splitting at the FIRST marker would let
/// request text become the "requirement" this nests into a collapsed clause.
/// Dropping the value and splitting at the LAST marker leaves nothing a caller
/// controls on the requirement side.
///
/// This is the requirement half of a validation message, which
/// `services/common/src/validation.rs` lists under "Still to migrate here" —
/// every service words it identically. Delete this function in favour of that
/// module's own once it carries it; only the envelope around the clauses
/// (`N validation error(s) detected`, the collapse of collection elements) is
/// this service's dialect.
fn requirement_of(violation: &smithy_server_runtime::Violation) -> String {
    let stated = smithy_server_runtime::Violation {
        path: violation.path.clone(),
        kind: violation.kind.clone(),
        value: None,
        sensitive: violation.sensitive,
    }
    .message();
    match stated.rsplit_once(CONSTRAINT_MARKER) {
        Some((_, requirement)) => requirement.to_string(),
        // Not a clause this framework built: keep what it said rather than
        // inventing a requirement for it.
        None => violation.message(),
    }
}

/// The collection member a dotted path ends in an ELEMENT of, if it does:
/// `attributeNames.1.member` -> `attributeNames`. A path that ends anywhere else
/// — a plain member, or a struct member inside a list element
/// (`optionSettings.1.member.namespace`, which AWS reports at that full path) —
/// yields `None`.
fn collection_member_path(path: &str) -> Option<&str> {
    let (head, index) = path.strip_suffix(".member")?.rsplit_once('.')?;
    let indexed = !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit());
    indexed.then_some(head)
}
