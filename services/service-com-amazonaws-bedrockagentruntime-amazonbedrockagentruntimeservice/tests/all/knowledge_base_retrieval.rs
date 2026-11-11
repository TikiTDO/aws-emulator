// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Retrieve` (`POST /knowledgebases/{knowledgeBaseId}/retrieve`).
//!
//! Every test drives real HTTP requests through the framework router over ONE shared
//! engine carrying four services — this one, the Bedrock Agent build-time plane that owns
//! knowledge bases and ingestion jobs, S3, which owns the objects a data source is
//! crawled from, and S3 Vectors, which owns the index a knowledge base is bound to — so
//! the knowledge base a retrieval reads is one a caller really created and the chunks it
//! answers are the ones an ingestion really recorded.
//!
//! THE RULE UNDER TEST: a retrieval answers one result per chunk the knowledge base's
//! bound vector index holds, nearest to the query first; each result's `content.text` is
//! the text the ingestion recorded for that chunk, its `location` names the source the
//! ingestion read it from (`s3://<bucket>/<key>` for an S3 data source) and its
//! `metadata` carries the chunk's own id, the id of the data source it came from and the
//! modality of its source file. A knowledge base the control plane does not hold is the
//! modelled `ResourceNotFoundException` (404). What an ingestion recorded does not move
//! when the SOURCE moves, so a citation of an object deleted since stands until another
//! job reconciles it.
//!
//! The [`Emulator`] harness below is `pub(crate)`, so the sibling suite that drives
//! `RetrieveAndGenerate` over the same four services uses it rather than growing a second
//! copy of it (see `tests/all/retrieve_and_generate.rs`).
//!
//! The rows below span what that rule generalises over: how many source documents a data
//! source holds and how long each one is, which keys the inclusion prefixes admit,
//! whether a metadata file sits beside a document, how many data sources one knowledge
//! base has, the form of the `knowledgeBaseId` (bare id and ARN), which query is asked,
//! and the filter and result bounds the request carries.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::ingestion_index as index_fields;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    INGESTION_JOB_RUN_SPAN, INGESTION_JOB_START_SPAN,
};
use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::retrieval_filter::{
    MAX_FILTER_DEPTH, MAX_FILTER_NODES,
};
use service_com_amazonaws_s3_amazons3::state as s3_state;
use service_com_amazonaws_s3vectors_s3vectors::state as vectors_state;

use crate::sessions::{call, error_code, label, REGION};

/// A role ARN matching the Bedrock Agent plane's modelled `KnowledgeBaseRoleArn`.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:us-west-2::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket every knowledge base here binds to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/retrieval-vectors";

/// The number of values the vectors of the embedding model above carry, which is the
/// dimension the bound index is created with.
const EMBEDDING_DIMENSION: i32 = 1024;

/// The `dataType` of the index: the one value the modelled `DataType` enum admits.
const VECTOR_DATA_TYPE: &str = "float32";

/// A margin over a span the clock owes, so a test waiting for a transition is not racing
/// the instant it comes due.
const MARGIN: Duration = Duration::from_millis(150);

/// The three metadata keys a retrieval result of an S3 data source carries.
const RESULT_METADATA_KEYS: [&str; 3] = [
    index_fields::CHUNK_ID_METADATA_KEY,
    index_fields::DATA_SOURCE_ID_METADATA_KEY,
    index_fields::SOURCE_FILE_MODALITY_METADATA_KEY,
];

