// SPDX-License-Identifier: CC0-1.0

//! What a knowledge-base retrieval reads: the chunks the knowledge base's bound vector
//! index holds, and the provenance recorded with each of them.
//!
//! `Retrieve` RANKS and REPORTS chunks; this module is where they come from, so the
//! provenance of every retrieval result is settled in one place.
//!
//! A knowledge base's content lives in the vector store it is BOUND to: an ingestion job
//! of the Bedrock Agent build-time plane reads the data source's source documents, writes
//! one vector per document into the index the knowledge base's `storageConfiguration`
//! names, and records with that vector the fields Amazon Bedrock records — the chunk's
//! text, the chunk's id, the data source it came from and where its source lives
//! (`service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda`'s
//! `state::ingestion_index`, the ONE place those field names are decided). This plane
//! writes none of it. That is what makes a result's provenance real, and what makes it
//! stay put:
//!
//! * a chunk exists only because an ingestion took its source in, so a knowledge base
//!   that has ingested nothing retrieves nothing;
//! * the text and the citation are the ones that ingestion recorded, so deleting or
//!   rewriting the source afterwards does not change what a retrieval cites — the
//!   citation keeps pointing at the object the chunk came from until another ingestion
//!   job replaces what the index holds;
//! * the RANKING is the index's own nearest-neighbour order: the query's vector is
//!   computed the way the ingestion computed each document's
//!   (`agent_state::document_vector`, a term-frequency vector in the index's dimension)
//!   and the distance is the one the index was created with
//!   (`vectors_state::vector_distance`), so a retrieval is a real vector search over the
//!   store the knowledge base is bound to rather than a second notion of relevance.
//!
//! A vector a caller wrote into that index themselves carries none of those fields: it is
//! not a chunk of this knowledge base, so no retrieval reports it.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::aws_smithy_types::Document;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;
use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::ingestion_index as index_fields;
use ::service_com_amazonaws_s3vectors_s3vectors::state as vectors_state;

use crate::ops::types::{
    RetrievalFilter, RetrievalResultLocation, RetrievalResultS3Location,
};

/// The `RetrievalResultLocationType` an ingested S3 object is reported under
/// (`smithy_query(query="shape",shape="RetrievalResultLocationType")`).
pub const LOCATION_TYPE_S3: &str = "S3";

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
    /// The knowledge base is backed by a vector store this emulator cannot query: the
    /// modelled `KnowledgeBaseStorageType` admits several, and the one this plane reads
    /// is the S3 Vectors index the Bedrock Agent plane's ingestion writes. The string is
    /// the store the knowledge base names, for the refusal to report as its
    /// `resourceName`.
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
/// <id> is not found."): the two planes refuse in their own words, and the recorded calls
/// this fix was derived from show the 501 the stub answered rather than either sentence,
/// so a later recording of this refusal is what would settle it.
pub fn knowledge_base_not_found_message(identifier: &str) -> String {
    format!("Could not find knowledge base {identifier}.")
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

/// The chunks of `knowledge_base_id` nearest to `query`, most relevant first, at most
/// `keep` of them, and only those whose own metadata `filter` admits.
///
/// READ-ONLY for everything a later caller can observe. The one write on its path is on
/// the Bedrock Agent plane's own clock-owed transitions (`materialise_due_transitions`):
/// the status an ingestion job, a data source or a knowledge base was owed at this
/// instant whether or not anybody called, which is the same write every reporting
/// operation of that plane performs.
///
/// A knowledge base backed by a vector store this emulator cannot query, or bound to an
/// S3 Vectors index that store no longer holds, is REFUSED rather than answered with an
/// empty success: its content does not live anywhere this plane can read, which is not
/// the same as a knowledge base that has ingested nothing.
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
    // The knowledge base and the store it binds to are read under the Bedrock Agent
    // plane's lock; the vector store is reached with that lock released, the lock order
    // every cross-plane path of these two services keeps.
    let binding = {
        let mut store = agent.lock();
        store.materialise_due_transitions(SystemTime::now());
        let Some(knowledge_base) = store.live_knowledge_base(knowledge_base_id) else {
            return Err(RetrievalRefusal::UnknownKnowledgeBase);
        };
        let storage = knowledge_base.storage_configuration.as_ref();
        agent_state::s3_vectors_binding(storage).ok_or_else(|| {
            // The store the knowledge base DOES name, so the refusal reports what it is
            // bound to rather than only that this plane cannot read it.
            RetrievalRefusal::UnreachableVectorStore(
                storage.map_or_else(
                    || "a knowledge base with no vector store".to_string(),
                    |storage| storage.r#type.clone(),
                ),
            )
        })
    }?;
    let Some(vectors) = vectors_state::service_state(engine) else {
        return Err(RetrievalRefusal::StoreUnavailable);
    };

    // PHASE ONE, under the vector store's guard: how far each stored vector is from the
    // query, and nothing else. No filter is evaluated and no chunk is copied here, so what
    // the guard is held for is one pass of the index's own distance function — work the
    // index's size bounds and no request can grow — rather than anything the request
    // chose.
    let (metric, mut ranked) = {
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

    // Nearest first, which is the ordering the index is read under. The sort is STABLE and
    // the vectors are walked in key order, so vectors at the same distance keep that order
    // and one request always answers the same way.
    ranked.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(::std::cmp::Ordering::Equal)
    });

    // PHASE TWO, with the guard RELEASED between candidates: the chunks are read in that
    // order, the filter is evaluated against one chunk at a time, and the walk stops as
    // soon as the page is full — so what is copied is the page the request asked for and
    // not the whole index, and a filter's own work never holds a store every other caller
    // shares.
    let mut chunks: Vec<RetrievedChunk> = Vec::new();
    for (distance, key) in ranked {
        if chunks.len() >= keep {
            break;
        }
        let Some(mut chunk) = read_chunk(&vectors, &binding, &key) else {
            continue;
        };
        if let Some(filter) = filter {
            if !crate::state::retrieval_filter::admits(&chunk.metadata, filter) {
                continue;
            }
        }
        chunk.score = relevance(&metric, distance);
        chunks.push(chunk);
    }
    Ok(chunks)
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
/// content does not live anywhere this plane can read it: the vector store it is backed by
/// is not the one this emulator serves, or the index it is bound to is gone from that
/// store. `resource` is the store or index the knowledge base names, which the refusal
/// also reports as its `resourceName`.
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
    let uri = field(metadata, agent_state::SOURCE_URI_METADATA_KEY)?;
    let location = s3_location(uri)?;
    // Everything the ingestion recorded ABOUT the chunk: the two fields the response
    // carries elsewhere are left out, so the metadata map is what the chunk is annotated
    // with and nothing else.
    let mut reported = BTreeMap::new();
    if let Document::Object(fields) = metadata {
        for (key, value) in fields {
            if key == index_fields::CHUNK_TEXT_METADATA_KEY
                || key == agent_state::SOURCE_URI_METADATA_KEY
            {
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
