// SPDX-License-Identifier: CC0-1.0

//! @op RetrieveAndGenerateStream
//!
//! `POST /retrieveAndGenerateStream -> 200`. The STREAMING delivery of the query
//! `RetrieveAndGenerate` answers in one piece: the same knowledge base is queried, the
//! same response is generated from what the query retrieved, and the same conversational
//! session records the turn — the whole rule lives in
//! [`crate::ops::retrieve_and_generate::RetrieveAndGenerate::serve`], which this operation
//! drives, so the two cannot disagree about what a `retrieveAndGenerateConfiguration`
//! asks for, which knowledge bases exist or what a response may cite.
//!
//! What differs is the DELIVERY and the action a policy has to allow
//! (`bedrock:RetrieveAndGenerateStream`): the answer leaves as a
//! `vnd.amazon.eventstream` body — the `output` event carrying the generated text, then one
//! `citation` event per cited passage, the members the modelled
//! `RetrieveAndGenerateStreamResponseOutput` union declares for a query that completes —
//! and the `sessionId` the turn was recorded under travels in the modelled response header.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR: the text is the one
//! `crate::state::generated_answer` composes, which says so and then quotes the retrieved
//! passages exactly as the ingestion recorded them.
//!
//! A knowledge base the Bedrock Agent plane does not hold is the modelled
//! `ResourceNotFoundException` (HTTP 404), never an empty stream.
//!
//! This operation is hand-authored rather than generated: its output is a `@streaming`
//! union, which the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and whose
//! generated `ErasedOperation` shim would decline the request, so there was no route for
//! `POST /retrieveAndGenerateStream` at all. The framing and the routing seam live in
//! `service_common::event_stream`, the shapes in
//! `super::types::retrieve_and_generate_stream_types`, and `crate::service` routes a
//! request here through `Service::override_operation`.
//!
//! ## Input
//!
//! * `input` (RetrieveAndGenerateInput, `@sensitive`) **required** — the query.
//! * `retrieveAndGenerateConfiguration` (RetrieveAndGenerateConfiguration) — the knowledge
//!   base queried and the model the response is generated with.
//! * `sessionId` (SessionId, len 2..100, pattern `^[0-9a-zA-Z._:-]+$`),
//!   `sessionConfiguration` (RetrieveAndGenerateSessionConfiguration), `userContext`
//!   (UserContext, `@sensitive`).
//!
//! ## Output
//!
//! * `stream` (RetrieveAndGenerateStreamResponseOutput, `@httpPayload`, `@streaming`)
//!   **required** — the body.
//! * `sessionId` (`@httpHeader x-amzn-bedrock-knowledge-base-session-id`) **required**.
//!
//! ## Errors
//!
//! The same nine `RetrieveAndGenerate` declares, over the same shapes:
//! `AccessDeniedException` (403), `BadGatewayException` (502), `ConflictException` (409),
//! `DependencyFailedException` (424), `InternalServerException` (500),
//! `ResourceNotFoundException` (404), `ServiceQuotaExceededException` (400),
//! `ThrottlingException` (429), `ValidationException` (400).

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, ServerHttpProtocol, Service};

use super::retrieve_and_generate::{RetrieveAndGenerate, RetrieveAndGenerateError};

/// The operation name, which names the IAM action this call is authorized against
/// (`bedrock:RetrieveAndGenerateStream`). Declared here because this operation has no
/// generated `impl Operation` to carry a `NAME`.
const OPERATION_NAME: &str = "RetrieveAndGenerateStream";

/// The `RetrieveAndGenerateStreamResponseOutput` members a completed query travels under,
/// which the frames below carry as their `:event-type`
/// (`smithy_query(query="shape",shape="RetrieveAndGenerateStreamResponseOutput")`).
const OUTPUT_EVENT: &str = "output";

/// See [`OUTPUT_EVENT`]: the member one cited passage travels under.
const CITATION_EVENT: &str = "citation";

/// The literal single segment of this operation's modelled URI,
/// `POST /retrieveAndGenerateStream`.
const SEGMENT: &str = "retrieveAndGenerateStream";

