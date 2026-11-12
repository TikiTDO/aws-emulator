// SPDX-License-Identifier: CC0-1.0

//! Which documents a data source takes, and what becomes of one that is
//! deleted, end to end over HTTP.
//!
//! Two rules are pinned here:
//!
//! * `IngestKnowledgeBaseDocuments` takes a document whose
//!   `content.dataSourceType` is the kind of content the addressed data source
//!   HOLDS — for a managed knowledge base connector the connector its
//!   `connectorParameters` name, for every other data source its own type —
//!   and refuses any other content, as well as every document of a request
//!   carrying one.
//! * `DeleteKnowledgeBaseDocuments` leaves the document VISIBLE to the document
//!   reads with the status that says it is gone, rather than dropping it from
//!   the data source's listing.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    DOCUMENT_DELETION_SPAN, DOCUMENT_DELETION_START_SPAN, DOCUMENT_INDEXING_SPAN,
    DOCUMENT_INGESTION_SPAN,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// The vector bucket and index the self-managed knowledge base here binds to.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/document-kinds-vectors";
const VECTOR_INDEX: &str = "document-kinds-index";

/// A margin over a span the clock owes, so a test that waits for a transition
/// is not racing the instant it comes due.
const MARGIN: Duration = Duration::from_millis(100);

/// The statuses an ingested document passes through, and the terminal one it
/// settles at.
///
/// Source: the `status` member of the `KnowledgeBaseDocumentDetail` shape of
/// this service's Smithy model
/// (`smithy_query(query="doc",shape="KnowledgeBaseDocumentDetail",member="status")`),
/// which documents `STARTING` as submitted, `IN_PROGRESS` as being ingested and
/// `INDEXED` as successfully indexed.
const INGESTION_STATUSES: [&str; 3] = ["STARTING", "IN_PROGRESS", "INDEXED"];
const INDEXED: &str = "INDEXED";

/// The statuses a deleted document passes through, from that same member, which
/// documents `DELETING` as the submitted delete, `DELETE_IN_PROGRESS` as being
/// deleted and `NOT_FOUND` as a document that "wasn't found".
const DELETION_STATUSES: [&str; 3] = ["DELETING", "DELETE_IN_PROGRESS", "NOT_FOUND"];
const NOT_FOUND: &str = "NOT_FOUND";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The response's `documentDetails`, which every document operation of this
    /// service answers with.
    fn details(&self) -> &Vec<Value> {
        self.body["documentDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("documentDetails is not a list in {:?}", self.body))
    }

    /// One detail of the response, by its position.
    fn detail(&self, index: usize) -> &Value {
        self.details()
            .get(index)
            .unwrap_or_else(|| panic!("documentDetails has no entry {index} in {:?}", self.body))
    }

    /// The detail reported for the document `identifier` names, whatever
    /// position it was reported in.
    fn detail_of(&self, identifier: &Value) -> &Value {
        self.details()
            .iter()
            .find(|detail| &detail["identifier"] == identifier)
            .unwrap_or_else(|| {
                panic!(
                    "no documentDetails entry for {identifier:?} in {:?}",
                    self.body
                )
            })
    }

    /// The `status` of one detail, by position.
    fn status_at(&self, index: usize) -> String {
        let detail = self.detail(index);
        detail["status"]
            .as_str()
            .unwrap_or_else(|| panic!("documentDetails[{index}].status is not a string in {detail:?}"))
            .to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // A self-managed vector knowledge base binds to an S3 Vectors index,
        // which `CreateKnowledgeBase` resolves in the service that owns it — so
        // the index is created first, as a caller creates it first against AWS.
        crate::vector_store_fixture::seed_index(
            &engine,
            VECTOR_BUCKET,
            VECTOR_INDEX,
            crate::vector_store_fixture::EMBEDDING_DIMENSION,
        );
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// Create a knowledge base and answer its minted `knowledgeBaseId`.
    async fn knowledge_base(&self, request: Value) -> String {
        let created = self.call("PUT", "/knowledgebases/", Some(request)).await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.body["knowledgeBase"]["knowledgeBaseId"]
            .as_str()
            .expect("a minted knowledgeBaseId")
            .to_string()
    }

    /// Attach a data source to `knowledge_base_id` and answer its minted
    /// `dataSourceId`.
    async fn data_source(&self, knowledge_base_id: &str, request: Value) -> String {
        let created = self
            .call(
                "PUT",
                &format!("/knowledgebases/{knowledge_base_id}/datasources/"),
                Some(request),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case setup: expected CreateDataSource -> 200, got {created:?}"
        );
        created.body["dataSource"]["dataSourceId"]
            .as_str()
            .expect("a minted dataSourceId")
            .to_string()
    }

    async fn ingest(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        documents: Vec<Value>,
    ) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            Some(json!({ "documents": documents })),
        )
        .await
    }

    async fn get_documents(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        identifiers: Vec<Value>,
    ) -> Answer {
        self.call(
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /documents/getDocuments"
            ),
            Some(json!({ "documentIdentifiers": identifiers })),
        )
        .await
    }

    async fn delete_documents(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        identifiers: Vec<Value>,
    ) -> Answer {
        self.call(
            "POST",
            &format!(
                "/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}\
                 /documents/deleteDocuments"
            ),
            Some(json!({ "documentIdentifiers": identifiers })),
        )
        .await
    }

    async fn list_documents(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "POST",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}/documents"),
            Some(json!({})),
        )
        .await
    }
}

