// SPDX-License-Identifier: CC0-1.0

//! What a knowledge-base retrieval reads: the chunks the knowledge base's ingestion
//! recorded, and the provenance recorded with each of them.
//!
//! `Retrieve` RANKS and REPORTS chunks and `RetrieveAndGenerate` CITES them; this module is
//! where they come from, and where what a request asks of the search is read
//! ([`search_configuration`]), so the provenance of every retrieval result and the meaning
//! of every `KnowledgeBaseRetrievalConfiguration` are settled in one place for both.
//!
//! A knowledge base's content is written by the Bedrock Agent build-time plane: one vector
//! per document it takes in — an ingestion job for a document it crawls from the data
//! source's own source, and `IngestKnowledgeBaseDocuments` for one pushed straight in —
//! with the fields Amazon Bedrock records beside it: the chunk's text, the chunk's id, the
//! data source it came from and how the document is addressed — where its source lives for
//! a document read from an object store, and the id it was ingested under for a document of
//! a custom data source
//! (`service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda`'s
//! `state::ingestion_index`, the ONE place those field names are decided). This plane
//! writes none of it, and reads it where the knowledge base keeps it ([`nearest_chunks`]):
//! in the S3 Vectors index a knowledge base bound to one holds its vectors in, whose data
//! plane this emulator serves, and otherwise in the ingestion's own record of what it took
//! in, because no API here writes the store such a knowledge base names. That is what makes
//! a result's provenance real:
//!
//! * a chunk exists only because an ingestion took its source in, so a knowledge base
//!   that has ingested nothing retrieves nothing;
//! * the text and the citation are the ones that ingestion recorded, so deleting or
//!   rewriting the source afterwards does not change what a retrieval cites — the citation
//!   keeps pointing at the object the chunk came from until another ingestion replaces
//!   what the store holds, whichever of the two stores answered it;
//! * the RANKING is a nearest-neighbour order: the query's vector is computed the way the
//!   ingestion computed each document's (`agent_state::document_vector`, a term-frequency
//!   vector in the store's dimension) and the distance is the store's own
//!   (`vectors_state::vector_distance`), so a retrieval is a real vector search rather
//!   than a second notion of relevance.
//!
//! A vector a caller wrote into a bound index themselves carries none of those fields: it
//! is not a chunk of this knowledge base, so no retrieval reports it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::aws_smithy_types::Document;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;
use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::ingestion_index as index_fields;
use ::service_com_amazonaws_s3vectors_s3vectors::state as vectors_state;

use crate::ops::types::{
    KnowledgeBaseRetrievalConfiguration, RetrievalFilter, RetrievalResultCustomDocumentLocation,
    RetrievalResultLocation, RetrievalResultS3Location,
};

/// The `RetrievalResultLocationType` an ingested S3 object is reported under
/// (`smithy_query(query="shape",shape="RetrievalResultLocationType")`).
pub const LOCATION_TYPE_S3: &str = "S3";

/// The `RetrievalResultLocationType` a document of a custom data source is reported
/// under: one pushed straight into the knowledge base through the build-time plane's
/// document API, which is addressed by its own id rather than by a source it was read
/// from. See [`LOCATION_TYPE_S3`].
pub const LOCATION_TYPE_CUSTOM: &str = "CUSTOM";

/// How many results a retrieval reports when the request names no `numberOfResults`: the
/// `default` the model puts on that member
/// (`smithy_query(query="shape",shape="KnowledgeBaseVectorSearchConfiguration")`).
pub const DEFAULT_NUMBER_OF_RESULTS: usize = 5;

/// What a request asks of the search: how many results it may carry, and the filter every
/// reported chunk has to satisfy.
///
/// Read by every operation of this service that queries a knowledge base, so `Retrieve` and
/// `RetrieveAndGenerate` cannot disagree about what a `KnowledgeBaseRetrievalConfiguration`
/// asks for — the ONE difference between them is the member PATH the configuration travelled
/// in, which each passes in so a refusal names the member the request actually carried.
#[derive(Debug)]
pub struct SearchConfiguration<'request> {
    /// The most results the search reports.
    pub results: usize,
    /// The filter a chunk's metadata has to satisfy, when the request carries one.
    pub filter: Option<&'request RetrievalFilter>,
    /// The member path the filter travelled in.
    pub member: String,
}

