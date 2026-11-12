// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-opensearchserverless-opensearchserverless` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name; EDITED HERE to
//!   claim a COLLECTION's own data-plane traffic too (see [`collection_rest`]).
//! - `override_operation` — EDITED HERE for the operations whose input carries a
//!   `CollectionName`: the key and the handler stay the generated table's, but the
//!   operation is served through [`CollectionNameRefusal`] so a `@pattern` refusal of a
//!   collection name reports the class this service's own name validator reports. See
//!   "How this service reports a @pattern".
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. This service restates one thing there: a `@pattern`
//!   clause names the regex without the trait's anchors. See the inline note on
//!   that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! # How this service reports a `@pattern`
//!
//! A refusal clause states the regular expression the SERVICE's own validator applies to
//! the member, which is the model's trait without its `^`/`$` anchors
//! ([`as_reported`]) — except for a collection name, whose validator reports a character
//! class the public trait does not carry ([`COLLECTION_NAME_REPORTED_PATTERN`]).
//!
//! Which member a violation belongs to is `(operation, member path)`: `CollectionName`,
//! `CollectionGroupName` and `PolicyName` all declare the SAME trait and all sit at the
//! member path `name`, so the path alone cannot tell a collection name from a policy
//! name. The operation is not reachable at the framework's refusal seam —
//! [`Service::override_refusal`] is handed the violations alone — so the operations whose
//! input carries a `CollectionName` are served through [`CollectionNameRefusal`], which
//! has the operation in scope as `O::NAME`. Every other operation keeps the framework's
//! own sequence and is reworded through `override_refusal`.
//!
//! Two costs of serving those operations this way are deliberate: the wrapper reads the
//! request a second time (the framework's own `deserialize_input` + `constraints::validate`,
//! which the generated `run` then repeats), and a refusal it answers itself is not counted
//! in the framework's per-operation telemetry. It answers ONLY the refusal whose clause the
//! operation changes — every other request, refused or served, goes to the generated
//! `ErasedOperation::run` untouched.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! Two of those seams are taken for a second reason: a COLLECTION this service creates
//! publishes the endpoint "used to submit index, search, and data upload requests"
//! (`CollectionDetail$collectionEndpoint`), and the protocol spoken there is the OpenSearch
//! REST API, which this model does not describe — so a REST request to that host reached
//! this service's `awsJson1_0` front door and was refused for carrying no `x-amz-target`.
//! What a collection publishes is now a listener of its own
//! ([`crate::state::collection_plane`]), which answers that REST API at the root of the
//! address it reports; the seams below keep the emulator's own FRONT DOOR serving a request
//! addressed to a collection it HOLDS as well, so an address a caller was handed earlier,
//! or one carried in a path, still resolves. [`can_handle`](Service::can_handle) claims such
//! a request and [`collection_rest`] is what resolves and serves it: as an OPERATION for a
//! request presenting a SigV4 credential, so the emulator verifies the signature, and from
//! [`override_routing_error`](Service::override_routing_error) for one presenting none.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{envelope, Violation, ViolationKind};
use aws_smithy_schema_server::{constraints, ProtocolError};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The COLLECTION's own OpenSearch REST data plane: the surface served at the host a
/// collection publishes as its `collectionEndpoint`, which this service's model does not
/// describe and which no RPC target names. See that module for why two of the seams below
/// take it into account.
pub mod collection_rest;

