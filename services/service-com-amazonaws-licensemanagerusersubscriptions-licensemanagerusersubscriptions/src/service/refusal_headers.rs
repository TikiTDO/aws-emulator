// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE a refusal of this service leaves under, rendered in one place: the status
//! the refusal carries, the error-type header that names its shape, and the `message` body.
//!
//! # The header set
//!
//! This service is `restJson1`, and on that protocol a client resolves a failure's error
//! SHAPE from the error-type header (`x-amzn-errortype`), which the protocol's error
//! serializer writes ONCE per response, naming the code the refusal carries. The body names
//! the same shape in its `__type` discriminator, so a client that reads the header and one
//! that reads the body agree about what was refused. That is the whole header contract of a
//! refusal here, and it is why every refusal this service answers — the front door's
//! authorization denial, the IAM denial, the tagging surface's own code, a modelled error, a
//! constraint violation — leaves with exactly one occurrence of it: a second occurrence
//! would fold, in any client that joins repeated header values, into a value naming no error
//! shape at all.
//!
//! # Why the rendering is here rather than in each gate
//!
//! The refusals this service answers above its handlers are decided in two places — the IAM
//! gate and the instance gate ([`super::AuthorizationGate`] and `super::InstanceGate`) — and
//! both have to answer the SAME envelope a handler's own error channel produces for the same
//! value ([`crate::scaffolded::Scaffolded::unauthorized`] and its siblings). Rendering it
//! once, through the service's own protocol, is what keeps the two from drifting apart.

use smithy_server_runtime::{Refused, Service};

/// Render `refusal` in this service's own protocol envelope, with the code, the sentence and
/// the STATUS the refusal carries.
///
/// The status is why this does not go through `ErasedServiceDefinition::refusal_response`:
/// that renderer states a pre-dispatch refusal, whose status the protocol fixes (400 for an
/// invalid request), and the fail-closed refusal is reported with its own
/// ([`crate::scaffolded::UNAUTHORIZED_EXCEPTION_STATUS`]). Serializing the refusal itself
/// through the protocol is also what makes this answer identical to the one a handler's
/// error channel produces for the same value: the same `__type`, the same one
/// `x-amzn-errortype` header and the same `message` body member.
///
/// A refusal that will not serialize still has to refuse: it falls back to the pre-dispatch
/// renderer, which carries the code and the sentence in this service's envelope even though
/// it cannot carry the status.
pub(super) fn refusal_response(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    refusal: &crate::scaffolded::ServiceRefusal,
) -> aws_smithy_runtime_api::http::Response {
    match svc.definition().protocol().serialize_error(
        refusal,
        &crate::scaffolded::SERVICE_REFUSAL_SCHEMA,
        refusal.code,
        refusal.status,
    ) {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                code = refusal.code,
                %error,
                "the fail-closed refusal would not serialize; it is answered through the \
                 pre-dispatch renderer instead"
            );
            svc.definition().refusal_response(
                aws_smithy_schema_server::ProtocolError::Invalid(Vec::new()),
                Some(Refused {
                    code: refusal.code.to_string(),
                    message: refusal.message.clone(),
                }),
                req,
            )
        }
    }
}