/// A service-managed knowledge base: the kind a managed connector data source
/// belongs to.
fn managed_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": {
            "type": "MANAGED",
            "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
        },
    })
}

/// A self-managed vector knowledge base: the kind whose data sources carry
/// their own connection details.
fn vector_knowledge_base(name: &str) -> Value {
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
                "indexName": VECTOR_INDEX,
            },
        },
    })
}

/// A `MANAGED_KNOWLEDGE_BASE_CONNECTOR` data source running the connector
/// `connector` names, with the connection details that connector needs.
fn managed_connector_data_source(name: &str, connector: &str, connection: Value) -> Value {
    let mut parameters = json!({ "type": connector, "version": "1" });
    if let Value::Object(connection) = connection {
        for (member, value) in connection {
            parameters[member] = value;
        }
    }
    json!({
        "name": name,
        "dataSourceConfiguration": {
            "type": "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
            "managedKnowledgeBaseConnectorConfiguration": {
                "connectorParameters": parameters,
            },
        },
    })
}

/// A data source of the kind `data_source_type` names, with the connection
/// member that kind needs.
///
/// Source: the `DataSourceType` enum and the `DataSourceConfiguration`
/// structure of this service's Smithy model
/// (`smithy_query(query="shape",shape="DataSourceType,DataSourceConfiguration")`),
/// which pairs each kind with the sibling member carrying its connection
/// details and gives `CUSTOM` and `REDSHIFT_METADATA` none.
fn self_managed_data_source(name: &str, data_source_type: &str) -> Value {
    let mut configuration = json!({ "type": data_source_type });
    match data_source_type {
        "S3" => {
            configuration["s3Configuration"] =
                json!({ "bucketArn": "arn:aws:s3:::document-kinds-docs" });
        }
        "WEB" => {
            configuration["webConfiguration"] = json!({
                "sourceConfiguration": {
                    "urlConfiguration": { "seedUrls": [{ "url": "https://example.com/docs" }] },
                },
            });
        }
        _ => {}
    }
    json!({ "name": name, "dataSourceConfiguration": configuration })
}

/// A document whose content is of the kind `content_type` names, identified by
/// `id`.
///
/// Source: the `ContentDataSourceType` enum and the `DocumentContent` structure
/// of this service's Smithy model
/// (`smithy_query(query="shape",shape="ContentDataSourceType,DocumentContent")`):
/// the kind names which sibling member carries the document's own content.
fn document_of_kind(content_type: &str, id: &str) -> Value {
    match content_type {
        "CUSTOM" => json!({
            "content": {
                "dataSourceType": "CUSTOM",
                "custom": {
                    "customDocumentIdentifier": { "id": id },
                    "sourceType": "IN_LINE",
                    "inlineContent": {
                        "type": "TEXT",
                        "textContent": { "data": "the document's text" },
                    },
                },
            },
        }),
        "S3" => json!({
            "content": {
                "dataSourceType": "S3",
                "s3": { "s3Location": { "uri": id } },
            },
        }),
        other => panic!("no document shape for the content kind {other}"),
    }
}

