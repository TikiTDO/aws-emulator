// SPDX-License-Identifier: CC0-1.0

//! What an ingestion job does, and the accounting it reports for it.
//!
//! `StartIngestionJob` "begins a data ingestion job. Data sources are ingested
//! into your knowledge base" — so a job is work over the data source's OWN
//! source, and `IngestionJobStatistics` is the service's accounting of that
//! work: `numberOfDocumentsScanned` is "the total number of source documents
//! that were scanned. Includes new, updated, and unchanged documents",
//! `numberOfNewDocumentsIndexed` "the number of new source documents in the data
//! source that were successfully indexed", beside the modified, deleted, failed
//! and skipped counts
//! (`smithy_query(query="shape",shape="IngestionJobStatistics")`). Every member
//! here is therefore MEASURED: a job accounts for the documents it actually
//! enumerated, and the modelled zero defaults mean a source that held nothing.
//!
//! For a self-managed S3 data source the source is the objects under the
//! configured `bucketArn` and `inclusionPrefixes`
//! (`smithy_query(query="shape",shape="S3DataSourceConfiguration")`), read in
//! the object store that owns them. Each source document becomes a document of
//! the data source, held in this service's own
//! [`documents`](ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState::documents)
//! map and reported by `ListKnowledgeBaseDocuments`, and one vector in the index
//! the knowledge base is bound to — written through to the service that OWNS
//! that store, and counted only where that store really took it
//! ([`IndexedVectors`]), so no counter here reports work that did not happen.
//!
//! The vector's VALUES are the one thing this emulator cannot have an embedding
//! model produce, so they are computed from the document's text instead
//! ([`document_vector`]) rather than invented: the geometry is a real
//! term-frequency vector space, and that doc comment is where the limit is
//! stated.
//!
//! A data source's `dataDeletionPolicy` governs those vectors when it is
//! deleted ([`remove_source_vectors`]): a `DELETE` policy takes exactly the
//! vectors of this data source's documents out of the bound index, and `RETAIN`
//! leaves them.
//!
//! A data source whose documents are pushed in through the service's own
//! document API has no source of its own to enumerate: the documents the store
//! holds for it ARE its source documents, so they are what such a job scans.
//!
//! LOCK ORDER: the functions that reach the object store or the vector store
//! take that store's lock and hold no other while they do, and hand back owned
//! values rather than guards — so each must be called with NO guard of this
//! service's store held.

use std::collections::HashSet;

use super::*;

/// The suffix of a metadata file: "a metadata file must be named
/// `<source file name>.metadata.json`" in the Amazon Bedrock knowledge-base
/// documentation, which is what makes such an object a metadata document rather
/// than a source document — the two the modelled statistics count separately.
pub const METADATA_FILE_SUFFIX: &str = ".metadata.json";

/// The `DataSourceType` of a self-managed S3 data source: the one kind whose
/// source this service enumerates itself
/// (`smithy_query(query="shape",shape="DataSourceType")`).
pub const S3_DATA_SOURCE_TYPE: &str = "S3";

/// One source document an ingestion job found in its data source's source.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceDocument {
    /// Where the document lives, as the modelled `S3Location$uri` spells it.
    pub uri: String,
    /// A digest of the document's bytes: what tells a document the job has seen
    /// before and that has not changed from one that was modified since.
    pub digest: u64,
    /// The vector of the document's text, as [`document_vector`] computes it for
    /// the dimension the store that will answer a retrieval holds; empty when
    /// the crawl was asked for no dimension at all, so there is nothing to
    /// compute it against.
    pub vector: Vec<f32>,
    /// The CHUNK of the document's text the index holds beside that vector
    /// ([`ingestion_index::leading_chunk`]), which is what a retrieval of this
    /// document answers as its content.
    pub chunk: String,
}

/// What one pass over a data source's source found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrawledSource {
    /// The source documents, in the order the source lists them.
    pub documents: Vec<SourceDocument>,
    /// How many metadata files sit beside them.
    pub metadata_documents: usize,
    /// How many source documents the job saw and did NOT take in, because the
    /// data source cannot hold more than [`DOCUMENT_QUOTA`] of them.
    pub skipped_documents: usize,
}