/// The wire error code a boundary refusal carries, which is also this service's own
/// modelled `ValidationException` (`smithy_query(query="shape", shape="ValidationException")`:
/// "Thrown when the HTTP request contains invalid input or is missing required input").
/// Rewording a refusal must not change it: a client branches on the code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A boundary violation as this service STATES it when the operation is not known: a
/// `@pattern` failure reports the trait's regex without its anchors.
///
/// A pattern clause already means "the whole value must match", so the `^` and `$` the
/// model carries say nothing to a caller and the aoss endpoint does not print them:
/// recorded refusals of a value outside `CollectionName`, `CollectionGroupName` and
/// `PolicyVersion` each state `Member must satisfy regular expression pattern: <regex>`
/// with the anchors absent, while `smithy_query(query="shape", shape="CollectionName")`
/// shows them on the trait. Every other violation kind is restated unchanged.
fn as_reported(violation: &Violation) -> Violation {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return violation.clone();
    };
    let unanchored = pattern.strip_prefix('^').unwrap_or(pattern);
    let unanchored = unanchored.strip_suffix('$').unwrap_or(unanchored);
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::Pattern(unanchored.to_string()),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// The `@pattern` the model declares on `CollectionName`, `CollectionGroupName` and
/// `PolicyName` alike (`smithy_query(query="shape",
/// shape="CollectionName,CollectionGroupName,PolicyName")`).
///
/// A violation of any of the three arrives at the seams below as this one string, which is
/// why a collection name is told apart by the member it sits at and not by its regex.
const NAME_PATTERN: &str = "^[a-z][a-z0-9-]+$";

/// The character class this service's own COLLECTION-name validator reports, which admits
/// `_` where [`NAME_PATTERN`] does not: a recorded refusal of a collection name outside it
/// states `Member must satisfy regular expression pattern: [a-z][a-z0-9_-]+`, while a
/// collection GROUP name and a policy name are reported as their own trait.
///
/// It is what the clause REPORTS, not what the service accepts: what a collection name may
/// hold stays the modelled trait, which is also the documented rule (a name starts with a
/// lowercase letter and holds lowercase letters, digits and hyphens).
const COLLECTION_NAME_REPORTED_PATTERN: &str = "[a-z][a-z0-9_-]+";

/// Where a value typed `CollectionName` sits in one operation's input, as the framework's
/// walk spells the dotted path to it.
#[derive(Debug)]
enum CollectionNameAt {
    /// Exactly this dotted path.
    Member(&'static str),
    /// An element of this list member, which the walk spells
    /// `<member>.<1-based index>.member`.
    ElementOf(&'static str),
}

impl CollectionNameAt {
    /// Whether `path` — a dotted member path from a [`Violation`] — is this position.
    fn holds(&self, path: &str) -> bool {
        match self {
            Self::Member(member) => path == *member,
            Self::ElementOf(member) => path
                .strip_prefix(member)
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.strip_suffix(".member"))
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())),
        }
    }
}

/// Every input member typed `CollectionName`, by the operation that carries it
/// (`smithy_query(query="shape", shape="CreateCollectionRequest,CollectionNames,
/// ListCollectionsRequest,CollectionFilters")`): `CreateCollection`'s `name`,
/// `BatchGetCollection`'s `names` list, and `ListCollections`'s `collectionFilters.name`.
///
/// An operation absent from here reports its `@pattern` as the unanchored trait, which is
/// correct for every member typed anything else — including the `collectionGroupName` that
/// sits beside `CreateCollection`'s `name`.
const COLLECTION_NAME_MEMBERS: &[(&str, &[CollectionNameAt])] = &[
    ("BatchGetCollection", &[CollectionNameAt::ElementOf("names")]),
    ("CreateCollection", &[CollectionNameAt::Member("name")]),
    (
        "ListCollections",
        &[CollectionNameAt::Member("collectionFilters.name")],
    ),
];

/// Whether `path` in `operation`'s input is a member typed `CollectionName`.
fn is_collection_name(operation: &str, path: &str) -> bool {
    COLLECTION_NAME_MEMBERS
        .iter()
        .filter(|(named, _)| *named == operation)
        .any(|(_, positions)| positions.iter().any(|position| position.holds(path)))
}

/// A boundary violation of `operation` as this service STATES it: [`as_reported`], plus the
/// one member whose reported class is not its trait's — a collection name, reported as
/// [`COLLECTION_NAME_REPORTED_PATTERN`].
fn as_reported_by(operation: &str, violation: &Violation) -> Violation {
    let stated = as_reported(violation);
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return stated;
    };
    if pattern != NAME_PATTERN || !is_collection_name(operation, &violation.path) {
        return stated;
    }
    Violation {
        kind: ViolationKind::Pattern(COLLECTION_NAME_REPORTED_PATTERN.to_string()),
        ..stated
    }
}