/// Read a request's `retrievalConfiguration`, which travelled at `member` (the member path
/// of the configuration itself, without a trailing dot).
///
/// The two sibling configurations are the two KINDS of knowledge base a retrieval serves, so
/// a request naming both names no one search; a request naming neither takes the model's own
/// default page of results.
///
/// `Err` carries the sentence the caller's modelled `ValidationException` states: a leg this
/// service cannot compute is REFUSED rather than ignored, because a retrieval that dropped
/// the member would report chunks the caller asked to have filtered, reordered or withheld.
pub fn search_configuration<'request>(
    member: &str,
    configuration: Option<&'request KnowledgeBaseRetrievalConfiguration>,
) -> Result<SearchConfiguration<'request>, String> {
    let vector_member = format!("{member}.vectorSearchConfiguration");
    let managed_member = format!("{member}.managedSearchConfiguration");
    let Some(configuration) = configuration else {
        return Ok(SearchConfiguration {
            results: DEFAULT_NUMBER_OF_RESULTS,
            filter: None,
            member: vector_member,
        });
    };
    match (
        configuration.vector_search_configuration.as_ref(),
        configuration.managed_search_configuration.as_ref(),
    ) {
        (Some(_), Some(_)) => Err(format!(
            "{member} must carry exactly one of vectorSearchConfiguration and \
             managedSearchConfiguration: they configure the retrieval of the two different \
             kinds of knowledge base, and this request carries both"
        )),
        (Some(vector), None) => {
            if vector.implicit_filter_configuration.is_some() {
                return Err(format!(
                    "{vector_member}.implicitFilterConfiguration: this service generates no \
                     metadata filter from the query text, so a retrieval cannot be narrowed \
                     the way an implicit filter would narrow it; supply \
                     {vector_member}.filter to filter on metadata"
                ));
            }
            if vector.reranking_configuration.is_some() {
                return Err(reranking_refusal(&vector_member));
            }
            Ok(SearchConfiguration {
                results: results_bound(vector.number_of_results),
                filter: vector.filter.as_ref(),
                member: vector_member,
            })
        }
        (None, Some(managed)) => {
            if managed.reranking_configuration.is_some() {
                return Err(reranking_refusal(&managed_member));
            }
            Ok(SearchConfiguration {
                results: results_bound(managed.number_of_results),
                filter: managed.filter.as_ref(),
                member: managed_member,
            })
        }
        (None, None) => Ok(SearchConfiguration {
            results: DEFAULT_NUMBER_OF_RESULTS,
            filter: None,
            member: vector_member,
        }),
    }
}

/// The sentence a `rerankingConfiguration` earns: a reranking model REORDERS the results a
/// retrieval would otherwise report, and no such model runs in this process — so honouring
/// only the count it keeps would answer a success whose ordering is the one the caller asked
/// to have replaced.
fn reranking_refusal(member: &str) -> String {
    format!(
        "{member}.rerankingConfiguration: this service runs no reranking model, so the order \
         a reranker would put the results in cannot be computed; omit \
         {member}.rerankingConfiguration to take the retrieval's own order"
    )
}

/// The most results one search reports: `numberOfResults`, or the model's default when the
/// request names none.
///
/// A value outside the modelled `range 1..100` never reaches here — the boundary validator
/// refuses it before a handler runs — so the filter below is the unreachable arm of a total
/// function rather than a page size quietly substituted for one the caller asked for.
fn results_bound(number_of_results: Option<i32>) -> usize {
    number_of_results
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(DEFAULT_NUMBER_OF_RESULTS)
}

