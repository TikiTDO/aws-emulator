// SPDX-License-Identifier: CC0-1.0

//! @op Retrieve
//!
//! ## What this handler does
//!
//! `Retrieve` answers the chunks of the content the knowledge base the URI names has
//! actually ingested, ranked by each chunk's relevance to the query text, highest first:
//!
//! * WHAT a chunk is, and where it came from, is decided by
//!   [`crate::state::knowledge_base_retrieval`]: it reads the vector INDEX the knowledge
//!   base is bound to, which the Bedrock Agent build-time plane's ingestion wrote — one
//!   vector per source document it took in, with the chunk's text, the chunk's id, the
//!   data source and the source's URI recorded beside it. A result's `location`
//!   therefore RESOLVES: the `s3://<bucket>/<key>` URI it reports is the one the
//!   ingestion read the chunk from, and `metadata` carries that chunk's own id, the id of
//!   the data source it came from and the modality of its source file. Nothing here reads
//!   an object store or re-derives a chunk, so a source deleted after it was ingested is
//!   still cited exactly as the ingestion took it in, until another ingestion job
//!   replaces what the index holds;
//! * a knowledge base the Bedrock Agent plane does not hold is the modelled
//!   `ResourceNotFoundException` (HTTP 404), not an empty success;
//! * WHICH chunks are reported is narrowed by the request's `filter`
//!   ([`crate::state::retrieval_filter`]) and bounded by `numberOfResults`;
//! * `score` is the chunk's relevance to the query, in `[0, 1]`, computed from the
//!   index's own nearest-neighbour geometry: the query's vector is built the way the
//!   ingestion built each document's and the distance is the one the index was created
//!   with. No embedding model runs inside this emulator and nothing here invents a score.
//!
//! This operation is `@readonly` and writes nothing a later caller can observe: the one
//! write on its path is the Bedrock Agent plane materialising the status transitions its
//! own clock had already made due, which is owed whether or not anybody calls.
//!
//! ## What it refuses rather than answers
//!
//! A leg this emulator cannot compute is REFUSED with the modelled `ValidationException`,
//! because a plausible-looking success is worse than an error: a retrieval that ignored
//! the member would report chunks the caller asked to have filtered, masked or withheld.
//! Those legs are an `IMAGE` query (no image embedding runs here), `userContext` (access
//! control filtering needs the per-document ACLs nothing in this emulator holds),
//! `guardrailConfiguration` (no guardrail policy is evaluated here, so no intervention
//! can be reported), `implicitFilterConfiguration` (the filter a model would have
//! generated from the query would only narrow the answer) and a `rerankingConfiguration`
//! (a reranking model REORDERS the results, and none runs here, so honouring only the
//! count it keeps would answer a success whose ordering is the one the caller asked to
//! have replaced).
//!
//! `overrideSearchType` IS served: `HYBRID` and `SEMANTIC` name the same ordering here,
//! because the vectors the ingestion wrote are term-frequency vectors and their
//! nearest-neighbour order already is the lexical one.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `Retrieve::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `Retrieve`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json")
//! ```
//!
//! Queries a knowledge base and retrieves information from it.
//!
//! ## Input
//!
//! * `guardrailConfiguration` (GuardrailConfiguration) — Guardrail settings.
//! * `knowledgeBaseId` (KnowledgeBaseIdentifier, len 10..2048, pattern
//!   ^[0-9a-zA-Z]{10}$|^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:knowledge-base/[0-9a-zA-Z]{10}$)
//!   **required** — The unique identifier of the knowledge base to query.
//! * `nextToken` (NextToken, len 1..2048, pattern ^\S*$) — If there are more results than can
//!   fit in the response, the response returns a nextToken. Use this token in the nextToken
//!   field of another request to retrieve the next batch of results.
//! * `retrievalConfiguration` (KnowledgeBaseRetrievalConfiguration) — Contains configurations
//!   for the knowledge base query and retrieval process. For more information, see Query
//!   configurations.
//! * `retrievalQuery` (KnowledgeBaseQuery) **required** — Contains the query to send the
//!   knowledge base.
//! * `userContext` (UserContext) — Contains information about the user making the request.
//!   This is used for access control filtering to ensure that retrieval results only include
//!   documents the user is authorized to access.
//!
//! ## Output
//!
//! * `guardrailAction` (GuadrailAction, one of: INTERVENED | NONE) — Specifies if there is a
//!   guardrail intervention in the response.
//! * `nextToken` (NextToken, len 1..2048, pattern ^\S*$) — If there are more results than can
//!   fit in the response, the response returns a nextToken. Use this token in the nextToken
//!   field of another request to retrieve the next batch of results.
//! * `retrievalResults` (KnowledgeBaseRetrievalResults) **required** — A list of results from
//!   querying the knowledge base.
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

