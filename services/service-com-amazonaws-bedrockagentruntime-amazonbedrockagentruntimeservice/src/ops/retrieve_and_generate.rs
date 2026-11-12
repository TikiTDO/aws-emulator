// SPDX-License-Identifier: CC0-1.0

//! @op RetrieveAndGenerate
//!
//! ## What this handler does
//!
//! `RetrieveAndGenerate` queries ONE knowledge base and answers the response generated from
//! what that query retrieved, with a citation for every segment of the response that came
//! from a source:
//!
//! * the RETRIEVAL is the same one `Retrieve` answers from
//!   ([`crate::state::knowledge_base_retrieval`]): the chunks of the vector index the
//!   knowledge base is bound to, ranked by the index's own nearest-neighbour geometry,
//!   narrowed by the request's `filter` and bounded by `numberOfResults`. The two
//!   operations read the request's `retrievalConfiguration` through the one reader, so they
//!   cannot disagree about what it asks for;
//! * the GENERATION is [`crate::state::generated_answer`]. NO LANGUAGE MODEL RUNS IN THIS
//!   EMULATOR: the response opens with the notice the Bedrock Runtime plane of this same
//!   emulator answers every inference surface with and then quotes the retrieved passages,
//!   most relevant first, exactly as the ingestion recorded them. It is never prose
//!   synthesised to look like inference and never the caller's own query answered back;
//! * every `citation` carries the SPAN of the response its passage occupies — `start` the
//!   first character of the segment and `end` the offset just past its last, so
//!   `output.text[start..end]` is `generatedResponsePart.textResponsePart.text` and
//!   `end - start` is that text's length — beside the `retrievedReferences` it was quoted
//!   from, whose `location` resolves to the object the chunk was ingested from;
//! * the `sessionId` is this service's own ([`crate::state::knowledge_base_session`]): a
//!   request carrying none OPENS a conversational session and is answered the id of a
//!   session this store really holds, one carrying an id CONTINUES that session, and one
//!   carrying an id this service never minted is refused — a caller cannot set the id
//!   itself.
//!
//! `modelArn` is resolved against the base-model catalogue the Bedrock Runtime data plane
//! serves inference from, so a response is only ever generated against a model this
//! emulator really publishes; its `maxTokens` ceiling and the request's own bound the
//! response, and `stopSequences` halt it, both through that plane's helpers.
//!
//! A knowledge base the Bedrock Agent plane does not hold is the modelled
//! `ResourceNotFoundException` (HTTP 404), never an empty success.
//!
//! ## What it refuses rather than answers
//!
//! A leg this emulator cannot compute is REFUSED with the modelled `ValidationException`,
//! because a plausible-looking success is worse than an error: a response that ignored the
//! member would cite passages the caller asked to have filtered, reordered or withheld, or
//! report an answer shaped by a prompt nothing read. Those legs are `userContext` (access
//! control filtering needs the per-document ACLs nothing in this emulator holds), a
//! `guardrailConfiguration` (no guardrail policy is evaluated here, so neither an
//! intervention nor its absence can be reported), a `promptTemplate` (the template is what
//! the model is ASKED, and no model is asked anything here), an `orchestrationConfiguration`
//! (it is "settings for how the model processes the prompt prior to retrieval and
//! generation", which no model does here), the `rerankingConfiguration` and
//! `implicitFilterConfiguration` of the retrieval, and an `EXTERNAL_SOURCES` configuration,
//! whose sources are documents this plane has ingested nothing of — so there is no chunk to
//! rank and nothing but the caller's own bytes to cite.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `RetrieveAndGenerate::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RetrieveAndGenerate`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json")
//! ```
//!
//! Queries a knowledge base and generates responses based on the retrieved results and
//! using the specified foundation model or inference profile. The response only cites
//! sources that are relevant to the query. This API cannot be used with managed knowledge
//! bases. Use AgenticRetrieveStream or Retrieve with managed knowledge bases.
//!
//! ## Input
//!
//! * `input` (RetrieveAndGenerateInput) **required** — Contains the query to be made to the
//!   knowledge base.
//! * `retrieveAndGenerateConfiguration` (RetrieveAndGenerateConfiguration) — Contains
//!   configurations for the knowledge base query and retrieval process. For more information,
//!   see Query configurations.
//! * `sessionConfiguration` (RetrieveAndGenerateSessionConfiguration) — Contains details
//!   about the session with the knowledge base.
//! * `sessionId` (SessionId, len 2..100, pattern ^[0-9a-zA-Z._:-]+$) — The unique identifier
//!   of the session. When you first make a RetrieveAndGenerate request, Amazon Bedrock
//!   automatically generates this value. You must reuse this value for all subsequent
//!   requests in the same conversational session. This value allows Amazon Bedrock to
//!   maintain context and knowledge from previous interactions. You can't explicitly set the
//!   sessionId yourself.
//! * `userContext` (UserContext) — Contains information about the user making the request.
//!   This is used for access control filtering to ensure that retrieval results only include
//!   documents the user is authorized to access.
//!
//! ## Output
//!
//! * `citations` (Citations) — A list of segments of the generated response that are based on
//!   sources in the knowledge base, alongside information about the sources.
//! * `guardrailAction` (GuadrailAction, one of: INTERVENED | NONE) — Specifies if there is a
//!   guardrail intervention in the response.
//! * `output` (RetrieveAndGenerateOutput) **required** — Contains the response generated from
//!   querying the knowledge base.
//! * `sessionId` (SessionId, len 2..100, pattern ^[0-9a-zA-Z._:-]+$) **required** — The
//!   unique identifier of the session. When you first make a RetrieveAndGenerate request,
//!   Amazon Bedrock automatically generates this value. You must reuse this value for all
//!   subsequent requests in the same conversational session. This value allows Amazon Bedrock
//!   to maintain context and knowledge from previous interactions. You can't explicitly set
//!   the sessionId yourself.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions. Check your permissions and retry your request.
//! * `BadGatewayException` (HTTP 502) — There was an issue with a dependency due to a server
//!   issue. Retry your request.
//! * `ConflictException` (HTTP 409) — There was a conflict performing an operation. Resolve
//!   the conflict and retry your request.
//! * `DependencyFailedException` (HTTP 424) — There was an issue with a dependency. Check the
//!   resource configurations and retry the request.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

