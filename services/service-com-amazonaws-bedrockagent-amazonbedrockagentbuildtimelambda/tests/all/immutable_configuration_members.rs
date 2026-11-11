// SPDX-License-Identifier: CC0-1.0

//! The members an update cannot change, end to end over HTTP.
//!
//! `UpdateDataSource` documents `chunkingConfiguration` as unchangeable once the
//! connector exists, and `UpdateKnowledgeBase` documents the same of
//! `knowledgeBaseConfiguration` and `storageConfiguration`
//! (`smithy_query(query="doc",shape="UpdateDataSource")` /
//! `smithy_query(query="doc",shape="UpdateKnowledgeBase")`). The rule both state
//! is one rule: an update that would leave a different value in such a member is
//! refused with the modelled `ValidationException` (HTTP 400) and writes nothing,
//! while one that re-supplies the stored value is served and still writes every
//! mutable member beside it.
//!
//! Each test drives a TABLE whose rows differ along what that rule generalises
//! over — which strategy or kind the configuration names, the values inside it,
//! how large it is, and whether the member is present at all — and asserts, for
//! every refused row, the status, the error shape, the message AND that the
//! following read still reports the stored resource untouched.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
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

/// A second role ARN, so a served row can change `roleArn` — a member this
/// operation DOES change — while the immutable ones stay as they are.
const OTHER_KB_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonBedrockKnowledgeBaseRoleTwo";

/// An embedding model ARN matching the modelled `BedrockEmbeddingModelArn`.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// A second embedding model, so a row can change the one member of a vector
/// knowledge base's configuration.
const OTHER_EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/cohere.embed-english-v3";

/// Every value the modelled `ChunkingStrategy` enum admits, which is what a
/// chunking configuration differs along.
///
/// Source: this service's Smithy model
/// (`smithy_query(query="shape",shape="ChunkingStrategy")`).
const CHUNKING_STRATEGIES: [&str; 4] = ["FIXED_SIZE", "HIERARCHICAL", "NONE", "SEMANTIC"];

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
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
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
        // The knowledge bases here bind to an S3 Vectors index, which
        // `CreateKnowledgeBase` resolves in the service that owns it — so every
        // index a row names is created first, as a caller creates it first
        // against AWS.
        for index in [
            "immutable-members-index",
            "atomic-refusal-index",
            "resupplied-chunking-index",
            "no-chunking-index",
            "stored-kb-index",
            "another-kb-index",
        ] {
            crate::vector_store_fixture::seed_index(
                &engine,
                "arn:aws:s3vectors:::bucket/immutable-members-vectors",
                index,
                crate::vector_store_fixture::EMBEDDING_DIMENSION,
            );
        }
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

    async fn knowledge_base_id(&self, request: Value) -> String {
        let created = self.call("PUT", "/knowledgebases/", Some(request)).await;
        assert_eq!(
            created.status, 202,
            "case setup: expected CreateKnowledgeBase -> 202, got {created:?}"
        );
        created.string(&["knowledgeBase", "knowledgeBaseId"])
    }

    async fn update_knowledge_base(&self, knowledge_base_id: &str, request: Value) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}"),
            Some(request),
        )
        .await
    }

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }

    async fn data_source_id(&self, knowledge_base_id: &str, request: Value) -> String {
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
        created.string(&["dataSource", "dataSourceId"])
    }

    async fn update_data_source(
        &self,
        knowledge_base_id: &str,
        data_source_id: &str,
        request: Value,
    ) -> Answer {
        self.call(
            "PUT",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            Some(request),
        )
        .await
    }

    async fn get_data_source(&self, knowledge_base_id: &str, data_source_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/knowledgebases/{knowledge_base_id}/datasources/{data_source_id}"),
            None,
        )
        .await
    }
}

