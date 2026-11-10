// SPDX-License-Identifier: CC0-1.0

//! What `GetDocumentContent` may hand a caller: the SOURCE OBJECT one ingested document
//! of a knowledge base was taken from, and where a caller can fetch it.
//!
//! A knowledge base, its data sources and the documents their ingestions took in all
//! belong to the Bedrock Agent build-time plane, and the bytes of a document taken from an
//! S3 data source belong to the object store. This plane writes neither, so everything
//! here is a READ of those two stores, in that lock order:
//!
//! * a knowledge base, a data source or a document that plane does not hold is the
//!   modelled `ResourceNotFoundException`, never an invented document;
//! * the content a caller is pointed at is the OBJECT the ingestion recorded for that
//!   document, so the `mimeType` and the length reported are the object store's own and
//!   the URL leads to that object in this emulator — a client that follows it reads the
//!   same bytes an ingestion read;
//! * a document whose bytes the ingestion carried INLINE (the `CUSTOM` content a caller
//!   pushes in through the build-time plane's document API) lives in no object store, so
//!   there is no address a later caller could fetch it from. That is refused rather than
//!   answered with a URL that leads nowhere.

use std::time::SystemTime;

use ::service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state as agent_state;
use ::service_com_amazonaws_s3_amazons3::state as s3_state;

/// Why a document's content cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRefusal {
    /// A store this read needs is not on this engine, so nothing can be said about the
    /// document — answered with a modelled server error, never with a not-found.
    StoreUnavailable,
    /// The Bedrock Agent plane holds no knowledge base of that identifier.
    UnknownKnowledgeBase,
    /// That knowledge base holds no data source of that identifier.
    UnknownDataSource,
    /// That data source holds no document of that identifier.
    UnknownDocument,
    /// The document was ingested with its bytes in the request rather than from an object
    /// store, so this emulator holds no address a later caller could fetch it from.
    NotInObjectStore,
    /// The document names a source object the object store no longer holds, so its content
    /// cannot be described or fetched. The string is the URI the ingestion recorded.
    SourceObjectGone(String),
}

/// The content one ingested document was taken from: where it lives, what it is, and how
/// long it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentContent {
    /// The bucket the source object lives in.
    pub bucket: String,
    /// The key of the source object within that bucket.
    pub key: String,
    /// The object's own content type, as the object store holds it.
    pub mime_type: String,
    /// How many bytes the object carries.
    pub length: i64,
}

/// The `DocumentOutputFormat` a request that names none takes: the document as it was
/// ingested (`smithy_query(query="shape",shape="DocumentOutputFormat")` -> RAW,
/// EXTRACTED).
pub const OUTPUT_FORMAT_RAW: &str = "RAW";

/// The `DocumentOutputFormat` naming the text extracted from the document rather than the
/// document itself. See [`OUTPUT_FORMAT_RAW`].
pub const OUTPUT_FORMAT_EXTRACTED: &str = "EXTRACTED";

/// The content of the document `document_id` of `data_source_id` in `knowledge_base_id`.
///
/// `document_id` is the document's own identity within its data source: the id a custom
/// document was given, or the `s3://` URI an S3 document lives at
/// (`agent_state::DocumentKey`), so both forms of identity resolve here the way the plane
/// that owns them records them.
///
/// READ-ONLY for everything a later caller can observe. The one write on its path is the
/// Bedrock Agent plane materialising the status transitions its own clock had already made
/// due, which is owed whether or not anybody calls.
///
/// LOCK ORDER: the Bedrock Agent plane's store first and released before the object store
/// is reached, the order every cross-plane path of these services keeps.
pub fn document_content<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    data_source_id: &str,
    document_id: &str,
) -> Result<DocumentContent, ContentRefusal> {
    let source_uri = source_uri_of(engine, knowledge_base_id, data_source_id, document_id)?;
    let Some((bucket, key)) = source_uri
        .strip_prefix("s3://")
        .and_then(|rest| rest.split_once('/'))
        .filter(|(bucket, key)| !bucket.is_empty() && !key.is_empty())
    else {
        return Err(ContentRefusal::SourceObjectGone(source_uri));
    };
    let Some(objects) = s3_state::service_state(engine) else {
        return Err(ContentRefusal::StoreUnavailable);
    };
    let store = objects.lock();
    let object = store
        .bucket(bucket)
        .and_then(|held| held.object(key))
        .ok_or_else(|| ContentRefusal::SourceObjectGone(source_uri.clone()))?;
    Ok(DocumentContent {
        bucket: bucket.to_string(),
        key: key.to_string(),
        // The object store's own content type, which is what a `GetObject` of the same
        // object reports; S3 stores its documented default for a write that named none.
        mime_type: object
            .content_type
            .clone()
            .unwrap_or_else(|| s3_state::DEFAULT_CONTENT_TYPE.to_string()),
        length: i64::try_from(object.body.len()).unwrap_or(i64::MAX),
    })
}