/// One emulator: a router over one shared engine carrying the four services the retrieval
/// path spans, plus the engine handle, so a test can set up the vector index and the
/// source objects the way a caller does before naming them.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// One emulator whose bound vector index exists, as a caller sets it up before the
    /// knowledge base names it.
    pub(crate) fn new(index: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        seed_index(&engine, index);
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
                engine.clone(),
            ),
        );
        router.register_service(
            service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
                engine.clone(),
            ),
        );
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(engine.clone()),
        );
        router.register_service(
            service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(engine.clone()),
        );
        Self { router, engine }
    }

    /// Store `text` at `bucket`/`key`, creating the bucket on the first write — the state
    /// a caller's `CreateBucket` + `PutObject` leaves behind, written through S3's own
    /// store so the objects an ingestion crawls are the objects S3 serves.
    pub(crate) fn put_object(&self, bucket: &str, key: &str, text: &str) {
        let state = s3_state::service_state(&*self.engine).expect("the S3 store is on this engine");
        let mut store = state.lock();
        let _ = store.create_bucket(bucket, REGION);
        let write = store.put_object(
            bucket,
            key,
            s3_state::Object::stored(text.as_bytes().to_vec(), Some("text/plain".to_string())),
        );
        assert!(
            write.is_some(),
            "setup: expected s3://{bucket}/{key} to be stored, got {write:?}"
        );
    }

    /// Delete the object at `bucket`/`key` — the state a caller's `DeleteObject` leaves
    /// behind, so a test can see what a retrieval cites once a source is gone.
    fn delete_object(&self, bucket: &str, key: &str) {
        let state = s3_state::service_state(&*self.engine).expect("the S3 store is on this engine");
        let removed = state.lock().remove_object(bucket, key);
        assert!(
            removed.is_some(),
            "setup: expected s3://{bucket}/{key} to be removed, got {removed:?}"
        );
    }

    /// Delete the index `index` of [`VECTOR_BUCKET`], so the knowledge base bound to it is
    /// bound to an index the vector store no longer holds.
    fn drop_index(&self, index: &str) {
        let bucket = vectors_state::bucket_from_arn("vectorBucketArn", VECTOR_BUCKET)
            .expect("the suite's vectorBucketArn is a vector bucket ARN");
        let state =
            vectors_state::service_state(&*self.engine).expect("the S3 Vectors store on this engine");
        let mut store = state.lock();
        let removed = store
            .bucket_mut(&bucket)
            .expect("the suite's vector bucket")
            .indexes
            .remove(index);
        assert!(
            removed.is_some(),
            "setup: expected the index {index:?} to be removed, got {removed:?}"
        );
    }

    /// Create a knowledge base bound to the index `index` of [`VECTOR_BUCKET`] and answer
    /// its minted id.
    pub(crate) async fn knowledge_base(&self, name: &str, index: &str) -> String {
        let (status, body) = call(
            &self.router,
            "PUT",
            "/knowledgebases/",
            json!({
                "name": name,
                "roleArn": KB_ROLE,
                "knowledgeBaseConfiguration": {
                    "type": "VECTOR",
                    "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": EMBEDDING_MODEL },
                },
                "storageConfiguration": {
                    "type": "S3_VECTORS",
                    "s3VectorsConfiguration": {
                        "vectorBucketArn": VECTOR_BUCKET,
                        "indexName": index,
                    },
                },
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

    /// Attach an S3 data source over `bucket` and answer its minted id.
    pub(crate) async fn data_source(
        &self,
        knowledge_base_id: &str,
        name: &str,
        bucket: &str,
        prefixes: Option<Vec<&str>>,
    ) -> String {
        let mut s3 = json!({ "bucketArn": format!("arn:aws:s3:::{bucket}") });
        if let Some(prefixes) = prefixes {
            s3["inclusionPrefixes"] = json!(prefixes);
        }
        let (status, body) = call(
            &self.router,
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
            json!({
                "name": name,
                "dataSourceConfiguration": { "type": "S3", "s3Configuration": s3 },
            }),
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

    /// Start one ingestion job WITHOUT waiting for it, and answer its minted id.
    async fn start_ingestion(&self, knowledge_base_id: &str, data_source_id: &str) -> String {
        let (status, body) = call(
            &self.router,
            "PUT",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/ingestionjobs/"
            ),
            json!({}),
        )
        .await;
        assert_eq!(
            status, 202,
            "setup: expected StartIngestionJob -> 202, got {status} {body:?}"
        );
        body["ingestionJob"]["ingestionJobId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no ingestionJobId in {body:?}"))
            .to_string()
    }

    /// Run one ingestion job to completion: start it, wait out the spans the clock owes
    /// it, and assert the plane reports it `COMPLETE`.
    pub(crate) async fn ingest(&self, knowledge_base_id: &str, data_source_id: &str) {
        let job = self.start_ingestion(knowledge_base_id, data_source_id).await;
        tokio::time::sleep(INGESTION_JOB_START_SPAN + INGESTION_JOB_RUN_SPAN + MARGIN).await;
        let (status, body) = call(
            &self.router,
            "GET",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /ingestionjobs/{job}"
            ),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: expected GetIngestionJob -> 200, got {status} {body:?}"
        );
        assert_eq!(
            body["ingestionJob"]["status"].as_str(),
            Some("COMPLETE"),
            "setup: expected the ingestion job to have completed, got {body:?}"
        );
    }

    /// Retrieve from `identifier`, answering the raw `(status, body)`.
    async fn retrieve(&self, identifier: &str, body: Value) -> (u16, Value) {
        call(
            &self.router,
            "POST",
            &format!("/knowledgebases/{}/retrieve", label(identifier)),
            body,
        )
        .await
    }

    /// Retrieve, asserting the call succeeded, and answer the `retrievalResults` list.
    async fn results(&self, case: &str, identifier: &str, body: Value) -> Vec<Value> {
        let (status, answer) = self.retrieve(identifier, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Retrieve to answer 200, got {status} {answer:?}"
        );
        assert!(
            answer.get("nextToken").is_none(),
            "case {case:?}: expected no nextToken, got {answer:?}"
        );
        answer["retrievalResults"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no retrievalResults in {answer:?}"))
            .clone()
    }

    /// `GET /<bucket>/<key>` against the S3 service on this engine: the call a client
    /// following a citation makes, answering `(status, bytes)`.
    async fn get_object(&self, bucket: &str, key: &str) -> (u16, Vec<u8>) {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("/{bucket}/{key}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{REGION}/s3/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .body(Bytes::new())
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }
}

/// Create the vector bucket [`VECTOR_BUCKET`] and the index `index` inside it, holding
/// vectors of [`EMBEDDING_DIMENSION`] values under the cosine metric — the store a
/// knowledge base is bound to, set up before it names it.
fn seed_index(engine: &Arc<engine::InMemoryEngine>, index: &str) {
    let bucket = vectors_state::bucket_from_arn("vectorBucketArn", VECTOR_BUCKET)
        .expect("the suite's vectorBucketArn is a vector bucket ARN");
    let state =
        vectors_state::service_state(&**engine).expect("the S3 Vectors store on this engine");
    let mut store = state.lock();
    let held = store
        .buckets
        .entry(bucket.clone())
        .or_insert_with(|| vectors_state::VectorBucketRecord {
            name: bucket.clone(),
            arn: VECTOR_BUCKET.to_string(),
            creation_time: std::time::SystemTime::now(),
            encryption: vectors_state::EncryptionRecord::default(),
            default_index_mode: vectors_state::DEFAULT_INDEX_MODE.to_string(),
            policy: None,
            indexes: BTreeMap::new(),
        });
    let encryption = held.encryption.clone();
    let index_mode = held.default_index_mode.clone();
    held.indexes.insert(
        index.to_string(),
        vectors_state::VectorIndexRecord {
            bucket_name: bucket.clone(),
            name: index.to_string(),
            arn: format!("{VECTOR_BUCKET}/index/{index}"),
            creation_time: std::time::SystemTime::now(),
            data_type: VECTOR_DATA_TYPE.to_string(),
            dimension: EMBEDDING_DIMENSION,
            distance_metric: vectors_state::DISTANCE_METRIC_COSINE.to_string(),
            non_filterable_metadata_keys: None,
            encryption,
            index_mode,
            vectors: BTreeMap::new(),
            tombstones: std::collections::VecDeque::new(),
        },
    );
}

/// `text` with every run of whitespace collapsed to one space — the form a chunk's text
/// is a substring of.
pub(crate) fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// The digest of some bytes, so a test compares what it uploaded against what the
/// citation resolved to without printing either.
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The `s3://<bucket>/<key>` URI one result's location names, asserting the location is
/// an S3 one shaped the way the rule requires.
fn s3_location(case: &str, result: &Value) -> (String, String) {
    let location = &result["location"];
    assert_eq!(
        location["type"].as_str(),
        Some("S3"),
        "case {case:?}: expected location.type S3, got {location:?}"
    );
    let uri = location["s3Location"]["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no location.s3Location.uri in {result:?}"));
    let (bucket, key) = uri
        .strip_prefix("s3://")
        .and_then(|rest| rest.split_once('/'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a s3://<bucket>/<key> uri, got {uri:?}")
        });
    assert!(
        location.get("webLocation").is_none() && location.get("customDocumentLocation").is_none(),
        "case {case:?}: expected only the S3 location member, got {location:?}"
    );
    (bucket.to_string(), key.to_string())
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

/// The score of one result, asserting it is inside the range a relevance carries.
fn score(case: &str, result: &Value) -> f64 {
    let score = result["score"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no score in {result:?}"));
    assert!(
        (0.0..=1.0).contains(&score),
        "case {case:?}: expected a score in [0, 1], got {score}"
    );
    score
}

/// The distinct source URIs a result list cites, sorted.
fn citations(case: &str, results: &[Value]) -> Vec<String> {
    let mut cited: BTreeSet<String> = BTreeSet::new();
    for result in results {
        let (bucket, key) = s3_location(case, result);
        cited.insert(format!("s3://{bucket}/{key}"));
    }
    cited.into_iter().collect()
}

/// A document long enough to be worth a chunk, with irregular whitespace.
const LAUNCH_CODES: &str = "The Thunderbird project launch code is TBX-9 and it is held by \
     the propulsion team.\n\n   The Albatross project launch code is ALB-3 and it is held \
     by the\tavionics team.";

/// A second document on a different topic, so a query distinguishes the two.
const CAFETERIA: &str = "The cafeteria serves lentil soup on Tuesday and roast squash on \
     Thursday. Coffee is free for anyone on the early shift.";

/// A third document, on a third topic.
const INVENTORY: &str = "Inventory note: the spare battery packs live in bay 4.";

/// One row of the main table: what the data source holds, how it is scoped, and which
/// keys the retrieval may cite.
struct Row {
    case: &'static str,
    /// `(key, text)` for every object put into the bucket.
    objects: Vec<(&'static str, &'static str)>,
    /// The data source's inclusion prefixes, or `None` for a data source that names none.
    prefixes: Option<Vec<&'static str>>,
    query: &'static str,
    /// The keys the retrieval is allowed to cite — every other object is one the data
    /// source does not ingest.
    retrievable: Vec<&'static str>,
}

/// RULE: every result cites, through `location.s3Location.uri`, the exact object its text
/// was ingested from — the bytes the citation resolves to at S3 are the bytes that were
/// uploaded, and the chunk's text is the whitespace-collapsed text of that object — while
/// `metadata` carries the chunk's own id, the data source it came from and the modality of
/// its source file.
#[tokio::test]
async fn every_retrieval_result_cites_the_s3_object_its_text_was_ingested_from() {
    let rows = vec![
        Row {
            case: "one source document",
            objects: vec![("codes.txt", LAUNCH_CODES)],
            prefixes: None,
            query: "Which launch code belongs to which project?",
            retrievable: vec!["codes.txt"],
        },
        Row {
            case: "three source documents on three topics",
            objects: vec![
                ("codes.txt", LAUNCH_CODES),
                ("menu.txt", CAFETERIA),
                ("inventory.txt", INVENTORY),
            ],
            prefixes: None,
            query: "launch code",
            retrievable: vec!["codes.txt", "menu.txt", "inventory.txt"],
        },
        Row {
            case: "inclusion prefixes leave the objects outside them un-ingested",
            objects: vec![
                ("public/codes.txt", LAUNCH_CODES),
                ("private/menu.txt", CAFETERIA),
            ],
            prefixes: Some(vec!["public/"]),
            query: "launch code",
            retrievable: vec!["public/codes.txt"],
        },
        Row {
            case: "a metadata file is the metadata of its sibling, never a document",
            objects: vec![
                ("codes.txt", LAUNCH_CODES),
                ("codes.txt.metadata.json", "{\"metadataAttributes\":{}}"),
            ],
            prefixes: None,
            query: "propulsion team",
            retrievable: vec!["codes.txt"],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new("cites-index");
        let bucket = "retrieval-source-docs";
        for (key, text) in &row.objects {
            emulator.put_object(bucket, key, text);
        }
        let knowledge_base = emulator.knowledge_base("retrieval-kb", "cites-index").await;
        let data_source = emulator
            .data_source(&knowledge_base, "s3-docs", bucket, row.prefixes.clone())
            .await;
        emulator.ingest(&knowledge_base, &data_source).await;

        let results = emulator
            .results(
                case,
                &knowledge_base,
                json!({
                    "retrievalQuery": { "text": row.query },
                    "retrievalConfiguration": {
                        "vectorSearchConfiguration": { "numberOfResults": 100 },
                    },
                }),
            )
            .await;
        assert!(
            !results.is_empty(),
            "case {case:?}: expected the ingested objects to be retrievable, got no results"
        );

        let mut chunk_ids: BTreeSet<String> = BTreeSet::new();
        let mut previous: Option<f64> = None;
        for result in &results {
            let (cited_bucket, key) = s3_location(case, result);
            assert_eq!(
                cited_bucket, bucket,
                "case {case:?}: expected the bucket the data source names, got {cited_bucket:?}"
            );
            assert!(
                row.retrievable.contains(&key.as_str()),
                "case {case:?}: expected a citation of one of {:?}, got {key:?}",
                row.retrievable
            );

            // The citation RESOLVES: the object it names is served by S3 and its bytes
            // are the ones that were uploaded.
            let source = row
                .objects
                .iter()
                .find(|(stored, _)| *stored == key)
                .map(|(_, text)| *text)
                .unwrap_or_else(|| panic!("case {case:?}: {key:?} was never uploaded"));
            let (status, fetched) = emulator.get_object(bucket, &key).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetObject on the cited s3://{bucket}/{key} to \
                 answer 200, got {status}"
            );
            assert_eq!(
                digest(&fetched),
                digest(source.as_bytes()),
                "case {case:?}: the bytes the citation resolved to are not the ones \
                 uploaded to s3://{bucket}/{key}"
            );

            let text = content_text(case, result);
            assert_eq!(
                text,
                collapsed(source),
                "case {case:?}: expected the chunk of s3://{bucket}/{key} to be that \
                 object's own whitespace-collapsed text"
            );

            let metadata = metadata(case, result);
            assert_eq!(
                metadata.keys().cloned().collect::<Vec<String>>(),
                RESULT_METADATA_KEYS
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: expected exactly the three S3 metadata keys, got {metadata:?}"
            );
            assert_eq!(
                metadata[index_fields::DATA_SOURCE_ID_METADATA_KEY], data_source,
                "case {case:?}: expected the data source the caller created, got {metadata:?}"
            );
            assert_eq!(
                metadata[index_fields::SOURCE_FILE_MODALITY_METADATA_KEY],
                index_fields::SOURCE_FILE_MODALITY_TEXT,
                "case {case:?}: expected the modality of a text source file, got {metadata:?}"
            );
            let chunk_id = metadata[index_fields::CHUNK_ID_METADATA_KEY].clone();
            assert!(
                !chunk_id.is_empty(),
                "case {case:?}: expected a non-empty chunk id, got {metadata:?}"
            );
            assert!(
                chunk_ids.insert(chunk_id.clone()),
                "case {case:?}: expected every chunk id of one call to be distinct, \
                 {chunk_id:?} repeated"
            );

            let score = score(case, result);
            if let Some(previous) = previous {
                assert!(
                    score <= previous,
                    "case {case:?}: expected scores to descend, got {score} after {previous}"
                );
            }
            previous = Some(score);
        }

        assert_eq!(
            citations(case, &results),
            row.retrievable
                .iter()
                .map(|key| format!("s3://{bucket}/{key}"))
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect::<Vec<String>>(),
            "case {case:?}: expected every ingested object to be cited"
        );
    }
}

/// RULE: the ranking is the bound index's own nearest-neighbour order, so the document
/// whose text the query shares terms with is reported ahead of one it shares none with.
#[tokio::test]
async fn the_document_the_query_shares_terms_with_ranks_first() {
    let emulator = Emulator::new("ranking-index");
    let bucket = "retrieval-ranking-docs";
    emulator.put_object(bucket, "codes.txt", LAUNCH_CODES);
    emulator.put_object(bucket, "menu.txt", CAFETERIA);
    emulator.put_object(bucket, "inventory.txt", INVENTORY);
    let knowledge_base = emulator.knowledge_base("ranking-kb", "ranking-index").await;
    let data_source = emulator
        .data_source(&knowledge_base, "s3-docs", bucket, None)
        .await;
    emulator.ingest(&knowledge_base, &data_source).await;

    // (case, the query, the object its terms come from)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a query about the launch codes", "Thunderbird launch code", "codes.txt"),
        ("a query about the menu", "lentil soup on Tuesday", "menu.txt"),
        ("a query about the inventory", "spare battery packs bay", "inventory.txt"),
    ];
    for (case, query, nearest) in cases {
        let results = emulator
            .results(
                case,
                &knowledge_base,
                json!({
                    "retrievalQuery": { "text": query },
                    "retrievalConfiguration": {
                        "vectorSearchConfiguration": { "numberOfResults": 100 },
                    },
                }),
            )
            .await;
        let (_, first) = s3_location(case, &results[0]);
        assert_eq!(
            first, nearest,
            "case {case:?}: expected the document the query shares terms with first, got \
             {:?}",
            citations(case, &results)
        );
        assert!(
            score(case, &results[0]) > 0.0,
            "case {case:?}: expected the nearest document to carry a positive relevance"
        );
    }
}

/// RULE: a chunk is retrievable only once an ingestion job has recorded it, what a job
/// recorded does not move when the source moves, and the next job is what reconciles it:
/// an object added after a job is retrievable only after another one, and one deleted
/// after a job keeps being cited until another one runs.
#[tokio::test]
async fn only_what_an_ingestion_job_has_recorded_is_retrievable() {
    let emulator = Emulator::new("window-index");
    let bucket = "retrieval-ingestion-window";
    emulator.put_object(bucket, "codes.txt", LAUNCH_CODES);
    let knowledge_base = emulator.knowledge_base("window-kb", "window-index").await;
    let data_source = emulator
        .data_source(&knowledge_base, "s3-docs", bucket, None)
        .await;

    let query = json!({ "retrievalQuery": { "text": "launch code" } });

    // Before any ingestion job has run, nothing has been recorded, whatever the bucket
    // holds.
    let results = emulator
        .results("no ingestion job yet", &knowledge_base, query.clone())
        .await;
    assert!(
        results.is_empty(),
        "case \"no ingestion job yet\": expected nothing retrievable, got {results:?}"
    );

    emulator.ingest(&knowledge_base, &data_source).await;
    assert_eq!(
        citations(
            "after the first ingestion",
            &emulator
                .results("after the first ingestion", &knowledge_base, query.clone())
                .await,
        ),
        vec![format!("s3://{bucket}/codes.txt")],
        "case \"after the first ingestion\": expected the ingested object to be cited"
    );

    // An object written after that job was not part of it.
    emulator.put_object(bucket, "menu.txt", CAFETERIA);
    assert_eq!(
        citations(
            "a new object, not yet ingested",
            &emulator
                .results("a new object, not yet ingested", &knowledge_base, query.clone())
                .await,
        ),
        vec![format!("s3://{bucket}/codes.txt")],
        "case \"a new object, not yet ingested\": expected the object written after the \
         job to stay out of the retrieval"
    );

    // Until the next job records it.
    emulator.ingest(&knowledge_base, &data_source).await;
    assert_eq!(
        citations(
            "after the second ingestion",
            &emulator
                .results("after the second ingestion", &knowledge_base, query.clone())
                .await,
        ),
        vec![
            format!("s3://{bucket}/codes.txt"),
            format!("s3://{bucket}/menu.txt"),
        ],
        "case \"after the second ingestion\": expected both objects to be cited"
    );

    // Deleting the SOURCE does not move what the last job recorded: the chunk and its
    // citation stand, a dangling reference, which is what a knowledge base holds until
    // its next sync.
    let before_delete = emulator
        .results("the source deleted", &knowledge_base, query.clone())
        .await;
    emulator.delete_object(bucket, "codes.txt");
    let (status, bytes) = emulator.get_object(bucket, "codes.txt").await;
    assert_eq!(
        status, 404,
        "case \"the source deleted\": expected the deleted object to be gone from S3, got \
         {status} with {} bytes",
        bytes.len()
    );
    let after_delete = emulator
        .results("the source deleted", &knowledge_base, query.clone())
        .await;
    assert_eq!(
        after_delete, before_delete,
        "case \"the source deleted\": expected deleting the source object to leave what \
         the ingestion recorded untouched"
    );

    // The next job is what drops it, because that job reads the bucket as it is now.
    emulator.ingest(&knowledge_base, &data_source).await;
    assert_eq!(
        citations(
            "after the ingestion following the delete",
            &emulator
                .results(
                    "after the ingestion following the delete",
                    &knowledge_base,
                    query.clone(),
                )
                .await,
        ),
        vec![format!("s3://{bucket}/menu.txt")],
        "case \"after the ingestion following the delete\": expected the next job to drop \
         the object the bucket no longer holds"
    );

    // And a job still RUNNING takes nothing away: what the previous one recorded stays
    // retrievable until this one has written its own.
    let before_reingestion = emulator
        .results("a re-ingestion in flight", &knowledge_base, query.clone())
        .await;
    let job = emulator
        .start_ingestion(&knowledge_base, &data_source)
        .await;
    assert!(
        !job.is_empty(),
        "case \"a re-ingestion in flight\": expected a started job to be named"
    );
    assert_eq!(
        citations(
            "a re-ingestion in flight",
            &emulator
                .results("a re-ingestion in flight", &knowledge_base, query)
                .await,
        ),
        citations("a re-ingestion in flight", &before_reingestion),
        "case \"a re-ingestion in flight\": expected what the last job recorded to stay \
         retrievable while another job runs"
    );
}

/// RULE: a retrieval addressed to a knowledge base the Bedrock Agent plane does not hold
/// answers the modelled `ResourceNotFoundException` (404) naming the identifier the
/// request carried — never a 501, and never an empty success.
#[tokio::test]
async fn an_unknown_knowledge_base_is_refused_as_not_found() {
    let emulator = Emulator::new("present-index");
    // A knowledge base that DOES exist, so the refusals below are about the identifier
    // and not about an empty store.
    let existing = emulator
        .knowledge_base("present-kb", "present-index")
        .await;

    // (case, the identifier the request carries)
    let cases: Vec<(&str, String)> = vec![
        ("a bare id nothing created", "SIM2REAK0B".to_string()),
        ("another bare id", "0123456789".to_string()),
        (
            "an ARN for an id nothing created",
            format!("arn:aws:bedrock:{REGION}:000000000000:knowledge-base/SIM2REAK0B"),
        ),
        (
            "an ARN in another partition",
            "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:knowledge-base/SIM2REAK0B"
                .to_string(),
        ),
    ];
    for (case, identifier) in cases {
        let (status, body) = emulator
            .retrieve(
                &identifier,
                json!({ "retrievalQuery": { "text": "launch code" } }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {identifier:?}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found error, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Could not find knowledge base {identifier}.").as_str()),
            "case {case:?}: expected the refusal to name the identifier, got {body:?}"
        );
    }

    // The knowledge base that exists is served, through both forms of its identifier,
    // which is what proves the refusals above were about the identifier.
    for identifier in [
        existing.clone(),
        format!("arn:aws:bedrock:{REGION}:000000000000:knowledge-base/{existing}"),
    ] {
        let (status, body) = emulator
            .retrieve(
                &identifier,
                json!({ "retrievalQuery": { "text": "launch code" } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"the knowledge base that exists\": expected 200 for {identifier:?}, got \
             {status} {body:?}"
        );
        assert_eq!(
            body["retrievalResults"].as_array().map(Vec::len),
            Some(0),
            "case \"the knowledge base that exists\": a knowledge base with no data source \
             has ingested nothing, got {body:?}"
        );
    }
}

/// RULE: `numberOfResults` bounds how many of the ranked chunks the response carries, and
/// a request naming none takes the model's own default.
#[tokio::test]
async fn the_requested_result_bound_decides_how_many_chunks_are_reported() {
    let emulator = Emulator::new("bounds-index");
    let bucket = "retrieval-result-bounds";
    // Seven source documents, so the modelled default page of five is smaller than what
    // the knowledge base holds.
    let documents = [
        ("codes.txt", LAUNCH_CODES),
        ("menu.txt", CAFETERIA),
        ("inventory.txt", INVENTORY),
        ("roster.txt", "Ana on Monday, Bo on Friday, Cal on Sunday."),
        ("routes.txt", "The shuttle route runs from the gate to the pad."),
        ("safety.txt", "Hearing protection is required inside the test cell."),
        ("weather.txt", "Winds are light and variable until Thursday."),
    ];
    for (key, text) in documents {
        emulator.put_object(bucket, key, text);
    }
    let knowledge_base = emulator.knowledge_base("bounds-kb", "bounds-index").await;
    let data_source = emulator
        .data_source(&knowledge_base, "s3-docs", bucket, None)
        .await;
    emulator.ingest(&knowledge_base, &data_source).await;

    // The whole ranking, read with the largest bound the model admits.
    let every = emulator
        .results(
            "every chunk",
            &knowledge_base,
            json!({
                "retrievalQuery": { "text": "launch code" },
                "retrievalConfiguration": { "vectorSearchConfiguration": { "numberOfResults": 100 } },
            }),
        )
        .await;
    assert_eq!(
        every.len(),
        documents.len(),
        "setup: expected every source document to be retrievable, got {}",
        every.len()
    );

    // (case, the `retrievalConfiguration` member, the number of results expected)
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no retrievalConfiguration at all", Value::Null, 5),
        (
            "an empty vectorSearchConfiguration takes the modelled default",
            json!({ "vectorSearchConfiguration": {} }),
            5,
        ),
        (
            "one result",
            json!({ "vectorSearchConfiguration": { "numberOfResults": 1 } }),
            1,
        ),
        (
            "three results",
            json!({ "vectorSearchConfiguration": { "numberOfResults": 3 } }),
            3,
        ),
        (
            "more results than there are chunks",
            json!({ "vectorSearchConfiguration": { "numberOfResults": 100 } }),
            documents.len(),
        ),
    ];
    for (case, configuration, expected) in cases {
        let mut body = json!({ "retrievalQuery": { "text": "launch code" } });
        if !configuration.is_null() {
            body["retrievalConfiguration"] = configuration;
        }
        let results = emulator.results(case, &knowledge_base, body).await;
        assert_eq!(
            results.len(),
            expected,
            "case {case:?}: expected {expected} results, got {}",
            results.len()
        );
        // The bound keeps the LEADING n of the one ranking: the kept results are the
        // nearest ones, in the same order the unbounded call reported them.
        let kept: Vec<String> = results
            .iter()
            .map(|result| metadata(case, result)[index_fields::CHUNK_ID_METADATA_KEY].clone())
            .collect();
        let leading: Vec<String> = every
            .iter()
            .take(expected)
            .map(|result| metadata(case, result)[index_fields::CHUNK_ID_METADATA_KEY].clone())
            .collect();
        assert_eq!(
            kept, leading,
            "case {case:?}: expected the leading {expected} of the one ranking"
        );
    }
}

/// RULE: a `filter` reports only the chunks whose own metadata satisfies it, and a chunk
/// carrying no attribute of the name a comparison tests satisfies no comparison over it.
#[tokio::test]
async fn the_metadata_filter_decides_which_chunks_are_reported() {
    let emulator = Emulator::new("filter-index");
    let first_bucket = "retrieval-filter-codes";
    let second_bucket = "retrieval-filter-menu";
    emulator.put_object(first_bucket, "codes.txt", LAUNCH_CODES);
    emulator.put_object(second_bucket, "menu.txt", CAFETERIA);
    let knowledge_base = emulator.knowledge_base("filter-kb", "filter-index").await;
    let codes = emulator
        .data_source(&knowledge_base, "codes", first_bucket, None)
        .await;
    let menu = emulator
        .data_source(&knowledge_base, "menu", second_bucket, None)
        .await;
    emulator.ingest(&knowledge_base, &codes).await;
    emulator.ingest(&knowledge_base, &menu).await;

    let data_source_key = index_fields::DATA_SOURCE_ID_METADATA_KEY;
    let modality_key = index_fields::SOURCE_FILE_MODALITY_METADATA_KEY;
    let both = vec![
        format!("s3://{first_bucket}/codes.txt"),
        format!("s3://{second_bucket}/menu.txt"),
    ];
    let only_codes = vec![format!("s3://{first_bucket}/codes.txt")];
    let only_menu = vec![format!("s3://{second_bucket}/menu.txt")];
    // The two minted ids order one way or the other; the range comparisons below are
    // stated against whichever is lower, so the rows hold whatever was minted.
    let (lower, higher) = if codes <= menu {
        (codes.clone(), menu.clone())
    } else {
        (menu.clone(), codes.clone())
    };

    // (case, the filter, the citations expected)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", Value::Null, both.clone()),
        (
            "equals on the data source a chunk came from",
            json!({ "equals": { "key": data_source_key, "value": codes } }),
            only_codes.clone(),
        ),
        (
            "notEquals on the same attribute",
            json!({ "notEquals": { "key": data_source_key, "value": codes } }),
            only_menu.clone(),
        ),
        (
            "in, over a list holding one of the data sources",
            json!({ "in": { "key": data_source_key, "value": [menu, "0000000000"] } }),
            only_menu.clone(),
        ),
        (
            "notIn, over that same list",
            json!({ "notIn": { "key": data_source_key, "value": [menu, "0000000000"] } }),
            only_codes.clone(),
        ),
        (
            "equals on the modality every text source carries",
            json!({
                "equals": {
                    "key": modality_key,
                    "value": index_fields::SOURCE_FILE_MODALITY_TEXT,
                },
            }),
            both.clone(),
        ),
        (
            "stringContains on an id's own characters",
            json!({ "stringContains": { "key": data_source_key, "value": &codes[..6] } }),
            only_codes.clone(),
        ),
        (
            "startsWith on an id's own prefix",
            json!({ "startsWith": { "key": data_source_key, "value": &menu[..6] } }),
            only_menu.clone(),
        ),
        (
            "andAll of two comparisons that both hold",
            json!({
                "andAll": [
                    { "equals": { "key": data_source_key, "value": codes } },
                    {
                        "equals": {
                            "key": modality_key,
                            "value": index_fields::SOURCE_FILE_MODALITY_TEXT,
                        },
                    },
                ],
            }),
            only_codes.clone(),
        ),
        (
            "andAll holding a comparison that cannot hold",
            json!({
                "andAll": [
                    { "equals": { "key": data_source_key, "value": codes } },
                    { "equals": { "key": data_source_key, "value": menu } },
                ],
            }),
            Vec::new(),
        ),
        (
            "orAll of one comparison per data source",
            json!({
                "orAll": [
                    { "equals": { "key": data_source_key, "value": codes } },
                    { "equals": { "key": data_source_key, "value": menu } },
                ],
            }),
            both.clone(),
        ),
        (
            "an attribute no chunk of this knowledge base carries",
            json!({ "equals": { "key": "department", "value": "operations" } }),
            Vec::new(),
        ),
        (
            "a numeric comparison over an attribute that is not a number",
            json!({ "greaterThan": { "key": data_source_key, "value": 7 } }),
            Vec::new(),
        ),
        (
            "greaterThanOrEquals over the lower of the two ids",
            json!({ "greaterThanOrEquals": { "key": data_source_key, "value": lower } }),
            both.clone(),
        ),
        (
            "lessThanOrEquals over the higher of the two ids",
            json!({ "lessThanOrEquals": { "key": data_source_key, "value": higher } }),
            both.clone(),
        ),
        (
            "lessThan over the lower of the two ids, which nothing is below",
            json!({ "lessThan": { "key": data_source_key, "value": lower } }),
            Vec::new(),
        ),
        (
            "lessThan over the higher of the two ids",
            json!({ "lessThan": { "key": data_source_key, "value": higher } }),
            if lower == codes {
                only_codes.clone()
            } else {
                only_menu.clone()
            },
        ),
        (
            "listContains over an attribute that is not a list",
            json!({ "listContains": { "key": data_source_key, "value": codes } }),
            Vec::new(),
        ),
    ];

    for (case, filter, expected) in cases {
        let mut configuration = json!({ "numberOfResults": 100 });
        if !filter.is_null() {
            configuration["filter"] = filter;
        }
        let results = emulator
            .results(
                case,
                &knowledge_base,
                json!({
                    "retrievalQuery": { "text": "launch code and lunch" },
                    "retrievalConfiguration": { "vectorSearchConfiguration": configuration },
                }),
            )
            .await;
        assert_eq!(
            citations(case, &results),
            expected,
            "case {case:?}: expected exactly the chunks the filter admits"
        );
    }
}

/// A `filter` nesting `andAll` one level deeper than the service walks, built here rather
/// than written out: the bound is the service's own, and a request past it is refused
/// instead of evaluated.
fn over_deep_filter() -> Value {
    let leaf = json!({ "equals": { "key": "department", "value": "operations" } });
    let mut filter = json!({ "andAll": [leaf.clone(), leaf.clone()] });
    for _ in 0..=MAX_FILTER_DEPTH {
        filter = json!({ "andAll": [filter, leaf.clone()] });
    }
    filter
}

/// A `filter` holding one more node than the service evaluates, in ONE `andAll` at depth
/// two: nesting is not the only way a filter grows, and the modelled list has no maximum
/// length of its own.
fn over_wide_filter() -> Value {
    let comparisons: Vec<Value> = (0..=MAX_FILTER_NODES)
        .map(|nth| json!({ "equals": { "key": "department", "value": format!("team-{nth}") } }))
        .collect();
    json!({ "orAll": comparisons })
}

/// RULE: a request naming a leg this service cannot compute is refused with the modelled
/// `ValidationException` (400) naming the member, rather than answered with a retrieval
/// that ignored it.
#[tokio::test]
async fn a_leg_this_service_cannot_compute_is_refused_not_ignored() {
    let emulator = Emulator::new("refusals-index");
    let knowledge_base = emulator
        .knowledge_base("refusals-kb", "refusals-index")
        .await;

    // (case, the request body, the member the refusal has to name)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a query with no text",
            json!({ "retrievalQuery": { "text": "   " } }),
            "retrievalQuery.text",
        ),
        (
            "an IMAGE query",
            json!({
                "retrievalQuery": {
                    "type": "IMAGE",
                    "text": "launch code",
                    "image": { "format": "png", "inlineContent": "iVBORw0=" },
                },
            }),
            "retrievalQuery.type",
        ),
        (
            "a userContext, which asks for per-document access control",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "userContext": { "userId": "alice" },
            }),
            "userContext",
        ),
        (
            "a guardrailConfiguration, which asks for a guardrail policy",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "guardrailConfiguration": { "guardrailId": "abc123", "guardrailVersion": "DRAFT" },
            }),
            "guardrailConfiguration",
        ),
        (
            "an implicitFilterConfiguration, which asks for a model-generated filter",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "retrievalConfiguration": {
                    "vectorSearchConfiguration": {
                        "implicitFilterConfiguration": {
                            "metadataAttributes": [
                                {
                                    "key": "department",
                                    "type": "STRING",
                                    "description": "the owning team",
                                },
                            ],
                            "modelArn": format!(
                                "arn:aws:bedrock:{REGION}::foundation-model/amazon.nova-lite-v1:0"
                            ),
                        },
                    },
                },
            }),
            "implicitFilterConfiguration",
        ),
        (
            "a nextToken this service never issued",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "nextToken": "not-a-token-from-here",
            }),
            "nextToken",
        ),
        (
            "a rerankingConfiguration, which asks a model to reorder the results",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "retrievalConfiguration": {
                    "vectorSearchConfiguration": {
                        "numberOfResults": 100,
                        "rerankingConfiguration": {
                            "type": "BEDROCK_RERANKING_MODEL",
                            "bedrockRerankingConfiguration": {
                                "modelConfiguration": {
                                    "modelArn": format!(
                                        "arn:aws:bedrock:{REGION}::foundation-model/amazon.rerank-v1:0"
                                    ),
                                },
                                "numberOfRerankedResults": 2,
                            },
                        },
                    },
                },
            }),
            "rerankingConfiguration",
        ),
        (
            "a filter nested deeper than the service walks",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "retrievalConfiguration": {
                    "vectorSearchConfiguration": { "filter": over_deep_filter() },
                },
            }),
            "filter",
        ),
        (
            "a filter holding more comparisons than the service evaluates",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "retrievalConfiguration": {
                    "vectorSearchConfiguration": { "filter": over_wide_filter() },
                },
            }),
            "filter",
        ),
        (
            "both search configurations at once",
            json!({
                "retrievalQuery": { "text": "launch code" },
                "retrievalConfiguration": {
                    "vectorSearchConfiguration": { "numberOfResults": 5 },
                    "managedSearchConfiguration": { "numberOfResults": 5 },
                },
            }),
            "managedSearchConfiguration",
        ),
    ];

    for (case, body, member) in cases {
        let (status, answer) = emulator.retrieve(&knowledge_base, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: expected the refusal to name {member:?}, got {message:?}"
        );
    }
}