/// How big a filter may be is decided BEFORE anything is ranked — how deeply it nests AND
/// how many nodes it holds, because the filter is evaluated once per chunk and the model
/// bounds neither. Past either bound the request is refused rather than evaluated, so no
/// caller can drive the filter walk without end.
///
/// `Some` carries the sentence the caller's modelled `ValidationException` states.
pub fn filter_bounds_refusal(search: &SearchConfiguration<'_>) -> Option<String> {
    use crate::state::retrieval_filter as filters;

    let filter = search.filter?;
    match filters::within_bounds(filter) {
        Ok(()) => None,
        Err(filters::FilterTooBig::Depth) => Some(format!(
            "{}.filter nests andAll/orAll more than {} levels deep",
            search.member,
            filters::MAX_FILTER_DEPTH
        )),
        Err(filters::FilterTooBig::Nodes) => Some(format!(
            "{}.filter holds more than the {} comparisons this service evaluates",
            search.member,
            filters::MAX_FILTER_NODES
        )),
    }
}

/// The `RetrievalResultContentType` of a chunk of text
/// (`smithy_query(query="shape",shape="RetrievalResultContentType")`).
pub const CONTENT_TYPE_TEXT: &str = "TEXT";

/// Why a retrieval cannot be served at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalRefusal {
    /// The store a retrieval reads is not on this engine, so nothing can be said about
    /// the knowledge base — answered with a modelled server error, never with an empty
    /// result list.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no knowledge base of that identifier.
    UnknownKnowledgeBase,
    /// The knowledge base names no vector store at all, or names one whose index this
    /// plane cannot resolve from the configuration — so its content lives nowhere a
    /// retrieval can read it. The string is what the knowledge base names, for the refusal
    /// to report as its `resourceName`.
    ///
    /// A refusal rather than an empty success: a knowledge base whose content lives
    /// somewhere this emulator cannot reach has not "ingested nothing", and a caller
    /// must be able to tell the two apart.
    UnreachableVectorStore(String),
    /// The knowledge base is bound to an S3 Vectors index the vector store no longer
    /// holds, so the dependency its content lived in is gone. The string is that index,
    /// for the refusal to report as its `resourceName`.
    MissingVectorIndex(String),
}

/// One chunk a retrieval reports: the text the ingestion recorded for it, where its
/// source lives, the metadata recorded with it, and how relevant it is to the query.
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    /// The relevance of the chunk to the query, in `[0, 1]`, highest first.
    pub score: f64,
    /// The chunk's text, exactly as the ingestion recorded it.
    pub text: String,
    /// Where the chunk's source lives.
    pub location: RetrievalResultLocation,
    /// The metadata of the result: the fields the ingestion recorded about the chunk,
    /// without the two the response carries elsewhere (the chunk's text, which is the
    /// content, and its source URI, which is the location).
    pub metadata: BTreeMap<String, Document>,
}

/// The knowledge base a `knowledgeBaseId` input names, for a request signed for `region`.
///
/// The modelled `KnowledgeBaseIdentifier` admits two forms: a bare 10-character id, which
/// names the knowledge base of that id in the caller's own account and region, and a
/// knowledge-base ARN. An ARN identifies ONE resource in ONE place, so it names a knowledge
/// base this call can read only when its partition, its service, its region and its account
/// are the ones this emulator would have minted for the caller: `None` for an ARN of
/// another partition, another region, another account or another service, so such a value
/// is never reduced to a bare id that resolves in the caller's place. The caller's
/// authorization was decided against the resource the ARN names
/// ([`knowledge_base_resource_arn`]), and this is what keeps the read keyed to the same
/// place that decision was made for.
///
/// Read through the ONE ARN parse this tree has (`service_common::arn`), against an ARN
/// this emulator itself mints, so nothing here spells an ARN form of its own.
pub fn knowledge_base_id_from_identifier<'identifier>(
    identifier: &'identifier str,
    region: &str,
) -> Option<&'identifier str> {
    if !identifier.starts_with("arn:") {
        return Some(identifier);
    }
    let named = ::service_common::arn::parse(identifier)?;
    let own = ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        region,
        &format!("{KNOWLEDGE_BASE_RESOURCE_TYPE}/own"),
    );
    let own = ::service_common::arn::parse(&own)?;
    if named.partition != own.partition
        || named.vendor != own.vendor
        || named.region != own.region
        || named.account != own.account
    {
        return None;
    }
    ::service_common::arn::resource_id_of_type(identifier, KNOWLEDGE_BASE_RESOURCE_TYPE)
        .filter(|id| !id.is_empty())
}

