// SPDX-License-Identifier: CC0-1.0

//! @op OptimizePrompt
//!
//! `POST /optimize-prompt -> 200`. Serves one prompt-optimization request as a
//! `vnd.amazon.eventstream` body: the `analyzePromptEvent` the service reports its analysis
//! of the submitted prompt in, then the `optimizedPromptEvent` carrying the prompt the
//! optimization answers with — the two events the modelled `OptimizedPromptStream` union
//! declares for a run that completes, in that order.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. The
//! optimized prompt is the notice the Bedrock Runtime plane of this same emulator answers
//! every inference surface with (`EMULATED_COMPLETION_FLOOR`, the one place that sentence is
//! worded), extended with the facts of the request that was actually served: which model it
//! was optimized for, and how long the submitted prompt was. It is never the submitted
//! prompt handed back as if it had been rewritten, and never text synthesised to look like
//! an optimization — a client reading the answer can always tell that no model ran.
//!
//! Everything AROUND that text is real and computed from the request:
//!
//! * `targetModelId` is resolved against the SAME base-model catalogue the Bedrock Runtime
//!   data plane serves inference from (`crate::state::target_model`), so a model this
//!   emulator publishes is optimized for and an identifier naming no such model is refused
//!   with the modelled `ValidationException` (HTTP 400) — the only client-fault error this
//!   operation declares for a model reference it cannot resolve;
//! * the caller's permission for `bedrock:OptimizePrompt` on that model is decided before
//!   anything else is read, the way every other operation of this service decides it.
//!
//! This operation is hand-authored rather than generated: its output is a `@streaming`
//! union, which the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and whose
//! generated `ErasedOperation` shim would decline the request, so there was no route for
//! `POST /optimize-prompt` at all. The framing and the routing seam live in
//! `service_common::event_stream`, the request half of its shapes in
//! `super::types::optimize_prompt_types`, and `crate::service` routes a request here
//! through `Service::override_operation`.
//!
//! ## Input
//!
//! * `input` (InputPrompt) **required** — the prompt to optimize; its `textPrompt.text` is
//!   `@length(1, 200000)`.
//! * `targetModelId` (String, len 1..2048, pattern naming a foundation model, a custom
//!   model, a provisioned model or a bare model id) **required**.
//!
//! ## Output
//!
//! * `optimizedPrompt` (OptimizedPromptStream, `@httpPayload`, `@streaming`) — the body;
//!   the response binds no modelled header.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — the input fails the model's constraints, or
//!   `targetModelId` names no model this emulator serves.
//! * `AccessDeniedException` (403), `BadGatewayException` (502),
//!   `DependencyFailedException` (424), `InternalServerException` (500),
//!   `ThrottlingException` (429).

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_only_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, ServerHttpProtocol, Service};

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it. No `Scaffolded` wrapper: this operation is implemented, so the 501
/// channel has nothing to carry.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#OptimizePrompt")]
#[derive(Debug)]
pub enum OptimizePromptError {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's modelled 400 `ValidationException`.
fn validation(message: impl Into<String>) -> OptimizePromptError {
    OptimizePromptError::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

/// This operation's modelled 500 `InternalServerException`.
fn internal(message: impl Into<String>) -> OptimizePromptError {
    OptimizePromptError::InternalServerException(InternalServerException {
        message: Some(message.into()),
        reason: None,
    })
}

/// The `OptimizedPromptStream` member names the two events a completed optimization
/// carries, which the frames below travel under as their `:event-type`
/// (`smithy_query(query="shape",shape="OptimizedPromptStream")`).
const ANALYZE_EVENT: &str = "analyzePromptEvent";

/// See [`ANALYZE_EVENT`].
const OPTIMIZED_EVENT: &str = "optimizedPromptEvent";

/// The `OptimizePrompt` operation.
///
/// Owns the shared engine (the constructor seam). Held by `crate::service` behind an
/// `EventStreamRoute`, because the generated routing table cannot serve a
/// `@streaming`-output operation.
pub struct OptimizePrompt<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> OptimizePrompt<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The literal single segment of this operation's modelled URI, `POST /optimize-prompt`.
const OPTIMIZE_PROMPT_SEGMENT: &str = "optimize-prompt";

/// Whether `req` addresses this operation's modelled route: `POST` on that one literal
/// segment.
///
/// The seam that routes it ([`crate::service::EmulatedService::override_operation`]) asks
/// this, so the path this operation recognises is stated once, beside the operation.
pub fn addresses(req: &Request) -> bool {
    if req.method() != "POST" {
        return false;
    }
    let path = service_common::uri_path(req.uri());
    let mut segments = path.trim_start_matches('/').split('/');
    matches!(
        (segments.next(), segments.next()),
        (Some(OPTIMIZE_PROMPT_SEGMENT), None)
    )
}

impl<Eng: Send + Sync + 'static> OptimizePrompt<Eng> {
    /// The whole request once it has been read: authorize it, resolve the model it is
    /// optimized for, and frame the two events the stream carries.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        input: &OptimizePromptInput,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, OptimizePromptError> {
        // Authorization is decided against the model the prompt is optimized FOR — the
        // resource an identity-based policy for `bedrock:OptimizePrompt` names — and before
        // the catalogue is read, so an unauthorized call is answered the same way whether
        // or not that model exists.
        let resource = crate::state::target_model::model_resource_arn(ctx, &input.target_model_id);
        if let Err(denied) =
            crate::state::authorize(&*self.engine, ctx, OPERATION_NAME, &resource)
        {
            return Err(OptimizePromptError::AccessDeniedException(denied));
        }
        let model = crate::state::target_model::resolve(
            &*self.engine,
            &crate::state::request_region(ctx),
            &input.target_model_id,
        )
        .map_err(|refusal| match refusal {
            crate::state::target_model::ModelRefusal::StoreUnavailable => {
                internal("the model catalogue is unavailable")
            }
            // Both halves of an unusable model reference are this operation's only
            // modelled client fault: it declares no `ResourceNotFoundException`, so an
            // identifier naming no model this emulator serves is stated as the
            // validation failure it is, in the words the data plane states it in.
            crate::state::target_model::ModelRefusal::Unusable(message) => validation(message),
        })?;

        let prompt = match &input.input {
            InputPrompt::textPrompt(text_prompt) => text_prompt.text.as_str(),
        };
        let mut frames = Vec::new();
        self.frame(
            protocol,
            &mut frames,
            ANALYZE_EVENT,
            &AnalyzePromptEvent {
                message: Some(crate::state::target_model::analysis_notice(&model)),
            },
            &AnalyzePromptEvent::SCHEMA,
        )?;
        self.frame(
            protocol,
            &mut frames,
            OPTIMIZED_EVENT,
            &OptimizedPromptEvent {
                optimized_prompt: Some(OptimizedPrompt::textPrompt(TextPrompt {
                    text: crate::state::target_model::optimized_prompt_notice(&model, prompt),
                })),
            },
            &OptimizedPromptEvent::SCHEMA,
        )?;
        // `OptimizePromptResponse` binds no header: the stream IS the response.
        Ok(event_stream_only_response(frames))
    }

