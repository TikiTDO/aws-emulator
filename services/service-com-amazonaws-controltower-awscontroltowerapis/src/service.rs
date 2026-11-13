// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-controltower-awscontroltowerapis` service: how it claims requests and how it words the
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
//!   correctly. This service serves the TAG-PLANE operations wrapped in the shared
//!   front door (see "The tag plane's ARN parameter" below); every other request
//!   resolves through the generated routing table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. This service answers a failed input validation with
//!   Control Tower's front-door bad-request refusal; see that method.
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
//! ## The tag plane's ARN parameter
//!
//! Control Tower's tag plane carries the resource it is about in the request LINE, as
//! the `resourceArn` path label of `/tags/{resourceArn}`, and refuses a value it cannot
//! read as an ARN with a sentence about THAT parameter rather than the generic one it
//! states for a request body it would not read ([`INVALID_RESOURCE_ARN_MESSAGE`] against
//! [`INVALID_REQUEST_BODY_MESSAGE`]).
//!
//! That choice cannot be made in [`Service::override_refusal`]: the centralized walk
//! hands that seam the violations ALONE — no operation, no request — so it cannot tell a
//! value the request carried in its body from one it carried in its request line.
//! [`service_common::front_door::FrontDoor`] is where the distinction lives: each
//! tag-plane operation is served wrapped in it, and the wrapper reads the HTTP BINDING of
//! each violated member off THAT operation's own input schema, then words the
//! request-line half through the wording attached to the door
//! ([`resource_arn_refusal`]).

use service_common::front_door::FrontDoor;
use service_common::validation::SubmittedEcho;
use smithy_server_runtime::{
    http, ErasedOperation, Operation, OperationRoutingTable, Refusal, Refused, Service, Violation,
};
use std::sync::Arc;

/// Whether IAM lets a request perform the action it names — the decision every operation
/// of this service whose model declares `AccessDeniedException` takes before it reads or
/// writes anything.
pub mod authorization;

/// The error code AWS Control Tower answers a request that failed input validation
/// with.
///
/// Every Control Tower operation is a restJson1 REST call, and a request whose
/// members do not satisfy the model's constraints is refused by the REST front door
/// before the modeled service error is reached: the answer is a generic bad-request
/// refusal, not the per-member `ValidationException` the operations declare. The
/// distinction is client-visible — an SDK unmarshals `ValidationException` into a
/// typed, modeled error and this code into a generic service error — so the code a
/// client branches on has to be this one. `ValidationException` remains what a
/// HANDLER returns when a request that satisfied every modeled constraint is still
/// not acceptable to the service; only the pre-handler refusal is reworded here.
const BAD_REQUEST_EXCEPTION: &str = service_common::front_door::BAD_REQUEST_EXCEPTION;

/// The message that front-door refusal carries.
///
/// It is constant: the front door refuses the request before it is dispatched to an
/// operation, so it names neither the member nor the constraint that failed. No model
/// trait or AWS document states this text, so it is this emulator's rendering of that
/// one refusal rather than a value a client may rely on — the status (400) and
/// [`BAD_REQUEST_EXCEPTION`] are the contract.
const INVALID_REQUEST_BODY_MESSAGE: &str = service_common::front_door::INVALID_REQUEST_BODY;

/// The path the framework's violation walk spells the tag plane's ARN parameter with: the
/// `resourceArn` `@httpLabel` of `/tags/{resourceArn}`, which `ListTagsForResource`,
/// `TagResource` and `UntagResource` all bind (`smithy_query(query="operation",
/// operation="ListTagsForResource,TagResource,UntagResource")`), lowerCamel as every
/// violation path is spelled.
const RESOURCE_ARN_PATH: &str = "resourceArn";

