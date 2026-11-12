// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudtraildata-cloudtraildataservice` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. The routing DECISION is still the model's: this
//!   service serves the operation the generated table names, through the wrapper in
//!   [`validation`] that states a pre-handler refusal the way this service states one,
//!   which the `override_refusal` seam cannot do because it never sees the request.
//! - `override_refusal` — EDITED HERE. A refusal ABOUT a channel reference is answered
//!   under the error this service's model declares for that fault; every other
//!   pre-handler refusal is stated by the wrapper in [`validation`], which serves this
//!   service's operation and can read the request the violations were found in. See the
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
//! Beside those seams, [`authorization`] holds the decision every operation of this
//! service takes before it reads anything: whether the calling principal's identity-based
//! IAM policies allow the action it asks for.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

pub mod authorization;
mod validation;

use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

use crate::ops::put_audit_events::{
    CHANNEL_ARN_PATTERN, INVALID_CHANNEL_ARN_CODE, INVALID_CHANNEL_ARN_MESSAGE,
};

/// Whether ONE violation is about a channel reference: a `@pattern` violation of the
/// expression every `ChannelArn` member is constrained with ([`CHANNEL_ARN_PATTERN`]).
///
/// Keyed to that expression rather than to a member path, so the rule holds for whichever
/// member of whichever operation carried the value.
pub(crate) fn refuses_channel_arn(violation: &Violation) -> bool {
    matches!(&violation.kind, ViolationKind::Pattern(pattern) if pattern == CHANNEL_ARN_PATTERN)
}

/// Whether a boundary refusal is ABOUT a channel reference: one of its violations is
/// [`refuses_channel_arn`].
///
/// A refusal that ALSO carries other violations is still answered as the channel-ARN
/// fault: the channel reference is what this service resolves the call against before it
/// reads anything else, so that is the fault it reports.
fn refused_channel_arn(violations: &[Violation]) -> bool {
    violations.iter().any(refuses_channel_arn)
}

/// The `com-amazonaws-cloudtraildata-cloudtraildataservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped so a refusal
    /// found before the handler runs is stated the way this service states one (see
    /// [`validation`]). Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `self`.
    stating_submitted_values: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut stating_submitted_values = OperationRoutingTable::new();
        stating_submitted_values.add_operation(
            "PutAuditEvents",
            Box::new(validation::StatingSubmittedValues::new(
                crate::ops::put_audit_events::PutAuditEvents::new(Arc::clone(&engine)),
            )) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stating_submitted_values,
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
        // Serve the operation the generated routing table names, through the wrapper that
        // states a pre-handler refusal the way this service states one (see
        // [`validation`]). The routing DECISION is not overridden: the key is resolved by
        // the model's own protocol, and a request the table cannot serve is declined here
        // so the framework's routing-error path stays exactly as it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        self.stating_submitted_values.get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // A channel reference is validated by the SERVICE, not by the generic constraint
        // walk: this service models an error for exactly that fault
        // (`InvalidChannelARN`, HTTP 400), and it is one of the errors every operation
        // that takes a channel reference declares — so a value the member's modelled
        // `@pattern` rejects is answered under that code and its documented sentence,
        // the same answer the handler gives a value that passes the pattern but names no
        // channel. `ValidationException` is not modelled anywhere in this service, so a
        // generated client has no handler for it at all.
        //
        // Every OTHER constraint refusal of this service is stated by the wrapper in
        // [`validation`], which is what serves this service's one operation: it re-reads
        // the request, so the clause about a COLLECTION can quote the collection the
        // caller sent — which is what AWS states and what no renderer reachable from here
        // could reproduce (this seam is handed the violations alone). Answering a second,
        // valueless rendering of that same clause here would make one service state one
        // class of clause two ways, so this path states nothing of its own about it and
        // keeps the framework's wording for a refusal that somehow arrives without
        // passing the wrapper.
        match refusal {
            Refusal::Invalid(violations) if refused_channel_arn(violations) => Some(Refused {
                code: INVALID_CHANNEL_ARN_CODE.to_string(),
                message: INVALID_CHANNEL_ARN_MESSAGE.to_string(),
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