/// The resource type a knowledge base's ARN addresses.
const KNOWLEDGE_BASE_RESOURCE_TYPE: &str = "knowledge-base";

/// The `ResourceNotFoundException` message a retrieval answers when the Bedrock Agent
/// plane holds no knowledge base the request's identifier names. The identifier the
/// request carried fills the one variable part; the rest is fixed text for this error
/// class.
///
/// The RUNTIME plane's own wording, which is not the build-time plane's sentence for the
/// same resource (`agent_state::knowledge_base_not_found_message`, "KnowledgeBase with id
/// <id> is not found."): the two planes refuse in their own words. This is the sentence
/// the recorded refusals of THIS plane carry, and every operation of it that resolves a
/// knowledge base states it, so a client matching the rule matches one sentence.
pub fn knowledge_base_not_found_message(identifier: &str) -> String {
    format!("Knowledge Base with id {identifier} does not exist")
}

/// The resource an identity-based policy for a knowledge-base operation of this plane
/// names: the knowledge base the call reads.
///
/// An identifier that is already a knowledge-base ARN IS that resource; a bare id names
/// the knowledge base of that id in the region the caller signed for and this emulator's
/// account — the same ARN the Bedrock Agent plane mints for it. Built without a store
/// lookup, so a decision made with it is made before, and independently of, whether the
/// knowledge base exists. Shared by every operation of this service addressed at a
/// knowledge base, so they all authorize against the same resource.
pub fn knowledge_base_resource_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    identifier: &str,
) -> String {
    if identifier.starts_with("arn:") {
        return identifier.to_string();
    }
    ::engine::emulator_arn(
        crate::state::ARN_NAMESPACE,
        &crate::state::request_region(ctx),
        &format!("knowledge-base/{identifier}"),
    )
}

/// Where the chunks of one knowledge base are read from.
#[derive(Debug, Clone)]
enum ChunkStore {
    /// The S3 Vectors index the knowledge base is bound to, whose data plane this
    /// emulator serves: the chunks ARE the vectors that index holds, so they are read
    /// there and the index's own metric orders them.
    S3Vectors(agent_state::S3VectorsBinding),
    /// The knowledge base's own ingestion record, for a vector store no service of this
    /// emulator serves: nothing here writes that store, so what the knowledge base holds
    /// is what its ingestion took in (`agent_state::recorded_chunks`). The number is the
    /// dimension that record's vectors carry.
    Ingestion(i32),
}

/// The chunks of `knowledge_base_id` nearest to `query`, most relevant first, at most
/// `keep` of them, and only those whose own metadata `filter` admits.
///
/// READ-ONLY for everything a later caller can observe. The one write on its path is on
/// the Bedrock Agent plane's own clock-owed transitions (`materialise_due_transitions`):
/// the status an ingestion job, a data source or a knowledge base was owed at this
/// instant whether or not anybody called, which is the same write every reporting
/// operation of that plane performs.
///
/// A knowledge base bound to an S3 Vectors index that store no longer holds is REFUSED
/// rather than answered with an empty success: its content does not live anywhere this
/// plane can read, which is not the same as a knowledge base that has ingested nothing.
pub fn nearest_chunks<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    query: &str,
    keep: usize,
    filter: Option<&RetrievalFilter>,
) -> Result<Vec<RetrievedChunk>, RetrievalRefusal> {
    let Some(agent) = agent_state::service_state(engine) else {
        return Err(RetrievalRefusal::StoreUnavailable);
    };
    // The knowledge base and the store that holds its chunks are read under the Bedrock
    // Agent plane's lock; every other store is reached with that lock released, the lock
    // order every cross-plane path of these two services keeps.
    let store = {
        let mut agent = agent.lock();
        agent.materialise_due_transitions(SystemTime::now());
        let Some(knowledge_base) = agent.live_knowledge_base(knowledge_base_id) else {
            return Err(RetrievalRefusal::UnknownKnowledgeBase);
        };
        chunk_store(knowledge_base)
    }?;
    match store {
        ChunkStore::S3Vectors(binding) => {
            nearest_index_chunks(engine, &binding, query, keep, filter)
        }
        ChunkStore::Ingestion(dimension) => Ok(nearest_recorded_chunks(
            engine,
            knowledge_base_id,
            dimension,
            query,
            keep,
            filter,
        )),
    }
}

