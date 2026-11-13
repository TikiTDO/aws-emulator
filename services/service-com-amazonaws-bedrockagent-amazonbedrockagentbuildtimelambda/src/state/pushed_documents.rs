// SPDX-License-Identifier: CC0-1.0

//! What a document pushed straight into a data source puts into the knowledge
//! base's bound vector index: the chunk of its text, and the provenance a
//! retrieval cites it by.
//!
//! `IngestKnowledgeBaseDocuments` "ingests documents directly into the knowledge
//! base that is connected to the data source", so such a document is part of
//! what the knowledge base HOLDS in exactly the way a document an ingestion job
//! crawled is ([`crate::state::ingestion`]): one vector per document in the
//! index the knowledge base's `storageConfiguration` binds it to, with the
//! chunk's text and the document's own provenance recorded beside it — which is
//! what the Bedrock Agent RUNTIME plane reads a retrieval out of. Without it a
//! caller could watch a document reach `INDEXED` and retrieve nothing.
//!
//! The two paths record the SAME fields ([`ingestion_index`]) and differ in one
//! thing: how a document is ADDRESSED. A document of an S3 data source is
//! addressed by the object it lives at and a document of a custom data source by
//! the id the request gave it — the identity this plane already keys it by
//! ([`DocumentKey`]) — so that is what each one's chunk carries as its
//! provenance, and what a retrieval reports as the result's location.
//!
//! The TEXT is the document's own bytes read as text: the bytes the request
//! carried for an inline document, and the object's bytes for one that names an
//! S3 location. A document whose bytes live nowhere this emulator holds (an S3
//! location no object answers) yields NO chunk: nothing here invents text for
//! it, and a retrieval reports a chunk only for a document whose content this
//! emulator really took in.
//!
//! LOCK ORDER: the functions here reach the object store and the vector store,
//! and hold no guard of this service's store while they do — call them with NO
//! guard of it held.

use super::*;

/// The `InlineContentType` of inline content carried as TEXT; the other value
/// the enum admits carries it as bytes
/// (`smithy_query(query="shape",shape="InlineContentType")` -> BYTE, TEXT).
const INLINE_CONTENT_TYPE_TEXT: &str = "TEXT";

/// What the vector key of a CUSTOM data source's document is prefixed with, so a
/// document id that happens to read like an object URI cannot displace the
/// vector of the object it reads like. No key an S3 document takes carries it:
/// those are the `s3://` URIs the objects live at.
const CUSTOM_VECTOR_KEY_PREFIX: &str = "custom-document/";

/// The key one document's chunk is held under in the knowledge base's bound
/// index.
///
/// A document of an S3 data source is held under the object it lives at, which
/// is the key an ingestion job of that same object writes
/// ([`source_vector_key`]), so one object has one vector however it was
/// ingested. A custom document is held under its own id, in a namespace of its
/// own ([`CUSTOM_VECTOR_KEY_PREFIX`]).
pub fn document_vector_key(key: &DocumentKey) -> String {
    match key {
        DocumentKey::S3(uri) => source_vector_key(uri),
        DocumentKey::Custom(id) => format!("{CUSTOM_VECTOR_KEY_PREFIX}{id}"),
    }
}

/// The text one pushed document carries, or `None` when its bytes live nowhere
/// this emulator holds.
///
/// `CustomContent` and `InlineContent` are both discriminated — each documents
/// its own "choose a type and include the field that corresponds to it" — so the
/// bytes are taken from the member the kind the content names carries them in.
/// Those bytes are read AS TEXT, which is the one modality an ingestion of this
/// emulator records ([`ingestion_index::SOURCE_FILE_MODALITY_TEXT`]: it runs no
/// extractor for the binary formats whose text it would otherwise have to
/// invent), so a byte that is not text reads as the replacement character rather
/// than as text nobody wrote.
///
/// LOCK ORDER: as the module doc says.
pub fn pushed_document_text<Eng: Send + Sync + 'static>(
    engine: &Eng,
    content: &DocumentContent,
) -> Option<String> {
    if let Some(inline) = content
        .custom
        .as_ref()
        .and_then(|custom| custom.inline_content.as_ref())
    {
        if inline.r#type == INLINE_CONTENT_TYPE_TEXT {
            return inline.text_content.as_ref().map(|text| text.data.clone());
        }
        return inline
            .byte_content
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(&bytes.data).into_owned());
    }
    // A document that carries no bytes of its own IS the object it names, for
    // both the custom content that names one and the content of an S3 data
    // source.
    let uri = content
        .custom
        .as_ref()
        .and_then(|custom| custom.s3_location.as_ref())
        .map(|location| location.uri.as_str())
        .or_else(|| content.s3.as_ref().map(|s3| s3.s3_location.uri.as_str()))?;
    object_text(engine, uri)
}