/// One modelled operation, served with the constraint refusals whose clause depends on
/// WHICH operation was refused stated this service's way.
///
/// The violations come from the framework's own deserializer and constraint walk, so not
/// one modelled constraint is restated here, and the response comes from the framework's
/// own refusal renderer, so the status, the envelope and the error code are the ones the
/// generated sequence would have answered with. What the wrapper adds is the single fact
/// that sequence's refusal seam cannot see: the operation, which a per-operation wrapper
/// has as `O::NAME`.
struct CollectionNameRefusal<O> {
    operation: O,
}

impl<O> CollectionNameRefusal<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for CollectionNameRefusal<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Computed before the await: `&dyn ErasedServiceDefinition` is not `Sync` and
            // this future is, so nothing read off the definition may cross one.
            if let Some(refusal) = operation_stated_refusal::<O>(svc, req) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `O`'s refusal of `req` when the clause it states depends on the operation, else `None`
/// so the generated sequence serves the request as it always does.
///
/// `None` covers every request this wrapper has no reason to answer: one the model accepts,
/// one the codec cannot read at all (a serialization failure is not a refusal, and the
/// framework words it per protocol), and one whose violations read the same with the
/// operation known as without it — a `@length` bound, an absent member, or a `@pattern` on
/// any member but a collection name.
fn operation_stated_refusal<O: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
) -> Option<RtResponse> {
    let definition = svc.definition();
    // The protocol the generated sequence would read this request with: the author's
    // override when there is one, else the model-derived protocol.
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| definition.protocol());
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?;
    drop(deserializer);

    let violations = constraints::validate(&input, O::INPUT_SCHEMA);
    let stated: Vec<Violation> = violations
        .iter()
        .map(|violation| as_reported_by(O::NAME, violation))
        .collect();
    if stated == violations.iter().map(as_reported).collect::<Vec<Violation>>() {
        return None;
    }
    let chosen = Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: envelope(&stated)?,
    };
    Some(definition.refusal_response(
        ProtocolError::Invalid(violations),
        Some(chosen),
        req,
    ))
}