/// A digest of `bytes`: the content-addressing this module tells an unchanged
/// document from a modified one by.
///
/// `DefaultHasher` is seeded with fixed keys, so one content has one digest in
/// every process — which is what makes a re-crawl of an untouched object report
/// it unchanged.
fn content_digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The coordinate of `term` in a vector of `dimension` values.
///
/// The hashing trick: the term's hash picks the coordinate it contributes to.
/// The hash is the standard library's (`DefaultHasher`, fixed-seeded), so one
/// term has one coordinate in every process.
fn term_coordinate(term: &str, dimension: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    term.to_lowercase().hash(&mut hasher);
    let dimension = u64::try_from(dimension).unwrap_or(1).max(1);
    usize::try_from(hasher.finish() % dimension).unwrap_or(0)
}

/// The vector of one document's text, in `dimension` values.
///
/// A TERM-FREQUENCY vector in the vector-space model, projected into the index's
/// dimension by the hashing trick ([`term_coordinate`]) and L2-normalised. So it
/// is a function of the document's TEXT and of the index's dimension, and the
/// geometry means what a vector store's geometry is for: two documents that
/// share terms are close under the cosine metric, two that share none are
/// orthogonal, and one text always has one vector.
///
/// It is NOT what an embedding model would produce — no embedding model runs in
/// this process (Amazon Bedrock's own `InvokeModel` serves none of the published
/// embedding models here), and lexical overlap is a weaker signal than a learned
/// embedding's. It IS computed from the document rather than invented, which is
/// what makes the vector the bound index holds something a later read, query or
/// delete can be ABOUT.
///
/// Every value is finite, and the vector is never the zero vector — the two
/// things the vector store requires of a vector beside its dimension
/// (`PutInputVector$data`): a document with no terms at all occupies the
/// coordinate of the empty term rather than none.
///
/// `dimension` is the bound index's own, which the vector store's model fixes at
/// `range 1..4096` and refuses outside before the index is created — so what
/// this allocates is bounded by that stored value, not by anything a request
/// carries.
pub fn document_vector(text: &str, dimension: i32) -> Vec<f32> {
    let dimension = usize::try_from(dimension.max(0)).unwrap_or(0);
    if dimension == 0 {
        return Vec::new();
    }
    let mut weights = vec![0.0f32; dimension];
    let mut terms = 0usize;
    for term in text.split(|character: char| !character.is_alphanumeric()) {
        if term.is_empty() {
            continue;
        }
        if let Some(weight) = weights.get_mut(term_coordinate(term, dimension)) {
            *weight += 1.0;
        }
        terms += 1;
    }
    if terms == 0 {
        // A document with no terms is still a document: it occupies the
        // coordinate of the empty term, so its vector has the norm the store
        // requires rather than being the zero vector.
        if let Some(weight) = weights.get_mut(term_coordinate("", dimension)) {
            *weight = 1.0;
        }
    }
    // L2-normalised, which is what makes the cosine metric the index is read
    // under compare term distributions rather than document lengths.
    let norm = weights
        .iter()
        .map(|weight| weight * weight)
        .sum::<f32>()
        .sqrt();
    if norm > 0.0 {
        for weight in &mut weights {
            *weight /= norm;
        }
    }
    weights
}

/// The bucket name an `S3DataSourceConfiguration$bucketArn` names.
///
/// The modelled pattern is `^arn:aws(|-cn|-us-gov):s3:::<bucket>$`
/// (`smithy_query(query="shape",shape="S3DataSourceConfiguration")`), so the
/// bucket is the ARN's resource and a bucket name carries no colon.
pub fn data_source_bucket(bucket_arn: &str) -> Option<&str> {
    bucket_arn
        .strip_prefix("arn:")
        .and_then(|_| bucket_arn.rsplit(':').next())
        .filter(|bucket| !bucket.is_empty())
}

/// The `s3://` URI of one object, which is how both `S3Location$uri` and a
/// knowledge-base document identify a source document.
pub fn source_uri(bucket: &str, key: &str) -> String {
    format!("s3://{bucket}/{key}")
}

