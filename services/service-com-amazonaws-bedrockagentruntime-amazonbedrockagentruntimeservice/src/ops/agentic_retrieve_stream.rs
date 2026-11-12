// SPDX-License-Identifier: CC0-1.0

//! @op AgenticRetrieveStream
//!
//! `POST /agenticRetrieveStream -> 200`. Serves one agentic retrieval as a
//! `vnd.amazon.eventstream` body: the `result` event carrying what the retrievers found and
//! the answer generated from it, and the `responseEvent` carrying that answer — the members
//! the modelled `AgenticRetrieveStreamResponseOutput` union declares for a retrieval that
//! completes.
//!
//! ## What is real here
//!
//! * every retriever names a KNOWLEDGE BASE of the Bedrock Agent build-time plane, and the
//!   items reported for it are the chunks of the vector index that knowledge base is bound
//!   to, ranked by the index's own nearest-neighbour geometry — the same retrieval
//!   `Retrieve` and `RetrieveAndGenerate` answer from
//!   (`crate::state::knowledge_base_retrieval`), so the three cannot disagree about what a
//!   knowledge base holds. A knowledge base that plane does not hold is the modelled
//!   `ResourceNotFoundException` (HTTP 404), never an empty result;
//! * the query is the text of the last message the CONVERSATION carries from the user,
//!   which is what the retrieval is run for;
//! * `retrievalOverrides` is honoured: `filter` narrows the items to those whose own
//!   metadata it admits and `maxNumberOfResults` bounds how many each retriever reports;
//! * every citation of the generated answer carries the SPAN of the answer its item was
//!   quoted into and the position of that item in `results`, so a client resolves a
//!   citation to the item it came from.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR: the answer is the one
//! `crate::state::generated_answer` composes, which says so and then quotes the retrieved
//! passages exactly as the ingestion recorded them. A `CUSTOM` retrieval resolves the model
//! it names against the catalogue the Bedrock Runtime data plane serves inference from, so
//! the answer names a model this emulator really publishes; a `MANAGED` one is generated
//! against the model the service selects, which the answer says it was.
//!
//! ## What it refuses rather than answers
//!
//! A leg this emulator cannot compute is REFUSED with the modelled `ValidationException`
//! (HTTP 400), because a plausible-looking success is worse than an error: a retrieval that
//! ignored the member would report items the caller asked to have filtered, reordered or
//! withheld. Those legs are `userContext` (access-control filtering needs per-document ACLs
//! nothing here holds), `policyConfiguration` (no guardrail policy is evaluated, so neither
//! an intervention nor its absence can be reported), `memoryConfiguration` (this emulator
//! holds no AgentCore memory to read), `retrievalInstructionConfiguration` (it names a
//! build-time instruction resource nothing here applies), a reranking model (no reranker
//! runs, so the order one would put the items in cannot be computed) and a Mantle
//! foundation model (not a model this emulator publishes). `nextToken` is refused too: this
//! service hands out none, so no value of it names a position in a retrieval.
//!
//! This operation is hand-authored rather than generated: its output is a `@streaming`
//! union, which the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and whose
//! generated `ErasedOperation` shim would decline the request. The framing and the routing
//! seam live in `service_common::event_stream`, the shapes in
//! `super::types::agentic_retrieve_stream_types`, and `crate::service` routes a request
//! here through `Service::override_operation`.
//!
//! ## Input
//!
//! * `messages` (AgenticRetrieveMessages, len 1..) **required** — the conversation.
//! * `retrievers` (AgenticRetrievers, len 1..) **required** — what to read.
//! * `agenticRetrieveConfiguration` (AgenticRetrieveConfiguration) **required** — its
//!   `foundationModelType` is one of CUSTOM | MANAGED (default MANAGED).
//! * `policyConfiguration`, `nextToken` (len 1..2048, pattern `^\S*$`), `userContext`,
//!   `memoryConfiguration`, `retrievalInstructionConfiguration`, `generateResponse`
//!   (Boolean, default true).
//!
//! ## Output
//!
//! * `stream` (AgenticRetrieveStreamResponseOutput, `@httpPayload`, `@streaming`)
//!   **required** — the body; the response binds no modelled header.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (403), `BadGatewayException` (502), `ConflictException` (409),
//!   `DependencyFailedException` (424), `InternalServerException` (500),
//!   `ResourceNotFoundException` (404), `ServiceQuotaExceededException` (400),
//!   `ThrottlingException` (429), `ValidationException` (400).

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_only_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, ServerHttpProtocol, Service};

