// SPDX-License-Identifier: CC0-1.0

//! @op InvokeEndpointWithResponseStream
//!
//! HAND-AUTHORED. The scaffold op-stub generator declined this operation (see
//! `src/ops/SKIPPED_STUBS.md`): its output binds a `@streaming` event-stream union
//! (`ResponseStream`), which the generator does not type and whose generated
//! `ErasedOperation::run` answers the framework's "modeled but not served" signal. So this
//! file carries TWO types, and the split is the whole reason it looks unusual:
//!
//! * [`InvokeEndpointWithResponseStream`] is the struct the GENERATED routing table holds.
//!   Its `ErasedOperation` impl is emitted by `#[smithy_service]`, declines, and is never
//!   reached — it exists so the model's route (`POST
//!   /endpoints/{EndpointName}/invocations-response-stream`) is in the service's route
//!   table and so the operation's modelled auth schemes are the ones the router verifies.
//! * [`ResponseStreamInvocation`] is the real implementation. It implements the shared
//!   `service_common::event_stream::EventStreamOperation` trait and `service.rs` holds one
//!   inside that module's `EventStreamRoute` adapter, returned from
//!   `Service::override_operation` — the documented seam for "serve a request the model's
//!   routing key cannot name correctly". Reading the request, checking it against the
//!   model and rendering a modelled error are that module's helpers, not local copies.
//!
//! It decides the CALLER's authorization first — the identity-based IAM policies in force
//! on the principal the request signed as, for
//! `sagemaker:InvokeEndpointWithResponseStream` on the endpoint it addressed
//! ([`crate::state::authorization`]), refused 403 `AccessDeniedException` before the
//! endpoint is resolved — then resolves the endpoint through the one resolver all three
//! operations of this service
//! share ([`crate::state::resolve_endpoint`]) and applies the same rules `InvokeEndpoint`
//! does — the account must host the endpoint, a deployment must be behind it, a
//! `TargetVariant` must name a variant it hosts, and a `SessionId` must name a stateful
//! session, and only then is the payload examined against the maximum the model declares
//! for it ([`crate::service::error_envelope`] states that order, and renders this route's
//! refusals in the same envelope the other two answer in) — then answers the modelled 424
//! `ModelError`, because a SageMaker model is
//! customer-provided code in a container image and this emulator runs none. Every call is
//! logged in [`crate::state`] beside the real-time invocations.
//!
//! ## Why no event-stream body is framed
//!
//! A `ResponseStream` carries `PayloadPart` events, whose `Bytes` member is the model's
//! own output. Nothing computed any output here, so there is no part to frame and no
//! stream to break: framing an empty or invented `PayloadPart` would claim an inference
//! that never ran, and the terminating `ModelStreamError` / `InternalStreamFailure` events
//! would claim a stream that started. The modelled pre-stream error is the honest answer,
//! and it is the same one `InvokeEndpoint` gives for the same reason. The two event shapes
//! stay declared below, as the model declares them, for the day a container can run.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.

#[allow(unused_imports)]
use super::types::*;

/// The SageMaker CONTROL PLANE, whose store owns every endpoint this data plane invokes.
use ::service_com_amazonaws_sagemaker_sagemaker as sagemaker;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::event_stream::{self, EventStreamOperation};
use smithy_server_runtime::{BoxFuture, OperationContext, Service};

smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json");

/// This operation's MODELED errors, declared here because the generator emitted no module
/// for the declined operation. The `#[operation_error]` macro accepts only variants the
/// model declares, and it is what gives each one its wire shape name and HTTP status, so
/// the shared renderer answers them exactly as it answers a generated operation's.
#[smithy_macros::operation_error(
    "com.amazonaws.sagemakerruntime#InvokeEndpointWithResponseStream"
)]
#[derive(Debug)]
pub enum InvokeEndpointWithResponseStreamErrorModeled {
    InternalFailure(InternalFailure),
    InternalStreamFailure(InternalStreamFailure),
    ModelError(ModelError),
    ModelStreamError(ModelStreamError),
    ServiceUnavailable(ServiceUnavailable),
    ValidationError(ValidationError),
}