    /// Append one `OptimizedPromptStream` event to the stream.
    fn frame(
        &self,
        protocol: &dyn ServerHttpProtocol,
        frames: &mut Vec<u8>,
        event_type: &str,
        event: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        schema: &::aws_smithy_schema::Schema,
    ) -> Result<(), OptimizePromptError> {
        let frame = frame_event(protocol, event_type, false, event, schema).map_err(|error| {
            internal(format!(
                "the {event_type} event could not be framed: {error}"
            ))
        })?;
        frames.extend_from_slice(&frame);
        Ok(())
    }
}

/// The operation name, which names the IAM action this call is authorized against
/// (`bedrock:OptimizePrompt`). Declared here because this operation has no generated
/// `impl Operation` to carry a `NAME`.
const OPERATION_NAME: &str = "OptimizePrompt";

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request.
///
/// The request half is the model's own: the framework's deserializer and constraint walk
/// over THIS operation's input (`service_common::event_stream::read_request`), refused in
/// this service's own wording (`Service::override_refusal`), so a refusal here reads
/// exactly like one from a non-streaming operation of the same service.
impl<Eng: Send + Sync + 'static> EventStreamOperation for OptimizePrompt<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let input = match read_request::<OptimizePromptInput>(
                protocol,
                req,
                &OptimizePromptInput::SCHEMA,
            ) {
                Ok(input) => input,
                Err(RequestRefusal::Invalid(violations)) => {
                    let chosen = svc.override_refusal(&Refusal::Invalid(violations.clone()));
                    return definition.refusal_response(
                        ProtocolError::Invalid(violations),
                        chosen,
                        req,
                    );
                }
                // A body the protocol's codec could not parse has no violation form, so
                // the reader's own sentence is what the modelled 400 carries.
                Err(RequestRefusal::Unreadable(message)) => {
                    return error_response(protocol, &validation(message))
                }
            };
            match self.respond(protocol, &input, ctx) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            }
        })
    }
}