/// Where `knowledge_base`'s chunks are read from, by the store its own
/// `storageConfiguration` names.
///
/// The one store whose data plane this emulator serves is the S3 Vectors index an
/// ingestion writes through to, so a knowledge base that names it is read THERE, and an
/// index this plane cannot resolve from the configuration is a dependency that failed. A
/// knowledge base that names any other store keeps its vectors somewhere no API here
/// reaches, so what it holds is read from the ingestion record of the plane that took it
/// in; a knowledge base that names no vector store at all holds its content nowhere this
/// plane can read, which is refused rather than answered empty.
fn chunk_store(
    knowledge_base: &agent_state::KnowledgeBaseRecord,
) -> Result<ChunkStore, RetrievalRefusal> {
    let Some(storage) = knowledge_base.storage_configuration.as_ref() else {
        return Err(RetrievalRefusal::UnreachableVectorStore(
            "a knowledge base with no vector store".to_string(),
        ));
    };
    if storage.r#type != agent_state::S3_VECTORS_STORAGE_TYPE {
        return Ok(ChunkStore::Ingestion(agent_state::record_dimension(
            knowledge_base,
        )));
    }
    agent_state::s3_vectors_binding(Some(storage))
        .map(ChunkStore::S3Vectors)
        // The store the knowledge base DOES name, so the refusal reports what it is
        // bound to rather than only that this plane cannot read it.
        .ok_or_else(|| RetrievalRefusal::UnreachableVectorStore(storage.r#type.clone()))
}

/// The page of chunks the S3 Vectors index `binding` names answers for `query`.
fn nearest_index_chunks<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &agent_state::S3VectorsBinding,
    query: &str,
    keep: usize,
    filter: Option<&RetrievalFilter>,
) -> Result<Vec<RetrievedChunk>, RetrievalRefusal> {
    let Some(vectors) = vectors_state::service_state(engine) else {
        return Err(RetrievalRefusal::StoreUnavailable);
    };

    // PHASE ONE, under the vector store's guard: how far each stored vector is from the
    // query, and nothing else. No filter is evaluated and no chunk is copied here, so what
    // the guard is held for is one pass of the index's own distance function — work the
    // index's size bounds and no request can grow — rather than anything the request
    // chose.
    let (metric, ranked) = {
        let store = vectors.lock();
        let Ok(index) = store.index(&binding.vector_bucket, &binding.index) else {
            return Err(RetrievalRefusal::MissingVectorIndex(format!(
                "{}/{}",
                binding.vector_bucket, binding.index
            )));
        };
        // The query's vector, computed the way the ingestion computed each document's, so
        // the two are comparable under the index's own metric.
        let query_vector = agent_state::document_vector(query, index.dimension);
        let ranked: Vec<(f32, String)> = index
            .vectors
            .values()
            .map(|stored| {
                (
                    vectors_state::vector_distance(
                        &index.distance_metric,
                        &query_vector,
                        &stored.data,
                    ),
                    stored.key.clone(),
                )
            })
            .collect();
        (index.distance_metric.clone(), ranked)
    };

    // PHASE TWO, with the guard RELEASED between candidates: the chunks are read in
    // nearest-first order, so what is copied is the page the request asked for and not the
    // whole index, and a filter's own work never holds a store every other caller shares.
    Ok(page_of_chunks(ranked, &metric, keep, filter, |key| {
        read_chunk(&vectors, binding, key)
    }))
}