/// The message the front door states for a `resourceArn` it could not read as an ARN.
///
/// A parameter-specific sentence rather than [`INVALID_REQUEST_BODY_MESSAGE`]: the error
/// code alone does not tell a caller WHICH input was wrong, and the tag plane's ARN is the
/// one input these operations carry outside the body, so its refusal names that parameter
/// class. Fixed text — it echoes neither the value the caller sent nor the constraint that
/// value broke, exactly as the generic sentence names neither.
const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// How a tag-plane operation words a refusal of a value its request carried OUTSIDE the
/// body: the ARN sentence when the `resourceArn` path label is what failed.
///
/// `None` for a refusal of anything else the request line carries (`UntagResource`'s
/// `tagKeys` query parameter), which leaves that request to the layers behind the door —
/// the framework's own walk, worded through [`EmulatedService::override_refusal`] — so
/// only the ARN parameter takes the ARN sentence.
///
/// One sentence for all three operations, so a syntax refusal of the tag plane's ARN reads
/// the same whichever of them a caller addressed.
fn resource_arn_refusal(refused: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    // The path segment is read before the query string, so an ARN that cannot be read is
    // what the refusal states even when both halves of the request line broke a rule.
    refused
        .iter()
        .any(|(violation, _)| violation.path == RESOURCE_ARN_PATH)
        .then(|| Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: INVALID_RESOURCE_ARN_MESSAGE.to_string(),
        })
}

/// Serve `operation` behind this service's front door, under its own modelled name — the
/// routing key the protocol resolves and the generated table is keyed by, read off the
/// operation rather than spelled again here.
///
/// The door states no body wording of its own, so a request BODY that breaks a modelled
/// constraint is refused exactly as [`EmulatedService::override_refusal`] refuses it for
/// every other operation of this service; what the door adds is the request-line wording
/// ([`resource_arn_refusal`]), which only an operation-level layer can place.
fn serve<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    operation: O,
) {
    table.add_operation(
        O::NAME,
        Box::new(FrontDoor::new(operation).wording_uri(resource_arn_refusal))
            as Box<dyn ErasedOperation>,
    );
}

/// The `com-amazonaws-controltower-awscontroltowerapis` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The tag-plane operations the generated table also holds, each served behind the
    /// shared front door so a `resourceArn` the service cannot read as an ARN is refused as
    /// that parameter. Keyed by the routing key the protocol resolves — the key the
    /// generated table uses — and built from the SAME shared engine, so a wrapped operation
    /// reads and writes the one store. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `self`.
    tag_plane: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut tag_plane = OperationRoutingTable::new();
        // The three operations that bind `/tags/{resourceArn}`, and only those: an
        // operation that carries nothing outside its body has no request-line refusal to
        // word, so wrapping it would add a second read of every request for nothing.
        serve(
            &mut tag_plane,
            crate::ops::list_tags_for_resource::ListTagsForResource::new(Arc::clone(&engine)),
        );
        serve(
            &mut tag_plane,
            crate::ops::tag_resource::TagResource::new(Arc::clone(&engine)),
        );
        serve(
            &mut tag_plane,
            crate::ops::untag_resource::UntagResource::new(Arc::clone(&engine)),
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            tag_plane,
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and for a tag-plane key the operation it names is the one served — behind the
        // shared front door, so the ARN in the request line is refused as that parameter
        // (see the module docs). Every other key resolves to `None` here, exactly as it
        // does for a key the model does not name, so the generated table serves it and the
        // router's routing-error path is unchanged. The door declares the wrapped
        // operation's own auth schemes, so wrapping cannot change which schemes sign it.
        //
        // This service declares one protocol (restJson1), so the entry's request-independent
        // codec is the codec every request of it is served in.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.tag_plane.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Control Tower refuses such a request at its REST front door, so the
        // answer is the generic bad-request code and message above rather than the
        // framework's per-member `ValidationException` envelope: the front door has
        // not dispatched the request to an operation, so it states neither the
        // member nor the constraint. One decision for every operation and every
        // constraint kind — a request refused before a handler ran is refused the
        // same way whichever member failed.
        //
        // The status stays with the protocol (400), and both the code and the
        // message are replaced together: a message without its matching code is
        // worse than leaving both alone.
        //
        // An authentication refusal is NOT an input-validation failure, so it keeps
        // the framework's 403 wording; `Refusal` is `#[non_exhaustive]`, and a
        // refusal class this crate has not seen keeps the default wording too.
        //
        // The ONE refusal this service words per parameter — the tag plane's
        // `resourceArn` path label — cannot be worded here, because the violations
        // alone do not say where in the request a value travelled; it is worded at
        // the door those operations are served behind (see the module docs), which
        // answers before this seam is reached.
        match refusal {
            Refusal::Invalid(_) => Some(Refused {
                code: BAD_REQUEST_EXCEPTION.to_string(),
                message: INVALID_REQUEST_BODY_MESSAGE.to_string(),
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