/// The `com-amazonaws-opensearchserverless-opensearchserverless` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because a COLLECTION's own data-plane traffic ([`collection_rest`]) is
    /// claimed by `can_handle` and served from `override_operation` /
    /// `override_routing_error`, none of which is an operation with a `self.engine` of its
    /// own: all of them must read the one store the control-plane operations write, not a
    /// second copy.
    engine: Arc<Eng>,
    /// The collection REST data plane as an [`ErasedOperation`], so the router verifies the
    /// SigV4 credential a signed data-plane request carries before anything is served.
    /// Held in a field, built once, because `override_operation` hands back a reference
    /// borrowed from `&self`.
    credentialed_data_plane: CredentialedDataPlane<Eng>,
    /// The operations named in [`COLLECTION_NAME_MEMBERS`], each the generated table's own
    /// operation wrapped in [`CollectionNameRefusal`] and built from the SAME shared
    /// engine, so a wrapped operation reads and writes the one store. Keyed by the routing
    /// key the protocol resolves, which is the key the generated table uses.
    collection_name_refusals: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut collection_name_refusals = OperationRoutingTable::new();
        collection_name_refusals.add_operation(
            <crate::ops::batch_get_collection::BatchGetCollection<Eng> as Operation>::NAME,
            Box::new(CollectionNameRefusal::new(
                crate::ops::batch_get_collection::BatchGetCollection::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        collection_name_refusals.add_operation(
            <crate::ops::create_collection::CreateCollection<Eng> as Operation>::NAME,
            Box::new(CollectionNameRefusal::new(
                crate::ops::create_collection::CreateCollection::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        collection_name_refusals.add_operation(
            <crate::ops::list_collections::ListCollections<Eng> as Operation>::NAME,
            Box::new(CollectionNameRefusal::new(
                crate::ops::list_collections::ListCollections::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            credentialed_data_plane: CredentialedDataPlane {
                engine: Arc::clone(&engine),
            },
            engine,
            collection_name_refusals,
        }
    }
}

/// The auth schemes a credentialed data-plane request is verified under.
///
/// SigV4 is the only scheme this service's model declares, and it is the one a client signs
/// a request to a collection's endpoint with, so a data-plane request arrives either signed
/// — which this names so the router's own authenticator verifies it — or unsigned, which
/// the routing-error seam answers without the router authenticating anything.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// The collection REST data plane for a request that PRESENTS a SigV4 credential.
///
/// It holds the SAME shared engine the generated entry threads into every operation, and
/// serves the request through exactly the function the routing-error seam uses
/// ([`collection_rest::serve`]), so the credentialed and the anonymous path cannot diverge
/// in what they serve — only in whether a signature was verified on the way in.
struct CredentialedDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for CredentialedDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let _ = ctx;
        Box::pin(async move {
            if collection_rest::claims_request(&*self.engine, req) {
                return collection_rest::serve(&*self.engine, req);
            }
            // Unreachable: `override_operation` names this only for a request this data
            // plane claims.
            svc.definition().routing_error_response(req)
        })
    }

    /// This is NOT a modelled operation, so there is no modelled operation for the router
    /// to read schemes off: without this the router would find no scheme for a data-plane
    /// request and could not verify the credential it carries.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
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
        let claims_control_plane = match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        };
        claims_control_plane
            // And a COLLECTION's own data-plane traffic: a request addressed to the
            // endpoint host a collection this service holds published
            // ([`collection_rest::claims_parts`]). Such a request names no modelled
            // operation — its path is an OpenSearch REST path and it carries no RPC target
            // — so an unsigned one could resolve no claimant at all, for the one address
            // this service told the caller to send index, search and data-upload requests
            // to.
            //
            // The claim is narrowed to a collection this service HOLDS, read from the one
            // shared store, rather than to a host or a path shape alone, so one request
            // keeps one claimant.
            || collection_rest::claims_parts(&*self.engine, parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in
        // [`CollectionNameRefusal`] for the operations whose input carries a
        // `CollectionName`, so a `@pattern` refusal of a collection name reports the class
        // this service's name validator reports rather than the member's trait. A key that
        // names no wrapped operation answers `None` here, exactly as it does for a key the
        // model does not name, and the generated table serves it unchanged.
        //
        // A data-plane request that PRESENTS a SigV4 credential is named as an operation
        // first, so the router verifies that credential's signature before anything is
        // served — the router authenticates only what it names an operation for. One
        // carrying no credential is left unnamed on purpose: it is answered by
        // `override_routing_error` below, which the router reaches before authentication.
        if collection_rest::claims_request(&*self.engine, req)
            && collection_rest::carries_sigv4_credential(req)
        {
            return Some(&self.credentialed_data_plane);
        }
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.collection_name_refusals.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. This service restates exactly one thing: a `@pattern` clause names
        // the regex without the trait's anchors, the way the aoss endpoint does (see
        // [`as_reported`]). Everything else — the envelope, the clause order, the error
        // code — stays the framework's, and a refusal carrying no pattern violation keeps
        // the framework's own wording by answering `None`.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let restated: Vec<Violation> = violations.iter().map(as_reported).collect();
        if restated == *violations {
            return None;
        }
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(&restated)?,
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
        //
        // A request to a COLLECTION's own endpoint carrying no SigV4 credential is exactly
        // that case by construction, so this is where it is served — BEFORE
        // authentication, which is what lets a client carrying no AWS credential be
        // answered at all rather than told its REST request named no RPC target.
        if collection_rest::claims_request(&*self.engine, req) {
            return Some(collection_rest::serve(&*self.engine, req));
        }
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
