// SPDX-License-Identifier: CC0-1.0

//! Documents pushed STRAIGHT into a knowledge base, read back through `Retrieve`
//! (`POST /knowledgebases/{knowledgeBaseId}/retrieve`).
//!
//! THE RULE UNDER TEST: a document `IngestKnowledgeBaseDocuments` takes into a data source
//! is part of what the knowledge base HOLDS, so a retrieval over that knowledge base
//! reports one result for it: `content.text` is the text that document carried — the bytes
//! the request held for an inline document and the object's bytes for one that names an S3
//! location, multi-byte UTF-8 included — and `location` addresses the document the way its
//! data source's kind addresses one, which is the `s3://<bucket>/<key>` URI of the object
//! for a document of an S3 data source and the id it was ingested under for a document of a
//! custom data source. A document the data source stops holding stops being reported, and a
//! document whose bytes this emulator never held is reported by nothing rather than with
//! invented text. Which vector store the knowledge base keeps its vectors in decides WHERE
//! those chunks are read from, not whether they can be read: a knowledge base backed by a
//! store no service of this emulator serves answers the chunk its own ingestion recorded,
//! which does not move when the source does.
//!
//! Every assertion drives real HTTP requests through the framework router over ONE shared
//! engine carrying the four services this path spans, through the harness the sibling
//! `Retrieve` suite builds (`tests/all/knowledge_base_retrieval.rs`), so the documents a
//! retrieval answers are documents a caller really ingested.
//!
//! The rows below span what the rule generalises over: the member the document's bytes
//! travelled in (inline text, inline bytes, an object the content names), whether that text
//! is ASCII or multi-byte UTF-8, how its whitespace runs, how many documents one request
//! carries, the kind of data source the document belongs to, and the vector store the
//! knowledge base is backed by.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::ingestion_index as index_fields;

use crate::knowledge_base_retrieval::{collapsed, Emulator};
use crate::sessions::call;

/// The `DocumentStatus` a document the plane has finished ingesting reports, which the
/// modelled member documents as the successfully indexed state.
const INDEXED: &str = "INDEXED";

/// How long a poll waits for every document of a request to reach [`INDEXED`]: far above
/// the spans the Bedrock Agent plane's clock owes a document it accepted, so a loaded host
/// converges too.
const INDEXED_DEADLINE: Duration = Duration::from_secs(90);

/// How long one poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The three metadata keys a retrieval result carries: the chunk's own id, the data source
/// it came from and the modality of its source file. The provenance that addresses the
/// document is reported as the result's `location` instead, so neither provenance field is
/// among them.
const RESULT_METADATA_KEYS: [&str; 3] = [
    index_fields::CHUNK_ID_METADATA_KEY,
    index_fields::DATA_SOURCE_ID_METADATA_KEY,
    index_fields::SOURCE_FILE_MODALITY_METADATA_KEY,
];

/// The bucket the objects a document names live in.
const OBJECT_BUCKET: &str = "pushed-document-objects";

/// A role ARN matching the Bedrock Agent plane's modelled `KnowledgeBaseRoleArn`.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-embed-text-v2:0";

/// A collection ARN matching the modelled `OpenSearchServerlessCollectionArn` pattern
/// (`smithy_query(query="shape",shape="OpenSearchServerlessConfiguration")`).
const COLLECTION_ARN: &str = "arn:aws:aoss:us-west-2:000000000000:collection/kb-collection";

/// A secret ARN matching the modelled `SecretArn` pattern
/// (`smithy_query(query="shape",shape="PineconeConfiguration")`).
const PINECONE_SECRET: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:pinecone-api-key";

/// How a retrieval result names the document its chunk came from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Address {
    /// A document of a custom data source, by the id it was ingested under.
    Custom(String),
    /// A document of an S3 data source, by the object it lives at.
    S3(String),
}

/// One row of the round-trip table: the document as a request carries it, the identifier it
/// is stored under, the text a retrieval must answer for it, and how a result addresses it.
struct Row {
    case: &'static str,
    /// Whether the document belongs to the custom data source or the S3 one.
    custom_data_source: bool,
    document: Value,
    identifier: Value,
    /// The text the document carries, before the chunk's whitespace collapsing.
    text: String,
    address: Address,
}