/// RULE: a knowledge base whose content does not live anywhere this plane can read it —
/// its bound index gone from the vector store — is refused with the modelled
/// `DependencyFailedException` (424) naming that index, never answered with an empty
/// success, because "nothing to read it from" is not "nothing was ingested".
#[tokio::test]
async fn a_knowledge_base_whose_vector_store_cannot_be_read_is_refused() {
    let emulator = Emulator::new("vanishing-index");
    let bucket = "retrieval-vanishing";
    emulator.put_object(bucket, "codes.txt", LAUNCH_CODES);
    let knowledge_base = emulator
        .knowledge_base("vanishing-kb", "vanishing-index")
        .await;
    let data_source = emulator
        .data_source(&knowledge_base, "s3-docs", bucket, None)
        .await;
    emulator.ingest(&knowledge_base, &data_source).await;

    let query = json!({ "retrievalQuery": { "text": "launch code" } });
    // While the index is there, the chunk it holds is retrievable: that is what makes the
    // refusal below about the store and not about the knowledge base.
    let served = emulator
        .results("the index is there", &knowledge_base, query.clone())
        .await;
    assert_eq!(
        served.len(),
        1,
        "case \"the index is there\": expected the ingested document to be retrievable, \
         got {served:?}"
    );

    emulator.drop_index("vanishing-index");
    let (status, body) = emulator.retrieve(&knowledge_base, query).await;
    assert_eq!(
        status, 424,
        "case \"the bound index is gone\": expected the modelled 424, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "DependencyFailedException",
        "case \"the bound index is gone\": expected DependencyFailedException, got {body:?}"
    );
    let named = format!(
        "{}/vanishing-index",
        vectors_state::bucket_from_arn("vectorBucketArn", VECTOR_BUCKET)
            .expect("the suite's vectorBucketArn is a vector bucket ARN")
    );
    assert_eq!(
        body["resourceName"].as_str(),
        Some(named.as_str()),
        "case \"the bound index is gone\": expected the refusal to name the index the \
         knowledge base is bound to, got {body:?}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains(&named),
        "case \"the bound index is gone\": expected the message to name that index too, \
         got {body:?}"
    );
}