/// The request this operation reads, declared here because the generator emitted no types
/// module entry for the declined operation. Deserialization and constraint validation then
/// run through the framework's own codec, exactly as a generated operation's do.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntime#InvokeEndpointWithResponseStreamInput")]
#[derive(Debug, Clone, Default)]
pub struct InvokeEndpointWithResponseStreamInput {
    pub endpoint_name: String,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub accept: Option<String>,
    pub custom_attributes: Option<String>,
    pub target_variant: Option<String>,
    pub target_container_hostname: Option<String>,
    pub inference_id: Option<String>,
    pub inference_component_name: Option<String>,
    pub session_id: Option<String>,
    pub prefix_aware_id: Option<String>,
}

/// The `ResponseStream` union's `ModelStreamError` event, declared here because the
/// generator emitted no types-module entry for the declined operation. No frame carries
/// one today: a stream that never starts has no event to carry it (see this module's
/// doc), but the operation MODELS it, so it is one of the error variants below.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntime#ModelStreamError")]
#[derive(Debug, Clone, Default)]
pub struct ModelStreamError {
    pub message: Option<String>,
    pub error_code: Option<String>,
}

/// The `ResponseStream` union's `InternalStreamFailure` event, declared here for the same
/// reason as [`ModelStreamError`].
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakerruntime#InternalStreamFailure")]
#[derive(Debug, Clone, Default)]
pub struct InternalStreamFailure {
    pub message: Option<String>,
}