/// The refusal message the service answers for a member that cannot change once
/// the resource exists.
///
/// Each member AWS words a sentence for has its OWN, entirely fixed text — the
/// message is the only discriminator a client has inside a generic
/// `ValidationException`, so an immutability refusal of the knowledge base's
/// configuration must not read like one of its storage configuration. A member
/// AWS words none for is refused by naming the member path the request carried.
fn immutable_member_message(member: &str) -> String {
    match member {
        "knowledgeBaseConfiguration" => {
            "You cannot modify the Vector knowledge base configuration once it is created."
                .to_string()
        }
        "storageConfiguration" => {
            "You cannot modify the storage configuration of the Vector knowledge base once \
             created."
                .to_string()
        }
        other => format!("{other} cannot be updated once created."),
    }
}

/// The sentence AWS words when an update would change the knowledge base's
/// TYPE: the configuration's discriminator has its own refusal, which is
/// entirely fixed text.
fn knowledge_base_type_message() -> String {
    "You can't modify the type of the knowledge base after creation.".to_string()
}

/// A self-managed vector knowledge base over an S3 vector store, as the pair of
/// configurations `CreateKnowledgeBase` takes.
fn vector_configurations(embedding_model: &str, index_name: &str) -> (Value, Value) {
    (
        json!({
            "type": "VECTOR",
            "vectorKnowledgeBaseConfiguration": { "embeddingModelArn": embedding_model },
        }),
        json!({
            "type": "S3_VECTORS",
            "s3VectorsConfiguration": {
                "vectorBucketArn": "arn:aws:s3vectors:::bucket/immutable-members-vectors",
                "indexName": index_name,
            },
        }),
    )
}

/// An S3 connector over `bucket`: the `type` discriminator names the
/// `s3Configuration` member that carries its connection details.
fn s3_configuration(bucket: &str) -> Value {
    json!({
        "type": "S3",
        "s3Configuration": { "bucketArn": format!("arn:aws:s3:::{bucket}") },
    })
}

/// A chunking configuration of `strategy`, carrying that strategy's own settings
/// member where the model declares one.
///
/// `maxTokens` is modelled `range 1..8192`, `overlapPercentage` `range 1..99`,
/// `bufferSize` `range 0..1`, `breakpointPercentileThreshold` `range 50..99` and
/// `levelConfigurations` `length 2..2`
/// (`smithy_query(query="shape",shape="FixedSizeChunkingConfiguration,\
/// SemanticChunkingConfiguration,HierarchicalChunkingConfiguration")`), so every
/// value below is inside its member's published bounds.
fn chunking(strategy: &str, tokens: i64) -> Value {
    match strategy {
        "FIXED_SIZE" => json!({
            "chunkingStrategy": "FIXED_SIZE",
            "fixedSizeChunkingConfiguration": {
                "maxTokens": tokens,
                "overlapPercentage": 20,
            },
        }),
        "HIERARCHICAL" => json!({
            "chunkingStrategy": "HIERARCHICAL",
            "hierarchicalChunkingConfiguration": {
                "levelConfigurations": [
                    { "maxTokens": tokens },
                    { "maxTokens": 1 },
                ],
                "overlapTokens": 1,
            },
        }),
        "SEMANTIC" => json!({
            "chunkingStrategy": "SEMANTIC",
            "semanticChunkingConfiguration": {
                "maxTokens": tokens,
                "bufferSize": 1,
                "breakpointPercentileThreshold": 50,
            },
        }),
        // "If you set the chunkingStrategy as NONE, exclude this field"
        // (`smithy_query(query="doc",shape="FixedSizeChunkingConfiguration")`),
        // so NONE carries no settings member at all.
        other => json!({ "chunkingStrategy": other }),
    }
}

/// An `UpdateDataSource` body: every required member, a name and description that
/// differ from the creation's, and whatever `vectorIngestionConfiguration` the
/// row carries.
fn update_request(name: &str, ingestion: Option<Value>) -> Value {
    let mut request = json!({
        "name": name,
        "description": "the description the refused update tried to write",
        "dataSourceConfiguration": s3_configuration("immutable-members-other-docs"),
    });
    if let Some(ingestion) = ingestion {
        request["vectorIngestionConfiguration"] = ingestion;
    }
    request
}