/// The source documents of the data source `configuration` describes, read in
/// the service that owns them, each with the vector of its text in `dimension`
/// values — the dimension of the store that will answer a retrieval of them
/// (`None` to compute no vector at all). `None` when the configuration
/// names no source this service enumerates.
///
/// Only the objects under one of the configured `inclusionPrefixes` are taken
/// where the configuration lists any — they are "a list of S3 prefixes to
/// include certain files or content" — and the whole bucket where it lists none.
///
/// A data source holds at most [`DOCUMENT_QUOTA`] documents, which is what
/// bounds what one crawl takes into this process however many objects the bucket
/// holds: past it the remaining source documents are counted as SKIPPED, which
/// is what the modelled `numberOfDocumentsSkipped` reports.
///
/// LOCK ORDER: as the module doc says — call with NO guard of this service's
/// store held.
pub fn crawl_data_source<Eng: Send + Sync + 'static>(
    engine: &Eng,
    configuration: &DataSourceConfiguration,
    dimension: Option<i32>,
) -> Option<CrawledSource> {
    let s3 = configuration.s3_configuration.as_ref()?;
    let bucket = data_source_bucket(&s3.bucket_arn)?;
    let prefixes = s3.inclusion_prefixes.clone().unwrap_or_default();
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    // A bucket the object store does not hold is a source that holds nothing,
    // which is exactly what the zero counters report.
    let Some(held) = store.bucket(bucket) else {
        return Some(CrawledSource::default());
    };

    let mut crawled = CrawledSource::default();
    for (key, object) in held.objects() {
        if !prefixes.is_empty() && !prefixes.iter().any(|prefix| key.starts_with(prefix)) {
            continue;
        }
        if key.ends_with(METADATA_FILE_SUFFIX) {
            crawled.metadata_documents += 1;
            continue;
        }
        if crawled.documents.len() >= DOCUMENT_QUOTA {
            crawled.skipped_documents += 1;
            continue;
        }
        // The document's text is what its vector is computed from; bytes that
        // are not text carry no terms, so such a document takes the vector a
        // document with no terms takes.
        let text = String::from_utf8_lossy(&object.body);
        let vector = dimension.map_or_else(Vec::new, |dimension| {
            document_vector(&text, dimension)
        });
        crawled.documents.push(SourceDocument {
            uri: source_uri(bucket, key),
            digest: content_digest(&object.body),
            vector,
            // The chunk the index holds beside the vector, so a retrieval of this
            // document answers the text the job really read rather than a later look
            // at the object.
            chunk: ingestion_index::leading_chunk(&text),
        });
    }
    Some(crawled)
}

/// The key a source document's vector is held under in the bound index: where
/// that document lives, so one document has one vector however often it is
/// ingested, and the vectors of a data source's documents can be named without
/// reading them.
pub fn source_vector_key(uri: &str) -> String {
    uri.to_string()
}

/// The metadata key a Bedrock knowledge base records a chunk's source location
/// under in its vector store, as the published knowledge-base metadata fields
/// name it.
pub const SOURCE_URI_METADATA_KEY: &str = "x-amz-bedrock-kb-source-uri";

/// Which of a job's source documents the bound vector store took a vector for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexedVectors {
    /// The knowledge base binds to no vector store this emulator holds, so
    /// there is nothing to reconcile the job's accounting against: a document
    /// is indexed into the knowledge base's own records and that is all this
    /// emulator can say about it.
    NotReconciled,
    /// The URIs of the documents whose vector the bound index really took. A
    /// document that is not here is one the store refused, which the job
    /// accounts for as FAILED rather than indexed.
    Taken(Vec<String>),
}

impl IndexedVectors {
    /// The URIs the job may report as indexed, as a set; `None` when there is
    /// nothing to reconcile against and every document counts.
    fn taken(&self) -> Option<HashSet<&str>> {
        match self {
            Self::NotReconciled => None,
            Self::Taken(taken) => Some(taken.iter().map(String::as_str).collect()),
        }
    }
}

