// SPDX-License-Identifier: CC0-1.0

//! The stages every request to this service passes before the operation it names is
//! dispatched, in the order Amazon Connect's front door takes them.
//!
//! A request to Contact Lens is answered by three decisions before any handler reads stored
//! state, and which of them answers decides the code the caller sees:
//!
//! 1. **the instance** — the `InstanceId` the request names is resolved to an Amazon Connect
//!    instance ([`super::instance_id`]); one that is not an instance identifier resolves to
//!    nothing and the call is refused `UnauthorizedException` / "Unauthorized". This runs
//!    AHEAD of the body's own `@length` / `@pattern` clauses, which is why a whitespace-only
//!    `InstanceId` is an authorization-stage refusal rather than a complaint about that
//!    member. A request that names NO instance identifier at all has nothing to resolve and
//!    is left to stage 3, which states the absent `@required` member;
//! 2. **the caller** — IAM's decision about the action on the contact the request addresses
//!    ([`super::authorization`]); a denied caller is refused `AccessDeniedException` (HTTP
//!    403) and told nothing about the contact, so the ordering AWS guarantees — authorize,
//!    then look up — holds;
//! 3. **the body** — the modelled input constraints, refused as the modelled
//!    `InvalidRequestException` (HTTP 400) with the finding list AWS's body validator states
//!    ([`super::invalid_request`]).
//!
//! Everything a request clears is served by the generated operation, which owns
//! deserialize -> validate -> handle -> serialize, its own telemetry and every modelled
//! error; the session lookup and its `ResourceNotFoundException` are its handler's and stay
//! there. A request this layer refuses never reaches it, so it reads nothing and reports
//! nothing.
//!
//! The wrapper carries the operation TYPE — which names the action, pins the input schema the
//! body is judged against and resolves the delegate — and a handle on the ONE shared engine,
//! which is what the caller's identity and its policies are read through. Every request it
//! delegates goes to the one operation instance the generated routing table built from that
//! same engine, so there is no second instance of any operation and no second store.
//!
//! ## Why the shared front-door wrapper does not serve this door
//!
//! [`::service_common::front_door::FrontDoor`] wraps an operation for the services whose
//! body rejection is the OTHER dialect — one `BadRequestException` with a fixed sentence, or
//! a `BodyWording` over the violations — and it has no stage ahead of the body at all. Two
//! things this door needs are outside it: the two stages that answer BEFORE the body (an
//! identity-stage refusal on its own status, and a decision that needs the engine the shared
//! wrapper does not hold), and a body finding that depends on which members the request
//! CARRIED, which a `BodyWording` cannot see because it is handed the violations alone. The
//! shared module's own collecting walk IS reused, through [`super::invalid_request`], as are
//! every sentence of its message and the IAM decision behind stage 2.
//!
//! The MECHANICS below — a wrapper carrying the operation type and the engine, delegating
//! through the generated routing table — are the shape `Gated<Op, Eng>` in
//! `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/service/authorization.rs`
//! already has, and they carry no IAM or STS dependency. The right home for them is
//! `service-common`, as a wrapper taking its stages as a parameter, with that service and
//! this one migrated onto it; that is a change to appsync's crate and its suite as much as to
//! this one, so it is left to the change that owns both rather than widened into this fix.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Instant;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, ServerHttpProtocol, Service,
};

use super::{authorization, instance_id, invalid_request};

/// One modelled operation, served behind the three stages above.
pub(super) struct FrontDoor<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> FrontDoor<Op, Eng> {
    /// Wrap one operation, judged against the identities and policies held in `engine`.
    pub(super) fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

/// A refusal one stage answered with, and the wire error code it carries — which is what
/// classifies the row the refusal is recorded under.
struct Refusal {
    response: RtResponse,
    code: &'static str,
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for FrontDoor<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = Instant::now();
            // Scoped so the borrowed `&dyn ErasedServiceDefinition` — which is not `Sync`,
            // while this future is — is dropped before the await below. The protocol itself
            // is.
            let refused = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                self.refuse(protocol, req, ctx)
            };
            if let Some(refusal) = refused {
                // The row the framework sequence would have recorded for this request, so a
                // refused one is not a hole in the telemetry series.
                record_refusal::<Op>(ctx, started.elapsed(), &refusal);
                return refusal.response;
            }
            // Served by the generated operation, reached through the routing table rather
            // than through `override_operation`, which would route straight back here.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key the
                // generated table registered. Answer the framework's own no-such-operation
                // response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed as the MODEL says
        // for the operation it names, so authentication is decided by the modelled operation
        // this wrapper stands in front of.
        None
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> FrontDoor<Op, Eng> {
    /// The first stage that refuses this request, or `None` for one that reaches the
    /// delegate. The order is the module's: the instance, then the caller, then the body.
    fn refuse(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Option<Refusal> {
        // The ONE reading of the request's body in this layer: the two stages ahead of the
        // body each need a member of it, and a second reader could drift from this one.
        let members = body_members(req);
        // The stage is about an identifier the request NAMES: a request that names none at
        // all has nothing to resolve, and the member it left out is the body stage's own
        // refusal (`InstanceId` is `@required`), so such a request is passed through rather
        // than answered here.
        if let Some(named) = instance_id::named_instance(members.as_ref()) {
            if !instance_id::resolves_to_instance(named) {
                return Some(Refusal {
                    response: instance_id::unauthorized_response(),
                    code: instance_id::UNAUTHORIZED_CODE,
                });
            }
        }
        let resource = authorization::judged_resource(ctx, members.as_ref());
        if let Err(denial) = authorization::authorize(&*self.engine, ctx, Op::NAME, &resource) {
            return Some(Refusal {
                response: authorization::refusal_response(protocol, &denial),
                code: authorization::ACCESS_DENIED_CODE,
            });
        }
        Some(Refusal {
            response: invalid_request::modelled_refusal::<Op>(protocol, req, members.as_ref())?,
            code: invalid_request::INVALID_REQUEST_CODE,
        })
    }
}

/// The members the request's JSON body carries, or `None` for a request with no readable
/// body — one the codec cannot make an object of, which the framework's own walk refuses.
///
/// Read HERE and nowhere else in this layer: each stage is handed what this found, so no two
/// of them can disagree about what the request said and the body is read once per request
/// rather than once per stage.
pub(super) fn body_members(
    req: &RtRequest,
) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    match ::serde_json::from_slice(req.body().bytes()?) {
        Ok(::serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// The string member `member` of the request's body, when it carries one.
pub(super) fn named<'members>(
    members: Option<&'members ::serde_json::Map<String, ::serde_json::Value>>,
    member: &str,
) -> Option<&'members str> {
    members?.get(member)?.as_str()
}

/// Record the refusal answered above as the operation row the framework sequence would have
/// recorded, so a request refused by this layer is not a hole in the telemetry series: the
/// same classification (from the refusal's status and error code), the same folded handle
/// (this service's signing name and the operation's own name), and the same already-sanitized
/// request metadata. Everything is gated on a client collecting, exactly as the framework
/// gates it.
fn record_refusal<Op: Operation>(
    ctx: &OperationContext<'_>,
    latency: ::std::time::Duration,
    refusal: &Refusal,
) {
    if !::emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        ::emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    ::emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(::emulator_telemetry::OperationFailure::classify(
            refusal.response.status().as_u16(),
            refusal.code,
        )),
        ::smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers)
            .into_metadata(),
    );
}