/// The identifier the store holds a document of that kind under.
fn identifier_of_kind(content_type: &str, id: &str) -> Value {
    match content_type {
        "CUSTOM" => json!({ "dataSourceType": "CUSTOM", "custom": { "id": id } }),
        "S3" => json!({ "dataSourceType": "S3", "s3": { "uri": id } }),
        other => panic!("no identifier shape for the content kind {other}"),
    }
}

/// The id a document of that kind is given: a custom document carries its own
/// id, an S3 document is the object at its URI.
fn document_id(content_type: &str, name: &str) -> String {
    match content_type {
        "S3" => format!("s3://document-kinds-docs/{name}.txt"),
        _ => name.to_string(),
    }
}

/// Assert `detail` reports the document of this knowledge base and data source
/// that `identifier` names, with a status among `statuses` and with the
/// `updatedAt` the record it came from carries.
fn assert_detail(
    case: &str,
    detail: &Value,
    knowledge_base_id: &str,
    data_source_id: &str,
    identifier: &Value,
    statuses: &[&str],
) {
    assert_eq!(
        detail["knowledgeBaseId"].as_str(),
        Some(knowledge_base_id),
        "case {case}: the document belongs to the knowledge base the URI named, got {detail:?}"
    );
    assert_eq!(
        detail["dataSourceId"].as_str(),
        Some(data_source_id),
        "case {case}: the document belongs to the data source the URI named, got {detail:?}"
    );
    assert_eq!(
        &detail["identifier"], identifier,
        "case {case}: expected the identifier of the document stored, got {detail:?}"
    );
    let status = detail["status"].as_str().unwrap_or_default();
    assert!(
        statuses.contains(&status),
        "case {case}: expected one of {statuses:?}, got {detail:?}"
    );
    let updated_at = detail["updatedAt"].as_str().unwrap_or_default();
    assert!(
        updated_at.contains('T') && updated_at.ends_with('Z'),
        "case {case}: expected updatedAt in the modelled date-time wire form, got {detail:?}"
    );
}

/// A document whose content is of the kind the addressed data source HOLDS is
/// ingested (202) and read back; content of any other kind is refused (400) and
/// nothing of that request is written.
///
/// The rows differ along what the rule generalises over: whether the data source
/// is a managed knowledge base connector or carries its own connection details,
/// which kind of content it therefore holds (each value of the modelled
/// `ContentDataSourceType`), which connector a managed one runs, and the kinds
/// that hold no directly ingestible content at all.
#[tokio::test]
async fn a_data_source_takes_the_kind_of_document_content_it_holds() {
    let emulator = Emulator::new();
    let managed = emulator
        .knowledge_base(managed_knowledge_base("document-kinds-managed"))
        .await;
    let self_managed = emulator
        .knowledge_base(vector_knowledge_base("document-kinds-vector"))
        .await;

    // (case, the knowledge base, the data source to create, the content kind it
    // takes — `None` for a data source that takes none).
    let cases: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "a managed connector running the CUSTOM connector",
            managed.as_str(),
            managed_connector_data_source("kinds-connector-custom", "CUSTOM", json!({})),
            Some("CUSTOM"),
        ),
        (
            "a managed connector running the S3 connector",
            managed.as_str(),
            managed_connector_data_source(
                "kinds-connector-s3",
                "S3",
                json!({ "connectionConfiguration": { "bucketName": "document-kinds-docs" } }),
            ),
            Some("S3"),
        ),
        (
            "a managed connector running a connector that crawls its own source",
            managed.as_str(),
            managed_connector_data_source(
                "kinds-connector-web",
                "WEB",
                json!({ "connectionConfiguration": { "url": "https://example.com/docs" } }),
            ),
            None,
        ),
        (
            "a self-managed CUSTOM data source",
            self_managed.as_str(),
            self_managed_data_source("kinds-custom", "CUSTOM"),
            Some("CUSTOM"),
        ),
        (
            "a self-managed S3 data source",
            self_managed.as_str(),
            self_managed_data_source("kinds-s3", "S3"),
            Some("S3"),
        ),
        (
            "a self-managed data source of a kind no content names",
            self_managed.as_str(),
            self_managed_data_source("kinds-redshift", "REDSHIFT_METADATA"),
            None,
        ),
        (
            "a self-managed data source crawling its own source",
            self_managed.as_str(),
            self_managed_data_source("kinds-web", "WEB"),
            None,
        ),
    ];

    // Both values of the modelled `ContentDataSourceType`: each is presented to
    // every data source, so each row asserts both the acceptance of the kind it
    // holds and the refusal of the kind it does not.
    let content_kinds = ["CUSTOM", "S3"];

    for (case, knowledge_base_id, request, held) in cases {
        let data_source_id = emulator.data_source(knowledge_base_id, request).await;

        for content_kind in content_kinds {
            let case = &format!("{case}, {content_kind} content");
            let id = document_id(content_kind, "kinds-document");
            let identifier = identifier_of_kind(content_kind, &id);
            let answered = emulator
                .ingest(
                    knowledge_base_id,
                    &data_source_id,
                    vec![document_of_kind(content_kind, &id)],
                )
                .await;

            if held == Some(content_kind) {
                assert_eq!(
                    answered.status, 202,
                    "case {case}: expected IngestKnowledgeBaseDocuments -> 202, got {answered:?}"
                );
                assert_eq!(
                    answered.details().len(),
                    1,
                    "case {case}: expected one detail per submitted document, got {:?}",
                    answered.body
                );
                assert_detail(
                    case,
                    answered.detail(0),
                    knowledge_base_id,
                    &data_source_id,
                    &identifier,
                    &INGESTION_STATUSES,
                );

                // The document the ingestion accepted is the one the store now
                // holds, which is what the read reports.
                let fetched = emulator
                    .get_documents(
                        knowledge_base_id,
                        &data_source_id,
                        vec![identifier.clone()],
                    )
                    .await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case}: expected GetKnowledgeBaseDocuments -> 200, got {fetched:?}"
                );
                assert_detail(
                    case,
                    fetched.detail(0),
                    knowledge_base_id,
                    &data_source_id,
                    &identifier,
                    &INGESTION_STATUSES,
                );
                continue;
            }

            assert_eq!(
                answered.status, 400,
                "case {case}: expected IngestKnowledgeBaseDocuments -> 400, got {answered:?}"
            );
            assert_eq!(
                answered.error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {answered:?}"
            );
            // A refused request writes nothing, so the data source still holds
            // no such document.
            let fetched = emulator
                .get_documents(knowledge_base_id, &data_source_id, vec![identifier])
                .await;
            assert_eq!(
                fetched.status_at(0),
                NOT_FOUND,
                "case {case}: a refused request persists no document, got {:?}",
                fetched.detail(0)
            );
        }
    }
}

