// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. THIS
//!   SERVICE uses it for authorization: a request whose resource member names no
//!   automation rule of this service is answered `AccessDeniedException` (403)
//!   there — with the denial built from the request's own principal, action and
//!   resource string — because authorization is evaluated against the supplied
//!   resource string before the service parses it; see
//!   [`DenyUnauthorizedResource`]. The resource string is read, and the denial
//!   answered, through the protocol THAT request is served in, so a client of
//!   either protocol this service declares is answered the same decision in its
//!   own envelope.
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

use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, ServerHttpProtocol,
    Service,
};
use std::sync::Arc;

/// The error code and HTTP status a request naming a resource the caller is not
/// authorized for is answered with.
///
/// `AccessDeniedException` is a service-common error of every operation of this
/// service, `@error("client")` at `@httpError 403`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The operations whose input names a resource by ARN, and the one-member body
/// schema each one's resource string is read through
/// ([`RULE_ARN_REQUEST`] / [`RESOURCE_ARN_REQUEST`], which name the member).
///
/// These are exactly the `RuleArn`-typed input members of this service:
/// `ruleArn` on the three rule operations and `resourceArn` on the three tagging
/// ones (`smithy_query(query="operation",operation="...")`). The table is keyed
/// by OPERATION, not by member name alone, so a future member of another
/// operation that happens to be called `resourceArn` without being a `RuleArn`
/// is not judged by this gate — and an operation that takes no resource never
/// has its body read here.
const RESOURCE_MEMBERS: [(&str, &Schema); 6] = [
    ("GetAutomationRule", &RULE_ARN_REQUEST),
    ("UpdateAutomationRule", &RULE_ARN_REQUEST),
    ("DeleteAutomationRule", &RULE_ARN_REQUEST),
    ("TagResource", &RESOURCE_ARN_REQUEST),
    ("UntagResource", &RESOURCE_ARN_REQUEST),
    ("ListTagsForResource", &RESOURCE_ARN_REQUEST),
];

/// Answers one request `AccessDeniedException` (HTTP 403), with the IAM denial
/// message built from that request.
///
/// Authorization is evaluated against the resource string a request supplies
/// BEFORE the service parses that string, so a string that identifies no
/// resource of this service is refused as unauthorized rather than as a bad
/// parameter: no permission a caller can hold names a resource the request does
/// not name. The framework's pre-handler validation would otherwise reject the
/// `RuleArn` `@pattern` with a 400, which classifies a permissions failure as a
/// request bug.
///
/// The three things such a decision is taken from are all stated by the request,
/// so the refusal names all three — the principal the credential speaks for, the
/// IAM action the operation asks for and the resource string exactly as sent
/// (`crate::state::unauthorized_resource_message`). It holds the shared engine
/// because the principal is resolved from the store
/// `sts:GetCallerIdentity` answers from.
///
/// It is an [`ErasedOperation`] rather than a handler because that refusal has
/// to come BEFORE the modelled-constraint walk, and `override_operation` is the
/// seam that sees a whole request first. The router still authenticates the
/// request against the modelled operation's auth schemes before running this.
struct DenyUnauthorizedResource<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for DenyUnauthorizedResource<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        // The answer is computed in one call that reads, uses and drops the
        // service definition: `&dyn ErasedServiceDefinition` is not `Sync`, so
        // holding it across the await below would make this future non-`Send`.
        let response = self.refusal(svc, req, ctx);
        Box::pin(async move { response })
    }
}