/// The `RetrieveAndGenerateStream` operation.
///
/// Holds the non-streaming sibling, which owns the shared engine and the whole query rule;
/// this operation adds the event-stream delivery and nothing else.
pub struct RetrieveAndGenerateStream<Eng> {
    query: RetrieveAndGenerate<Eng>,
}

impl<Eng> RetrieveAndGenerateStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self {
            query: RetrieveAndGenerate::new(engine),
        }
    }
}

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
    matches!((segments.next(), segments.next()), (Some(SEGMENT), None))
}

impl<Eng: Send + Sync + 'static> RetrieveAndGenerateStream<Eng> {
    /// The whole request once it has been read: serve the query through the sibling, then
    /// frame what it answered.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        input: RetrieveAndGenerateStreamInput,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, RetrieveAndGenerateError> {
        // The request shapes are the sibling's own, member for member, so the query is
        // handed over as the shape that rule reads rather than re-read here.
        let query = RetrieveAndGenerateInput {
            session_id: input.session_id,
            input: input.input,
            retrieve_and_generate_configuration: input.retrieve_and_generate_configuration,
            session_configuration: input.session_configuration,
            user_context: input.user_context,
        };
        let answered = self.query.serve(OPERATION_NAME, &query, ctx)?;

        let mut frames = Vec::new();
        frame(
            protocol,
            &mut frames,
            OUTPUT_EVENT,
            &RetrieveAndGenerateOutputEvent {
                text: answered.output.text,
            },
            &RetrieveAndGenerateOutputEvent::SCHEMA,
        )?;
        for citation in answered.citations.unwrap_or_default() {
            frame(
                protocol,
                &mut frames,
                CITATION_EVENT,
                &CitationEvent {
                    // `citation` is `@deprecated` in favour of the two members beside it,
                    // so a served event carries the same citation in all three: a client
                    // reading either the old member or the new pair finds it.
                    generated_response_part: citation.generated_response_part.clone(),
                    retrieved_references: citation.retrieved_references.clone(),
                    citation: Some(citation),
                },
                &CitationEvent::SCHEMA,
            )?;
        }
        event_stream_response(
            protocol,
            OPERATION_NAME,
            &RetrieveAndGenerateStreamOutput {
                session_id: answered.session_id,
            },
            &RetrieveAndGenerateStreamOutput::SCHEMA,
            frames,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }
}

/// Append one `RetrieveAndGenerateStreamResponseOutput` event to the stream.
fn frame(
    protocol: &dyn ServerHttpProtocol,
    frames: &mut Vec<u8>,
    event_type: &str,
    event: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &::aws_smithy_schema::Schema,
) -> Result<(), RetrieveAndGenerateError> {
    let frame = frame_event(protocol, event_type, false, event, schema)
        .map_err(|error| internal(format!("the {event_type} event could not be framed: {error}")))?;
    frames.extend_from_slice(&frame);
    Ok(())
}

/// This operation's modelled 500 `InternalServerException`.
fn internal(message: String) -> RetrieveAndGenerateError {
    crate::ops::retrieve_and_generate::RetrieveAndGenerateErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(message),
            reason: None,
        },
    )
    .into()
}

/// This operation's modelled 400 `ValidationException`, for a body the protocol's codec
/// could not parse at all.
fn validation(message: String) -> RetrieveAndGenerateError {
    crate::ops::retrieve_and_generate::RetrieveAndGenerateErrorModeled::ValidationException(
        ValidationException {
            message: Some(message),
        },
    )
    .into()
}

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request.
///
/// The request half is the model's own: the framework's deserializer and constraint walk
/// over THIS operation's input, refused in this service's own wording
/// (`Service::override_refusal`), so a refusal here reads exactly like one from a
/// non-streaming operation of the same service.
impl<Eng: Send + Sync + 'static> EventStreamOperation for RetrieveAndGenerateStream<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let input = match read_request::<RetrieveAndGenerateStreamInput>(
                protocol,
                req,
                &RetrieveAndGenerateStreamInput::SCHEMA,
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
                Err(RequestRefusal::Unreadable(message)) => {
                    return error_response(protocol, &validation(message))
                }
            };
            match self.respond(protocol, input, ctx) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            }
        })
    }
}