/// Assert `answer` is the modelled refusal for `member`: HTTP 400, the
/// `ValidationException` shape, and the message AWS answers for that class.
fn assert_immutable_member_refusal(case: &str, member: &str, answer: &Answer) {
    assert_validation_refusal(case, &immutable_member_message(member), answer);
}

/// Assert `answer` is the modelled `ValidationException` (HTTP 400) worded
/// `expected_message`.
fn assert_validation_refusal(case: &str, expected_message: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected the refused update to answer HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some("ValidationException"),
        "case {case}: expected the modelled ValidationException, got {:?}",
        answer.error_type
    );
    assert_eq!(
        answer.string(&["message"]),
        expected_message,
        "case {case}: expected the refusal to be worded {expected_message:?}, got {:?}",
        answer.member(&["message"])
    );
}

/// `UpdateDataSource` refuses every update that would leave a different
/// `chunkingConfiguration` on the data source, and writes nothing when it does.
#[tokio::test]
async fn changing_a_stored_chunking_configuration_is_refused() {
    let emulator = Emulator::new();
    let (knowledge_base_configuration, storage_configuration) =
        vector_configurations(EMBEDDING_MODEL, "immutable-members-index");
    let knowledge_base_id = emulator
        .knowledge_base_id(json!({
            "name": "immutable-members-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": knowledge_base_configuration,
            "storageConfiguration": storage_configuration,
        }))
        .await;

    let stored_chunking = chunking("FIXED_SIZE", 300);
    let data_source_id = emulator
        .data_source_id(
            &knowledge_base_id,
            json!({
                "name": "immutable-members-source",
                "description": "the description the create supplied",
                "dataSourceConfiguration": s3_configuration("immutable-members-docs"),
                "vectorIngestionConfiguration": {
                    "chunkingConfiguration": stored_chunking.clone(),
                },
            }),
        )
        .await;

    // The rows differ along the strategy the configuration names, the values
    // inside one strategy (including each published bound of `maxTokens` and of
    // `overlapPercentage`), the size of a nested list, and whether the member is
    // there at all.
    let mut rows: Vec<(String, Option<Value>)> = vec![
        (
            "the same strategy with maxTokens at its published minimum".to_string(),
            Some(json!({ "chunkingConfiguration": chunking("FIXED_SIZE", 1) })),
        ),
        (
            "the same strategy with maxTokens at its published maximum".to_string(),
            Some(json!({ "chunkingConfiguration": chunking("FIXED_SIZE", 8192) })),
        ),
        (
            "the same strategy with a different overlapPercentage".to_string(),
            Some(json!({
                "chunkingConfiguration": {
                    "chunkingStrategy": "FIXED_SIZE",
                    "fixedSizeChunkingConfiguration": {
                        "maxTokens": 300,
                        "overlapPercentage": 99,
                    },
                },
            })),
        ),
        (
            "no vectorIngestionConfiguration at all, which would drop the chunking \
             configuration"
                .to_string(),
            None,
        ),
        (
            "a vectorIngestionConfiguration carrying only a parsingConfiguration, which \
             would drop the chunking configuration"
                .to_string(),
            Some(json!({
                "parsingConfiguration": { "parsingStrategy": "SMART_PARSING" },
            })),
        ),
    ];
    // Every OTHER strategy the model admits is a different chunking
    // configuration too, whatever its settings.
    for strategy in CHUNKING_STRATEGIES {
        if strategy == "FIXED_SIZE" {
            continue;
        }
        rows.push((
            format!("the {strategy} strategy in place of the stored one"),
            Some(json!({ "chunkingConfiguration": chunking(strategy, 300) })),
        ));
    }

    for (case, ingestion) in rows {
        let refused = emulator
            .update_data_source(
                &knowledge_base_id,
                &data_source_id,
                update_request("renamed-immutable-members-source", ingestion),
            )
            .await;
        assert_immutable_member_refusal(
            &case,
            "vectorIngestionConfiguration.chunkingConfiguration",
            &refused,
        );

        // A refusal persists nothing: neither the chunking configuration it
        // tried to change nor the mutable members it carried alongside.
        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetDataSource -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.member(&[
                "dataSource",
                "vectorIngestionConfiguration",
                "chunkingConfiguration"
            ]),
            &stored_chunking,
            "case {case}: a refused update leaves the stored chunkingConfiguration in \
             place, got {:?}",
            fetched.member(&["dataSource", "vectorIngestionConfiguration"])
        );
        assert_eq!(
            fetched.string(&["dataSource", "name"]),
            "immutable-members-source",
            "case {case}: a refused update writes none of the members it carried, got \
             {fetched:?}"
        );
    }
}