/// The page of chunks the knowledge base's own ingestion record answers for `query`.
///
/// The record holds the chunk each ingestion read, with the vector it computed for it, so
/// the ranking is the geometry the bound index would be read under: both vectors are the
/// term-frequency vectors the ingestion computes, in the dimension the knowledge base's
/// chunks carry, and the distance is the cosine one — the metric a knowledge base's
/// vectors are compared under.
///
/// Two phases, as for the index: the distances first, with nothing copied, and then one
/// chunk read per candidate until the page is full.
fn nearest_recorded_chunks<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    dimension: i32,
    query: &str,
    keep: usize,
    filter: Option<&RetrievalFilter>,
) -> Vec<RetrievedChunk> {
    let query_vector = agent_state::document_vector(query, dimension);
    let ranked = agent_state::ranked_chunks(engine, knowledge_base_id, &query_vector);
    page_of_chunks(
        ranked,
        vectors_state::DISTANCE_METRIC_COSINE,
        keep,
        filter,
        |chunk| {
            agent_state::recorded_chunk_metadata(engine, knowledge_base_id, chunk)
                .as_ref()
                .and_then(chunk_of)
        },
    )
}

/// The page a retrieval reports, from candidates each carrying its distance from the
/// query: nearest first, only the chunks `filter` admits, and at most `keep` of them.
///
/// The ONE place the ordering, the filter and the page bound are applied, so the two
/// stores a knowledge base's chunks can live in cannot disagree about any of the three.
/// The sort is STABLE and the candidates arrive in key order, so candidates at the same
/// distance keep that order and one request always answers the same way. The walk stops
/// as soon as the page is full, and `chunk` is what reads one candidate's chunk.
fn page_of_chunks<Candidate>(
    mut ranked: Vec<(f32, Candidate)>,
    metric: &str,
    keep: usize,
    filter: Option<&RetrievalFilter>,
    mut chunk: impl FnMut(&Candidate) -> Option<RetrievedChunk>,
) -> Vec<RetrievedChunk> {
    ranked.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(::std::cmp::Ordering::Equal)
    });
    let mut chunks: Vec<RetrievedChunk> = Vec::new();
    for (distance, candidate) in &ranked {
        if chunks.len() >= keep {
            break;
        }
        let Some(mut read) = chunk(candidate) else {
            continue;
        };
        if let Some(filter) = filter {
            if !crate::state::retrieval_filter::admits(&read.metadata, filter) {
                continue;
            }
        }
        read.score = relevance(metric, *distance);
        chunks.push(read);
    }
    chunks
}

/// The chunk the vector at `key` carries, read under its own brief hold of the vector
/// store. `None` when the index no longer holds that vector, or when the vector carries
/// none of what an ingestion records for a chunk.
fn read_chunk(
    vectors: &::engine::ServiceState<vectors_state::ComAmazonawsS3vectorsS3vectorsState>,
    binding: &agent_state::S3VectorsBinding,
    key: &str,
) -> Option<RetrievedChunk> {
    let store = vectors.lock();
    let index = store.index(&binding.vector_bucket, &binding.index).ok()?;
    chunk_of(&index.vectors.get(key)?.metadata)
}

/// The `DependencyFailedException` message a retrieval answers when the knowledge base's
/// content does not live anywhere this plane can read it: it names no vector store, or the
/// index it is bound to cannot be resolved or is gone from the store that held it.
/// `resource` is the store or index the knowledge base names, which the refusal also
/// reports as its `resourceName`.
pub fn unreachable_store_message(resource: &str) -> String {
    format!(
        "The vector store the knowledge base is backed by could not be queried: \
         {resource}. Check the resource configurations and retry the request."
    )
}