/// A custom document whose content is defined inline as text.
fn inline_text_document(id: &str, text: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": { "type": "TEXT", "textContent": { "data": text } },
            },
        },
    })
}

/// A custom document whose content is defined inline in bytes, which travel base64-encoded
/// the way the restJson1 wire carries a `@blob`.
fn inline_byte_document(id: &str, text: &str) -> Value {
    let data = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "IN_LINE",
                "inlineContent": {
                    "type": "BYTE",
                    "byteContent": { "mimeType": "text/plain", "data": data },
                },
            },
        },
    })
}

/// A custom document whose content is the object it names.
fn custom_object_document(id: &str, uri: &str) -> Value {
    json!({
        "content": {
            "dataSourceType": "CUSTOM",
            "custom": {
                "customDocumentIdentifier": { "id": id },
                "sourceType": "S3_LOCATION",
                "s3Location": { "uri": uri },
            },
        },
    })
}

/// A document of an S3 data source, which the object's URI identifies.
fn s3_document(uri: &str) -> Value {
    json!({
        "content": { "dataSourceType": "S3", "s3": { "s3Location": { "uri": uri } } },
    })
}

/// The identifier a custom document is stored under.
fn custom_identifier(id: &str) -> Value {
    json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } })
}

/// The identifier a document of an S3 data source is stored under.
fn s3_identifier(uri: &str) -> Value {
    json!({ "dataSourceType": "S3", "s3": { "uri": uri } })
}

