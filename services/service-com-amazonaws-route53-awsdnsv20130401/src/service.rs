// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-route53-awsdnsv20130401` service: how it claims requests and how it words the
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
//!   before any handler ran. Route 53 states such a refusal under the
//!   `InvalidInput` code its model declares, except where the value refused is
//!   one the service RESOLVES rather than one the request has to have; see the
//!   inline note on that method.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! ## The error envelope
//!
//! `override_operation` below claims every request the model's routing table resolves
//! and runs it through [`envelope::ErrorEnvelope`], which dispatches the modelled
//! operation unchanged and then finishes the response's error envelope: Route 53's
//! `<ErrorResponse>` names the request it answers (`RequestId`), which is a property of
//! the envelope rather than a member of any error shape the model declares. That
//! module's docs carry the rule; routing stays with the generated table, which the seam
//! reads rather than replaces.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

pub mod envelope;

/// The request-schema check every request this service serves passes through
/// before it is read: an element whose accepted values the schema enumerates is
/// held to that set.
pub mod xml_schema;

/// The field a refusal of a tag operation's resource id is stated at: the id field of
/// the resource TYPE the request names, rather than the path the framework walked to
/// the offending value.
pub mod tag_resource_id;

/// Authoritative DNS responder serving this store's hosted zones over UDP and TCP inside the
/// local container sandbox when [`sandbox_dns::LISTEN_ADDRESS_VARIABLE`] is configured.
pub mod sandbox_dns;

use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_schema_server::{Violation, ViolationKind};

/// The wire error code Route 53 refuses an input it cannot use with — the
/// `InvalidInput` (HTTP 400) its model declares, which is the code a client
/// branches on for every input-level refusal this service answers.
const INVALID_INPUT: &str = "InvalidInput";

/// The wire error code Route 53 refuses a change batch with once the request
/// itself has been read — `InvalidChangeBatch` (HTTP 400), whose own
/// documentation is a LIST of "error messages. Each error message indicates one
/// error in the change batch".
const INVALID_CHANGE_BATCH: &str = "InvalidChangeBatch";

/// The validation path of a record set's `Region`, as the framework spells a
/// member path (lowerCamel, dotted, with `member` for a step out of a
/// collection): `changeBatch.changes.<n>.member.resourceRecordSet.region`.
const RECORD_SET_REGION_PATH: &str = "resourceRecordSet.region";

/// The order this service states one enum's value set in, one row per enum.
///
/// The value SET is the model's and the CHECK is the framework's; only the ORDER
/// the sentence lists the set in is chosen here, because no model records it:
/// Route 53 states a value set in neither the model's declaration order nor
/// ascending order, so the row below is READ OFF an answer the service gave for a
/// value outside that set rather than computed from the model.
///
/// A row is matched by its value SET rather than by the member the clause names
/// (see [`stated_value_order`]), because the order is a property of the enum
/// SHAPE: every member targeting that enum, in any operation, is stated alike. An
/// enum with no row — every other enum this model declares — keeps the order the
/// value set arrived in, the model's declaration order, which is the only order
/// there is evidence for.
const STATED_VALUE_ORDERS: &[&[&str]] = &[
    // com.amazonaws.route53#RRType, the DNS record type a record set, a query and
    // a traffic policy are all typed by.
    &[
        "AAAA", "A", "SVCB", "CAA", "NS", "SOA", "TLSA", "SPF", "HTTPS", "MX", "PTR", "CNAME",
        "DS", "TXT", "SSHFP", "SRV", "NAPTR",
    ],
];

/// `allowed` in the order this service states that enum's value set in, or in the
/// order it arrived in when there is no stated order for it.
fn stated_value_order(allowed: &[String]) -> Vec<String> {
    match STATED_VALUE_ORDERS
        .iter()
        .find(|stated| is_same_set(stated, allowed))
    {
        Some(stated) => stated.iter().map(|value| (*value).to_string()).collect(),
        None => allowed.to_vec(),
    }
}

/// Whether two value sets hold the same values. An enum's values are unique by
/// definition, so equal lengths plus containment is set equality.
fn is_same_set(stated: &[&str], allowed: &[String]) -> bool {
    stated.len() == allowed.len()
        && stated
            .iter()
            .all(|value| allowed.iter().any(|candidate| candidate == value))
}

/// Every violation as this service states it: the framework's clause, with an
/// enum's value set listed in the order this service publishes it
/// ([`stated_value_order`]).
fn as_stated(violations: &[Violation]) -> Vec<Violation> {
    violations
        .iter()
        .map(|violation| match &violation.kind {
            ViolationKind::EnumValue(allowed) => Violation {
                path: violation.path.clone(),
                kind: ViolationKind::EnumValue(stated_value_order(allowed)),
                value: violation.value.clone(),
                sensitive: violation.sensitive,
            },
            _ => violation.clone(),
        })
        .collect()
}