/// A refused update writes NO part of itself: the members it carries beside the
/// chunking configuration stay at the values the last SERVED update left, not at
/// the ones the refusal tried to write.
///
/// This is the whole sequence a caller runs: an update that re-supplies the
/// stored chunking configuration and changes the mutable members is served, and
/// every later update that would leave a different chunking configuration there
/// is refused without disturbing what that served update wrote.
#[tokio::test]
async fn a_refused_update_writes_none_of_the_members_it_carries() {
    let emulator = Emulator::new();
    let (knowledge_base_configuration, storage_configuration) =
        vector_configurations(EMBEDDING_MODEL, "atomic-refusal-index");
    let knowledge_base_id = emulator
        .knowledge_base_id(json!({
            "name": "atomic-refusal-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": knowledge_base_configuration,
            "storageConfiguration": storage_configuration,
        }))
        .await;

    let stored_chunking = chunking("FIXED_SIZE", 300);
    let data_source_id = emulator
        .data_source_id(
            &knowledge_base_id,
            json!({
                "name": "atomic-refusal-source",
                "description": "the description the create supplied",
                "dataSourceConfiguration": s3_configuration("atomic-refusal-docs"),
                "vectorIngestionConfiguration": {
                    "chunkingConfiguration": stored_chunking.clone(),
                },
            }),
        )
        .await;

    // The served update: the stored chunking configuration, re-supplied as this
    // operation's contract tells the caller to, beside new mutable members.
    let served_name = "served-atomic-refusal-source";
    let served_description = "the description the served update wrote";
    let served = emulator
        .update_data_source(
            &knowledge_base_id,
            &data_source_id,
            json!({
                "name": served_name,
                "description": served_description,
                "dataSourceConfiguration": s3_configuration("atomic-refusal-docs"),
                "vectorIngestionConfiguration": {
                    "chunkingConfiguration": stored_chunking.clone(),
                },
            }),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case the served update: an update keeping the stored chunkingConfiguration is \
         served, got {served:?}"
    );
    assert_eq!(
        served.string(&["dataSource", "description"]),
        served_description,
        "case the served update: expected the description it supplied, got {:?}",
        served.member(&["dataSource", "description"])
    );

    // The rows differ along HOW the effective chunking configuration would
    // diverge: a different value inside the stored strategy, a different
    // strategy, and the member left out of the request altogether (this
    // operation replaces `vectorIngestionConfiguration` whole rather than
    // patching it, so an omission drops the stored chunking).
    let rows: Vec<(&str, Option<Value>)> = vec![
        (
            "a different maxTokens inside the stored strategy",
            Some(json!({ "chunkingConfiguration": chunking("FIXED_SIZE", 512) })),
        ),
        (
            "a different overlapPercentage inside the stored strategy",
            Some(json!({
                "chunkingConfiguration": {
                    "chunkingStrategy": "FIXED_SIZE",
                    "fixedSizeChunkingConfiguration": {
                        "maxTokens": 300,
                        "overlapPercentage": 10,
                    },
                },
            })),
        ),
        (
            "another strategy the model admits",
            Some(json!({ "chunkingConfiguration": chunking("HIERARCHICAL", 300) })),
        ),
        (
            "a strategy that carries no settings member",
            Some(json!({ "chunkingConfiguration": chunking("NONE", 300) })),
        ),
        ("no vectorIngestionConfiguration at all", None),
        (
            "a vectorIngestionConfiguration carrying only a sibling member",
            Some(json!({
                "parsingConfiguration": { "parsingStrategy": "SMART_PARSING" },
            })),
        ),
    ];

    for (case, ingestion) in rows {
        let refused = emulator
            .update_data_source(
                &knowledge_base_id,
                &data_source_id,
                update_request("refused-atomic-refusal-source", ingestion),
            )
            .await;
        assert_immutable_member_refusal(
            case,
            "vectorIngestionConfiguration.chunkingConfiguration",
            &refused,
        );

        let fetched = emulator
            .get_data_source(&knowledge_base_id, &data_source_id)
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetDataSource -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.member(&[
                "dataSource",
                "vectorIngestionConfiguration",
                "chunkingConfiguration"
            ]),
            &stored_chunking,
            "case {case}: expected the stored chunkingConfiguration to be untouched, got {:?}",
            fetched.member(&["dataSource", "vectorIngestionConfiguration"])
        );
        assert_eq!(
            fetched.string(&["dataSource", "description"]),
            served_description,
            "case {case}: expected the description the served update wrote, got {:?}",
            fetched.member(&["dataSource", "description"])
        );
        assert_eq!(
            fetched.string(&["dataSource", "name"]),
            served_name,
            "case {case}: expected the name the served update wrote, got {:?}",
            fetched.member(&["dataSource", "name"])
        );
    }
}

