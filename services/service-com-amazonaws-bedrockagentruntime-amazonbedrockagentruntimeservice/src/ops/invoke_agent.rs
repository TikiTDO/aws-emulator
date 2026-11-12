// SPDX-License-Identifier: CC0-1.0

//! @op InvokeAgent
//!
//! `POST /agents/{agentId}/agentAliases/{agentAliasId}/sessions/{sessionId}/text -> 200`.
//! Serves one agent invocation as a `vnd.amazon.eventstream` body: the `chunk` event the
//! modelled `ResponseStream` union declares for the agent's answer.
//!
//! ## What is real here
//!
//! * the AGENT ALIAS is the Bedrock Agent build-time plane's resource, so it is resolved
//!   THERE (`crate::state::agent_invocation`): an agent that plane does not hold, or an
//!   alias it holds nothing for, is the modelled `ResourceNotFoundException` (HTTP 404) and
//!   never an empty stream;
//! * the three URI labels are checked against the model before anything is looked up, so a
//!   malformed `sessionId`, `agentId` or `agentAliasId` is the modelled `ValidationException`
//!   (HTTP 400) rather than a not-found;
//! * the caller's permission for `bedrock:InvokeAgent` on that alias is decided before any
//!   store is read, so an unauthorized call is answered the same way whether or not the
//!   agent exists;
//! * the MEMORY of the alias is written when the invocation ends a session under a
//!   `memoryId` and the stored agent is configured to keep a session summary — which is
//!   what makes `GetAgentMemory` report a session that was really invoked.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. The single
//! `chunk` carries the notice the Bedrock Runtime plane of this same emulator answers every
//! inference surface with, extended with the alias that answered and the foundation model
//! the stored agent is bound to. The caller's own `inputText` is never handed back as if an
//! agent had replied to it, and the chunk carries no `attribution`: no knowledge base was
//! queried, so there is no source to cite.
//!
//! ## What it refuses rather than answers
//!
//! A leg this emulator cannot compute is REFUSED with the modelled `ValidationException`,
//! because a plausible-looking success is worse than an error: `sessionState` (its
//! attributes replace prompt placeholders and its results answer an action group, neither of
//! which anything here reads), `enableTrace` (a trace reports the orchestration steps a
//! model took, and no model takes any), `streamingConfigurations.applyGuardrailInterval` (no
//! guardrail is evaluated, so neither an intervention nor its absence can be reported) and
//! `promptCreationConfigurations` (it selects which earlier turns populate the agent prompt,
//! and no prompt is read).
//!
//! This operation is hand-authored rather than generated: its output is a `@streaming`
//! union, which the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and whose
//! generated `ErasedOperation` shim would decline the request. The framing and the routing
//! seam live in `service_common::event_stream`, the shapes in
//! `super::types::invoke_agent_types`, and `crate::service` routes a request here through
//! `Service::override_operation`.
//!
//! ## Input
//!
//! * `agentId` (AgentId, len ..10, pattern `^[0-9a-zA-Z]+$`, `@httpLabel`) **required**.
//! * `agentAliasId` (AgentAliasId, len ..10, pattern `^[0-9a-zA-Z]+$`, `@httpLabel`)
//!   **required**.
//! * `sessionId` (SessionId, len 2..100, pattern `^[0-9a-zA-Z._:-]+$`, `@httpLabel`)
//!   **required**.
//! * `inputText` (InputText, len ..25000000, `@sensitive`), `memoryId` (MemoryId, len
//!   2..100), `endSession` (Boolean), `enableTrace` (Boolean), `sourceArn`
//!   (`@httpHeader x-amz-source-arn`), `sessionState`, `bedrockModelConfigurations`,
//!   `streamingConfigurations`, `promptCreationConfigurations`.
//!
//! ## Output
//!
//! * `completion` (ResponseStream, `@httpPayload`, `@streaming`) **required** — the body.
//! * `contentType` (`@httpHeader x-amzn-bedrock-agent-content-type`) **required**,
//!   `sessionId` (`@httpHeader x-amz-bedrock-agent-session-id`) **required**, `memoryId`
//!   (`@httpHeader x-amz-bedrock-agent-memory-id`).
//!
//! ## Errors
//!
//! * `AccessDeniedException` (403), `BadGatewayException` (502), `ConflictException` (409),
//!   `DependencyFailedException` (424), `InternalServerException` (500),
//!   `ModelNotReadyException` (424), `ResourceNotFoundException` (404),
//!   `ServiceQuotaExceededException` (400), `ThrottlingException` (429),
//!   `ValidationException` (400).

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, ServerHttpProtocol, Service};

