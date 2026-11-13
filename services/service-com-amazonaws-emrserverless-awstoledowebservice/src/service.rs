// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-emrserverless-awstoledowebservice` service: how it claims requests and how it words the
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
//!   before any handler ran. EDITED here: this service words two refusal classes
//!   itself — a resource ARN of another vendor, and a `@range` violation — and
//!   keeps the framework's wording for the rest. The two classes also carry
//!   DIFFERENT error-type identities, because two different layers refuse them:
//!   see `WRONG_VENDOR_ERROR_TYPE` and `invalid_refusal` below.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! A sixth seam, `override_protocol`, is one this service DOES take: the codec every
//!   modelled response is rendered through. It is the model's own `restJson1` in every
//!   respect but two:
//!
//!   * a read of an application or a session carries the whole effective-configuration
//!     projection the service's wire response carries, including the keys the model does
//!     not declare — see [`read_projection`] for that rule;
//!   * an answer whose status carries NO CONTENT carries no body, and states the length
//!     such an answer has. The two tag mutations answer `204` (see
//!     [`crate::state::TAG_MUTATION_SUCCESS_STATUS`]) and declare an output STRUCTURE with
//!     no members rather than `smithy.api#Unit`, which the protocol's serializer renders as
//!     the empty JSON object `{}` — a body a 204 has no room for. The correction itself is
//!     not written here: it is [`service_common::no_content_responses::without_content`],
//!     the one shared rule every service whose success answers 204 calls, and only the
//!     delegation below is this crate's. An answer served over HTTP/1.1 reaches a client
//!     WITHOUT the framing header, which is below anything this crate decides;
//!     `tests/all/tag_mutation_status.rs` pins what this service states and what a socket
//!     then delivers.
//!
//!   Both are applied here, at the one place every response of this service passes
//!   through, rather than per operation, because no handler can reach the rendering of its
//!   own output.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

mod read_projection;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::no_content_responses::without_content;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

use read_projection::ReadProjection;

/// The wire error code every input refusal this service words carries.
///
/// `ValidationException` is the client-input error the model declares on every
/// operation that takes one (`smithy_query(query="operation",…)` lists it with
/// `httpError` 400), and it is the code the framework's own default picks, so rewording
/// a refusal never moves the code a client branches on.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error-type identity the resource-ARN vendor check answers with.
///
/// The vendor check is a FRONT-DOOR check: a resource ARN of another vendor names a
/// resource this service's API cannot address at all, so the request is refused as a
/// bad request before it is dispatched to an operation — and the identity it carries is
/// the front door's generic one, not a shape this service's model declares. The
/// distinction is client-visible, because restJson1 resolves the error SHAPE from
/// `x-amzn-errortype` (and this body's `__type`): an SDK unmarshals a modelled
/// `ValidationException` into a typed error and this identity into a generic service
/// error, so a caller's `catch ValidationException` must NOT fire here.
///
/// RECORDED behaviour, not modelled: no operation declares this shape (every one
/// declares `ValidationException` for the values it refuses itself), and the recorded
/// answer to a tag request carrying an ARN of another vendor is this identity with the
/// vendor-naming message below, while the recorded answer to a member-constraint
/// refusal of this service's own surface (`maxResults` outside its `@range`) is the
/// modelled `ValidationException` that [`VALIDATION_EXCEPTION`] names. The two layers
/// keep their own identities here for that reason.
const WRONG_VENDOR_ERROR_TYPE: &str = "BadRequestException";