/// An update that re-supplies the stored `chunkingConfiguration` is served, and
/// still writes every mutable member beside it — including the other members of
/// `vectorIngestionConfiguration`, which this operation does change.
#[tokio::test]
async fn resupplying_the_stored_chunking_configuration_is_served() {
    let emulator = Emulator::new();
    let (knowledge_base_configuration, storage_configuration) =
        vector_configurations(EMBEDDING_MODEL, "resupplied-chunking-index");
    let knowledge_base_id = emulator
        .knowledge_base_id(json!({
            "name": "resupplied-chunking-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": knowledge_base_configuration,
            "storageConfiguration": storage_configuration,
        }))
        .await;

    // One data source per stored chunking configuration, so the rule is
    // exercised over every strategy the model admits rather than over one.
    for strategy in CHUNKING_STRATEGIES {
        let stored_chunking = chunking(strategy, 300);
        let data_source_id = emulator
            .data_source_id(
                &knowledge_base_id,
                json!({
                    "name": "resupplied-chunking-source",
                    "dataSourceConfiguration": s3_configuration("resupplied-chunking-docs"),
                    "vectorIngestionConfiguration": {
                        "chunkingConfiguration": stored_chunking.clone(),
                    },
                }),
            )
            .await;

        // The rows differ along which mutable member the update changes beside
        // the unchanged chunking configuration.
        let rows: Vec<(String, Value)> = vec![
            (
                format!("{strategy}: the stored chunking configuration, resent as it stands"),
                json!({ "chunkingConfiguration": stored_chunking.clone() }),
            ),
            (
                format!("{strategy}: the stored chunking configuration and a new sibling"),
                json!({
                    "chunkingConfiguration": stored_chunking.clone(),
                    "parsingConfiguration": { "parsingStrategy": "SMART_PARSING" },
                }),
            ),
        ];

        for (case, ingestion) in rows {
            let updated = emulator
                .update_data_source(
                    &knowledge_base_id,
                    &data_source_id,
                    update_request("renamed-resupplied-source", Some(ingestion.clone())),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: an update that keeps the stored chunkingConfiguration is \
                 served, got {updated:?}"
            );
            assert_eq!(
                updated.member(&["dataSource", "vectorIngestionConfiguration"]),
                &ingestion,
                "case {case}: expected the vectorIngestionConfiguration the update \
                 supplied, got {:?}",
                updated.member(&["dataSource", "vectorIngestionConfiguration"])
            );

            let fetched = emulator
                .get_data_source(&knowledge_base_id, &data_source_id)
                .await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected GetDataSource -> 200, got {fetched:?}"
            );
            assert_eq!(
                fetched.member(&["dataSource", "vectorIngestionConfiguration"]),
                &ingestion,
                "case {case}: expected GetDataSource to report the stored \
                 vectorIngestionConfiguration, got {:?}",
                fetched.member(&["dataSource", "vectorIngestionConfiguration"])
            );
            assert_eq!(
                fetched.string(&["dataSource", "name"]),
                "renamed-resupplied-source",
                "case {case}: the update writes the mutable members it carried, got \
                 {fetched:?}"
            );
        }
    }
}

