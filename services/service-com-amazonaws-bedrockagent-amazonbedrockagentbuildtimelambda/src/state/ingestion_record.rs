// SPDX-License-Identifier: CC0-1.0

//! The CHUNK an ingestion records on each document it takes in, and the
//! knowledge base's own record of those chunks — which is what a retrieval
//! reads for a knowledge base whose vector store is not one this emulator
//! serves.
//!
//! A knowledge base keeps its vectors in the store its `storageConfiguration`
//! names, and the modelled `KnowledgeBaseStorageType` admits several. This
//! emulator serves the data plane of exactly one of them — the S3 Vectors index
//! an ingestion writes through to ([`crate::state::ingestion`],
//! [`crate::state::pushed_documents`]) — and no API here can write a vector into
//! any of the others. So every ingestion records the chunk it read ON the
//! document it read it from ([`IngestedChunk`]), and what a knowledge base
//! backed by one of those other stores HOLDS is that record: real work, written
//! by the ingestion that did it, read back here:
//!
//! * a knowledge base that has ingested nothing records nothing, so it retrieves
//!   nothing;
//! * a document whose deletion has been accepted is on its way out of its data
//!   source, so its chunk goes with it and no retrieval reports it;
//! * a document whose bytes live nowhere this emulator holds carries no chunk at
//!   all: nothing here invents text for one.
//!
//! The chunk is recorded ONCE, when the ingestion takes the document in, and
//! never re-derived — so what a retrieval answers does not change when the
//! source does, exactly as for a knowledge base bound to an index this emulator
//! holds. The fields reported with it are the ones that index would hold
//! ([`crate::state::ingestion_index`]), so the RUNTIME plane reads a chunk back
//! the same way whichever of the two answered it.
//!
//! LOCK ORDER: the reads here take this service's store and hold no other guard
//! while they do — call them with NO guard of it held.

use super::*;

/// How many values a chunk's vector carries when the knowledge base's embedding
/// model publishes no dimension.
///
/// CHOSEN, not documented: it is the dimension the ingestion projects a
/// document's terms into, which is internal to this record — it is NOT a claim
/// about what any embedding model produces, and a knowledge base whose model
/// publishes a dimension is recorded in that one
/// ([`effective_embedding_dimension`]). It matches the most common published
/// embedding dimension, so the projection is as sparse there as it is for a
/// knowledge base that names one.
pub const DEFAULT_RECORD_DIMENSION: i32 = 1024;

/// How many values the vectors of `knowledge_base`'s chunks carry: the number
/// the embedding model its configuration names produces, and
/// [`DEFAULT_RECORD_DIMENSION`] when that model publishes none.
///
/// Bounded by the modelled `range 0..4096` the configuration's own `dimensions`
/// member carries
/// (`smithy_query(query="shape",shape="BedrockEmbeddingModelConfiguration")`),
/// which the boundary validator enforces before any knowledge base is stored —
/// so what one chunk's vector allocates is bounded by that stored value and by
/// nothing a request carries. A declared zero names no geometry at all, so the
/// record keeps the one coordinate a vector space needs.
pub fn record_dimension(knowledge_base: &KnowledgeBaseRecord) -> i32 {
    knowledge_base
        .knowledge_base_configuration
        .vector_knowledge_base_configuration
        .as_ref()
        .and_then(effective_embedding_dimension)
        .unwrap_or(DEFAULT_RECORD_DIMENSION)
        .max(1)
}

/// The chunk one ingestion recorded for one document.
///
/// Both members are bounded by the ingestion rather than by any request: the
/// text by [`ingestion_index::CHUNK_MAX_BYTES`], and the vector by the dimension
/// [`record_dimension`] answers, which the model bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestedChunk {
    /// The chunk of the document's text the ingestion read
    /// ([`ingestion_index::leading_chunk`]), which is what a retrieval of this
    /// document answers as its content.
    pub text: String,
    /// The vector of the document's text ([`document_vector`]), which is what
    /// orders this chunk against a query.
    pub vector: Vec<f32>,
}

impl IngestedChunk {
    /// The chunk of `text`, for a knowledge base whose chunks carry `dimension`
    /// values.
    pub fn of(text: &str, dimension: i32) -> Self {
        Self {
            text: ingestion_index::leading_chunk(text),
            vector: document_vector(text, dimension),
        }
    }
}

/// Which chunk of a knowledge base's record a candidate is: the data source its
/// document belongs to and the document's own identity within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedChunkKey {
    pub data_source_id: String,
    pub key: DocumentKey,
}

/// How far each chunk of `knowledge_base_id`'s record is from `query_vector`,
/// and nothing else.
///
/// ONE pass of the distance function over the chunks the store holds, under one
/// hold of this service's store: no chunk's text and no vector is copied, so
/// what this allocates is one entry per document the knowledge base holds and
/// nothing a request carries grows it. The chunks themselves are read afterwards
/// and one at a time ([`recorded_chunk_metadata`]), so only the page a retrieval
/// reports is ever copied.
///
/// A document whose deletion has been accepted carries no chunk, and neither
/// does one whose bytes this emulator never held; both are simply absent.
///
/// LOCK ORDER: as the module doc says.
pub fn ranked_chunks<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    query_vector: &[f32],
) -> Vec<(f32, RecordedChunkKey)> {
    let Some(state) = service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let mut ranked: Vec<(f32, RecordedChunkKey)> = store
        .documents
        .values()
        .filter(|record| record.knowledge_base_id == knowledge_base_id)
        .filter_map(|record| {
            let chunk = record.chunk.as_ref()?;
            Some((
                ::service_com_amazonaws_s3vectors_s3vectors::state::vector_distance(
                    ::service_com_amazonaws_s3vectors_s3vectors::state::DISTANCE_METRIC_COSINE,
                    query_vector,
                    &chunk.vector,
                ),
                RecordedChunkKey {
                    data_source_id: record.data_source_id.clone(),
                    key: record.key.clone(),
                },
            ))
        })
        .collect();
    // In the key's own order, so chunks at the same distance from the query are
    // always reported in the same order however the store is laid out.
    ranked.sort_by(|left, right| {
        (left.1.data_source_id.as_str(), left.1.key.identity())
            .cmp(&(right.1.data_source_id.as_str(), right.1.key.identity()))
    });
    ranked
}

/// The fields the record holds for one chunk — the ones an ingestion records,
/// with the chunk's own text ([`ingestion_index::chunk_metadata`]) — read under
/// its own brief hold of this service's store. `None` when the record no longer
/// holds a chunk for that document.
///
/// LOCK ORDER: as the module doc says.
pub fn recorded_chunk_metadata<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    chunk: &RecordedChunkKey,
) -> Option<Document> {
    let state = service_state(engine)?;
    let store = state.lock();
    let record = store.document_of(knowledge_base_id, &chunk.data_source_id, &chunk.key)?;
    let held = record.chunk.as_ref()?;
    Some(chunk_metadata_of(
        &chunk.data_source_id,
        &chunk.key,
        &held.text,
    ))
}

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// Record `chunk` as what the ingestion read for one document of
    /// `(knowledge_base_id, data_source_id)`, answering whether that document is
    /// still held.
    ///
    /// A document the store no longer holds takes nothing: the delete that
    /// removed it is what a later read answers.
    pub fn record_document_chunk(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
        key: &DocumentKey,
        chunk: Option<IngestedChunk>,
    ) -> bool {
        let Some(record) = self.documents.get_mut(&(
            knowledge_base_id.to_string(),
            data_source_id.to_string(),
            key.clone(),
        )) else {
            return false;
        };
        record.chunk = chunk;
        true
    }
}