impl<Eng: Send + Sync + 'static> DenyUnauthorizedResource<Eng> {
    /// The 403 for `req`, rendered through this service's own protocol — the
    /// same envelope (`__type` + `message`, and the same error-type header) a
    /// handler-raised `AccessDeniedException` goes out in, so a refusal taken
    /// here and one taken in a handler are indistinguishable on the wire.
    fn refusal(
        &self,
        svc: &dyn Service,
        req: &aws_smithy_runtime_api::http::Request,
        ctx: &OperationContext<'_>,
    ) -> aws_smithy_runtime_api::http::Response {
        let definition = svc.definition();
        // The protocol THIS request is served in: the refusal is read out of the
        // request and answered back in the one codec the request arrived in, so
        // a client of either of this service's protocols reads the denial in
        // its own envelope.
        let wire = definition.wire_protocol(req);
        let Some((operation, resource)) = unauthorized_resource(wire.protocol(), req) else {
            // Unreachable: this operation is reached only for a request whose
            // operation and resource member [`Service::override_operation`] has
            // already read out of these same bytes. A request whose resource
            // this cannot read names none, so the refusal can name none either,
            // and falls back to the service's resource-independent sentence.
            return definition.framework_error_response(
                aws_smithy_schema_server::ProtocolError::Unauthenticated {
                    code: ACCESS_DENIED_CODE,
                    message: crate::state::ACCESS_DENIED_MESSAGE,
                },
                req,
            );
        };
        let denial = crate::ops::types::AccessDeniedException {
            message: Some(crate::state::unauthorized_resource_message(
                &crate::state::caller_identity_arn(&*self.engine, ctx),
                &crate::state::iam_action(definition.identity().signing_name(), operation),
                &resource,
            )),
        };
        wire.protocol()
            .serialize_error(
                &denial,
                &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
                ACCESS_DENIED_CODE,
                ACCESS_DENIED_STATUS,
            )
            .unwrap_or_else(|error| definition.framework_error_response(error, req))
    }
}

/// The operation `req` names and the resource string it carries, when that
/// string names no automation rule of this service — the one read both the
/// dispatch gate and the refusal it answers with are taken from.
///
/// Resolves the operation first, so only the operations that take a resource ARN
/// ([`RESOURCE_MEMBERS`]) are judged at all, then reads that operation's
/// resource member out of the body ([`supplied_resource`]) and asks
/// [`crate::state::names_automation_rule`] whether it names an automation rule
/// of this service. A request this cannot read — an unknown operation, a
/// streamed body, a body that is not the serving protocol's object — is `None`
/// and is left to the modelled walk, which refuses it on its own terms.
///
/// `protocol` is the codec the request is SERVED in, so both reads are the ones
/// the rest of the request uses: a service that declares several protocols is
/// reached over each of them, and an authorization decision is taken from what a
/// request states, never from the encoding it stated it in.
fn unauthorized_resource(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<(&'static str, String)> {
    let routing_key = protocol.routing_key(req).ok()?;
    let (operation, request_schema) = RESOURCE_MEMBERS
        .iter()
        .find(|(candidate, _)| *candidate == routing_key)?;
    let supplied = supplied_resource(protocol, req, request_schema)?;
    (!crate::state::names_automation_rule(&supplied)).then_some((*operation, supplied))
}

/// The namespace the two read-only schemas below name themselves in. They stand
/// for the READ this gate performs, not for a shape of the model — the model's
/// own input shapes carry every other member of their operation, which this read
/// deliberately does not look at — so they are named after the gate.
const AUTHORIZATION_READ_NAMESPACE: &str = "emulator.authorization";

/// The `ruleArn` member of a request, typed as the string it is on the wire.
static RULE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "emulator.authorization#RuleArnRequest$ruleArn",
        AUTHORIZATION_READ_NAMESPACE,
        "ruleArn",
    ),
    ShapeType::String,
    "ruleArn",
    0,
);

/// The `resourceArn` member of a request, the name the tagging operations spell
/// the same `RuleArn` with.
static RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "emulator.authorization#ResourceArnRequest$resourceArn",
        AUTHORIZATION_READ_NAMESPACE,
        "resourceArn",
    ),
    ShapeType::String,
    "resourceArn",
    0,
);