/// The regions a change batch was refused over, when THAT is what it was
/// refused over, in the order the batch named them.
///
/// `ResourceRecordSet$Region` is a modelled enum of Amazon EC2 Regions, so the
/// framework's constraint walk refuses a value outside that set before any
/// handler runs — but a region is something Route 53 RESOLVES while it
/// evaluates the batch, not a shape the request has to have: the region a
/// latency record set routes from is looked up, and a value that resolves to no
/// region fails that lookup rather than the request's shape. It is therefore
/// stated as a change-batch problem naming the region that could not be found,
/// and only when every refused value is one of those: a batch that also broke a
/// rule of its own shape is still a request fault.
fn unresolvable_regions(violations: &[Violation]) -> Option<Vec<String>> {
    let regions: Vec<String> = violations
        .iter()
        .filter(|violation| {
            matches!(violation.kind, ViolationKind::EnumValue(_))
                && violation.path.ends_with(RECORD_SET_REGION_PATH)
        })
        .filter_map(|violation| violation.value.clone())
        .collect();
    (!regions.is_empty() && regions.len() == violations.len()).then_some(regions)
}

/// The `com-amazonaws-route53-awsdnsv20130401` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: envelope::Route53Definition<Eng>,
    /// Finishes the error envelope of every response this service answers. Held here
    /// because [`Service::override_operation`] hands back a borrow of it.
    envelope: envelope::ErrorEnvelope,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        sandbox_dns::start_if_configured(&engine);
        Self {
            entry: envelope::Route53Definition::new(engine),
            envelope: envelope::ErrorEnvelope,
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
        // Every request the model's routing table resolves is served through the
        // envelope seam, which runs that same table entry and then finishes the
        // response's error envelope (see the module docs and `envelope`). A request the
        // table does NOT resolve is left alone, so naming no modelled operation is still
        // refused in front of the service — in the entry's own envelope, which
        // `envelope::Route53Definition` finishes the same way.
        self.entry
            .routes(req)
            .then_some(&self.envelope as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal — a member
        // the shape requires that the request left out, or a value outside a
        // constraint the model declares (an enum's value set, a `@length` bound) —
        // to let this service state it in its own terms.
        //
        // Route 53's model declares ONE client-input error for an input it cannot
        // use: `InvalidInput`, HTTP 400 ("The input is not valid"), which every
        // operation that can refuse an input lists
        // (`smithy_query(query="operation",operation="ListHostedZones")`). It
        // declares no shape of any other name for that class — there is no
        // `ValidationException` in this model at all
        // (`smithy_query(query="shape",shape="ValidationException")` finds none) —
        // so the framework's generic code names an error no Route 53 client can
        // catch. Every refusal the model's own rules produce therefore comes back
        // under `InvalidInput`, whatever operation and member it was about.
        //
        // The MESSAGE is the one the framework already renders and the one the
        // service answers: the per-member envelope `N validation error(s)
        // detected: Value '<value>' at '<member path>' failed to satisfy
        // constraint: <requirement>`, built from the request. TWO renderings
        // inside it are this service's rather than the framework's, because no
        // model records either — the order an enum's value set is listed in
        // ([`STATED_VALUE_ORDERS`]), and the field a tag operation's resource id
        // is named by ([`tag_resource_id`]); everything else about the envelope
        // is taken from the framework rather than restated here.
        //
        // The ONE exception is a value this service RESOLVES rather than one the
        // request has to have — see [`unresolvable_regions`].
        match refusal {
            Refusal::Invalid(violations) => {
                // A region that resolves to no Amazon EC2 Region is refused by
                // the region LOOKUP a change batch does, not by the request's
                // shape, so it is stated as the change-batch problem it is —
                // rendered as a list, in brackets, the way this service states
                // the problems of one batch.
                if let Some(regions) = unresolvable_regions(violations) {
                    let problems: Vec<String> = regions
                        .iter()
                        .map(|region| format!("Cannot find region '{region}'. No changes made."))
                        .collect();
                    return Some(Refused {
                        code: INVALID_CHANGE_BATCH.to_string(),
                        message: format!("[{}]", problems.join(", ")),
                    });
                }
                Some(Refused {
                    code: INVALID_INPUT.to_string(),
                    // `None` when there are no violations to state, which keeps
                    // the framework's own wording rather than answering an empty
                    // message. The clauses are the framework's; the ORDER an
                    // enum's value set is listed in is this service's
                    // ([`as_stated`]), as is the field a tag operation's resource
                    // id is named by ([`tag_resource_id::stated`]).
                    message: aws_smithy_schema_server::envelope(&tag_resource_id::stated(
                        &as_stated(violations),
                    ))?,
                })
            }
            // An unauthenticated caller is not an input problem at all: keep the
            // framework's 403 code and wording.
            Refusal::Unauthenticated => None,
            // A refusal class this crate does not know about keeps the
            // framework's own wording (`Refusal` is `#[non_exhaustive]`).
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