use crate::state::generated_answer as generation;
use crate::state::knowledge_base_retrieval as retrieval;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it. No `Scaffolded` wrapper: this operation is implemented, so the 501
/// channel has nothing to carry.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#AgenticRetrieveStream")]
#[derive(Debug)]
pub enum AgenticRetrieveStreamError {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's modelled 400 `ValidationException`, naming the member that was wrong
/// and what it requires.
fn validation(message: impl Into<String>) -> AgenticRetrieveStreamError {
    AgenticRetrieveStreamError::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

/// This operation's modelled 404 `ResourceNotFoundException`.
fn not_found(message: String) -> AgenticRetrieveStreamError {
    AgenticRetrieveStreamError::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
}

/// This operation's modelled 500 `InternalServerException`.
fn internal(message: impl Into<String>) -> AgenticRetrieveStreamError {
    AgenticRetrieveStreamError::InternalServerException(InternalServerException {
        message: Some(message.into()),
        reason: None,
    })
}

/// This operation's modelled 424 `DependencyFailedException`: the knowledge base's content
/// does not live anywhere this plane can read it.
fn dependency_failed(resource: String) -> AgenticRetrieveStreamError {
    AgenticRetrieveStreamError::DependencyFailedException(DependencyFailedException {
        message: Some(retrieval::unreachable_store_message(&resource)),
        resource_name: Some(resource),
    })
}

/// This operation's modelled 400 `ServiceQuotaExceededException`, answered when a resource
/// guard of this emulator is reached.
fn quota(message: String) -> AgenticRetrieveStreamError {
    AgenticRetrieveStreamError::ServiceQuotaExceededException(ServiceQuotaExceededException {
        message: Some(message),
    })
}

/// The `AgenticRetrieveStreamResponseOutput` members a completed retrieval travels under,
/// which the frames carry as their `:event-type`
/// (`smithy_query(query="shape",shape="AgenticRetrieveStreamResponseOutput")`).
const RESULT_EVENT: &str = "result";

/// See [`RESULT_EVENT`]: the member the generated answer travels under.
const RESPONSE_EVENT: &str = "responseEvent";

/// The `ConversationRole` whose message a retrieval is run for
/// (`smithy_query(query="shape",shape="ConversationRole")` -> user, assistant).
const USER_ROLE: &str = "user";

/// The `FoundationModelType` that names a model in the REQUEST
/// (`smithy_query(query="shape",shape="FoundationModelType")` -> CUSTOM, MANAGED).
const CUSTOM_MODEL: &str = "CUSTOM";

/// The `FoundationModelConfigurationType` whose model this emulator publishes
/// (`smithy_query(query="shape",shape="FoundationModelConfigurationType")` ->
/// BEDROCK_FOUNDATION_MODEL, MANTLE_FOUNDATION_MODEL).
const BEDROCK_FOUNDATION_MODEL: &str = "BEDROCK_FOUNDATION_MODEL";

/// The `AgenticRetrieveRerankingModelType` that asks for NO reranking
/// (`smithy_query(query="shape",shape="AgenticRetrieveRerankingModelType")` -> CUSTOM,
/// MANAGED, NONE).
const NO_RERANKING: &str = "NONE";

/// The media type of a retrieved item's content: the chunk's text, exactly as the
/// ingestion recorded it.
const TEXT_MEDIA_TYPE: &str = "text/plain";

/// The operation name, which names the IAM action this call is authorized against
/// (`bedrock:AgenticRetrieveStream`). Declared here because this operation has no generated
/// `impl Operation` to carry a `NAME`.
const OPERATION_NAME: &str = "AgenticRetrieveStream";

/// The literal single segment of this operation's modelled URI,
/// `POST /agenticRetrieveStream`.
const SEGMENT: &str = "agenticRetrieveStream";

/// The `AgenticRetrieveStream` operation.
///
/// Owns the shared engine (the constructor seam). Held by `crate::service` behind an
/// `EventStreamRoute`, because the generated routing table cannot serve a
/// `@streaming`-output operation.
pub struct AgenticRetrieveStream<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AgenticRetrieveStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
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

/// One retriever's read, once the request has been understood.
struct RetrieverRead<'request> {
    /// The knowledge base the retriever names.
    knowledge_base_id: &'request str,
    /// What the request overrides about the read.
    search: retrieval::SearchConfiguration<'request>,
}

impl<Eng: Send + Sync + 'static> AgenticRetrieveStream<Eng> {
    /// The whole request once it has been read: refuse what cannot be computed, authorize
    /// it, read every retriever and frame what they found.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        input: &AgenticRetrieveStreamInput,
        ctx: &OperationContext<'_>,
    ) -> Result<Response, AgenticRetrieveStreamError> {
        // The request is read in full BEFORE anything is looked up: a leg this service
        // cannot compute is refused whatever the stores hold, and that refusal depends on
        // the request alone, so it says nothing about which knowledge bases exist.
        unservable_leg(input)?;
        let query = query_text(input)?;
        let reads = retriever_reads(input)?;

        // Authorization is decided against every knowledge base the call reads — the
        // resources an identity-based policy for `bedrock:AgenticRetrieveStream` names —
        // and before any store is read, so an unauthorized call is answered the same way
        // whether or not those knowledge bases exist.
        for read in &reads {
            let resource = retrieval::knowledge_base_resource_arn(ctx, read.knowledge_base_id);
            if let Err(denied) =
                crate::state::authorize(&*self.engine, ctx, OPERATION_NAME, &resource)
            {
                return Err(AgenticRetrieveStreamError::AccessDeniedException(denied));
            }
        }

        // The model the answer is generated against, resolved before anything is retrieved
        // so a reference this emulator cannot use is refused whatever the retrieval finds.
        let model = self.answer_model(&input.agentic_retrieve_configuration)?;

        let mut chunks: Vec<(String, retrieval::RetrievedChunk)> = Vec::new();
        for read in &reads {
            let found = retrieval::nearest_chunks(
                &*self.engine,
                read.knowledge_base_id,
                &query,
                read.search.results,
                read.search.filter,
            )
            .map_err(|refusal| match refusal {
                retrieval::RetrievalRefusal::UnknownKnowledgeBase => not_found(
                    retrieval::knowledge_base_not_found_message(read.knowledge_base_id),
                ),
                retrieval::RetrievalRefusal::StoreUnavailable => {
                    internal("the knowledge base store is unavailable")
                }
                retrieval::RetrievalRefusal::UnreachableVectorStore(resource)
                | retrieval::RetrievalRefusal::MissingVectorIndex(resource) => {
                    dependency_failed(resource)
                }
            })?;
            for chunk in found {
                chunks.push((read.knowledge_base_id.to_string(), chunk));
            }
        }
        // One order for one request: the items are reported most relevant first across
        // every retriever that was read, and a tie keeps the order the retrievers were
        // named in.
        chunks.sort_by(|left, right| {
            right
                .1
                .score
                .partial_cmp(&left.1.score)
                .unwrap_or(::std::cmp::Ordering::Equal)
        });

        let results: Vec<AgenticRetrieveResultItem> = chunks
            .iter()
            .map(|(retriever, chunk)| result_item(retriever, chunk))
            .collect();
        // The answer quotes the retrieved chunks in the order the items report them, so the
        // n-th passage of the answer is the n-th item and a citation names it by position.
        let answer = input.generate_response.then(|| {
            let quoted: Vec<retrieval::RetrievedChunk> =
                chunks.into_iter().map(|(_, chunk)| chunk).collect();
            match &model {
                AnswerModel::Managed => generation::compose_managed_answer(quoted),
                AnswerModel::Named(named) => {
                    generation::compose_answer(&named.model_id, quoted, named.budget(None), &[])
                }
            }
        });

        let mut frames = Vec::new();
        self.frame(
            protocol,
            &mut frames,
            RESULT_EVENT,
            &AgenticRetrieveResultEvent {
                results,
                generated_response: answer.as_ref().map(generated_response),
                // This service reports every item it found in one page, so there is no
                // position for a later page to resume from.
                next_token: None,
            },
            &AgenticRetrieveResultEvent::SCHEMA,
        )?;
        if let Some(answer) = answer {
            self.frame(
                protocol,
                &mut frames,
                RESPONSE_EVENT,
                &AgenticRetrieveResponseEvent { text: answer.text },
                &AgenticRetrieveResponseEvent::SCHEMA,
            )?;
        }
        // `AgenticRetrieveStreamResponse` binds no header: the stream IS the response.
        Ok(event_stream_only_response(frames))
    }

    /// The model the answer is generated against: the one a `CUSTOM` retrieval names,
    /// resolved against the catalogue the Bedrock Runtime data plane serves inference from,
    /// or the model the SERVICE selects.
    fn answer_model(
        &self,
        configuration: &AgenticRetrieveConfiguration,
    ) -> Result<AnswerModel, AgenticRetrieveStreamError> {
        if configuration.foundation_model_type != CUSTOM_MODEL {
            return Ok(AnswerModel::Managed);
        }
        let Some(named) = configuration.foundation_model_configuration.as_ref() else {
            return Err(validation(format!(
                "{MODEL_MEMBER} is required when {CONFIGURATION_MEMBER}.foundationModelType \
                 is {CUSTOM_MODEL}: it names the model the answer is generated with"
            )));
        };
        let Some(bedrock) = named.bedrock_foundation_model_configuration.as_ref() else {
            return Err(validation(format!(
                "{MODEL_MEMBER}.bedrockFoundationModelConfiguration is required when \
                 {MODEL_MEMBER}.type is {BEDROCK_FOUNDATION_MODEL}: it names the model the \
                 answer is generated with"
            )));
        };
        generation::resolve_model(
            &*self.engine,
            &format!("{MODEL_MEMBER}.bedrockFoundationModelConfiguration.modelConfiguration.modelArn"),
            &bedrock.model_configuration.model_arn,
        )
        .map(AnswerModel::Named)
        .map_err(|refusal| match refusal {
            generation::GenerationRefusal::StoreUnavailable => {
                internal("the model catalogue is unavailable")
            }
            generation::GenerationRefusal::InvalidModel(message) => validation(message),
            generation::GenerationRefusal::ModelNotFound(message) => not_found(message),
        })
    }

    /// Append one `AgenticRetrieveStreamResponseOutput` event to the stream.
    fn frame(
        &self,
        protocol: &dyn ServerHttpProtocol,
        frames: &mut Vec<u8>,
        event_type: &str,
        event: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        schema: &::aws_smithy_schema::Schema,
    ) -> Result<(), AgenticRetrieveStreamError> {
        let frame = frame_event(protocol, event_type, false, event, schema).map_err(|error| {
            internal(format!(
                "the {event_type} event could not be framed: {error}"
            ))
        })?;
        frames.extend_from_slice(&frame);
        Ok(())
    }
}