/// Every document of a request is refused when ONE of them names a kind the
/// data source does not hold, and a request naming only the kind it holds is
/// taken whole.
#[tokio::test]
async fn one_mismatched_document_refuses_the_whole_request() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(managed_knowledge_base("document-kinds-batch"))
        .await;
    let data_source_id = emulator
        .data_source(
            &knowledge_base_id,
            managed_connector_data_source("kinds-batch-custom", "CUSTOM", json!({})),
        )
        .await;

    // Rows: where in the request the mismatched document sits, and a request
    // with none.
    let cases: Vec<(&str, Vec<&str>, bool)> = vec![
        ("the mismatched document first", vec!["S3", "CUSTOM"], false),
        ("the mismatched document last", vec!["CUSTOM", "S3"], false),
        ("every document of the kind held", vec!["CUSTOM", "CUSTOM"], true),
    ];

    for (index, (case, kinds, accepted)) in cases.into_iter().enumerate() {
        let documents: Vec<Value> = kinds
            .iter()
            .enumerate()
            .map(|(position, kind)| {
                document_of_kind(kind, &document_id(kind, &format!("batch-{index}-{position}")))
            })
            .collect();
        let identifiers: Vec<Value> = kinds
            .iter()
            .enumerate()
            .map(|(position, kind)| {
                identifier_of_kind(kind, &document_id(kind, &format!("batch-{index}-{position}")))
            })
            .collect();

        let answered = emulator
            .ingest(&knowledge_base_id, &data_source_id, documents)
            .await;
        let expected = if accepted { 202 } else { 400 };
        assert_eq!(
            answered.status, expected,
            "case {case}: expected IngestKnowledgeBaseDocuments -> {expected}, got {answered:?}"
        );

        let fetched = emulator
            .get_documents(&knowledge_base_id, &data_source_id, identifiers.clone())
            .await;
        for (position, identifier) in identifiers.iter().enumerate() {
            let status = fetched.detail_of(identifier)["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let stored = status != NOT_FOUND;
            assert_eq!(
                stored, accepted,
                "case {case}: document {position} stored={stored}, expected stored={accepted}, \
                 got {:?}",
                fetched.detail_of(identifier)
            );
        }
    }
}