/// RULE: `Retrieve` is a reporting operation — reading a knowledge base changes nothing a
/// knowledge base and data source read back unchanged, and the bound index holds exactly
/// the vectors it held before.
#[tokio::test]
async fn retrieving_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new("readonly-index");
    let bucket = "retrieval-readonly";
    emulator.put_object(bucket, "codes.txt", LAUNCH_CODES);
    let knowledge_base = emulator
        .knowledge_base("readonly-kb", "readonly-index")
        .await;
    let data_source = emulator
        .data_source(&knowledge_base, "s3-docs", bucket, None)
        .await;
    emulator.ingest(&knowledge_base, &data_source).await;

    let read_knowledge_base = || async {
        let (status, body) = call(
            &emulator.router,
            "GET",
            &format!("/knowledgebases/{knowledge_base}"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"read-only\": expected GetKnowledgeBase -> 200, got {status} {body:?}"
        );
        body
    };
    let read_data_source = || async {
        let (status, body) = call(
            &emulator.router,
            "GET",
            &format!("/knowledgebases/{knowledge_base}/datasources/{data_source}"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"read-only\": expected GetDataSource -> 200, got {status} {body:?}"
        );
        body
    };
    let index_keys = || {
        let bucket = vectors_state::bucket_from_arn("vectorBucketArn", VECTOR_BUCKET)
            .expect("the suite's vectorBucketArn is a vector bucket ARN");
        let state = vectors_state::service_state(&*emulator.engine)
            .expect("the S3 Vectors store on this engine");
        let store = state.lock();
        store
            .index(&bucket, "readonly-index")
            .map(|index| index.vectors.keys().cloned().collect::<Vec<String>>())
            .unwrap_or_default()
    };

    let knowledge_base_before = read_knowledge_base().await;
    let data_source_before = read_data_source().await;
    let vectors_before = index_keys();

    let query = json!({
        "retrievalQuery": { "text": "launch code" },
        "retrievalConfiguration": { "vectorSearchConfiguration": { "numberOfResults": 100 } },
    });
    let first = emulator
        .results("read-only, first call", &knowledge_base, query.clone())
        .await;
    for call_number in 2..=3 {
        let again = emulator
            .results("read-only, repeated call", &knowledge_base, query.clone())
            .await;
        assert_eq!(
            again, first,
            "case \"read-only\": expected call {call_number} to answer exactly as the first"
        );
    }

    assert_eq!(
        read_knowledge_base().await,
        knowledge_base_before,
        "case \"read-only\": expected the knowledge base to read back unchanged"
    );
    assert_eq!(
        read_data_source().await,
        data_source_before,
        "case \"read-only\": expected the data source to read back unchanged"
    );
    assert_eq!(
        index_keys(),
        vectors_before,
        "case \"read-only\": expected the bound index to hold exactly the vectors it held"
    );
}