/// The model one answer is generated against.
enum AnswerModel {
    /// The model the SERVICE selects, which this emulator publishes no catalogue entry for.
    Managed,
    /// The model the request named, as the catalogue holds it.
    Named(generation::GenerationModel),
}

/// The member path the retrieval's configuration travels in, so a refusal names the member
/// the request actually carried.
const CONFIGURATION_MEMBER: &str = "agenticRetrieveConfiguration";

/// See [`CONFIGURATION_MEMBER`]: the model leg of that configuration.
const MODEL_MEMBER: &str = "agenticRetrieveConfiguration.foundationModelConfiguration";

/// The first leg of the request this service cannot compute, as the sentence the modelled
/// `ValidationException` states. See the module docs for why each one is refused.
fn unservable_leg(input: &AgenticRetrieveStreamInput) -> Result<(), AgenticRetrieveStreamError> {
    if input.user_context.is_some() {
        return Err(validation(
            "userContext: this service applies no per-document access control to a \
             retrieval, so the items it reports cannot be narrowed to the documents one \
             user may read; omit userContext to retrieve over every ingested document",
        ));
    }
    if input.next_token.is_some() {
        return Err(validation(
            "nextToken: this service reports every item it found in one page and hands out \
             no nextToken, so no value of it names a position in a retrieval; omit \
             nextToken",
        ));
    }
    if input.policy_configuration.is_some() {
        return Err(validation(
            "policyConfiguration: it names the guardrail a retrieval is run under, and this \
             service evaluates no guardrail policy, so neither an intervention nor its \
             absence can be reported; omit policyConfiguration to retrieve ungoverned",
        ));
    }
    if input.memory_configuration.is_some() {
        return Err(validation(
            "memoryConfiguration: it names an AgentCore memory to retrieve from beside the \
             knowledge bases, and this emulator holds none, so nothing could be read from \
             it; omit memoryConfiguration to retrieve from the retrievers alone",
        ));
    }
    if input.retrieval_instruction_configuration.is_some() {
        return Err(validation(
            "retrievalInstructionConfiguration: it names a retrieval instruction that \
             steers how the query is planned, and no model plans one here, so the \
             instruction cannot change what is retrieved; omit it",
        ));
    }
    let configuration = &input.agentic_retrieve_configuration;
    if configuration
        .reranking_model_type
        .as_deref()
        .is_some_and(|kind| kind != NO_RERANKING)
        || configuration.reranking_configuration.is_some()
    {
        return Err(validation(format!(
            "{CONFIGURATION_MEMBER}.rerankingModelType: this service runs no reranking \
             model, so the order a reranker would put the items in cannot be computed; \
             supply {NO_RERANKING} to take the retrieval's own order"
        )));
    }
    if configuration
        .foundation_model_configuration
        .as_ref()
        .is_some_and(|named| named.r#type != BEDROCK_FOUNDATION_MODEL)
    {
        return Err(validation(format!(
            "{MODEL_MEMBER}.type: {:?} names no model this emulator publishes. An answer is \
             generated against a model the Bedrock Runtime plane of this emulator serves \
             inference from; supply {BEDROCK_FOUNDATION_MODEL} with one of those models",
            configuration
                .foundation_model_configuration
                .as_ref()
                .map(|named| named.r#type.as_str())
                .unwrap_or_default()
        )));
    }
    Ok(())
}

/// The text the retrievers are read for: the last message of the conversation the USER
/// carries.
///
/// A conversation whose last user message carries no text carries no query, and a retrieval
/// ranked against nothing would report the index's first chunks as if they answered
/// something, so it is refused.
fn query_text(input: &AgenticRetrieveStreamInput) -> Result<String, AgenticRetrieveStreamError> {
    let text = input
        .messages
        .iter()
        .rev()
        .find(|message| message.role == USER_ROLE)
        .and_then(|message| message.content.text.as_deref())
        .filter(|text| text.split_whitespace().next().is_some());
    text.map(str::to_string).ok_or_else(|| {
        validation(format!(
            "messages must carry a message with role {USER_ROLE} whose content.text is the \
             query the retrievers are read for"
        ))
    })
}

/// What every retriever of the request asks to have read.
fn retriever_reads(
    input: &AgenticRetrieveStreamInput,
) -> Result<Vec<RetrieverRead<'_>>, AgenticRetrieveStreamError> {
    // Each retriever is one pass over a vector index, and the modelled list has no length
    // ceiling, so how many passes one request may ask for is bounded before any of them
    // runs.
    if input.retrievers.len() > crate::state::MAX_RETRIEVERS_PER_RETRIEVAL {
        return Err(quota(format!(
            "The number of requests exceeds the service quota: one retrieval reads at most \
             {} retrievers",
            crate::state::MAX_RETRIEVERS_PER_RETRIEVAL
        )));
    }
    let mut reads = Vec::new();
    for (index, retriever) in input.retrievers.iter().enumerate() {
        let RetrieverConfiguration::knowledgeBase(knowledge_base) = &retriever.configuration;
        let member = format!("retrievers[{index}].configuration.knowledgeBase.retrievalOverrides");
        let overrides = knowledge_base.retrieval_overrides.as_ref();
        let search = retrieval::SearchConfiguration {
            results: overrides
                .and_then(|overrides| overrides.max_number_of_results)
                .and_then(|requested| usize::try_from(requested).ok())
                .filter(|requested| *requested > 0)
                .unwrap_or(retrieval::DEFAULT_NUMBER_OF_RESULTS),
            filter: overrides.and_then(|overrides| overrides.filter.as_ref()),
            member,
        };
        // How big a filter may be is decided BEFORE anything is ranked: past the bounds
        // this service evaluates, the request is refused rather than walked.
        if let Some(too_big) = retrieval::filter_bounds_refusal(&search) {
            return Err(validation(too_big));
        }
        reads.push(RetrieverRead {
            knowledge_base_id: knowledge_base.knowledge_base_id.as_str(),
            search,
        });
    }
    Ok(reads)
}

/// One retrieved chunk as the modelled result item: its text, the metadata the ingestion
/// recorded with it, and the retriever it came from.
fn result_item(retriever: &str, chunk: &retrieval::RetrievedChunk) -> AgenticRetrieveResultItem {
    AgenticRetrieveResultItem {
        content: RetrievalContent {
            // The chunk's content is the text the ingestion recorded; nothing here carries
            // bytes of another media type.
            byte_content: None,
            text: Some(chunk.text.clone()),
            mime_type: TEXT_MEDIA_TYPE.to_string(),
        },
        // `AgenticRetrieveMetadata` has no length floor, but a chunk the ingestion recorded
        // nothing about is reported with the member absent rather than with an empty map.
        metadata: Some(
            chunk
                .metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
        .filter(|metadata: &::std::collections::HashMap<_, _>| !metadata.is_empty()),
        source_retriever: AgenticRetrieveSourceRetriever {
            identifier: retriever.to_string(),
        },
    }
}

/// The generated answer as the modelled response, with one citation per quoted passage: the
/// span of the answer the passage occupies, and the position of the item it was quoted from.
fn generated_response(answer: &generation::GeneratedAnswer) -> AgenticRetrieveGeneratedResponse {
    AgenticRetrieveGeneratedResponse {
        answer: answer.text.clone(),
        citations: Some(
            answer
                .passages
                .iter()
                .enumerate()
                .map(|(index, passage)| AgenticRetrieveCitation {
                    start_index: span_index(passage.start),
                    end_index: span_index(passage.end),
                    references: vec![AgenticRetrieveCitationReference {
                        result_index: span_index(index),
                    }],
                })
                .collect(),
        )
        .filter(|citations: &Vec<_>| !citations.is_empty()),
    }
}

/// One index as the modelled `Integer` members carry it.
///
/// An answer long enough to overflow it cannot be composed — what bounds it is the
/// retrieval page the request asked for — so the saturation is the unreachable arm of a
/// total conversion.
fn span_index(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request.
///
/// The request half is the model's own: the framework's deserializer and constraint walk
/// over THIS operation's input, refused in this service's own wording
/// (`Service::override_refusal`), so a `foundationModelType` outside the modelled enum is
/// refused exactly as one from a non-streaming operation of the same service.
impl<Eng: Send + Sync + 'static> EventStreamOperation for AgenticRetrieveStream<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let input = match read_request::<AgenticRetrieveStreamInput>(
                protocol,
                req,
                &AgenticRetrieveStreamInput::SCHEMA,
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
                // A body the protocol's codec could not parse has no violation form, so the
                // reader's own sentence is what the modelled 400 carries.
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