/// A deleted document stays VISIBLE: `DeleteKnowledgeBaseDocuments` answers 202
/// with the document on its way out, the clock carries it to `NOT_FOUND`, and
/// both the listing and the per-identifier read keep reporting it with that
/// status and its `updatedAt` — rather than dropping it, which a caller cannot
/// tell from a document that never existed.
///
/// The rows differ along what the rule generalises over: the kind of data source
/// the document lives in, the kind of content (each value of the modelled
/// `ContentDataSourceType`) and how many of the data source's documents one
/// delete names.
#[tokio::test]
async fn a_deleted_document_stays_listed_with_the_not_found_status() {
    let emulator = Emulator::new();
    let managed = emulator
        .knowledge_base(managed_knowledge_base("document-deletion-managed"))
        .await;
    let self_managed = emulator
        .knowledge_base(vector_knowledge_base("document-deletion-vector"))
        .await;

    // (case, the knowledge base, the data source, the content kind it holds,
    // how many documents to ingest, how many of them to delete).
    let cases: Vec<(&str, &str, Value, &str, usize, usize)> = vec![
        (
            "the only document of a managed CUSTOM connector",
            managed.as_str(),
            managed_connector_data_source("deletion-connector-custom", "CUSTOM", json!({})),
            "CUSTOM",
            1,
            1,
        ),
        (
            "one of three documents of a self-managed CUSTOM data source",
            self_managed.as_str(),
            self_managed_data_source("deletion-custom", "CUSTOM"),
            "CUSTOM",
            3,
            1,
        ),
        (
            "every document of a self-managed S3 data source",
            self_managed.as_str(),
            self_managed_data_source("deletion-s3", "S3"),
            "S3",
            2,
            2,
        ),
    ];

    for (case, knowledge_base_id, request, content_kind, held, deleted) in cases {
        let data_source_id = emulator.data_source(knowledge_base_id, request).await;
        let ids: Vec<String> = (0..held)
            .map(|position| document_id(content_kind, &format!("deletion-{position}")))
            .collect();
        let identifiers: Vec<Value> = ids
            .iter()
            .map(|id| identifier_of_kind(content_kind, id))
            .collect();
        let ingested = emulator
            .ingest(
                knowledge_base_id,
                &data_source_id,
                ids.iter()
                    .map(|id| document_of_kind(content_kind, id))
                    .collect(),
            )
            .await;
        assert_eq!(
            ingested.status, 202,
            "case {case}: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
        );

        // The documents reach the terminal ingestion status on the clock, so
        // the deletion below starts from an indexed document.
        tokio::time::sleep(DOCUMENT_INGESTION_SPAN + DOCUMENT_INDEXING_SPAN + MARGIN).await;
        let listed = emulator
            .list_documents(knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            listed.details().len(),
            held,
            "case {case}: expected every ingested document listed, got {:?}",
            listed.body
        );
        for identifier in &identifiers {
            assert_eq!(
                listed.detail_of(identifier)["status"].as_str(),
                Some(INDEXED),
                "case {case}: expected the ingested document indexed, got {:?}",
                listed.detail_of(identifier)
            );
        }

        let doomed: Vec<Value> = identifiers.iter().take(deleted).cloned().collect();
        let answered = emulator
            .delete_documents(knowledge_base_id, &data_source_id, doomed.clone())
            .await;
        assert_eq!(
            answered.status, 202,
            "case {case}: expected DeleteKnowledgeBaseDocuments -> 202, got {answered:?}"
        );
        assert_eq!(
            answered.details().len(),
            deleted,
            "case {case}: expected one detail per identifier deleted, got {:?}",
            answered.body
        );
        for (position, identifier) in doomed.iter().enumerate() {
            assert_detail(
                case,
                answered.detail(position),
                knowledge_base_id,
                &data_source_id,
                identifier,
                &DELETION_STATUSES,
            );
        }

        // The clock carries the deletion to its terminal status, and the
        // document is still one of the data source's documents there.
        tokio::time::sleep(DOCUMENT_DELETION_START_SPAN + DOCUMENT_DELETION_SPAN + MARGIN).await;
        let listed = emulator
            .list_documents(knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            listed.details().len(),
            held,
            "case {case}: a deleted document is still one of the data source's documents, \
             got {:?}",
            listed.body
        );
        let fetched = emulator
            .get_documents(knowledge_base_id, &data_source_id, identifiers.clone())
            .await;
        for (position, identifier) in identifiers.iter().enumerate() {
            let expected = if position < deleted { NOT_FOUND } else { INDEXED };
            for (operation, detail) in [
                ("ListKnowledgeBaseDocuments", listed.detail_of(identifier)),
                ("GetKnowledgeBaseDocuments", fetched.detail_of(identifier)),
            ] {
                assert_detail(
                    case,
                    detail,
                    knowledge_base_id,
                    &data_source_id,
                    identifier,
                    &[expected],
                );
                assert_eq!(
                    detail["status"].as_str(),
                    Some(expected),
                    "case {case}: {operation} reports document {position} as {expected}, \
                     got {detail:?}"
                );
            }
        }
    }
}