/// A data source stored WITHOUT a chunking configuration has none to keep: an
/// update that adds one is refused, and one that leaves the member out is served.
#[tokio::test]
async fn a_data_source_without_a_chunking_configuration_keeps_having_none() {
    let emulator = Emulator::new();
    let (knowledge_base_configuration, storage_configuration) =
        vector_configurations(EMBEDDING_MODEL, "no-chunking-index");
    let knowledge_base_id = emulator
        .knowledge_base_id(json!({
            "name": "no-chunking-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": knowledge_base_configuration,
            "storageConfiguration": storage_configuration,
        }))
        .await;
    let data_source_id = emulator
        .data_source_id(
            &knowledge_base_id,
            json!({
                "name": "no-chunking-source",
                "dataSourceConfiguration": s3_configuration("no-chunking-docs"),
            }),
        )
        .await;

    for strategy in CHUNKING_STRATEGIES {
        let case = format!("adding the {strategy} strategy to a data source that has none");
        let refused = emulator
            .update_data_source(
                &knowledge_base_id,
                &data_source_id,
                update_request(
                    "renamed-no-chunking-source",
                    Some(json!({ "chunkingConfiguration": chunking(strategy, 300) })),
                ),
            )
            .await;
        assert_immutable_member_refusal(
            &case,
            "vectorIngestionConfiguration.chunkingConfiguration",
            &refused,
        );
    }

    // The rows that keep the member absent are served, whether or not they carry
    // the other members of `vectorIngestionConfiguration`.
    let served: Vec<(&str, Option<Value>)> = vec![
        ("no vectorIngestionConfiguration at all", None),
        (
            "a vectorIngestionConfiguration with no chunkingConfiguration",
            Some(json!({
                "parsingConfiguration": { "parsingStrategy": "SMART_PARSING" },
            })),
        ),
    ];
    for (case, ingestion) in served {
        let updated = emulator
            .update_data_source(
                &knowledge_base_id,
                &data_source_id,
                update_request("renamed-no-chunking-source", ingestion),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: a data source with no stored chunkingConfiguration keeps having \
             none, so the update is served, got {updated:?}"
        );
        assert!(
            updated
                .member(&[
                    "dataSource",
                    "vectorIngestionConfiguration",
                    "chunkingConfiguration"
                ])
                .is_null(),
            "case {case}: expected the data source to still carry no chunkingConfiguration, \
             got {:?}",
            updated.member(&["dataSource", "vectorIngestionConfiguration"])
        );
    }
}

