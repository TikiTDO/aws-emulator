// SPDX-License-Identifier: CC0-1.0

//! What this service does with a request BEFORE a handler runs, in the order it does it:
//! the IAM authorization decision, then the pre-dispatch contracts, then the modelled
//! operation the routing key names.
//!
//! The order is the one AWS keeps. A request is authorized at the IAM layer first
//! ([`super::authorization`]) — a caller that may not perform the action is refused
//! `AccessDeniedException` whatever else is wrong with the request, and nothing it asked
//! for is read or written — and only then does the service judge the request itself: the
//! tag contract, the add-on `resolveConflicts` contract and the precedence layer, each of
//! which answers a request its own way (see [`super::tag_contract`],
//! [`super::resolve_conflicts_contract`] and [`super::precedence`]). A request none of
//! them claims is served by the generated operation exactly as it always was.
//!
//! It is ONE [`ErasedOperation`] rather than four decisions in
//! [`smithy_server_runtime::Service::override_operation`], because that seam is handed no
//! request context: the authorization decision needs the credential store the context
//! carries ([`smithy_server_runtime::OperationContext::credential_store`]), which is
//! reachable only once the claimed operation RUNS. So the seam claims this layer for every
//! request the routing table serves and this layer takes the decision and then hands the
//! request on — so the layers below keep answering exactly what they answered before, for
//! every caller.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refused,
    ServerHttpProtocol, Service,
};

use crate::ops::types::{AccessDeniedException, SCHEMA_ACCESS_DENIED_EXCEPTION};

use super::{precedence, resolve_conflicts_contract, tag_contract};

/// The error code the model pins on `AccessDeniedException`, which is also its shape name.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The status the model pins on `AccessDeniedException`: `@httpError 403`
/// (`smithy_query(query="operation",operation="RegisterCluster",…)`).
const ACCESS_DENIED_STATUS: u16 = 403;

/// The pre-dispatch sequence of this service, holding the layers it runs in order.
///
/// Holds the shared engine, because the authorization decision is made from the IAM and STS
/// stores of that engine, and the three contract answerers, because
/// [`ErasedOperation::run`] hands the request to whichever of them claims it and each must
/// be a borrow that outlives the call.
pub struct RequestPipeline<Eng> {
    engine: Arc<Eng>,
    /// Answers a `TagResource` / `UntagResource` request the model's own tag contract
    /// refuses, in the error channel those two operations declare (see
    /// [`tag_contract`]).
    refused_tag_request: tag_contract::RefusedTagRequest,
    /// Answers a `CreateAddon` / `UpdateAddon` request whose `resolveConflicts` is outside
    /// the modelled enum, in this service's own sentence for that value (see
    /// [`resolve_conflicts_contract`]).
    refused_addon_request: resolve_conflicts_contract::RefusedAddonRequest,
    /// Answers a request whose body the framework's validating walk refuses but whose
    /// operation answers something else FIRST — the pass-role authorization of a role it
    /// hands Amazon EKS, a capability type this API does not define, or the cluster it is
    /// addressed to (see [`precedence`]).
    precedence_first: precedence::PrecedenceFirst<Eng>,
}