/// A delete is idempotent and reports only what the data source holds: a
/// repeated delete of the same document reports it exactly as it stands, an
/// identifier the data source never held is reported as a document that was not
/// found and nothing is stored for it, and re-ingesting a deleted id starts a
/// fresh ingestion of it.
#[tokio::test]
async fn deleting_again_and_deleting_what_was_never_held() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(managed_knowledge_base("document-deletion-repeat"))
        .await;
    let data_source_id = emulator
        .data_source(
            &knowledge_base_id,
            managed_connector_data_source("deletion-repeat-custom", "CUSTOM", json!({})),
        )
        .await;

    let identifier = identifier_of_kind("CUSTOM", "repeat-document");
    let ingested = emulator
        .ingest(
            &knowledge_base_id,
            &data_source_id,
            vec![document_of_kind("CUSTOM", "repeat-document")],
        )
        .await;
    assert_eq!(
        ingested.status, 202,
        "case setup: expected IngestKnowledgeBaseDocuments -> 202, got {ingested:?}"
    );

    // An identifier nothing was ever stored for: a document whose status says it
    // was not found, and no record of it afterwards.
    let unknown = identifier_of_kind("CUSTOM", "never-ingested-document");
    let answered = emulator
        .delete_documents(&knowledge_base_id, &data_source_id, vec![unknown.clone()])
        .await;
    assert_eq!(
        answered.status, 202,
        "case an identifier never ingested: expected DeleteKnowledgeBaseDocuments -> 202, \
         got {answered:?}"
    );
    assert_eq!(
        answered.status_at(0),
        NOT_FOUND,
        "case an identifier never ingested: expected {NOT_FOUND}, got {:?}",
        answered.detail(0)
    );
    let listed = emulator
        .list_documents(&knowledge_base_id, &data_source_id)
        .await;
    assert_eq!(
        listed.details().len(),
        1,
        "case an identifier never ingested: deleting it stores nothing, got {:?}",
        listed.body
    );

    // The first delete puts the document on its way out; a repeat reports it
    // exactly as it stands rather than restarting the chain, so the instant it
    // last changed does not move — only the clock's own transitions move it.
    let first = emulator
        .delete_documents(
            &knowledge_base_id,
            &data_source_id,
            vec![identifier.clone()],
        )
        .await;
    assert_eq!(
        first.status, 202,
        "case a repeated delete: expected DeleteKnowledgeBaseDocuments -> 202, got {first:?}"
    );
    let accepted_at = first.detail(0)["updatedAt"].clone();
    let repeated = emulator
        .delete_documents(
            &knowledge_base_id,
            &data_source_id,
            vec![identifier.clone()],
        )
        .await;
    assert_eq!(
        repeated.detail(0)["updatedAt"],
        accepted_at,
        "case a repeated delete: the deletion is not accepted a second time, got {:?}",
        repeated.detail(0)
    );

    // Once the clock has carried the deletion to its terminal status, the
    // document stays there: repeating the delete changes neither the status nor
    // the instant it last changed.
    tokio::time::sleep(DOCUMENT_DELETION_START_SPAN + DOCUMENT_DELETION_SPAN + MARGIN).await;
    let gone = emulator
        .delete_documents(
            &knowledge_base_id,
            &data_source_id,
            vec![identifier.clone()],
        )
        .await;
    assert_eq!(
        gone.status_at(0),
        NOT_FOUND,
        "case a repeated delete: the document is gone, got {:?}",
        gone.detail(0)
    );
    let gone_at = gone.detail(0)["updatedAt"].clone();
    let again = emulator
        .delete_documents(
            &knowledge_base_id,
            &data_source_id,
            vec![identifier.clone()],
        )
        .await;
    assert_eq!(
        again.status_at(0),
        NOT_FOUND,
        "case a repeated delete of a document already gone: expected {NOT_FOUND}, got {:?}",
        again.detail(0)
    );
    assert_eq!(
        again.detail(0)["updatedAt"],
        gone_at,
        "case a repeated delete of a document already gone: the instant it last changed \
         does not move, got {:?}",
        again.detail(0)
    );

    // The id is free again: ingesting it is a fresh ingestion, reported from
    // the start of the ingestion progression.
    let reingested = emulator
        .ingest(
            &knowledge_base_id,
            &data_source_id,
            vec![document_of_kind("CUSTOM", "repeat-document")],
        )
        .await;
    assert_eq!(
        reingested.status, 202,
        "case re-ingesting a deleted id: expected IngestKnowledgeBaseDocuments -> 202, \
         got {reingested:?}"
    );
    assert_detail(
        "re-ingesting a deleted id",
        reingested.detail(0),
        &knowledge_base_id,
        &data_source_id,
        &identifier,
        &INGESTION_STATUSES,
    );
    let fetched = emulator
        .get_documents(&knowledge_base_id, &data_source_id, vec![identifier.clone()])
        .await;
    assert_detail(
        "re-ingesting a deleted id",
        fetched.detail(0),
        &knowledge_base_id,
        &data_source_id,
        &identifier,
        &INGESTION_STATUSES,
    );
}