/// The body schema a `ruleArn` request is read through: a structure declaring
/// THAT ONE member.
///
/// Authorization is taken BEFORE the request is parsed as its operation's input,
/// so the read that feeds it is not the modelled input shape: a codec skips every
/// member a schema does not declare, so this reads the resource string and
/// nothing else, and a body that also misses another required member, or carries
/// one outside its constraints, still names the resource the decision is about.
static RULE_ARN_REQUEST: Schema = Schema::new_struct(
    ShapeId::from_static(
        "emulator.authorization#RuleArnRequest",
        AUTHORIZATION_READ_NAMESPACE,
        "RuleArnRequest",
    ),
    ShapeType::Structure,
    &[&RULE_ARN],
);

/// The body schema a `resourceArn` request is read through; see
/// [`RULE_ARN_REQUEST`].
static RESOURCE_ARN_REQUEST: Schema = Schema::new_struct(
    ShapeId::from_static(
        "emulator.authorization#ResourceArnRequest",
        AUTHORIZATION_READ_NAMESPACE,
        "ResourceArnRequest",
    ),
    ShapeType::Structure,
    &[&RESOURCE_ARN],
);

/// The resource string `req` supplies, read through the codec the request is
/// SERVED in and against `request_schema`'s one member.
///
/// `None` when the body is not a structure in that codec, carries no such member,
/// or carries something other than a string there: then the request names no
/// resource this gate can judge, and the modelled walk answers it.
fn supplied_resource(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    request_schema: &Schema,
) -> Option<String> {
    let mut reader = protocol.deserialize_input(req, request_schema).ok()?;
    let mut supplied = None;
    reader
        .read_struct(request_schema, &mut |member, value| {
            supplied = Some(value.read_string(member)?);
            Ok(())
        })
        .ok()?;
    supplied
}

/// The `com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The 403 answer for a request naming a resource the caller cannot hold a
    /// permission for. Held here so [`Service::override_operation`] can hand out
    /// a reference to it; it carries the SAME shared engine the entry does, so
    /// the principal it names is read from the one store, not a second one.
    deny_unauthorized_resource: DenyUnauthorizedResource<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            deny_unauthorized_resource: DenyUnauthorizedResource { engine },
        }
    }

    /// Does `req` name a resource no permission of this service can cover?
    ///
    /// The read itself is [`unauthorized_resource`], the one the refusal is
    /// built from too, so the gate and the message it answers with cannot
    /// disagree about which request is refused.
    fn names_unauthorized_resource(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        unauthorized_resource(self.entry.wire_protocol(req).protocol(), req).is_some()
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
        // Authorization comes before parameter parsing: a request whose
        // resource member does not name an automation rule of this service at
        // all is refused `AccessDeniedException` (403) here, rather than falling
        // to the modelled `RuleArn` `@pattern` rejection (400) that the
        // framework's pre-handler walk would answer with. See
        // [`DenyUnauthorizedResource`]. Every other request resolves through the
        // generated routing table.
        self.names_unauthorized_resource(req)
            .then_some(&self.deny_unauthorized_resource as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound). This service's
        // model declares NO `ValidationException`: the one client-input error it
        // declares — on every operation, as a service-common error — is
        // `InvalidParameterValueException` (HTTP 400), "One or more parameter
        // values are not valid". So a value that failed a modeled rule is worded
        // with that code, which is what a client of this service branches on;
        // the framework's default `ValidationException` names a shape this
        // service never declares. The status stays 400, which the protocol owns.
        //
        // Both the code and the message are replaced together: the message is
        // this service's validation report (see
        // `crate::state::validation_report`) — how many members failed, then
        // one clause per member naming the member and the requirement it
        // failed, with no rejected value echoed back; except for a value
        // outside one of the service's enum PARAMETER TYPES, which that
        // reporter words as the type's own rejection (the type, the value as
        // sent, and the type's modelled values). Authentication refusals keep
        // the framework's wording (`None`).
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidParameterValueException".to_string(),
                message: crate::state::validation_report(violations),
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