impl<Eng: Send + Sync + 'static> RequestPipeline<Eng> {
    /// Build the sequence from the shared engine — the same one every operation of this
    /// service is constructed from, never a store of its own.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            refused_tag_request: tag_contract::RefusedTagRequest::new(),
            refused_addon_request: resolve_conflicts_contract::RefusedAddonRequest::new(),
            precedence_first: precedence::PrecedenceFirst::new(Arc::clone(&engine)),
            engine,
        }
    }

    /// The authorization refusal this request earns, or `None` when IAM permits it (and for
    /// every operation and caller the gate does not decide — see
    /// [`super::authorization`]).
    fn authorization_refusal(
        &self,
        key: &str,
        body: &dyn ServerHttpProtocol,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
        definition: &dyn ErasedServiceDefinition,
    ) -> Option<RtResponse> {
        // A resource ARN names a resource of one Region, so it is built in the Region the
        // request is signed for — the same key every handler reads.
        let region = crate::state::request_region(ctx.parts());
        let resource = super::authorization::judged_resource(key, body, req, &region)?;
        let denied = super::authorization::authorize(&*self.engine, ctx, key, &resource).err()?;
        Some(access_denied_response(definition, req, &denied))
    }

    /// Which pre-dispatch contract claims this request, or `None` when none does and the
    /// modelled operation serves it.
    ///
    /// The ORDER is the one this sequence keeps, and it is stated here once: the two
    /// contracts that reword a refusal of a named operation, then the precedence layer.
    fn contract<'pipeline>(
        &'pipeline self,
        key: &str,
        body: &dyn ServerHttpProtocol,
        req: &RtRequest,
    ) -> Option<&'pipeline dyn ErasedOperation> {
        // A tag request the model's own tag contract refuses is claimed so that the
        // refusal is stated in the error code the CALLED operation declares (see
        // [`tag_contract`]).
        if tag_contract::refusal_for(key, body, req).is_some() {
            return Some(&self.refused_tag_request as &dyn ErasedOperation);
        }
        // An add-on request whose `resolveConflicts` is outside the modelled enum is
        // refused in a sentence that names the OPERATION it was sent to (see
        // [`resolve_conflicts_contract`]).
        if resolve_conflicts_contract::refusal_for(key, body, req).is_some() {
            return Some(&self.refused_addon_request as &dyn ErasedOperation);
        }
        // A request whose body the framework's validating walk would refuse, sent to an
        // operation that answers the pass-role authorization of a role it hands Amazon EKS,
        // a capability type this API does not define, or the cluster it is addressed to,
        // BEFORE it judges the body (see [`precedence`]).
        if precedence::has_a_question(key, body, req) {
            return Some(&self.precedence_first as &dyn ErasedOperation);
        }
        None
    }
}

impl<Eng: Send + Sync + 'static> std::fmt::Debug for RequestPipeline<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestPipeline").finish_non_exhaustive()
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for RequestPipeline<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service's model-derived half is read inside this block and not held past
            // it: `&dyn ErasedServiceDefinition` is not `Sync`, and the future returned
            // here is `Send`. Only the operation this layer hands the request to — which is
            // `Send + Sync` — crosses the await below.
            let served = {
                let definition = svc.definition();
                let protocol = definition.protocol();
                // The body is read with the codec dispatch reads one with (see
                // [`crate::service::body_codec`]).
                let body = super::body_codec(svc, definition);
                match protocol.routing_key(req).ok() {
                    // A request naming no operation of this service is answered the way one
                    // that names none is, rather than judged against an action nothing
                    // serves.
                    None => Ok(definition.routing_error_response(req)),
                    Some(key) => {
                        match self.authorization_refusal(key, body, req, ctx, definition) {
                            Some(response) => Ok(response),
                            None => match self
                                .contract(key, body, req)
                                .or_else(|| definition.routing_table().get(key))
                            {
                                Some(operation) => Err(operation),
                                None => Ok(definition.routing_error_response(req)),
                            },
                        }
                    }
                }
            };
            match served {
                Ok(response) => response,
                Err(operation) => operation.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so this layer
        // cannot weaken the schemes the model declares for it.
        None
    }
}

/// The authorization refusal as a response in this service's own protocol envelope,
/// serialized through the same protocol call the framework's error tail makes — the same
/// rendering [`precedence`] gives the pass-role denial, so a refusal worded here and one
/// worded there are indistinguishable on the wire.
///
/// FAIL CLOSED: where the modelled envelope cannot be rendered at all, the same code and
/// message are answered through the service's framework-refusal envelope instead, so a
/// denied request can never fall through to a handler.
fn access_denied_response(
    definition: &dyn ErasedServiceDefinition,
    req: &RtRequest,
    denied: &AccessDeniedException,
) -> RtResponse {
    let message = denied.message.clone().unwrap_or_default();
    definition
        .protocol()
        .serialize_error(
            denied,
            &SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .ok()
        .unwrap_or_else(|| {
            definition.refusal_response(
                ProtocolError::Invalid(Vec::new()),
                Some(Refused {
                    code: ACCESS_DENIED_EXCEPTION.to_string(),
                    message,
                }),
                req,
            )
        })
}