/// The modelled member that names the resource a tag operation targets.
///
/// `TagResource`, `UntagResource` and `ListTagsForResource` all bind `resourceArn` to
/// `/tags/{resourceArn}` and all target `ResourceArn`, the ARN of a resource of THIS
/// service's vendor (`smithy_query(query="operation",operation="UntagResource")`). It is
/// the one input member whose value this service reads as its own resource ARN.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The vendor (service namespace) segment of an ARN — the third of
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>` — or `None` for a value that
/// is not an ARN with a vendor and something after it.
fn arn_vendor(value: &str) -> Option<&str> {
    let mut segments = value.split(':');
    if segments.next()? != "arn" {
        return None;
    }
    let _partition = segments.next()?;
    let vendor = segments.next().filter(|vendor| !vendor.is_empty())?;
    // An ARN names a region, an account and a resource after its vendor; a value that
    // stops earlier carries no vendor this check can trust.
    segments.next()?;
    Some(vendor)
}

/// True when this violation is a resource ARN belonging to another vendor.
///
/// The resource-ARN check is a VENDOR check: the value is read as an ARN and its vendor
/// segment compared with this service's own. An ARN of another vendor is refused by that
/// check, whichever modelled constraint the value also happens to fail; an ARN of this
/// vendor that fails a constraint for some other reason is not this check's business and
/// keeps the framework's wording.
fn wrong_vendor_resource_arn(violation: &Violation) -> bool {
    if violation.path != RESOURCE_ARN_MEMBER {
        return false;
    }
    violation
        .value
        .as_deref()
        .and_then(arn_vendor)
        .is_some_and(|vendor| vendor != crate::state::ARN_NAMESPACE)
}

/// The clause this service states for one violated constraint.
///
/// A `@range` violation names the member and the bound and does NOT quote the submitted
/// value — the value is already known to the caller, and the constraint is what the
/// refusal is about. Every other class keeps the framework's clause, which is the one
/// this service's refusals are otherwise worded with.
fn clause(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::MinValue(min) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value greater than \
             or equal to {min}",
            violation.path
        ),
        ViolationKind::MaxValue(max) => format!(
            "Value at '{}' failed to satisfy constraint: Member must have value less than or \
             equal to {max}",
            violation.path
        ),
        _ => violation.message(),
    }
}

/// True for the violation classes this service words differently from the framework.
fn reworded(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)
    )
}

/// How this service states a refusal of the values a request carried, or `None` to keep
/// the framework's wording.
///
/// Two rules, in the order the service applies them:
///
/// 1. A resource ARN of another vendor is refused by the ARN vendor check, which names
///    the vendor that was expected and neither the caller's ARN nor the member's regex,
///    and carries the front door's [`WRONG_VENDOR_ERROR_TYPE`] identity rather than a
///    shape this service's model declares.
/// 2. Otherwise the refusal keeps the `N validation error(s) detected: …` envelope, with
///    a `@range` clause stating the member and the bound but not the submitted value.
fn invalid_refusal(violations: &[Violation]) -> Option<Refused> {
    if violations.iter().any(wrong_vendor_resource_arn) {
        return Some(Refused {
            code: WRONG_VENDOR_ERROR_TYPE.to_string(),
            message: format!(
                "Invalid input resource arn: expected vendor {}",
                crate::state::ARN_NAMESPACE
            ),
        });
    }
    if !violations.iter().any(reworded) {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    let message = match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message,
    })
}

/// The `com-amazonaws-emrserverless-awstoledowebservice` emulated service: the generated [`ServiceDefinition`] plus
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own protocol, with the read projection below applied to every
        // modelled response. Implemented on the service itself, so the protocol it
        // delegates to is the generated entry's own and there is no second routing table
        // to drift from the model.
        Some(self)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Both the code and the message are replaced together:
        // this service's client-input code is `ValidationException`, which is also
        // what the framework's default picks, so a client branches on the same code
        // either way.
        //
        // What is reworded, and why, is in [`invalid_refusal`]: the resource-ARN
        // vendor check, and the `@range` clause that names the member and the bound
        // without quoting the value. Everything else keeps the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => invalid_refusal(violations),
            // An unauthenticated caller is the framework's own 403; this service has
            // nothing of its own to say about it.
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
        error: &ProtocolError,
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

/// The protocol this service answers modelled requests in: the model's own, with the one
/// read projection [`read_projection`] states.
///
/// Implemented on the service itself because the model-derived protocol it delegates to
/// is owned by the generated entry and reached by reference (`self.entry.protocol()`),
/// and [`Service::override_protocol`] hands back a borrow of `&self` — so there is one
/// protocol instance, the generated one, and no second routing table to drift from the
/// model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        // The REQUEST side is the framework's own: the projection is about what a read
        // reports, and no input member of this model carries any of it.
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        // The status, the headers and every modelled member stay the framework's: only
        // the resource a read carries gains the rest of its effective-configuration
        // projection, from the record the handler already returned.
        let response = self.entry.protocol().serialize_output(
            &ReadProjection::new(output),
            output_schema,
            operation_name,
            success_status,
        )?;
        // An answer whose status carries no content carries no body: the empty JSON object
        // the serializer renders for a members-less output goes no further. The rule is the
        // shared one, not a copy of it.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        // No error shape of this model carries a configuration block, so a failure is
        // rendered exactly as the framework renders it. Nor is any of them a no-content
        // status — every modelled failure of this service carries a JSON error envelope at
        // a 4xx or 5xx — so there is nothing here to reframe either.
        self.entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names
/// the protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", self.entry.protocol().protocol_id())
            .finish()
    }
}