/// Write the vector of each source document into the index `binding` names,
/// answering which of them the store really took.
///
/// Every vector is checked against the index by the owning service's OWN rule
/// ([`::service_com_amazonaws_s3vectors_s3vectors::state::vector_data_defect`])
/// and against its ceiling before it is written, so what comes back is what the
/// store holds and nothing else: an index the store no longer holds takes
/// nothing, and an index that is full takes what it has room for. The job's
/// accounting then reports the rest as failed.
///
/// LOCK ORDER: as the module doc says.
pub fn index_source_vectors<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &S3VectorsBinding,
    data_source_id: &str,
    documents: &[SourceDocument],
) -> IndexedVectors {
    use ::service_com_amazonaws_s3vectors_s3vectors::state as vectors;

    let Some(state) = vectors::service_state(engine) else {
        return IndexedVectors::NotReconciled;
    };
    let mut store = state.lock();
    let Ok(index) = store.index_mut(&binding.vector_bucket, &binding.index) else {
        return IndexedVectors::Taken(Vec::new());
    };
    let mut taken: Vec<String> = Vec::new();
    for document in documents {
        let vector = vectors::StoredVector {
            key: source_vector_key(&document.uri),
            data: document.vector.clone(),
            // What the knowledge base records with the chunk: its text, its id, the
            // data source it came from and its source file's modality, beside the
            // source URI this plane keys its own bookkeeping by. One place decides
            // those fields, so the plane that READS them back for a retrieval cannot
            // drift from this one ([`ingestion_index`]).
            metadata: ingestion_index::chunk_metadata(
                data_source_id,
                &document.uri,
                &document.chunk,
                SOURCE_URI_METADATA_KEY,
            ),
        };
        if vectors::vector_data_defect(index, &vector.data).is_some() {
            continue;
        }
        if vectors::vector_quota_exceeded(index, ::std::slice::from_ref(&vector)).is_some() {
            break;
        }
        // An upsert, as a put of the same key is: the document's one vector is
        // replaced rather than joined by a second.
        index.clear_tombstone(&vector.key);
        taken.push(document.uri.clone());
        index.vectors.insert(vector.key.clone(), vector);
    }
    IndexedVectors::Taken(taken)
}

/// Take the vectors of `uris` out of the index `binding` names, answering how
/// many were there to take.
///
/// LOCK ORDER: as the module doc says.
pub fn remove_source_vectors<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &S3VectorsBinding,
    uris: &[String],
) -> usize {
    let keys: Vec<String> = uris.iter().map(|uri| source_vector_key(uri)).collect();
    remove_vectors(engine, binding, &keys)
}

/// Take the vectors held under `keys` out of the index `binding` names,
/// answering how many were there to take.
///
/// The keys are the ones this plane holds a chunk under, whichever path wrote
/// it: a crawled document's is where its source object lives, and a pushed
/// document's is [`crate::state::document_vector_key`].
///
/// LOCK ORDER: as the module doc says.
pub fn remove_vectors<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &S3VectorsBinding,
    keys: &[String],
) -> usize {
    use ::service_com_amazonaws_s3vectors_s3vectors::state as vectors;

    let Some(state) = vectors::service_state(engine) else {
        return 0;
    };
    let mut store = state.lock();
    let Ok(index) = store.index_mut(&binding.vector_bucket, &binding.index) else {
        return 0;
    };
    let mut removed = 0usize;
    for key in keys {
        if index.vectors.contains_key(key) {
            index.tombstone(key);
            removed += 1;
        }
    }
    removed
}

/// The `dataDeletionPolicy` that takes the data source's vectors out of the
/// vector store when the data source is deleted.
///
/// `DataDeletionPolicy` admits `RETAIN` and `DELETE`
/// (`smithy_query(query="shape",shape="DataDeletionPolicy")`), and
/// `CreateDataSource` documents the second as the one that "deletes all data
/// from your data source that's converted into vector embeddings upon deletion
/// of a knowledge base or data source resource" — so it alone removes them and
/// `RETAIN` leaves them where they are.
pub const DATA_DELETION_POLICY_DELETE: &str = "DELETE";