/// `UpdateKnowledgeBase` refuses every update that would leave a different
/// `knowledgeBaseConfiguration` or `storageConfiguration` on the knowledge base,
/// and serves the one that re-supplies both while changing the mutable members.
#[tokio::test]
async fn changing_a_stored_knowledge_base_configuration_is_refused() {
    let emulator = Emulator::new();
    let (stored_configuration, stored_storage) =
        vector_configurations(EMBEDDING_MODEL, "stored-kb-index");
    let knowledge_base_id = emulator
        .knowledge_base_id(json!({
            "name": "stored-config-kb",
            "roleArn": KB_ROLE,
            "knowledgeBaseConfiguration": stored_configuration.clone(),
            "storageConfiguration": stored_storage.clone(),
        }))
        .await;

    // The rows differ along which immutable member changes and how: a value
    // inside it, the kind its `type` discriminator names, and the vector store
    // it points at. A changed `type` is the configuration's discriminator rather
    // than one more value inside it, so it carries its own sentence.
    let (other_model_configuration, _) =
        vector_configurations(OTHER_EMBEDDING_MODEL, "stored-kb-index");
    let (_, other_index_storage) = vector_configurations(EMBEDDING_MODEL, "another-kb-index");
    let rows: Vec<(&str, String, Value, Value)> = vec![
        (
            "a different embedding model in the knowledge base configuration",
            immutable_member_message("knowledgeBaseConfiguration"),
            other_model_configuration,
            stored_storage.clone(),
        ),
        (
            "a knowledge base configuration of another type",
            knowledge_base_type_message(),
            json!({
                "type": "MANAGED",
                "managedKnowledgeBaseConfiguration": { "embeddingModelType": "MANAGED" },
            }),
            stored_storage.clone(),
        ),
        (
            "a different vector index in the storage configuration",
            immutable_member_message("storageConfiguration"),
            stored_configuration.clone(),
            other_index_storage,
        ),
        (
            "a storage configuration of another type",
            immutable_member_message("storageConfiguration"),
            stored_configuration.clone(),
            json!({
                "type": "OPENSEARCH_SERVERLESS",
                "opensearchServerlessConfiguration": {
                    "collectionArn": "arn:aws:aoss:eu-west-1:000000000000:collection/stored-kb",
                    "vectorIndexName": "stored-kb-index",
                    "fieldMapping": {
                        "vectorField": "vector",
                        "textField": "text",
                        "metadataField": "metadata",
                    },
                },
            }),
        ),
    ];

    for (case, expected_message, configuration, storage) in rows {
        let refused = emulator
            .update_knowledge_base(
                &knowledge_base_id,
                json!({
                    "name": "renamed-stored-config-kb",
                    "roleArn": OTHER_KB_ROLE,
                    "knowledgeBaseConfiguration": configuration,
                    "storageConfiguration": storage,
                }),
            )
            .await;
        assert_validation_refusal(case, &expected_message, &refused);

        let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetKnowledgeBase -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.member(&["knowledgeBase", "knowledgeBaseConfiguration"]),
            &stored_configuration,
            "case {case}: a refused update leaves the stored knowledgeBaseConfiguration in \
             place, got {:?}",
            fetched.member(&["knowledgeBase", "knowledgeBaseConfiguration"])
        );
        assert_eq!(
            fetched.member(&["knowledgeBase", "storageConfiguration"]),
            &stored_storage,
            "case {case}: a refused update leaves the stored storageConfiguration in place, \
             got {:?}",
            fetched.member(&["knowledgeBase", "storageConfiguration"])
        );
        assert_eq!(
            fetched.string(&["knowledgeBase", "name"]),
            "stored-config-kb",
            "case {case}: a refused update writes none of the members it carried, got \
             {fetched:?}"
        );
        assert_eq!(
            fetched.string(&["knowledgeBase", "roleArn"]),
            KB_ROLE,
            "case {case}: a refused update writes none of the members it carried, got \
             {fetched:?}"
        );
    }

    // The same two configurations, re-supplied as the contract asks, with the
    // mutable members changed: served, and the change is what the read reports.
    let updated = emulator
        .update_knowledge_base(
            &knowledge_base_id,
            json!({
                "name": "renamed-stored-config-kb",
                "description": "the description the served update supplied",
                "roleArn": OTHER_KB_ROLE,
                "knowledgeBaseConfiguration": stored_configuration.clone(),
                "storageConfiguration": stored_storage.clone(),
            }),
        )
        .await;
    assert_eq!(
        updated.status, 202,
        "case the stored configurations, re-supplied: expected UpdateKnowledgeBase -> 202, \
         got {updated:?}"
    );
    let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        fetched.string(&["knowledgeBase", "name"]),
        "renamed-stored-config-kb",
        "case the stored configurations, re-supplied: expected the name the update \
         supplied, got {fetched:?}"
    );
    assert_eq!(
        fetched.string(&["knowledgeBase", "roleArn"]),
        OTHER_KB_ROLE,
        "case the stored configurations, re-supplied: expected the roleArn the update \
         supplied, got {fetched:?}"
    );
    assert_eq!(
        fetched.member(&["knowledgeBase", "knowledgeBaseConfiguration"]),
        &stored_configuration,
        "case the stored configurations, re-supplied: the knowledgeBaseConfiguration is \
         still the stored one, got {:?}",
        fetched.member(&["knowledgeBase", "knowledgeBaseConfiguration"])
    );
}