use crate::state::generated_answer as generation;
use crate::state::knowledge_base_retrieval as retrieval;
use crate::state::knowledge_base_session as conversations;

/// The `RetrieveAndGenerateType` this service serves: a query against a knowledge base it
/// holds (`smithy_query(query="shape",shape="RetrieveAndGenerateType")` -> KNOWLEDGE_BASE,
/// EXTERNAL_SOURCES).
const TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// The member path the request's configuration travels in, so a refusal about it names the
/// member the caller actually carried.
const CONFIGURATION_MEMBER: &str = "retrieveAndGenerateConfiguration";

/// See [`CONFIGURATION_MEMBER`]: the knowledge-base leg of that configuration.
const KNOWLEDGE_BASE_MEMBER: &str = "retrieveAndGenerateConfiguration.knowledgeBaseConfiguration";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RetrieveAndGenerateError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RetrieveAndGenerateError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#RetrieveAndGenerate")]
#[derive(Debug)]
pub enum RetrieveAndGenerateErrorModeled {
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

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::retrieve_and_generate::RetrieveAndGenerateError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RetrieveAndGenerateError = crate::scaffolded::Scaffolded<RetrieveAndGenerateErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RetrieveAndGenerateErrorModeled> for RetrieveAndGenerateError {
    fn from(modeled: RetrieveAndGenerateErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RetrieveAndGenerate` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RetrieveAndGenerate<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RetrieveAndGenerate<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RetrieveAndGenerate<Eng>
{
    type Input = RetrieveAndGenerateInput;
    type Output = RetrieveAndGenerateOutput;
    type Error = RetrieveAndGenerateError;

    const NAME: &'static str = "RetrieveAndGenerate";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RETRIEVE_AND_GENERATE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RETRIEVE_AND_GENERATE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        self.serve(Self::NAME, &input, ctx)
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> RetrieveAndGenerate<Eng> {
    /// The whole knowledge-base query: read the request, authorize it, retrieve, generate
    /// and record the turn.
    ///
    /// `operation` is the operation the caller invoked, which names the IAM action the call
    /// is authorized against (`bedrock:<operation>`). It is a parameter because the
    /// STREAMING sibling of this operation serves the same query — the same input shapes,
    /// the same retrieval, the same generation and the same session — and differs only in
    /// how the answer is delivered and in which action a policy has to allow, so
    /// `crate::ops::retrieve_and_generate_stream` drives this one function rather than
    /// carrying a second copy of the rule.
    pub(crate) fn serve(
        &self,
        operation: &str,
        input: &RetrieveAndGenerateInput,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<RetrieveAndGenerateOutput, RetrieveAndGenerateError> {
        // The request is read in full BEFORE anything is looked up: a leg this service
        // cannot compute is refused whatever the stores hold, and that refusal depends on
        // the request alone, so it says nothing about which knowledge bases exist.
        let query = query_text(&input.input)?;
        if input.user_context.is_some() {
            return Err(refuse(
                "userContext: this service applies no per-document access control to a \
                 retrieval, so the passages a response cites cannot be narrowed to the \
                 documents one user may read; omit userContext to generate over every \
                 ingested document"
                    .to_string(),
            ));
        }
        let configuration = input
            .retrieve_and_generate_configuration
            .as_ref()
            .ok_or_else(|| {
                refuse(format!(
                    "{CONFIGURATION_MEMBER} is required: it names the resource the query is \
                     made to and the model the response is generated with"
                ))
            })?;
        let knowledge_base = knowledge_base_configuration(configuration)?;
        let asked = generation_configuration(knowledge_base)?;
        let search = retrieval::search_configuration(
            &format!("{KNOWLEDGE_BASE_MEMBER}.retrievalConfiguration"),
            knowledge_base.retrieval_configuration.as_ref(),
        )
        .map_err(refuse)?;
        // How big a filter may be is decided ONCE, before anything is ranked: past the
        // bounds the service evaluates, the request is refused rather than walked.
        if let Some(too_big) = retrieval::filter_bounds_refusal(&search) {
            return Err(refuse(too_big));
        }

        // Authorization is decided against the knowledge base the call queries — the
        // resource an identity-based policy for `bedrock:<operation>` names — and before
        // any store is read, so an unauthorized call is answered the same way whether or
        // not that knowledge base exists.
        let resource =
            retrieval::knowledge_base_resource_arn(ctx, &knowledge_base.knowledge_base_id);
        if let Err(denied) = crate::state::authorize(&*self.engine, ctx, operation, &resource) {
            return Err(RetrieveAndGenerateErrorModeled::AccessDeniedException(denied).into());
        }

        let store = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the session store is unavailable"))?;
        // The session the turn will be recorded in is settled before the work is done, so a
        // request naming a session this service never issued is refused rather than served
        // and then dropped.
        self.session_preflight(&store, input)?;

        let model = generation::resolve_model(
            &*self.engine,
            &format!("{KNOWLEDGE_BASE_MEMBER}.modelArn"),
            &knowledge_base.model_arn,
        )
        .map_err(|refusal| match refusal {
            generation::GenerationRefusal::StoreUnavailable => {
                internal("the model catalogue is unavailable")
            }
            generation::GenerationRefusal::InvalidModel(message) => refuse(message),
            generation::GenerationRefusal::ModelNotFound(message) => not_found(message),
        })?;

        // The retrieval itself: the chunks of the knowledge base's bound index nearest to
        // the query, narrowed by the filter and cut to the page the request asked for.
        // `knowledgeBaseId` is the bare id here — the modelled member admits no ARN
        // (`smithy_query(query="shape",shape="KnowledgeBaseRetrieveAndGenerateConfiguration")`).
        let retrieved = retrieval::nearest_chunks(
            &*self.engine,
            &knowledge_base.knowledge_base_id,
            &query,
            search.results,
            search.filter,
        )
        .map_err(|refusal| match refusal {
            retrieval::RetrievalRefusal::UnknownKnowledgeBase => not_found(
                retrieval::knowledge_base_not_found_message(&knowledge_base.knowledge_base_id),
            ),
            retrieval::RetrievalRefusal::StoreUnavailable => {
                internal("the knowledge base store is unavailable")
            }
            // The knowledge base's content does not live anywhere this plane can read it:
            // the store it is backed by is not the one this emulator serves, or the index it
            // was bound to is gone. Its DEPENDENCY failed, which is the modelled error for
            // exactly that, and never an empty success.
            retrieval::RetrievalRefusal::UnreachableVectorStore(resource)
            | retrieval::RetrievalRefusal::MissingVectorIndex(resource) => {
                RetrieveAndGenerateErrorModeled::DependencyFailedException(
                    DependencyFailedException {
                        message: Some(retrieval::unreachable_store_message(&resource)),
                        resource_name: Some(resource),
                    },
                )
                .into()
            }
        })?;

        let answer = generation::compose_answer(
            &model.model_id,
            retrieved,
            model.budget(asked.max_tokens),
            &asked.stop_sequences,
        );
        // The turn is recorded under the session that carries it, which is also where a
        // session is OPENED when the request named none: the id the response reports is
        // therefore always the id of a session this store holds.
        let session_id = self.record_turn(&store, input, &answer, &model, knowledge_base, query)?;

        Ok(RetrieveAndGenerateOutput {
            session_id,
            citations: Some(citations_of(&answer)),
            output: RetrieveAndGenerateOutput2 { text: answer.text },
            // No guardrail is evaluated on this path — a request naming one is refused
            // above — so there is no intervention, and no absence of one, to report.
            guardrail_action: None,
        })
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> RetrieveAndGenerate<Eng> {
    /// Everything about the session that can be refused before the query is served: that a
    /// named session is one this service issued, that the request does not re-key it, and
    /// that neither the store nor the session is at its ceiling.
    fn session_preflight(
        &self,
        store: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState,
        >,
        input: &RetrieveAndGenerateInput,
    ) -> Result<(), RetrieveAndGenerateError> {
        let guard = store.lock();
        let Some(session_id) = input.session_id.as_deref() else {
            // A request naming no session opens one, so what is bounded is the store.
            if guard.knowledge_base_sessions.len() >= conversations::MAX_KNOWLEDGE_BASE_SESSIONS {
                return Err(quota(conversations::session_quota_message(
                    conversations::MAX_KNOWLEDGE_BASE_SESSIONS,
                    "knowledge base sessions",
                )));
            }
            return Ok(());
        };
        let Some(session) = guard.knowledge_base_sessions.get(session_id) else {
            return Err(refuse(conversations::session_not_valid_message(session_id)));
        };
        if let Some(configuration) = input.session_configuration.as_ref() {
            if session.encryption_key_arn.as_deref() != Some(configuration.kms_key_arn.as_str()) {
                return Err(refuse(
                    conversations::session_configuration_conflict_message(session_id),
                ));
            }
        }
        if session.turns.len() >= conversations::MAX_TURNS_PER_KNOWLEDGE_BASE_SESSION {
            return Err(quota(conversations::session_quota_message(
                conversations::MAX_TURNS_PER_KNOWLEDGE_BASE_SESSION,
                "turns in one knowledge base session",
            )));
        }
        Ok(())
    }

    /// Record the served turn, opening the session when the request named none, and answer
    /// the `sessionId` the response reports.
    ///
    /// The lookup and the write happen under ONE hold of the store, so the id answered
    /// always addresses a session this store holds: a caller "can't explicitly set the
    /// sessionId", and an id this service never minted is refused here too rather than
    /// turned into a session of the caller's own naming.
    fn record_turn(
        &self,
        store: &::engine::ServiceState<
            crate::state::ComAmazonawsBedrockagentruntimeAmazonbedrockagentruntimeserviceState,
        >,
        input: &RetrieveAndGenerateInput,
        answer: &generation::GeneratedAnswer,
        model: &generation::GenerationModel,
        knowledge_base: &KnowledgeBaseRetrieveAndGenerateConfiguration,
        query: String,
    ) -> Result<String, RetrieveAndGenerateError> {
        let at = crate::state::now();
        let turn = conversations::KnowledgeBaseTurn {
            query,
            knowledge_base_id: knowledge_base.knowledge_base_id.clone(),
            model_id: model.model_id.clone(),
            text: answer.text.clone(),
            citations: answer.passages.len(),
            at,
        };
        let mut guard = store.lock();
        match input.session_id.as_deref() {
            Some(session_id) => match guard.knowledge_base_sessions.get_mut(session_id) {
                Some(session) => {
                    session.record(turn);
                    Ok(session_id.to_string())
                }
                None => Err(refuse(conversations::session_not_valid_message(session_id))),
            },
            None => {
                let session_id = crate::state::new_uuid();
                let mut session = conversations::KnowledgeBaseSession::opened(
                    session_id.clone(),
                    input
                        .session_configuration
                        .as_ref()
                        .map(|configuration| configuration.kms_key_arn.clone()),
                    at,
                );
                session.record(turn);
                guard
                    .knowledge_base_sessions
                    .insert(session_id.clone(), session);
                Ok(session_id)
            }
        }
    }
}

/// The modelled `citations` of one generated response: one per quoted passage, carrying the
/// span of the response that passage occupies and the retrieved reference it was quoted
/// from.
///
/// `span` is a pair of CHARACTER offsets into `output.text`: `start` the first character of
/// the cited segment and `end` the offset just past its last, so a client highlights the
/// segment with `text[start..end]` and its length is `end - start`. They are taken from the
/// composition that built the text ([`generation::compose_answer`]), so a span always indexes
/// the text it is reported beside.
fn citations_of(answer: &generation::GeneratedAnswer) -> Vec<Citation> {
    answer
        .passages
        .iter()
        .map(|passage| Citation {
            generated_response_part: Some(GeneratedResponsePart {
                text_response_part: Some(TextResponsePart {
                    text: Some(passage.text.clone()),
                    span: Some(Span {
                        start: Some(span_index(passage.start)),
                        end: Some(span_index(passage.end)),
                    }),
                }),
            }),
            retrieved_references: Some(vec![RetrievedReference {
                content: Some(RetrievalResultContent {
                    r#type: Some(retrieval::CONTENT_TYPE_TEXT.to_string()),
                    text: Some(passage.chunk.text.clone()),
                    ..RetrievalResultContent::default()
                }),
                // Recorded with the chunk by the ingestion that took its source in, so the
                // citation names the object a client following it can read back.
                location: Some(passage.chunk.location.clone()),
                // `RetrievalResultMetadata` is `@length(min: 1)`, so a chunk the ingestion
                // recorded nothing about is cited with the member absent rather than with an
                // empty map the model does not admit.
                metadata: Some(
                    passage
                        .chunk
                        .metadata
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
                .filter(|metadata: &::std::collections::HashMap<_, _>| !metadata.is_empty()),
            }]),
        })
        .collect()
}

/// One end of a span as the modelled `Span` carries it: an `Integer` with `range {min: 0}`.
///
/// A response long enough to overflow it cannot be composed — the generation budget is a
/// token count that `Integer` itself bounds — so the saturation below is the unreachable arm
/// of a total conversion.
fn span_index(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

/// This operation's modelled `ValidationException` (HTTP 400), naming the member that was
/// wrong and what it requires — the caller has to fix the request from this text alone.
fn refuse(message: String) -> RetrieveAndGenerateError {
    RetrieveAndGenerateErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
    .into()
}

/// This operation's modelled `ResourceNotFoundException` (HTTP 404).
fn not_found(message: String) -> RetrieveAndGenerateError {
    RetrieveAndGenerateErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
    .into()
}

/// This operation's modelled `InternalServerException` (HTTP 500): a store this emulator
/// should carry is not on the engine, which is never reported as a client fault.
fn internal(message: &str) -> RetrieveAndGenerateError {
    RetrieveAndGenerateErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.to_string()),
        reason: None,
    })
    .into()
}

/// This operation's modelled `ServiceQuotaExceededException` (HTTP 400), answered when a
/// resource guard of this emulator's store is reached.
fn quota(message: String) -> RetrieveAndGenerateError {
    RetrieveAndGenerateErrorModeled::ServiceQuotaExceededException(ServiceQuotaExceededException {
        message: Some(message),
    })
    .into()
}

/// The text the knowledge base is queried with: `input.text`, which the model requires.
///
/// A query of nothing but whitespace carries no query at all, and a retrieval ranked against
/// it would report the index's first chunks as if they answered something, so it is refused.
fn query_text(input: &RetrieveAndGenerateInput2) -> Result<String, RetrieveAndGenerateError> {
    if input.text.split_whitespace().next().is_none() {
        return Err(refuse(
            "input.text is required: it carries the query made to the knowledge base"
                .to_string(),
        ));
    }
    Ok(input.text.clone())
}

/// The knowledge base leg of the request's configuration.
///
/// `type` names which KIND of resource the query is made to, so a request naming
/// `EXTERNAL_SOURCES` is refused: an external source is a document supplied with the
/// request, which this plane has ingested nothing of — there is no chunk of it to rank and
/// nothing but the caller's own bytes to cite, so a response generated from it would report
/// a citation that resolves to nothing.
fn knowledge_base_configuration(
    configuration: &RetrieveAndGenerateConfiguration,
) -> Result<&KnowledgeBaseRetrieveAndGenerateConfiguration, RetrieveAndGenerateError> {
    if configuration.r#type != TYPE_KNOWLEDGE_BASE {
        return Err(refuse(format!(
            "{CONFIGURATION_MEMBER}.type: {:?} names no resource this service generates a \
             response from. A response is generated from the passages a knowledge base of \
             this emulator retrieved, so each one can be cited to the source it was ingested \
             from; supply {TYPE_KNOWLEDGE_BASE} with a knowledge base this emulator holds",
            configuration.r#type
        )));
    }
    if configuration.external_sources_configuration.is_some() {
        return Err(refuse(format!(
            "{CONFIGURATION_MEMBER} must carry only the configuration its type names: \
             externalSourcesConfiguration configures a query this service does not serve, \
             and this request carries it beside type {TYPE_KNOWLEDGE_BASE}"
        )));
    }
    configuration.knowledge_base_configuration.as_ref().ok_or_else(|| {
        refuse(format!(
            "{KNOWLEDGE_BASE_MEMBER} is required when {CONFIGURATION_MEMBER}.type is \
             {TYPE_KNOWLEDGE_BASE}: it names the knowledge base queried and the model the \
             response is generated with"
        ))
    })
}

/// What the request asks of the generation stage.
#[derive(Debug, Default)]
struct GenerationAsked {
    /// `inferenceConfig.textInferenceConfig.maxTokens`: the most tokens to allow in the
    /// generated response.
    max_tokens: Option<i32>,
    /// `inferenceConfig.textInferenceConfig.stopSequences`: the sequences whose first
    /// completed occurrence halts generation.
    stop_sequences: Vec<String>,
}

/// Read the request's `generationConfiguration` and `orchestrationConfiguration`.
///
/// What bounds the TEXT is honoured ([`generation::compose_answer`]); what asks a model to
/// read, rewrite or police the prompt is refused, because no model reads one here and a
/// response that ignored the member would answer something the caller did not ask for.
/// `temperature` and `topP` steer which tokens a model would pick, `additionalModelRequestFields`
/// are a model family's own such parameters and `performanceConfig` names a latency profile,
/// so none of them bounds the text that is served and none is refused.
fn generation_configuration(
    knowledge_base: &KnowledgeBaseRetrieveAndGenerateConfiguration,
) -> Result<GenerationAsked, RetrieveAndGenerateError> {
    if knowledge_base.orchestration_configuration.is_some() {
        return Err(refuse(format!(
            "{KNOWLEDGE_BASE_MEMBER}.orchestrationConfiguration: it configures how a model \
             processes the prompt before retrieval and generation, and no model processes \
             one in this emulator, so neither the queries it would decompose the prompt into \
             nor the prompt it would send can be computed; omit it to query the knowledge \
             base with the text of the request"
        )));
    }
    let Some(configuration) = knowledge_base.generation_configuration.as_ref() else {
        return Ok(GenerationAsked::default());
    };
    if configuration.prompt_template.is_some() {
        return Err(refuse(format!(
            "{KNOWLEDGE_BASE_MEMBER}.generationConfiguration.promptTemplate: it is the \
             prompt a model is asked to generate the response from, and no model is asked \
             anything in this emulator, so a template cannot change what the response says; \
             omit it to take the response this service composes from the retrieved passages"
        )));
    }
    if configuration.guardrail_configuration.is_some() {
        return Err(refuse(format!(
            "{KNOWLEDGE_BASE_MEMBER}.generationConfiguration.guardrailConfiguration: this \
             service evaluates no guardrail policy, so neither an intervention nor its \
             absence can be reported; omit it to generate without a guardrail"
        )));
    }
    let inference = configuration
        .inference_config
        .as_ref()
        .and_then(|inference| inference.text_inference_config.as_ref());
    Ok(GenerationAsked {
        max_tokens: inference.and_then(|inference| inference.max_tokens),
        stop_sequences: inference
            .and_then(|inference| inference.stop_sequences.clone())
            .unwrap_or_default(),
    })
}