/// The `s3://` URI the ingestion recorded as the source of that one document, read under
/// the Bedrock Agent plane's own guard.
fn source_uri_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_id: &str,
    data_source_id: &str,
    document_id: &str,
) -> Result<String, ContentRefusal> {
    let Some(agents) = agent_state::service_state(engine) else {
        return Err(ContentRefusal::StoreUnavailable);
    };
    let mut store = agents.lock();
    store.materialise_due_transitions(SystemTime::now());
    if store.live_knowledge_base(knowledge_base_id).is_none() {
        return Err(ContentRefusal::UnknownKnowledgeBase);
    }
    if store
        .live_data_source_of(knowledge_base_id, data_source_id)
        .is_none()
    {
        return Err(ContentRefusal::UnknownDataSource);
    }
    // A document is identified within its data source by the kind of source it came from
    // and its own id in that source, so both kinds the plane records are looked for: the
    // id a custom document was pushed in under, and the URI an S3 document lives at.
    let document = [
        agent_state::DocumentKey::Custom(document_id.to_string()),
        agent_state::DocumentKey::S3(document_id.to_string()),
    ]
    .into_iter()
    .find_map(|key| store.document_of(knowledge_base_id, data_source_id, &key))
    .ok_or(ContentRefusal::UnknownDocument)?;

    // Where the document's bytes live. An S3 data source's document IS the object at that
    // URI, and a custom document pushed in with an S3 location names one too; a custom
    // document whose bytes travelled in the request has no such address.
    let content = &document.content;
    content
        .s3
        .as_ref()
        .map(|s3| s3.s3_location.uri.clone())
        .or_else(|| {
            content
                .custom
                .as_ref()
                .and_then(|custom| custom.s3_location.as_ref())
                .map(|location| location.uri.clone())
        })
        .ok_or(ContentRefusal::NotInObjectStore)
}

/// The `ResourceNotFoundException` message a content read answers for a knowledge base,
/// data source or document this emulator holds nothing for.
///
/// One sentence per resource KIND, built from the identifiers the request carried: the
/// variable part is the identifier of the resource that was not found, the rest is fixed
/// text for this error class, as AWS words it for this operation ("Knowledge Base with id
/// <id> does not exist"). The sibling resources are worded the same way, naming the kind
/// that was not found — a client reads the kind and the id out of the sentence to fix its
/// request.
pub fn not_found_message(
    refusal: &ContentRefusal,
    knowledge_base_id: &str,
    data_source_id: &str,
    document_id: &str,
) -> String {
    match refusal {
        ContentRefusal::UnknownDataSource => {
            format!("Data Source with id {data_source_id} does not exist")
        }
        ContentRefusal::UnknownDocument => {
            format!("Document with id {document_id} does not exist")
        }
        ContentRefusal::SourceObjectGone(uri) => format!(
            "Document with id {document_id} does not exist: its source {uri} is no longer \
             held by the object store"
        ),
        // Every other refusal is answered with another error entirely; the knowledge base
        // is the resource a request that got no further than its identifier names.
        _ => format!("Knowledge Base with id {knowledge_base_id} does not exist"),
    }
}