/// The text of the object at `uri`, read in the service that OWNS it. `None`
/// when the object store holds no such object: that is a document with no bytes
/// to chunk rather than one whose text is empty.
///
/// LOCK ORDER: as the module doc says.
fn object_text<Eng: Send + Sync + 'static>(engine: &Eng, uri: &str) -> Option<String> {
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    let object = store.bucket(bucket)?.object(key)?;
    Some(String::from_utf8_lossy(&object.body).into_owned())
}

/// The chunk each of `documents` carries, for a knowledge base whose chunks hold
/// `dimension` values: the text the document's content holds, chunked and
/// vectorised the way an ingestion does it ([`IngestedChunk`]). One entry per
/// document in the order given, carrying `None` for a document whose bytes live
/// nowhere this emulator holds.
///
/// The chunk is computed ONCE here, for the ingestion to record on the document
/// and to write into the bound index ([`index_pushed_chunks`]), so the two hold
/// the same chunk.
///
/// LOCK ORDER: as the module doc says.
pub fn pushed_document_chunks<Eng: Send + Sync + 'static>(
    engine: &Eng,
    documents: &[(DocumentKey, DocumentContent)],
    dimension: i32,
) -> Vec<(DocumentKey, Option<IngestedChunk>)> {
    documents
        .iter()
        .map(|(key, content)| {
            let chunk = pushed_document_text(engine, content)
                .map(|text| IngestedChunk::of(&text, dimension));
            (key.clone(), chunk)
        })
        .collect()
}

/// Write the chunk of each of `chunks` into the index `binding` names, answering
/// the documents whose chunk the vector store really took.
///
/// Every vector is checked against the index by the owning service's own rule
/// before it is written, exactly as an ingestion job's is
/// ([`index_source_vectors`]) — so what comes back is what the store holds: an
/// index this emulator does not hold takes nothing, an index that cannot take
/// the vector takes none of it, and an index that is full takes what it has room
/// for.
///
/// LOCK ORDER: as the module doc says.
pub fn index_pushed_chunks<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &S3VectorsBinding,
    data_source_id: &str,
    chunks: &[(DocumentKey, Option<IngestedChunk>)],
) -> Vec<DocumentKey> {
    use ::service_com_amazonaws_s3vectors_s3vectors::state as vectors;

    // What goes into the store is assembled with no guard held, so the vector
    // store is held for the writes alone.
    let pending: Vec<(DocumentKey, vectors::StoredVector)> = chunks
        .iter()
        .filter_map(|(key, chunk)| {
            let chunk = chunk.as_ref()?;
            Some((
                key.clone(),
                vectors::StoredVector {
                    key: document_vector_key(key),
                    data: chunk.vector.clone(),
                    metadata: chunk_metadata_of(data_source_id, key, &chunk.text),
                },
            ))
        })
        .collect();
    if pending.is_empty() {
        return Vec::new();
    }

    let Some(state) = vectors::service_state(engine) else {
        return Vec::new();
    };
    let mut store = state.lock();
    let Ok(index) = store.index_mut(&binding.vector_bucket, &binding.index) else {
        return Vec::new();
    };
    let mut taken: Vec<DocumentKey> = Vec::new();
    for (document, vector) in pending {
        if vectors::vector_data_defect(index, &vector.data).is_some() {
            continue;
        }
        if vectors::vector_quota_exceeded(index, ::std::slice::from_ref(&vector)).is_some() {
            break;
        }
        // An upsert, as a put of the same key is: re-ingesting a document
        // replaces its one chunk rather than adding a second.
        index.clear_tombstone(&vector.key);
        index.vectors.insert(vector.key.clone(), vector);
        taken.push(document);
    }
    taken
}

/// The metadata the index records with one document's chunk: the fields an
/// ingestion records ([`ingestion_index::chunk_metadata`]), with the document's
/// own identity under the field that names how the document is addressed.
///
/// Read by the knowledge base's own ingestion record too
/// ([`crate::state::ingestion_record`]), so a chunk carries the same fields
/// whichever store answers it.
pub(super) fn chunk_metadata_of(
    data_source_id: &str,
    key: &DocumentKey,
    chunk: &str,
) -> Document {
    let provenance = match key {
        DocumentKey::S3(_) => SOURCE_URI_METADATA_KEY,
        DocumentKey::Custom(_) => ingestion_index::DOCUMENT_ID_METADATA_KEY,
    };
    ingestion_index::chunk_metadata(data_source_id, key.identity(), chunk, provenance)
}

/// Take the chunks of `documents` out of the index `binding` names, answering
/// how many were there to take.
///
/// LOCK ORDER: as the module doc says.
pub fn remove_document_vectors<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &S3VectorsBinding,
    documents: &[DocumentKey],
) -> usize {
    let keys: Vec<String> = documents.iter().map(document_vector_key).collect();
    remove_vectors(engine, binding, &keys)
}

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The keys the chunks of `(knowledge_base_id, data_source_id)` are held
    /// under in the bound index: one per document the data source holds, however
    /// that document was ingested.
    pub fn document_vector_keys(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Vec<String> {
        self.documents
            .values()
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
            })
            .map(|record| document_vector_key(&record.key))
            .collect()
    }
}