/// How many of something a statistics member counts, as the modelled
/// `PrimitiveLong` members carry it.
fn counted(amount: usize) -> i64 {
    i64::try_from(amount).unwrap_or(i64::MAX)
}

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The statistics of an ingestion job over a data source that names no
    /// source of its own to enumerate.
    ///
    /// The documents the store holds for it ARE its source documents — they were
    /// pushed in through the service's own document API — so the job scans them
    /// and finds each of them unchanged: nothing about them is new, modified or
    /// gone.
    pub fn pushed_ingestion_statistics(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> IngestionJobStatistics {
        IngestionJobStatistics {
            number_of_documents_scanned: counted(
                self.document_count(knowledge_base_id, data_source_id),
            ),
            ..IngestionJobStatistics::default()
        }
    }

    /// Record what an ingestion job over `(knowledge_base_id, data_source_id)`
    /// found, answering the statistics that measure it.
    ///
    /// A source document the data source did not hold is NEW and becomes one of
    /// its documents; one it held whose content has changed since is MODIFIED
    /// and takes the new content; one it held unchanged is scanned and nothing
    /// else; and one the data source held that the source no longer offers is
    /// DELETED and leaves the store. A source document the crawl could not take
    /// in is SKIPPED.
    ///
    /// A document whose vector the bound store did NOT take (`indexed`) is
    /// FAILED: it is not indexed, so it does not become a document of the data
    /// source and no counter claims it was. That is what keeps every counter a
    /// statement about work that really happened.
    pub fn record_ingestion(
        &mut self,
        knowledge_base_id: &str,
        data_source_id: &str,
        crawled: &CrawledSource,
        indexed: &IndexedVectors,
        now: SystemTime,
    ) -> IngestionJobStatistics {
        let mut statistics = IngestionJobStatistics {
            number_of_documents_scanned: counted(
                crawled.documents.len() + crawled.skipped_documents,
            ),
            number_of_metadata_documents_scanned: counted(crawled.metadata_documents),
            number_of_documents_skipped: counted(crawled.skipped_documents),
            ..IngestionJobStatistics::default()
        };

        let taken = indexed.taken();
        for document in &crawled.documents {
            if taken
                .as_ref()
                .is_some_and(|taken| !taken.contains(document.uri.as_str()))
            {
                // The vector store refused this document's vector, so nothing
                // indexed it.
                statistics.number_of_documents_failed += 1;
                continue;
            }
            let key = DocumentKey::S3(document.uri.clone());
            // The chunk this job read, recorded on the document: what a
            // retrieval answers for it, for a knowledge base whose vector store
            // no API of this emulator writes ([`ingestion_record`]).
            let chunk = Some(IngestedChunk {
                text: document.chunk.clone(),
                vector: document.vector.clone(),
            });
            let index = (
                knowledge_base_id.to_string(),
                data_source_id.to_string(),
                key.clone(),
            );
            match self.documents.get(&index).map(|held| held.source_digest) {
                // Held with the same content: scanned, and nothing more.
                Some(Some(digest)) if digest == document.digest => {}
                Some(_) => {
                    statistics.number_of_modified_documents_indexed += 1;
                    if let Some(held) = self.documents.get_mut(&index) {
                        *held = DocumentRecord::crawled(
                            knowledge_base_id.to_string(),
                            data_source_id.to_string(),
                            key,
                            document.digest,
                            chunk,
                            now,
                        );
                    }
                }
                None => {
                    statistics.number_of_new_documents_indexed += 1;
                    self.documents.insert(
                        index,
                        DocumentRecord::crawled(
                            knowledge_base_id.to_string(),
                            data_source_id.to_string(),
                            key,
                            document.digest,
                            chunk,
                            now,
                        ),
                    );
                }
            }
        }

        // A document the data source held that this pass did not find in its
        // source is gone from the source, so the job accounts for it as deleted
        // and the data source stops holding it. Only the documents the job
        // enumerates are compared: a pushed document is not a crawled one.
        let offered: HashSet<&str> = crawled
            .documents
            .iter()
            .map(|document| document.uri.as_str())
            .collect();
        let dropped: Vec<(String, String, DocumentKey)> = self
            .documents
            .values()
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
                    && record.source_digest.is_some()
                    && !offered.contains(record.key.identity())
            })
            .map(|record| {
                (
                    record.knowledge_base_id.clone(),
                    record.data_source_id.clone(),
                    record.key.clone(),
                )
            })
            .collect();
        statistics.number_of_documents_deleted = counted(dropped.len());
        for index in dropped {
            self.documents.remove(&index);
        }

        statistics
    }

    /// Where every document of `(knowledge_base_id, data_source_id)` that an
    /// ingestion job took from the data source's source lives, which is what
    /// names the vectors written for them.
    ///
    /// A document pushed in through the service's document API is not one of
    /// them: no job enumerated it, so no vector was written for it here.
    pub fn ingested_source_uris(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
    ) -> Vec<String> {
        self.documents
            .values()
            .filter(|record| {
                record.knowledge_base_id == knowledge_base_id
                    && record.data_source_id == data_source_id
                    && record.source_digest.is_some()
            })
            .map(|record| record.key.identity().to_string())
            .collect()
    }
}