/// Attach a CUSTOM data source — the kind documents are pushed into rather than crawled
/// from a source of its own — and answer its minted id.
async fn custom_data_source(emulator: &Emulator, knowledge_base_id: &str, name: &str) -> String {
    let (status, body) = call(
        &emulator.router,
        "PUT",
        &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
        json!({ "name": name, "dataSourceConfiguration": { "type": "CUSTOM" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateDataSource -> 200, got {status} {body:?}"
    );
    body["dataSource"]["dataSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no dataSourceId in {body:?}"))
        .to_string()
}

/// Push `documents` into one data source, asserting the ingestion was accepted, and answer
/// the `documentDetails` it reported.
async fn ingest(
    emulator: &Emulator,
    knowledge_base_id: &str,
    data_source_id: &str,
    documents: Vec<Value>,
) -> Vec<Value> {
    let (status, body) = call(
        &emulator.router,
        "PUT",
        &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
        json!({ "documents": documents }),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: expected IngestKnowledgeBaseDocuments -> 202, got {status} {body:?}"
    );
    body["documentDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("setup: no documentDetails in {body:?}"))
        .clone()
}

/// Accept the deletion of `identifiers` from one data source.
async fn delete_documents(
    emulator: &Emulator,
    knowledge_base_id: &str,
    data_source_id: &str,
    identifiers: Vec<Value>,
) {
    let (status, body) = call(
        &emulator.router,
        "POST",
        &format!(
            "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
             /documents/deleteDocuments"
        ),
        json!({ "documentIdentifiers": identifiers }),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: expected DeleteKnowledgeBaseDocuments -> 202, got {status} {body:?}"
    );
}

/// Look again until every document of `identifiers` reports [`INDEXED`], the terminal
/// status the plane's clock owes a document it accepted — the CONDITION the retrieval below
/// is about, rather than a fixed wait.
async fn wait_until_indexed(
    emulator: &Emulator,
    knowledge_base_id: &str,
    data_source_id: &str,
    identifiers: &[Value],
) {
    let deadline = Instant::now() + INDEXED_DEADLINE;
    loop {
        let (status, body) = call(
            &emulator.router,
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /documents/getDocuments"
            ),
            json!({ "documentIdentifiers": identifiers }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: expected GetKnowledgeBaseDocuments -> 200, got {status} {body:?}"
        );
        let indexed = body["documentDetails"]
            .as_array()
            .map(|details| {
                details
                    .iter()
                    .filter(|detail| detail["status"].as_str() == Some(INDEXED))
                    .count()
            })
            .unwrap_or_default();
        if indexed == identifiers.len() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: expected all {} documents to reach {INDEXED:?}, got {body:?}",
            identifiers.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Retrieve from one knowledge base, asserting the call succeeded, and answer the
/// `retrievalResults` list. `keep` bounds the page, so a bound above the number of
/// documents ingested reports every one of them.
async fn results(
    emulator: &Emulator,
    case: &str,
    knowledge_base_id: &str,
    query: &str,
    keep: usize,
) -> Vec<Value> {
    let (status, body) = call(
        &emulator.router,
        "POST",
        &format!("/knowledgebases/{knowledge_base_id}/retrieve"),
        json!({
            "retrievalQuery": { "text": query },
            "retrievalConfiguration": {
                "vectorSearchConfiguration": { "numberOfResults": keep },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Retrieve to answer 200, got {status} {body:?}"
    );
    body["retrievalResults"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no retrievalResults in {body:?}"))
        .clone()
}

/// The document one result addresses, asserting the location carries the member its own
/// `type` names and no other.
fn address(case: &str, result: &Value) -> Address {
    let location = &result["location"];
    match location["type"].as_str() {
        Some("CUSTOM") => {
            assert!(
                location.get("s3Location").is_none(),
                "case {case:?}: expected only the custom document location, got {location:?}"
            );
            let id = location["customDocumentLocation"]["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no customDocumentLocation.id in {result:?}")
                });
            Address::Custom(id.to_string())
        }
        Some("S3") => {
            assert!(
                location.get("customDocumentLocation").is_none(),
                "case {case:?}: expected only the S3 location, got {location:?}"
            );
            let uri = location["s3Location"]["uri"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no s3Location.uri in {result:?}"));
            Address::S3(uri.to_string())
        }
        other => panic!("case {case:?}: expected location.type S3 or CUSTOM, got {other:?}"),
    }
}

/// The text of one result, asserting the content is reported as text.
fn content_text(case: &str, result: &Value) -> String {
    let content = &result["content"];
    assert_eq!(
        content["type"].as_str(),
        Some("TEXT"),
        "case {case:?}: expected content.type TEXT, got {content:?}"
    );
    content["text"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no content.text in {result:?}"))
        .to_string()
}

/// The metadata map of one result, asserting every value is a string.
fn metadata(case: &str, result: &Value) -> BTreeMap<String, String> {
    result["metadata"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: no metadata in {result:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: metadata {key:?} is not a string in {result:?}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The one result of `results` addressing `address`, asserting exactly one does: a document
/// has ONE chunk in the index, however often it was ingested.
fn result_for<'results>(
    case: &str,
    results: &'results [Value],
    address_wanted: &Address,
) -> &'results Value {
    let matching: Vec<&Value> = results
        .iter()
        .filter(|result| &address(case, result) == address_wanted)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one result addressing {address_wanted:?}, got {:?}",
        results
            .iter()
            .map(|result| address(case, result))
            .collect::<Vec<Address>>()
    );
    matching[0]
}

/// A document with irregular whitespace, so the chunk's own collapsing is exercised.
const SHIFT_NOTES: &str = "Shift notes: the klystron bay was purged on Tuesday.\n\n   The \
     spare waveguide\tis in locker nine.";

/// A document carrying multi-byte UTF-8 in several scripts, including an emoji outside the
/// basic multilingual plane.
const MULTI_BYTE: &str = "Zeppelin dossier: café münchen 日本語 emoji 🎉 round trip";

/// RULE: every document pushed into a data source is retrievable from the knowledge base
/// with the text it carried and a location that addresses it the way its data source's kind
/// addresses a document.
#[tokio::test]
async fn every_pushed_document_is_retrievable_with_the_text_it_carried() {
    let emulator = Emulator::new("pushed-index");
    let object_uri = format!("s3://{OBJECT_BUCKET}/reports/q3.txt");
    let crawlable_uri = format!("s3://{OBJECT_BUCKET}/reports/ledger.txt");
    let object_text = "Quarterly report: the klystron inventory closed at forty units.";
    let crawlable_text = "Ledger: the propulsion team drew nine spare waveguides.";
    emulator.put_object(OBJECT_BUCKET, "reports/q3.txt", object_text);
    emulator.put_object(OBJECT_BUCKET, "reports/ledger.txt", crawlable_text);

    let knowledge_base = emulator.knowledge_base("pushed-kb", "pushed-index").await;
    let custom = custom_data_source(&emulator, &knowledge_base, "pushed-custom").await;
    let crawled = emulator
        .data_source(&knowledge_base, "pushed-s3", OBJECT_BUCKET, None)
        .await;

    let rows = vec![
        Row {
            case: "inline text",
            custom_data_source: true,
            document: inline_text_document("launch-codes", "The Thunderbird launch code is TBX-9."),
            identifier: custom_identifier("launch-codes"),
            text: "The Thunderbird launch code is TBX-9.".to_string(),
            address: Address::Custom("launch-codes".to_string()),
        },
        Row {
            case: "inline text with irregular whitespace",
            custom_data_source: true,
            document: inline_text_document("shift-notes", SHIFT_NOTES),
            identifier: custom_identifier("shift-notes"),
            text: SHIFT_NOTES.to_string(),
            address: Address::Custom("shift-notes".to_string()),
        },
        Row {
            case: "inline bytes",
            custom_data_source: true,
            document: inline_byte_document("cafeteria", "The cafeteria serves lentil soup."),
            identifier: custom_identifier("cafeteria"),
            text: "The cafeteria serves lentil soup.".to_string(),
            address: Address::Custom("cafeteria".to_string()),
        },
        Row {
            case: "inline bytes carrying multi-byte utf-8",
            custom_data_source: true,
            document: inline_byte_document("zeppelin", MULTI_BYTE),
            identifier: custom_identifier("zeppelin"),
            text: MULTI_BYTE.to_string(),
            address: Address::Custom("zeppelin".to_string()),
        },
        Row {
            case: "a custom document whose content is the object it names",
            custom_data_source: true,
            document: custom_object_document("quarterly", &object_uri),
            identifier: custom_identifier("quarterly"),
            text: object_text.to_string(),
            address: Address::Custom("quarterly".to_string()),
        },
        Row {
            case: "a document of an S3 data source",
            custom_data_source: false,
            document: s3_document(&crawlable_uri),
            identifier: s3_identifier(&crawlable_uri),
            text: crawlable_text.to_string(),
            address: Address::S3(crawlable_uri.clone()),
        },
    ];

    // Each data source takes the documents that belong to it, in ONE request: what a
    // request carries is a list, so several documents are ingested together.
    for into_custom in [true, false] {
        let data_source = if into_custom { &custom } else { &crawled };
        let documents: Vec<Value> = rows
            .iter()
            .filter(|row| row.custom_data_source == into_custom)
            .map(|row| row.document.clone())
            .collect();
        let identifiers: Vec<Value> = rows
            .iter()
            .filter(|row| row.custom_data_source == into_custom)
            .map(|row| row.identifier.clone())
            .collect();
        let details = ingest(&emulator, &knowledge_base, data_source, documents).await;
        assert_eq!(
            details.len(),
            identifiers.len(),
            "setup: expected one detail per document ingested, got {details:?}"
        );
        wait_until_indexed(&emulator, &knowledge_base, data_source, &identifiers).await;
    }

    // One retrieval, bounded above the number of documents ingested, so every chunk the
    // knowledge base holds is in the answer.
    let answered = results(
        &emulator,
        "every pushed document",
        &knowledge_base,
        "launch code cafeteria klystron waveguide zeppelin",
        rows.len() + 2,
    )
    .await;
    let addressed: BTreeSet<Address> = answered
        .iter()
        .map(|result| address("every pushed document", result))
        .collect();
    let expected: BTreeSet<Address> = rows.iter().map(|row| row.address.clone()).collect();
    assert_eq!(
        addressed, expected,
        "case \"every pushed document\": expected one result per ingested document, got {addressed:?}"
    );

    for row in &rows {
        let case = row.case;
        let result = result_for(case, &answered, &row.address);
        assert_eq!(
            content_text(case, result),
            collapsed(&row.text),
            "case {case:?}: expected the text the document carried, got {result:?}"
        );
        let metadata = metadata(case, result);
        let keys: BTreeSet<&str> = metadata.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            RESULT_METADATA_KEYS.into_iter().collect::<BTreeSet<&str>>(),
            "case {case:?}: expected the published chunk metadata fields, got {metadata:?}"
        );
        let data_source = if row.custom_data_source {
            &custom
        } else {
            &crawled
        };
        assert_eq!(
            metadata
                .get(index_fields::DATA_SOURCE_ID_METADATA_KEY)
                .map(String::as_str),
            Some(data_source.as_str()),
            "case {case:?}: expected the data source the document was ingested into, got {metadata:?}"
        );
        assert_eq!(
            metadata
                .get(index_fields::SOURCE_FILE_MODALITY_METADATA_KEY)
                .map(String::as_str),
            Some(index_fields::SOURCE_FILE_MODALITY_TEXT),
            "case {case:?}: expected the modality of a text source, got {metadata:?}"
        );
        assert!(
            metadata
                .get(index_fields::CHUNK_ID_METADATA_KEY)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the chunk's own id, got {metadata:?}"
        );
        let score = result["score"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: no score in {result:?}"));
        assert!(
            (0.0..=1.0).contains(&score),
            "case {case:?}: expected a score in [0, 1], got {score}"
        );
        assert!(
            result.get("documentId").is_none() || result["documentId"].is_string(),
            "case {case:?}: expected documentId absent or a string, got {result:?}"
        );
    }
}

/// RULE, the other half: a document the data source stops holding stops being retrievable,
/// and the documents beside it are untouched.
#[tokio::test]
async fn a_deleted_document_is_no_longer_retrievable() {
    let emulator = Emulator::new("deleted-index");
    let knowledge_base = emulator
        .knowledge_base("deleted-kb", "deleted-index")
        .await;
    let data_source = custom_data_source(&emulator, &knowledge_base, "deleted-custom").await;

    let kept_text = "The Albatross launch code is ALB-3.";
    let dropped_text = "The Thunderbird launch code is TBX-9.";
    let identifiers = vec![custom_identifier("kept"), custom_identifier("dropped")];
    ingest(
        &emulator,
        &knowledge_base,
        &data_source,
        vec![
            inline_text_document("kept", kept_text),
            inline_text_document("dropped", dropped_text),
        ],
    )
    .await;
    wait_until_indexed(&emulator, &knowledge_base, &data_source, &identifiers).await;

    let before = results(
        &emulator,
        "both documents ingested",
        &knowledge_base,
        "launch code",
        10,
    )
    .await;
    let addressed: BTreeSet<Address> = before
        .iter()
        .map(|result| address("both documents ingested", result))
        .collect();
    assert_eq!(
        addressed,
        BTreeSet::from([
            Address::Custom("kept".to_string()),
            Address::Custom("dropped".to_string()),
        ]),
        "case \"both documents ingested\": expected both documents retrievable, got {addressed:?}"
    );

    delete_documents(
        &emulator,
        &knowledge_base,
        &data_source,
        vec![custom_identifier("dropped")],
    )
    .await;

    let case = "one document deleted";
    let after = results(&emulator, case, &knowledge_base, "launch code", 10).await;
    let addressed: BTreeSet<Address> = after.iter().map(|result| address(case, result)).collect();
    assert_eq!(
        addressed,
        BTreeSet::from([Address::Custom("kept".to_string())]),
        "case {case:?}: expected only the document still held, got {addressed:?}"
    );
    let kept = result_for(case, &after, &Address::Custom("kept".to_string()));
    assert_eq!(
        content_text(case, kept),
        collapsed(kept_text),
        "case {case:?}: expected the surviving document's own text, got {kept:?}"
    );
}

/// Create a knowledge base whose vectors live in the store `storage` names, and answer its
/// minted id — the sibling harness mints the one backed by an S3 Vectors index, and this is
/// every other store the modelled `KnowledgeBaseStorageType` admits.
async fn knowledge_base_backed_by(emulator: &Emulator, name: &str, storage: Value) -> String {
    let (status, body) = call(
        &emulator.router,
        "PUT",
        "/knowledgebases/",
        json!({
            "name": name,
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": {
                "type": "VECTOR",
                "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
            },
            "storageConfiguration": storage,
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: expected CreateKnowledgeBase -> 202, got {status} {body:?}"
    );
    body["knowledgeBase"]["knowledgeBaseId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no knowledgeBaseId in {body:?}"))
        .to_string()
}

/// RULE: which vector store a knowledge base keeps its vectors in decides WHERE a retrieval
/// reads its chunks, not whether it can read them — a knowledge base backed by a store no
/// service of this emulator serves answers the chunk its own ingestion recorded, with the
/// same text, location and metadata, and never a refusal; and that chunk is the text the
/// ingestion read, so rewriting the source afterwards does not change it.
///
/// The rows are two of the stores the modelled `KnowledgeBaseStorageType` admits beside the
/// S3 Vectors index the sibling suite covers
/// (`smithy_query(query="shape",shape="KnowledgeBaseStorageType")`).
#[tokio::test]
async fn a_knowledge_base_backed_by_an_external_vector_store_is_still_retrievable() {
    let rows: Vec<(&str, Value)> = vec![
        (
            "an OpenSearch Serverless collection",
            json!({
                "type": "OPENSEARCH_SERVERLESS",
                "opensearchServerlessConfiguration": {
                    "collectionArn": COLLECTION_ARN,
                    "vectorIndexName": "bedrock-knowledge-base-index",
                    "fieldMapping": {
                        "vectorField": "bedrock-knowledge-base-default-vector",
                        "textField": "AMAZON_BEDROCK_TEXT_CHUNK",
                        "metadataField": "AMAZON_BEDROCK_METADATA",
                    },
                },
            }),
        ),
        (
            "a Pinecone index",
            json!({
                "type": "PINECONE",
                "pineconeConfiguration": {
                    "connectionString": "https://example-index.svc.pinecone.io",
                    "credentialsSecretArn": PINECONE_SECRET,
                    "fieldMapping": {
                        "textField": "text",
                        "metadataField": "metadata",
                    },
                },
            }),
        ),
    ];

    for (case, storage) in rows {
        let emulator = Emulator::new("external-index");
        let ledger_uri = format!("s3://{OBJECT_BUCKET}/reports/ledger.txt");
        let ledger = "Ledger: the propulsion team drew nine spare waveguides.";
        emulator.put_object(OBJECT_BUCKET, "reports/ledger.txt", ledger);
        let knowledge_base = knowledge_base_backed_by(&emulator, "external-kb", storage).await;
        let data_source = custom_data_source(&emulator, &knowledge_base, "external-custom").await;
        let manifest = "Manifest: the klystron bay holds forty spare units.";
        let identifiers = vec![
            custom_identifier("manifest"),
            custom_identifier("zeppelin"),
            custom_identifier("ledger"),
        ];
        ingest(
            &emulator,
            &knowledge_base,
            &data_source,
            vec![
                inline_text_document("manifest", manifest),
                inline_byte_document("zeppelin", MULTI_BYTE),
                custom_object_document("ledger", &ledger_uri),
            ],
        )
        .await;
        wait_until_indexed(&emulator, &knowledge_base, &data_source, &identifiers).await;

        let answered = results(&emulator, case, &knowledge_base, "zeppelin dossier", 10).await;
        let addressed: BTreeSet<Address> =
            answered.iter().map(|result| address(case, result)).collect();
        assert_eq!(
            addressed,
            BTreeSet::from([
                Address::Custom("manifest".to_string()),
                Address::Custom("zeppelin".to_string()),
                Address::Custom("ledger".to_string()),
            ]),
            "case {case:?}: expected every ingested document retrievable, got {addressed:?}"
        );
        for (id, text) in [
            ("manifest", manifest),
            ("zeppelin", MULTI_BYTE),
            ("ledger", ledger),
        ] {
            let result = result_for(case, &answered, &Address::Custom(id.to_string()));
            assert_eq!(
                content_text(case, result),
                collapsed(text),
                "case {case:?}: expected the text document {id:?} carried, got {result:?}"
            );
            let metadata = metadata(case, result);
            let keys: BTreeSet<&str> = metadata.keys().map(String::as_str).collect();
            assert_eq!(
                keys,
                RESULT_METADATA_KEYS.into_iter().collect::<BTreeSet<&str>>(),
                "case {case:?}: expected the published chunk metadata fields, got {metadata:?}"
            );
            assert_eq!(
                metadata
                    .get(index_fields::DATA_SOURCE_ID_METADATA_KEY)
                    .map(String::as_str),
                Some(data_source.as_str()),
                "case {case:?}: expected the data source the document was ingested into, \
                 got {metadata:?}"
            );
        }

        // What the ingestion recorded does not move when the SOURCE moves: the chunk is
        // the text that ingestion read, so rewriting the object afterwards leaves the
        // citation where it was — the same invariant the bound-index store holds.
        emulator.put_object(
            OBJECT_BUCKET,
            "reports/ledger.txt",
            "Ledger: every waveguide was returned to the klystron bay.",
        );
        let rewritten = results(&emulator, case, &knowledge_base, "ledger waveguides", 10).await;
        let held = result_for(case, &rewritten, &Address::Custom("ledger".to_string()));
        assert_eq!(
            content_text(case, held),
            collapsed(ledger),
            "case {case:?}: expected the text the ingestion read, got {held:?}"
        );

        // The same half of the rule as for the store this emulator serves: a document the
        // data source stops holding stops being reported.
        delete_documents(
            &emulator,
            &knowledge_base,
            &data_source,
            vec![custom_identifier("manifest")],
        )
        .await;
        let after = results(&emulator, case, &knowledge_base, "zeppelin dossier", 10).await;
        let addressed: BTreeSet<Address> =
            after.iter().map(|result| address(case, result)).collect();
        assert_eq!(
            addressed,
            BTreeSet::from([
                Address::Custom("zeppelin".to_string()),
                Address::Custom("ledger".to_string()),
            ]),
            "case {case:?}: expected only the documents still held, got {addressed:?}"
        );
    }
}

/// RULE, the refusal to invent: a document whose bytes this emulator never held is reported
/// by no retrieval — nothing answers a chunk for content it did not read.
#[tokio::test]
async fn a_document_whose_bytes_are_nowhere_is_reported_by_nothing() {
    let emulator = Emulator::new("missing-index");
    let present_uri = format!("s3://{OBJECT_BUCKET}/reports/present.txt");
    let absent_uri = format!("s3://{OBJECT_BUCKET}/reports/absent.txt");
    let present_text = "Present report: the klystron inventory closed at forty units.";
    emulator.put_object(OBJECT_BUCKET, "reports/present.txt", present_text);

    let knowledge_base = emulator
        .knowledge_base("missing-kb", "missing-index")
        .await;
    let data_source = custom_data_source(&emulator, &knowledge_base, "missing-custom").await;
    let identifiers = vec![custom_identifier("present"), custom_identifier("absent")];
    ingest(
        &emulator,
        &knowledge_base,
        &data_source,
        vec![
            custom_object_document("present", &present_uri),
            custom_object_document("absent", &absent_uri),
        ],
    )
    .await;
    wait_until_indexed(&emulator, &knowledge_base, &data_source, &identifiers).await;

    let case = "one document's object is not in the object store";
    let answered = results(&emulator, case, &knowledge_base, "klystron inventory", 10).await;
    let addressed: BTreeSet<Address> = answered
        .iter()
        .map(|result| address(case, result))
        .collect();
    assert_eq!(
        addressed,
        BTreeSet::from([Address::Custom("present".to_string())]),
        "case {case:?}: expected only the document whose bytes the emulator holds, got {addressed:?}"
    );
    let present = result_for(case, &answered, &Address::Custom("present".to_string()));
    assert_eq!(
        content_text(case, present),
        collapsed(present_text),
        "case {case:?}: expected the object's own text, got {present:?}"
    );
}