use crate::state::agent_invocation as invocation;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it. No `Scaffolded` wrapper: this operation is implemented, so the 501
/// channel has nothing to carry.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#InvokeAgent")]
#[derive(Debug)]
pub enum InvokeAgentError {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ModelNotReadyException(ModelNotReadyException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's modelled 400 `ValidationException`, naming the member that was wrong
/// and what it requires — the caller has to fix the request from this text alone.
fn validation(message: impl Into<String>) -> InvokeAgentError {
    InvokeAgentError::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

/// This operation's modelled 404 `ResourceNotFoundException`.
fn not_found(message: String) -> InvokeAgentError {
    InvokeAgentError::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
}

/// This operation's modelled 500 `InternalServerException`: a store this emulator should
/// carry is not on the engine, which is never reported as a client fault.
fn internal(message: impl Into<String>) -> InvokeAgentError {
    InvokeAgentError::InternalServerException(InternalServerException {
        message: Some(message.into()),
        reason: None,
    })
}

/// This operation's modelled 400 `ServiceQuotaExceededException`, answered when a resource
/// guard of this emulator's store is reached.
fn quota(message: String) -> InvokeAgentError {
    InvokeAgentError::ServiceQuotaExceededException(ServiceQuotaExceededException {
        message: Some(message),
    })
}

/// The `ResponseStream` member one part of the agent's answer travels under, which the
/// frame carries as its `:event-type`
/// (`smithy_query(query="shape",shape="ResponseStream")`).
const CHUNK_EVENT: &str = "chunk";

/// The operation name, which names the IAM action this call is authorized against
/// (`bedrock:InvokeAgent`). Declared here because this operation has no generated
/// `impl Operation` to carry a `NAME`.
const OPERATION_NAME: &str = "InvokeAgent";

/// The literal segments of this operation's modelled URI, in order:
/// `POST /agents/{agentId}/agentAliases/{agentAliasId}/sessions/{sessionId}/text`.
const ROUTE_SEGMENTS: [Option<&str>; 6] = [
    Some("agents"),
    None,
    Some("agentAliases"),
    None,
    Some("sessions"),
    None,
];

/// The trailing literal segment of that URI, after the `sessionId` label.
const TEXT_SEGMENT: &str = "text";

/// The `InvokeAgent` operation.
///
/// Owns the shared engine (the constructor seam). Held by `crate::service` behind an
/// `EventStreamRoute`, because the generated routing table cannot serve a
/// `@streaming`-output operation.
pub struct InvokeAgent<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeAgent<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Whether `req` addresses this operation's modelled route.
///
/// The seam that routes it ([`crate::service::EmulatedService::override_operation`]) asks
/// this, so the path this operation recognises is stated once, beside the operation.
pub fn addresses(req: &Request) -> bool {
    if req.method() != "POST" {
        return false;
    }
    let path = service_common::uri_path(req.uri());
    let mut segments = path.trim_matches('/').split('/');
    for expected in ROUTE_SEGMENTS {
        let Some(segment) = segments.next() else {
            return false;
        };
        match expected {
            Some(literal) if segment != literal => return false,
            // A label: any non-empty segment, whose own constraints the model checks.
            None if segment.is_empty() => return false,
            _ => {}
        }
    }
    matches!((segments.next(), segments.next()), (Some(TEXT_SEGMENT), None))
}

impl<Eng: Send + Sync + 'static> InvokeAgent<Eng> {
    /// The whole request once it has been read: authorize it, resolve the alias, refuse
    /// what cannot be computed, record the session's summary and frame the answer.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        input: &InvokeAgentInput,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, InvokeAgentError> {
        // Authorization is decided against the agent alias the call invokes — the resource
        // an identity-based policy for `bedrock:InvokeAgent` names — and before any store
        // is read, so an unauthorized call is answered the same way whether or not that
        // alias exists.
        let resource =
            invocation::alias_resource_arn(ctx, &input.agent_id, &input.agent_alias_id);
        if let Err(denied) =
            crate::state::authorize(&*self.engine, ctx, OPERATION_NAME, &resource)
        {
            return Err(InvokeAgentError::AccessDeniedException(denied));
        }

        let alias = invocation::resolve(&*self.engine, &input.agent_id, &input.agent_alias_id)
            .map_err(|refusal| match refusal {
                invocation::InvocationRefusal::StoreUnavailable => {
                    internal("the agent store is unavailable")
                }
                invocation::InvocationRefusal::NotFound => {
                    not_found(invocation::alias_not_found_message())
                }
            })?;

        // Only once the alias has RESOLVED is a leg this service cannot compute refused:
        // the recorded refusal of an agent nobody created answers the missing resource
        // whatever else the request carries, so resolution comes first and this is what a
        // caller whose agent exists is told about the member.
        unservable_leg(input)?;

        // An invocation that ENDS its session is what leaves a summary in the alias's
        // memory, and only under the `memoryId` the request recorded it against.
        if let Some(memory_id) = input.memory_id.as_deref() {
            if input.end_session == Some(true) {
                self.remember(&alias, memory_id, &input.session_id)?;
            }
        }

        let frame = frame_event(
            protocol,
            CHUNK_EVENT,
            false,
            &PayloadPart {
                bytes: Some(invocation::completion_notice(&alias).into_bytes()),
                // No knowledge base was queried on this path, so there is no retrieved
                // source for the chunk to be attributed to.
                attribution: None,
            },
            &PayloadPart::SCHEMA,
        )
        .map_err(|error| internal(format!("the {CHUNK_EVENT} event could not be framed: {error}")))?;

        event_stream_response(
            protocol,
            OPERATION_NAME,
            &InvokeAgentOutput {
                content_type: invocation::RESPONSE_CONTENT_TYPE.to_string(),
                session_id: input.session_id.clone(),
                // The memory the invocation was recorded against, which the response
                // reports only when the request named one.
                memory_id: input.memory_id.clone(),
            },
            &InvokeAgentOutput::SCHEMA,
            frame,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }

    /// Record the summary of the ended session in the memory of the alias that served it.
    ///
    /// An agent that is not configured to keep a session summary keeps none: nothing is
    /// written, and `GetAgentMemory` goes on reporting the empty memory it really holds.
    fn remember(
        &self,
        alias: &invocation::InvokedAlias,
        memory_id: &str,
        session_id: &str,
    ) -> Result<(), InvokeAgentError> {
        let Some(summary) =
            invocation::session_summary(alias, memory_id, session_id, crate::state::now())
        else {
            return Ok(());
        };
        let store = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the agent memory store is unavailable"))?;
        let mut guard = store.lock();
        let key = crate::state::agent_memory::memory_key(
            &alias.agent_id,
            &alias.agent_alias_id,
            memory_id,
        );
        let memory = guard.agent_memories.entry(key).or_default();
        if memory.len() >= crate::state::MAX_SESSION_SUMMARIES_PER_MEMORY {
            return Err(quota(format!(
                "The number of requests exceeds the service quota: one agent memory holds \
                 at most {} session summaries",
                crate::state::MAX_SESSION_SUMMARIES_PER_MEMORY
            )));
        }
        // One summary per session: an invocation that ends the same session twice replaces
        // the summary rather than remembering the session twice.
        memory.retain(|held| held.session_id != summary.session_id);
        memory.push(summary);
        Ok(())
    }
}

/// The first leg of the request this service cannot compute, as the sentence the modelled
/// `ValidationException` states. See the module docs for why each one is refused.
fn unservable_leg(input: &InvokeAgentInput) -> Result<(), InvokeAgentError> {
    if input.session_state.is_some() {
        return Err(validation(
            "sessionState: its attributes replace the placeholders of a prompt nothing reads \
             here, and its returnControlInvocationResults answer an action group nothing \
             invokes, so neither can change what this invocation answers; omit sessionState \
             to invoke the agent with the request's own inputText",
        ));
    }
    if input.enable_trace == Some(true) {
        return Err(validation(
            "enableTrace: a trace reports the orchestration steps the agent's model took, \
             and no model takes any in this emulator, so there is no step to report; omit \
             enableTrace to take the answer without a trace",
        ));
    }
    if input
        .streaming_configurations
        .as_ref()
        .is_some_and(|streaming| streaming.apply_guardrail_interval.is_some())
    {
        return Err(validation(
            "streamingConfigurations.applyGuardrailInterval: it is how often a guardrail is \
             applied while the response is generated, and this service evaluates no \
             guardrail policy, so neither an intervention nor its absence can be reported; \
             omit it to take the response ungoverned",
        ));
    }
    if input.prompt_creation_configurations.is_some() {
        return Err(validation(
            "promptCreationConfigurations: it selects which of the session's earlier turns \
             populate the agent prompt, and no prompt is read in this emulator, so the \
             selection cannot change what this invocation answers; omit it",
        ));
    }
    Ok(())
}

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request.
///
/// The request half is the model's own: the framework's deserializer and constraint walk
/// over THIS operation's input, refused in this service's own wording
/// (`Service::override_refusal`), so a malformed URI label is refused exactly as one from a
/// non-streaming operation of the same service.
impl<Eng: Send + Sync + 'static> EventStreamOperation for InvokeAgent<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let input =
                match read_request::<InvokeAgentInput>(protocol, req, &InvokeAgentInput::SCHEMA) {
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
