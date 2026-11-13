// SPDX-License-Identifier: CC0-1.0

//! What an ingestion records in the bound vector index BESIDE the vector: the published
//! knowledge-base metadata fields, which is where a retrieval reads a chunk back from.
//!
//! A knowledge base's vector store holds, for every chunk it ingested, the chunk's vector
//! AND the fields Amazon Bedrock records with it — the chunk's text, the chunk's own id,
//! the data source it came from and where that source lives. That is what makes a
//! retrieval possible at all: the Bedrock Agent RUNTIME plane serves `Retrieve` by
//! reading the index this plane's ingestion wrote, so the text and the provenance a
//! retrieval result carries are the ones the ingestion really took in and NOT a fresh
//! look at the data source. An object deleted after a job keeps being cited exactly as it
//! was ingested, until another job replaces what the index holds.
//!
//! This module is the ONE place those field names and their values are decided, so the
//! plane that writes them and the plane that reads them cannot drift.
//!
//! ## What one chunk is here
//!
//! The LEADING chunk of a source document's text ([`leading_chunk`]): one vector per
//! document is what [`crate::state::ingestion`] writes, so one chunk per document is what
//! the index holds. AWS splits a document into as many chunks as its
//! `chunkingConfiguration` calls for and holds a vector for each; this emulator holds the
//! first of them, bounded by the chunk size the default chunking documents AND by a byte
//! ceiling of its own ([`CHUNK_MAX_BYTES`]), which is what keeps the text the store
//! retains bounded by the number of documents a data source may hold rather than by the
//! size of the objects a caller put in a bucket.

use ::aws_smithy_types::Document;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The metadata field a knowledge base records a chunk's TEXT under in its vector store,
/// as the published knowledge-base metadata fields name it. A retrieval of the chunk
/// answers this value as the result's content.
pub const CHUNK_TEXT_METADATA_KEY: &str = "AMAZON_BEDROCK_TEXT_CHUNK";

/// The metadata field a chunk's own id is recorded under. See
/// [`CHUNK_TEXT_METADATA_KEY`].
pub const CHUNK_ID_METADATA_KEY: &str = "x-amz-bedrock-kb-chunk-id";

/// The metadata field the data source the chunk was ingested from is recorded under. See
/// [`CHUNK_TEXT_METADATA_KEY`].
pub const DATA_SOURCE_ID_METADATA_KEY: &str = "x-amz-bedrock-kb-data-source-id";

/// The metadata field the id of a chunk's document is recorded under, for a document of a
/// CUSTOM data source: such a document is addressed by the id the request that ingested it
/// gave it rather than by the URI of a source it was read from, so that id is its
/// provenance, and a retrieval reports it as the result's own document location. The
/// sibling field a document read from an object store records instead is
/// `crate::state::SOURCE_URI_METADATA_KEY`. See [`CHUNK_TEXT_METADATA_KEY`].
pub const DOCUMENT_ID_METADATA_KEY: &str = "x-amz-bedrock-kb-document-id";

/// The metadata field the modality of the chunk's source file is recorded under. See
/// [`CHUNK_TEXT_METADATA_KEY`].
pub const SOURCE_FILE_MODALITY_METADATA_KEY: &str = "x-amz-bedrock-kb-source-file-modality";

/// The one `source-file-modality` an ingestion of this emulator records: it reads a
/// source document's bytes as text, and runs no extractor for the binary formats whose
/// text it would otherwise have to invent.
pub const SOURCE_FILE_MODALITY_TEXT: &str = "TEXT";

/// How many tokens the chunk an ingestion records holds at most: the default chunking of
/// a knowledge base keeps at most approximately 300 tokens in a chunk, and a document
/// shorter than that is not split.
pub const CHUNK_MAX_TOKENS: usize = 300;

/// How many BYTES that chunk holds at most.
///
/// The token bound alone does not bound the text: a token is a run between whitespace, so
/// a document with no whitespace at all is one token, and 300 of them can be as large as
/// the objects a caller put in a bucket. This is the emulator's own ceiling on what the
/// index retains per document, far above 300 tokens of ordinary prose (which is a couple
/// of kilobytes), and it is what keeps the retained text bounded by the number of
/// documents a data source may hold.
pub const CHUNK_MAX_BYTES: usize = 16 * 1024;

/// The leading chunk of `text`: its first [`CHUNK_MAX_TOKENS`] whitespace-separated
/// words, joined by single spaces, and at most [`CHUNK_MAX_BYTES`] of them.
///
/// Collapsing the whitespace is what makes the chunk a run of the document's own bytes a
/// client can find in it, and the two bounds are what keep what the index retains
/// bounded. A document carrying no word at all yields an empty chunk; one whose first
/// word is already past the byte bound yields that word's leading bytes, cut at a
/// character boundary so the chunk is always text.
pub fn leading_chunk(text: &str) -> String {
    let mut chunk = String::new();
    for word in text.split_whitespace().take(CHUNK_MAX_TOKENS) {
        let separator = usize::from(!chunk.is_empty());
        if chunk.len() + separator + word.len() > CHUNK_MAX_BYTES {
            if chunk.is_empty() {
                // One word longer than the whole bound: its leading bytes, cut where a
                // character ends.
                let mut end = CHUNK_MAX_BYTES;
                while end > 0 && !word.is_char_boundary(end) {
                    end -= 1;
                }
                chunk.push_str(&word[..end]);
            }
            break;
        }
        if separator == 1 {
            chunk.push(' ');
        }
        chunk.push_str(word);
    }
    chunk
}

/// The id of the chunk of the document at `uri` in `data_source_id`.
///
/// Derived from the chunk's own provenance — the data source and where its document
/// lives — so one chunk has one id for as long as this process serves it, it is distinct
/// for every chunk a knowledge base holds, and it is a value no request carries. The
/// digest is the standard library's (`DefaultHasher`, fixed-seeded), the same one this
/// plane tells an unchanged source document from a modified one by
/// ([`crate::state::ingestion`]); like that one it is not promised to survive a toolchain
/// change, which nothing here needs it to, since the index lives in this process's memory.
pub fn chunk_id(data_source_id: &str, uri: &str) -> String {
    let mut hasher = DefaultHasher::new();
    data_source_id.hash(&mut hasher);
    uri.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The metadata an ingestion records with one chunk's vector: the published fields above,
/// beside the source URI the ingestion keys its own bookkeeping by.
pub fn chunk_metadata(data_source_id: &str, uri: &str, chunk: &str, source_uri_key: &str) -> Document {
    Document::Object(HashMap::from([
        (
            source_uri_key.to_string(),
            Document::String(uri.to_string()),
        ),
        (
            CHUNK_TEXT_METADATA_KEY.to_string(),
            Document::String(chunk.to_string()),
        ),
        (
            CHUNK_ID_METADATA_KEY.to_string(),
            Document::String(chunk_id(data_source_id, uri)),
        ),
        (
            DATA_SOURCE_ID_METADATA_KEY.to_string(),
            Document::String(data_source_id.to_string()),
        ),
        (
            SOURCE_FILE_MODALITY_METADATA_KEY.to_string(),
            Document::String(SOURCE_FILE_MODALITY_TEXT.to_string()),
        ),
    ]))
}