/// The relevance a distance under `metric` stands for: a number in `[0, 1]` that is
/// larger for a nearer vector, which is what the modelled `score` reports.
///
/// Under `cosine` the index's distance is `1 - cosine similarity`
/// (`vectors_state::vector_distance`), so the relevance IS that similarity. Under
/// `euclidean` there is no such bounded similarity, so the distance is mapped by
/// `1 / (1 + d)`: 1 for a vector at the query's own position, falling monotonically with
/// distance and never reaching 0.
fn relevance(metric: &str, distance: f32) -> f64 {
    let distance = f64::from(distance);
    if metric == vectors_state::DISTANCE_METRIC_EUCLIDEAN {
        1.0 / (1.0 + distance.max(0.0))
    } else {
        (1.0 - distance).clamp(0.0, 1.0)
    }
}

/// The chunk one stored vector carries, or `None` when it carries none of what an
/// ingestion records for a chunk — a vector a caller wrote into the index themselves,
/// which is no chunk of this knowledge base.
///
/// `score` is filled by the caller once the ranking is known.
fn chunk_of(metadata: &Document) -> Option<RetrievedChunk> {
    let text = field(metadata, index_fields::CHUNK_TEXT_METADATA_KEY)?.to_string();
    let location = location_of(metadata)?;
    // Everything the ingestion recorded ABOUT the chunk: the fields the response
    // carries elsewhere are left out, so the metadata map is what the chunk is annotated
    // with and nothing else.
    let mut reported = BTreeMap::new();
    if let Document::Object(fields) = metadata {
        for (key, value) in fields {
            if PROVENANCE_METADATA_KEYS.contains(&key.as_str()) {
                continue;
            }
            reported.insert(key.clone(), value.clone());
        }
    }
    Some(RetrievedChunk {
        score: 0.0,
        text,
        location,
        metadata: reported,
    })
}

/// The fields of a chunk's recorded metadata the response carries ELSEWHERE than in its
/// `metadata` map: the chunk's text, which is the content, and the two forms of the
/// provenance [`location_of`] reports as the location.
const PROVENANCE_METADATA_KEYS: [&str; 3] = [
    index_fields::CHUNK_TEXT_METADATA_KEY,
    agent_state::SOURCE_URI_METADATA_KEY,
    index_fields::DOCUMENT_ID_METADATA_KEY,
];

/// Where the chunk's document lives, as the ingestion that took it in recorded it.
///
/// A document is addressed the way its DATA SOURCE addresses one, which is the member of
/// the modelled `RetrievalResultLocation` that carries the detail: a document read from an
/// object store by the object it lives at, and a document pushed straight into a custom
/// data source by the id it was ingested under. `None` for a vector recording neither,
/// which is no chunk of this knowledge base.
fn location_of(metadata: &Document) -> Option<RetrievalResultLocation> {
    if let Some(uri) = field(metadata, agent_state::SOURCE_URI_METADATA_KEY) {
        return s3_location(uri);
    }
    let id = field(metadata, index_fields::DOCUMENT_ID_METADATA_KEY)?;
    Some(RetrievalResultLocation {
        r#type: LOCATION_TYPE_CUSTOM.to_string(),
        custom_document_location: Some(RetrievalResultCustomDocumentLocation {
            id: Some(id.to_string()),
        }),
        ..RetrievalResultLocation::default()
    })
}

/// The value of one metadata field, when the vector carries it as a string.
fn field<'metadata>(metadata: &'metadata Document, key: &str) -> Option<&'metadata str> {
    match metadata {
        Document::Object(fields) => match fields.get(key) {
            Some(Document::String(value)) if !value.is_empty() => Some(value.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The modelled location of a source at `uri`, with the member its `type` names carrying
/// the detail and every other member absent. `None` for a URI that names no S3 object:
/// an ingestion of this emulator records only those.
fn s3_location(uri: &str) -> Option<RetrievalResultLocation> {
    uri.strip_prefix("s3://")
        .and_then(|rest| rest.split_once('/'))
        .filter(|(bucket, key)| !bucket.is_empty() && !key.is_empty())
        .map(|_| RetrievalResultLocation {
            r#type: LOCATION_TYPE_S3.to_string(),
            s3_location: Some(RetrievalResultS3Location {
                uri: Some(uri.to_string()),
            }),
            ..RetrievalResultLocation::default()
        })
}