use crate::state::knowledge_base_retrieval as retrieval;

/// The `KnowledgeBaseQueryType` whose query travels in `retrievalQuery.text`, and the
/// one the model defaults to (`KNOWLEDGE_BASE_QUERY_TYPE_VALUES`).
const QUERY_TYPE_TEXT: &str = "TEXT";

/// How many results a retrieval reports when the request names no `numberOfResults`: the
/// `default` the model puts on that member
/// (`smithy_query(query="shape",shape="KnowledgeBaseVectorSearchConfiguration")`).
const DEFAULT_NUMBER_OF_RESULTS: usize = 5;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RetrieveError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RetrieveError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#Retrieve")]
#[derive(Debug)]
pub enum RetrieveErrorModeled {
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
/// (`crate::ops::retrieve::RetrieveError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RetrieveError = crate::scaffolded::Scaffolded<RetrieveErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RetrieveErrorModeled> for RetrieveError {
    fn from(modeled: RetrieveErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Retrieve` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Retrieve<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Retrieve<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Retrieve<Eng>
{
    type Input = RetrieveInput;
    type Output = RetrieveOutput;
    type Error = RetrieveError;

    const NAME: &'static str = "Retrieve";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RETRIEVE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::RETRIEVE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The request is read in full BEFORE anything is looked up: a leg this service
        // cannot compute is refused whatever the stores hold, and that refusal depends on
        // the request alone, so it says nothing about which knowledge bases exist.
        let query = query_text(&input.retrieval_query)?;
        if input.user_context.is_some() {
            return Err(refuse(
                "userContext: this service applies no per-document access control to a \
                 retrieval, so a retrieval for one user cannot be narrowed to the \
                 documents that user may read; omit userContext to retrieve over every \
                 ingested document"
                    .to_string(),
            ));
        }
        if input.guardrail_configuration.is_some() {
            return Err(refuse(
                "guardrailConfiguration: this service evaluates no guardrail policy on a \
                 retrieval, so neither an intervention nor its absence can be reported; \
                 omit guardrailConfiguration to retrieve without a guardrail"
                    .to_string(),
            ));
        }
        // `nextToken` names a page of a response this service never splits: a retrieval
        // reports at most `numberOfResults` results, which the model bounds at 100, so
        // the whole answer always fits one response and NO call issues a token. A token
        // reaching here cannot have come from this service.
        if input.next_token.is_some() {
            return Err(refuse(
                "nextToken is not a pagination token this service issued: a Retrieve \
                 response carries every result of the retrieval it was asked for"
                    .to_string(),
            ));
        }
        let search = search_configuration(input.retrieval_configuration.as_ref())?;
        // How big a filter may be is decided ONCE, before anything is ranked — how deeply
        // it nests AND how many nodes it holds, because the filter is evaluated once per
        // chunk and the model bounds neither. Past either bound the request is refused
        // rather than evaluated, so no caller can drive the walk below without end.
        if let Some(filter) = search.filter {
            use crate::state::retrieval_filter as filters;

            if let Err(too_big) = filters::within_bounds(filter) {
                return Err(refuse(match too_big {
                    filters::FilterTooBig::Depth => format!(
                        "{}.filter nests andAll/orAll more than {} levels deep",
                        search.member,
                        filters::MAX_FILTER_DEPTH
                    ),
                    filters::FilterTooBig::Nodes => format!(
                        "{}.filter holds more than the {} comparisons this service \
                         evaluates",
                        search.member,
                        filters::MAX_FILTER_NODES
                    ),
                }));
            }
        }

        // Authorization is decided against the knowledge base the call retrieves from —
        // the resource an identity-based policy for `bedrock:Retrieve` names — and before
        // the store is read, so an unauthorized call is answered the same way whether or
        // not that knowledge base exists.
        let resource = retrieval::knowledge_base_resource_arn(ctx, &input.knowledge_base_id);
        if let Err(denied) = crate::state::authorize(&*self.engine, ctx, Self::NAME, &resource) {
            return Err(RetrieveErrorModeled::AccessDeniedException(denied).into());
        }

        // The retrieval itself: the chunks of the knowledge base's bound index nearest to
        // the query, narrowed by the filter and cut to the page the request asked for. An
        // identifier that names a knowledge base of another account, region or partition
        // names none this call can read, so it is answered not-found rather than reduced to
        // a bare id that would resolve in the caller's own place.
        let knowledge_base_id = retrieval::knowledge_base_id_from_identifier(
            &input.knowledge_base_id,
            &crate::state::request_region(ctx),
        );
        let retrieved = match knowledge_base_id {
            Some(knowledge_base_id) => retrieval::nearest_chunks(
                &*self.engine,
                knowledge_base_id,
                &query,
                search.results,
                search.filter,
            ),
            None => Err(retrieval::RetrievalRefusal::UnknownKnowledgeBase),
        };
        let retrieved = match retrieved {
            Ok(retrieved) => retrieved,
            Err(retrieval::RetrievalRefusal::UnknownKnowledgeBase) => {
                return Err(RetrieveErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(retrieval::knowledge_base_not_found_message(
                            &input.knowledge_base_id,
                        )),
                    },
                )
                .into())
            }
            Err(retrieval::RetrievalRefusal::StoreUnavailable) => {
                return Err(RetrieveErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some("the knowledge base store is unavailable".to_string()),
                        reason: None,
                    },
                )
                .into())
            }
            // The knowledge base's content does not live anywhere this plane can read it:
            // the store it is backed by is not the one this emulator serves, or the index
            // it was bound to is gone. Its DEPENDENCY failed, which is the modelled error
            // for exactly that ("There was an issue with a dependency. Check the resource
            // configurations and retry the request."), and never an empty success.
            Err(retrieval::RetrievalRefusal::UnreachableVectorStore(resource))
            | Err(retrieval::RetrievalRefusal::MissingVectorIndex(resource)) => {
                return Err(RetrieveErrorModeled::DependencyFailedException(
                    DependencyFailedException {
                        message: Some(retrieval::unreachable_store_message(&resource)),
                        resource_name: Some(resource),
                    },
                )
                .into())
            }
        };

        Ok(RetrieveOutput {
            retrieval_results: retrieved
                .into_iter()
                .map(|chunk| KnowledgeBaseRetrievalResult {
                    content: RetrievalResultContent {
                        r#type: Some(retrieval::CONTENT_TYPE_TEXT.to_string()),
                        text: Some(chunk.text),
                        ..RetrievalResultContent::default()
                    },
                    // Recorded with the chunk by the ingestion that took its source in, so
                    // it names the object a client following the citation can read back.
                    location: Some(chunk.location),
                    score: Some(chunk.score),
                    metadata: Some(chunk.metadata.into_iter().collect()),
                    // `documentId` is optional and nothing in this emulator's state gives
                    // a chunk one beside the location that addresses its source.
                    document_id: None,
                })
                .collect(),
            // No guardrail is evaluated on this path — a request naming one is refused
            // above — so there is no intervention, and no absence of one, to report.
            guardrail_action: None,
            // See the `nextToken` refusal above: the whole retrieval is in this response.
            next_token: None,
        })
    }
}

/// This operation's modelled `ValidationException` (HTTP 400), naming the member that was
/// wrong and what it requires — the caller has to fix the request from this text alone.
fn refuse(message: String) -> RetrieveError {
    RetrieveErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
    .into()
}

/// The text the query is ranked against.
///
/// `retrievalQuery.text` carries it for the `TEXT` query the model defaults to. An
/// `IMAGE` query is refused: ranking stored text against an image needs a multimodal
/// embedding model, and none runs inside this emulator — so there is no chunk it could
/// honestly report as matching one.
fn query_text(query: &KnowledgeBaseQuery) -> Result<String, RetrieveError> {
    if !query.r#type.is_empty() && query.r#type != QUERY_TYPE_TEXT {
        return Err(refuse(format!(
            "retrievalQuery.type: {:?} is not a query type this service ranks a knowledge \
             base against. Supported types: {QUERY_TYPE_TEXT}",
            query.r#type
        )));
    }
    if query.image.is_some() {
        return Err(refuse(format!(
            "retrievalQuery.image: this service ranks a knowledge base against the text of \
             a {QUERY_TYPE_TEXT} query, so an image carries no query it can rank"
        )));
    }
    let text = query.text.as_deref().unwrap_or_default();
    if text.split_whitespace().next().is_none() {
        return Err(refuse(
            "retrievalQuery.text is required: it carries the text the knowledge base is \
             queried with"
                .to_string(),
        ));
    }
    Ok(text.to_string())
}

/// What the request asks of the search: how many results it may carry, and the filter
/// every reported chunk has to satisfy.
#[derive(Debug)]
struct SearchConfiguration<'request> {
    /// The most results the response carries.
    results: usize,
    /// The filter a chunk's metadata has to satisfy, when the request carries one.
    filter: Option<&'request RetrievalFilter>,
    /// The member path the filter travelled in, so a refusal about it names the member
    /// the request actually carried.
    member: &'static str,
}

/// The `vectorSearchConfiguration` member path, which a vector knowledge base's retrieval
/// is configured through.
const VECTOR_SEARCH_MEMBER: &str = "retrievalConfiguration.vectorSearchConfiguration";

/// The `managedSearchConfiguration` member path, which a service-managed knowledge
/// base's retrieval is configured through.
const MANAGED_SEARCH_MEMBER: &str = "retrievalConfiguration.managedSearchConfiguration";

/// Read the request's `retrievalConfiguration`.
///
/// The two sibling configurations are the two KINDS of knowledge base this operation
/// serves, so a request naming both names no one search; a request naming neither takes
/// the model's own default page of results.
fn search_configuration(
    configuration: Option<&KnowledgeBaseRetrievalConfiguration>,
) -> Result<SearchConfiguration<'_>, RetrieveError> {
    let Some(configuration) = configuration else {
        return Ok(SearchConfiguration {
            results: DEFAULT_NUMBER_OF_RESULTS,
            filter: None,
            member: VECTOR_SEARCH_MEMBER,
        });
    };
    match (
        configuration.vector_search_configuration.as_ref(),
        configuration.managed_search_configuration.as_ref(),
    ) {
        (Some(_), Some(_)) => Err(refuse(
            "retrievalConfiguration must carry exactly one of vectorSearchConfiguration \
             and managedSearchConfiguration: they configure the retrieval of the two \
             different kinds of knowledge base, and this request carries both"
                .to_string(),
        )),
        (Some(vector), None) => {
            if vector.implicit_filter_configuration.is_some() {
                return Err(refuse(format!(
                    "{VECTOR_SEARCH_MEMBER}.implicitFilterConfiguration: this service \
                     generates no metadata filter from the query text, so a retrieval \
                     cannot be narrowed the way an implicit filter would narrow it; \
                     supply {VECTOR_SEARCH_MEMBER}.filter to filter on metadata"
                )));
            }
            if vector.reranking_configuration.is_some() {
                return Err(refuse(reranking_refusal(VECTOR_SEARCH_MEMBER)));
            }
            Ok(SearchConfiguration {
                results: results_bound(vector.number_of_results),
                filter: vector.filter.as_ref(),
                member: VECTOR_SEARCH_MEMBER,
            })
        }
        (None, Some(managed)) => {
            if managed.reranking_configuration.is_some() {
                return Err(refuse(reranking_refusal(MANAGED_SEARCH_MEMBER)));
            }
            Ok(SearchConfiguration {
                results: results_bound(managed.number_of_results),
                filter: managed.filter.as_ref(),
                member: MANAGED_SEARCH_MEMBER,
            })
        }
        (None, None) => Ok(SearchConfiguration {
            results: DEFAULT_NUMBER_OF_RESULTS,
            filter: None,
            member: VECTOR_SEARCH_MEMBER,
        }),
    }
}

/// The `ValidationException` message a `rerankingConfiguration` earns: a reranking model
/// REORDERS the results a retrieval would otherwise report, and no such model runs in
/// this process — so honouring only the count it keeps would answer a success whose
/// ordering is the one the caller asked to have replaced.
fn reranking_refusal(member: &str) -> String {
    format!(
        "{member}.rerankingConfiguration: this service runs no reranking model, so the \
         order a reranker would put the results in cannot be computed; omit \
         {member}.rerankingConfiguration to take the retrieval's own order"
    )
}

/// The most results one response carries: `numberOfResults`, or the model's default when
/// the request names none.
///
/// A value outside the modelled `range 1..100` never reaches here — the boundary validator
/// refuses it before the handler runs — so the filter below is the unreachable arm of a
/// total function rather than a page size quietly substituted for one the caller asked
/// for.
fn results_bound(number_of_results: Option<i32>) -> usize {
    number_of_results
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(DEFAULT_NUMBER_OF_RESULTS)
}