/// The document reads and the delete are scoped to the knowledge base and data
/// source the URI names, and a knowledge base or data source the store does not
/// hold is a missing resource.
#[tokio::test]
async fn deleting_a_document_is_scoped_to_the_data_source_the_uri_names() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base(managed_knowledge_base("document-deletion-scope"))
        .await;
    let held = emulator
        .data_source(
            &knowledge_base_id,
            managed_connector_data_source("deletion-scope-held", "CUSTOM", json!({})),
        )
        .await;
    let other = emulator
        .data_source(
            &knowledge_base_id,
            managed_connector_data_source("deletion-scope-other", "CUSTOM", json!({})),
        )
        .await;

    let identifier = identifier_of_kind("CUSTOM", "scoped-document");
    emulator
        .ingest(
            &knowledge_base_id,
            &held,
            vec![document_of_kind("CUSTOM", "scoped-document")],
        )
        .await;

    // The same id in another data source is another document, so deleting it
    // there leaves the one held here untouched.
    let answered = emulator
        .delete_documents(&knowledge_base_id, &other, vec![identifier.clone()])
        .await;
    assert_eq!(
        answered.status_at(0),
        NOT_FOUND,
        "case another data source of the same knowledge base: expected {NOT_FOUND}, got {:?}",
        answered.detail(0)
    );
    let fetched = emulator
        .get_documents(&knowledge_base_id, &held, vec![identifier.clone()])
        .await;
    assert_detail(
        "another data source of the same knowledge base",
        fetched.detail(0),
        &knowledge_base_id,
        &held,
        &identifier,
        &INGESTION_STATUSES,
    );

    // A knowledge base or data source nothing holds has no document to delete.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an unknown knowledge base",
            "0000000000".to_string(),
            held.clone(),
        ),
        (
            "an unknown data source",
            knowledge_base_id.clone(),
            "0000000000".to_string(),
        ),
    ];
    for (case, knowledge_base, data_source) in cases {
        let refused = emulator
            .delete_documents(&knowledge_base, &data_source, vec![identifier.clone()])
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case}: expected DeleteKnowledgeBaseDocuments -> 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {refused:?}"
        );
    }

    // An identifier carrying none of the details its kind needs names no
    // document at all: a bad request rather than a document that is missing.
    let refused = emulator
        .delete_documents(
            &knowledge_base_id,
            &held,
            vec![json!({ "dataSourceType": "CUSTOM" })],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case an identifier carrying none of its kind's details: \
         expected DeleteKnowledgeBaseDocuments -> 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ValidationException"),
        "case an identifier carrying none of its kind's details: \
         expected ValidationException, got {refused:?}"
    );
}