/// The generated routing-table entry for `InvokeEndpointWithResponseStream`.
///
/// Carries the engine so the generated `{Op}::new(engine.clone())` construction compiles;
/// its generated `run` declines (the generator does not serve a `@streaming` output), and
/// `service.rs` routes the request to [`ResponseStreamInvocation`] before the table is
/// consulted.
pub struct InvokeEndpointWithResponseStream<Eng> {
    #[allow(dead_code)]
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeEndpointWithResponseStream<Eng> {
    /// Construct the operation from the shared engine, as the generated
    /// `ServiceDefinition::new` does for every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The hand-authored `InvokeEndpointWithResponseStream` implementation.
pub struct ResponseStreamInvocation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ResponseStreamInvocation<Eng> {
    /// Construct from the shared engine — the same constructor seam every operation uses.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> ResponseStreamInvocation<Eng> {
    /// Whether `req` is the request this operation serves: the model's method and URI
    /// shape for `InvokeEndpointWithResponseStream`
    /// (`POST /endpoints/{EndpointName}/invocations-response-stream`). Matched on the
    /// whole path, segment by segment — not on a suffix — so a path the model does not
    /// publish is left to the framework's own routing failure. Read by `service.rs`'s
    /// `override_operation`.
    pub fn claims(req: &RtRequest) -> bool {
        if !req.method().eq_ignore_ascii_case("POST") {
            return false;
        }
        // `override_operation` sees the URI as a string that may be absolute, which is the
        // form an AWS SDK sends; `service_common::uri_path` is the ONE reader of that in
        // this workspace, and `nth_path_segment` compares segments as written (an empty
        // one is `Some("")`, so it cannot disappear).
        let path = ::service_common::uri_path(req.uri());
        let segment = |index: usize| ::service_common::nth_path_segment(path, index);
        segment(0) == Some("endpoints")
            && segment(1).is_some_and(|name| !name.is_empty())
            && segment(2) == Some("invocations-response-stream")
            && segment(3).is_none()
    }

    /// Resolve and answer. Reading the request into the modelled input shape and checking
    /// it against the model is the shared event-stream path
    /// (`service_common::event_stream::deserialize_request`), so a hand-authored operation
    /// reads a request exactly as a generated one does.
    fn serve(&self, svc: &dyn Service, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let protocol = svc.definition().protocol();

        let input: InvokeEndpointWithResponseStreamInput =
            match crate::service::error_envelope::read_addressed_request(
                protocol,
                req,
                INVOKE_ENDPOINT_WITH_RESPONSE_STREAM_INPUT_SCHEMA,
            ) {
                Ok(input) => input,
                // A request the model refuses is answered with the modelled client error
                // carrying the refusal sentence the shared reader produced. A payload
                // outside its modelled bound is NOT refused here: this service resolves
                // the endpoint a request addressed before it examines the payload, which
                // is checked below.
                Err(message) => return self.validation(svc, message),
            };

        // AWS evaluates the caller's authorization at the front door, before the endpoint
        // the request addresses is resolved — the same decision the other two routes take,
        // under this route's own action name (`crate::state::authorization`). A refused
        // call is answered 403 `AccessDeniedException` and leaves no invocation record.
        if let Some(denial) = crate::state::authorization::refusal(
            &*self.engine,
            ctx,
            "InvokeEndpointWithResponseStream",
            &input.endpoint_name,
        ) {
            return self.refusal(
                svc,
                &crate::scaffolded::Scaffolded::<
                    InvokeEndpointWithResponseStreamErrorModeled,
                >::access_denied(denial),
            );
        }

        let Some(log) = crate::state::service_state(&*self.engine) else {
            return self.internal_failure(svc, crate::state::NO_STATE.to_string());
        };
        let now = ::std::time::SystemTime::now();
        // Everything the clock already owes this service's own store is materialised
        // before this request extends it, as the other two routes do.
        log.lock().settle_async_invocations(now);
        // "Endpoints are scoped to an individual account" (`InvokeEndpoint`'s model doc),
        // and this emulator serves the one account every emulated resource belongs to.
        let account = ::engine::EMULATOR_ACCOUNT_ID;

        // This call, as the data-plane log replays it once its outcome is known — the same
        // record `InvokeEndpoint` writes, so a streamed invocation is observable in state
        // the same way. The `@sensitive` body and custom attributes are not stored; only
        // the body's SIZE is.
        let record = |outcome: crate::state::InvocationOutcome| {
            let invocation = crate::state::EndpointInvocation {
                endpoint_name: input.endpoint_name.clone(),
                target_variant: input.target_variant.clone(),
                request_bytes: input.body.len(),
                content_type: input.content_type.clone(),
                accept: input.accept.clone(),
                inference_id: input.inference_id.clone(),
                // This operation's input binds no `TargetModel`.
                target_model: None,
                session_id: input.session_id.clone(),
                outcome,
                at: now,
            };
            ::tracing::info!(
                operation = "InvokeEndpointWithResponseStream",
                endpoint = %invocation.endpoint_name,
                target_variant = invocation.target_variant.as_deref().unwrap_or_default(),
                request_bytes = invocation.request_bytes,
                outcome = outcome.label(),
                access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
                "sagemaker runtime streamed endpoint invocation"
            );
            log.lock().record_invocation(invocation);
        };

        // The endpoint is the control plane's resource, resolved through the one resolver
        // all three operations of this service share, so none of them can disagree about
        // which endpoints exist or serve traffic.
        let facts = match crate::state::resolve_endpoint(&*self.engine, &input.endpoint_name, now) {
            crate::state::EndpointLookup::NoControlPlane => {
                return self
                    .internal_failure(svc, crate::state::NO_CONTROL_PLANE_STATE.to_string())
            }
            crate::state::EndpointLookup::NotFound => {
                // The one refusal the three routes state identically, discriminator and
                // all (`crate::scaffolded::Scaffolded::no_such_endpoint`).
                record(crate::state::InvocationOutcome::EndpointNotFound);
                return self.refusal(
                    svc,
                    &crate::scaffolded::Scaffolded::<
                        InvokeEndpointWithResponseStreamErrorModeled,
                    >::no_such_endpoint(&input.endpoint_name),
                );
            }
            crate::state::EndpointLookup::Found(facts) => facts,
        };
        if facts.settled {
            // The transition is a state change of the control plane's resource, so it
            // leaves the same record there as the call that created the endpoint.
            sagemaker::state::record_state_change(
                "InvokeEndpointWithResponseStream",
                &facts.arn,
                ctx,
            );
        }
        // The same serving rule the other two operations apply, stated once in
        // `crate::state::NON_SERVING_ENDPOINT_STATUSES`. This operation models no
        // not-ready error, so an endpoint with no deployment behind it is a 400.
        if crate::state::NON_SERVING_ENDPOINT_STATUSES.contains(&facts.status.as_str()) {
            record(crate::state::InvocationOutcome::EndpointNotInService);
            return self.validation(
                svc,
                format!(
                    "Endpoint {} of account {account} is not in service: its status is {}.",
                    input.endpoint_name, facts.status
                ),
            );
        }
        // `TargetVariant` has to name one of the variants this endpoint hosts, and a
        // `SessionId` has to name a live stateful session — both asked once, beside
        // `resolve_endpoint`, so this route and `InvokeEndpoint` cannot drift. This
        // operation's `SessionId` models no `NEW_SESSION` constant at all, which is what
        // `NotModelled` says.
        if let Some((outcome, message)) = crate::state::check_target_variant(
            &facts,
            &input.endpoint_name,
            input.target_variant.as_deref(),
        ) {
            record(outcome);
            return self.validation(svc, message);
        }
        if let Some((outcome, message)) = crate::state::check_session_id(
            &input.endpoint_name,
            input.session_id.as_deref(),
            crate::state::NewSessionConstant::NotModelled,
        ) {
            record(outcome);
            return self.validation(svc, message);
        }

        // The payload is examined HERE, after everything the request's addressing
        // resolved — the same order the other two routes answer in, stated once in
        // `crate::service::error_envelope::payload_over_maximum`.
        if let Some(message) = crate::service::error_envelope::payload_over_maximum(
            INVOKE_ENDPOINT_WITH_RESPONSE_STREAM_INPUT_SCHEMA,
            input.body.len(),
        ) {
            record(crate::state::InvocationOutcome::PayloadOverMaximum);
            return self.validation(svc, message);
        }

        // Everything the control plane owns resolved; what is left is the inference
        // itself, and no container ran it — so there is no `PayloadPart` to frame and no
        // stream to break. The answer is the modelled `ModelError` (424), the same one
        // `InvokeEndpoint` gives for the same reason, rather than an event stream
        // delivering bytes nothing produced.
        record(crate::state::InvocationOutcome::NoInferenceContainer);
        self.model_error(
            svc,
            format!(
                "This emulator runs no model inference container, so endpoint {} returned no \
                 inference for this request.",
                input.endpoint_name
            ),
        )
    }

    /// The modelled `ValidationError` (HTTP 400), in this service's envelope.
    fn validation(&self, svc: &dyn Service, message: String) -> RtResponse {
        self.render(
            svc,
            InvokeEndpointWithResponseStreamErrorModeled::ValidationError(ValidationError {
                message: Some(message),
            }),
        )
    }

    /// The modelled `ModelError` (HTTP 424), for an invocation no container answered.
    ///
    /// Nothing ran, so there is no container status code, no container message and no
    /// container log stream to report beside the service-level message.
    fn model_error(&self, svc: &dyn Service, message: String) -> RtResponse {
        self.render(
            svc,
            InvokeEndpointWithResponseStreamErrorModeled::ModelError(ModelError {
                message: Some(message),
                original_status_code: None,
                original_message: None,
                log_stream_arn: None,
            }),
        )
    }

    /// The modelled `InternalFailure` (HTTP 500).
    fn internal_failure(&self, svc: &dyn Service, message: String) -> RtResponse {
        self.render(
            svc,
            InvokeEndpointWithResponseStreamErrorModeled::InternalFailure(InternalFailure {
                message: Some(message),
            }),
        )
    }

    /// Render one of this operation's modelled errors in the service's own envelope, with
    /// the status the model gives it — through the shared event-stream helper, so a
    /// hand-authored operation answers exactly as a generated one does.
    fn render(
        &self,
        svc: &dyn Service,
        error: InvokeEndpointWithResponseStreamErrorModeled,
    ) -> RtResponse {
        self.refusal(svc, &crate::scaffolded::Scaffolded::Modeled(error))
    }

    /// Render anything on this operation's error channel, then name the error shape the
    /// way this service names it (`crate::service::error_envelope`), so this route's
    /// refusals read exactly as the other two routes' do.
    fn refusal(
        &self,
        svc: &dyn Service,
        error: &crate::scaffolded::Scaffolded<InvokeEndpointWithResponseStreamErrorModeled>,
    ) -> RtResponse {
        crate::service::error_envelope::with_error_type_locator(event_stream::error_response(
            svc.definition().protocol(),
            error,
        ))
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> EventStreamOperation
    for ResponseStreamInvocation<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        ::std::boxed::Box::pin(async move { ResponseStreamInvocation::serve(self, svc, req, ctx) })
    }
}
