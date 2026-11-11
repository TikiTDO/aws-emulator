// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RetrieveAndGenerate` (`POST /retrieveAndGenerate`).
//!
//! Every test drives real HTTP requests through the framework router over ONE shared engine
//! carrying the four services the path spans — this one, the Bedrock Agent build-time plane
//! that owns knowledge bases and ingestion jobs, S3, which owns the objects a data source is
//! crawled from, and S3 Vectors, which owns the index a knowledge base is bound to — through
//! the harness the sibling retrieval suite already has (`crate::knowledge_base_retrieval`),
//! so the knowledge base a response is generated from is one a caller really created and the
//! passages it cites are the ones an ingestion really recorded.
//!
//! THE RULE UNDER TEST: every `citation` of a generated response indexes that response. The
//! `span` of a citation is a pair of CHARACTER offsets into `output.text` — `start` the first
//! character of the cited segment and `end` the offset just PAST its last — so
//! `output.text[span.start..span.end]` is exactly
//! `generatedResponsePart.textResponsePart.text` and `span.end - span.start` is that text's
//! length; the spans of one response are disjoint and ascending; and the segment is a passage
//! of the `retrievedReferences` reported beside it, whose `location` names the source that
//! passage was ingested from. The rule holds however the response was bounded: the rows below
//! span which query is asked, how many results it may cite, which filter narrows it, the form
//! of `modelArn`, the generation budget (`maxTokens`) and a configured stop sequence — each of
//! which changes how much of the response is cited, and none of which may leave a span that
//! does not index the text.

use serde_json::{json, Value};

use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::ingestion_index as index_fields;
use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state as agent_runtime_state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state as runtime_state;

use crate::knowledge_base_retrieval::{collapsed, Emulator};
use crate::sessions::{call, error_code, is_model_uuid, REGION};

/// A text model the Bedrock Runtime plane of this emulator publishes
/// (`runtime_state::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::models`,
/// seeded from that plane's `model_catalog`).
const MODEL: &str = "anthropic.claude-3-5-sonnet-20240620-v1:0";

/// A second published text model, so a row can name one the first row did not.
const SECOND_MODEL: &str = "amazon.nova-lite-v1:0";

/// A model the same catalogue publishes that generates NO text: an embedding model, which
/// generates no response for a knowledge base query.
const EMBEDDING_MODEL: &str = "amazon.titan-embed-text-v2:0";

/// The bucket both data sources of the suite's knowledge base are crawled from.
const BUCKET: &str = "generation-sources";

/// The documents the `line/` data source holds: two on one topic, so a query retrieves more
/// than one passage and the response quotes them in relevance order.
const PUMP: &str = "Hydraulic pump P-12 in bay 2 is serviced every 90 days by the \
     maintenance crew.\n\n   Its filter is replaced at the same interval.";

/// See [`PUMP`].
const COOLANT: &str = "Coolant for the hydraulic system is stored in drum 7 and must stay \
     above 4 degrees.";

/// The document the `mess/` data source holds, on an unrelated topic, so a filter on the
/// data source a chunk came from narrows the response to a known set of passages.
const MENU: &str = "Canteen service: barley stew on Monday and flatbread on Friday.";

/// The suite's knowledge base, its two data sources and the passages each one ingested.
struct Knowledge {
    emulator: Emulator,
    knowledge_base_id: String,
    /// The `line/` data source, whose chunks a filter on the data source admits.
    line: String,
    /// The passage each ingested object contributes, keyed by its S3 URI: a chunk's text is
    /// the whitespace-collapsed text of the object it was read from.
    passages: Vec<(String, String)>,
}

impl Knowledge {
    /// One emulator holding a knowledge base over two ingested S3 data sources.
    async fn new(index: &str) -> Self {
        let emulator = Emulator::new(index);
        emulator.put_object(BUCKET, "line/pump.txt", PUMP);
        emulator.put_object(BUCKET, "line/coolant.txt", COOLANT);
        emulator.put_object(BUCKET, "mess/menu.txt", MENU);
        let knowledge_base_id = emulator.knowledge_base("plant", index).await;
        let line = emulator
            .data_source(&knowledge_base_id, "line", BUCKET, Some(vec!["line/"]))
            .await;
        let mess = emulator
            .data_source(&knowledge_base_id, "mess", BUCKET, Some(vec!["mess/"]))
            .await;
        emulator.ingest(&knowledge_base_id, &line).await;
        emulator.ingest(&knowledge_base_id, &mess).await;
        Self {
            emulator,
            knowledge_base_id,
            line,
            passages: vec![
                (format!("s3://{BUCKET}/line/pump.txt"), collapsed(PUMP)),
                (format!("s3://{BUCKET}/line/coolant.txt"), collapsed(COOLANT)),
                (format!("s3://{BUCKET}/mess/menu.txt"), collapsed(MENU)),
            ],
        }
    }

    /// `POST /retrieveAndGenerate` with `body`, answering the raw `(status, body)`.
    async fn generate(&self, body: Value) -> (u16, Value) {
        call(&self.emulator.router, "POST", "/retrieveAndGenerate", body).await
    }

    /// The request body for one query against this knowledge base, with the knowledge-base
    /// configuration `configuration` adds to.
    fn request(&self, query: &str, configuration: Value) -> Value {
        let mut knowledge_base = json!({
            "knowledgeBaseId": self.knowledge_base_id,
            "modelArn": MODEL,
        });
        for (member, value) in configuration.as_object().expect("a JSON object") {
            knowledge_base[member] = value.clone();
        }
        json!({
            "input": { "text": query },
            "retrieveAndGenerateConfiguration": {
                "type": "KNOWLEDGE_BASE",
                "knowledgeBaseConfiguration": knowledge_base,
            },
        })
    }

    /// Generate, asserting the call succeeded, and answer the served response.
    async fn served(&self, case: &str, body: Value) -> Served {
        let (status, answer) = self.generate(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RetrieveAndGenerate to answer 200, got {status} {answer:?}"
        );
        assert!(
            answer.get("guardrailAction").is_none(),
            "case {case:?}: no guardrail is evaluated, so no guardrailAction may be \
             reported, got {answer:?}"
        );
        let session_id = answer["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {answer:?}"))
            .to_string();
        let text = answer["output"]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no output.text in {answer:?}"))
            .to_string();
        let citations = answer["citations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no citations in {answer:?}"))
            .clone();
        let cited = self.cited_segments(case, &text, &citations);
        Served {
            session_id,
            text,
            cited,
        }
    }

    /// THE RULE: every citation's span indexes the response text, the spans are disjoint and
    /// ascending, and each cited segment is a passage of the reference reported beside it.
    fn cited_segments(&self, case: &str, text: &str, citations: &[Value]) -> Vec<Cited> {
        let characters: Vec<char> = text.chars().collect();
        let mut cited: Vec<Cited> = Vec::new();
        for citation in citations {
            let part = &citation["generatedResponsePart"]["textResponsePart"];
            let segment = part["text"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: no generatedResponsePart.textResponsePart.text in {citation:?}")
            });
            let span = &part["span"];
            let start = span["start"].as_u64().unwrap_or_else(|| {
                panic!("case {case:?}: no textResponsePart.span.start in {citation:?}")
            }) as usize;
            let end = span["end"].as_u64().unwrap_or_else(|| {
                panic!("case {case:?}: no textResponsePart.span.end in {citation:?}")
            }) as usize;
            assert!(
                start < end && end <= characters.len(),
                "case {case:?}: expected a span inside the {} characters of the response, \
                 got start {start} end {end}",
                characters.len()
            );
            let indexed: String = characters[start..end].iter().collect();
            assert_eq!(
                indexed, segment,
                "case {case:?}: expected the span to index the cited segment, got {indexed:?} \
                 at start {start} end {end}"
            );
            assert_eq!(
                end - start,
                segment.chars().count(),
                "case {case:?}: expected the span to match the cited text with no slack, got \
                 start {start} end {end} for {segment:?}"
            );
            if let Some(previous) = cited.last() {
                assert!(
                    previous.end <= start,
                    "case {case:?}: expected the spans to be disjoint and ascending, got \
                     start {start} after end {}",
                    previous.end
                );
            }
            let references = citation["retrievedReferences"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no retrievedReferences in {citation:?}"));
            assert_eq!(
                references.len(),
                1,
                "case {case:?}: expected the one reference the segment was quoted from, got \
                 {references:?}"
            );
            let reference = &references[0];
            assert_eq!(
                reference["content"]["type"].as_str(),
                Some("TEXT"),
                "case {case:?}: expected the cited content to be reported as text, got \
                 {reference:?}"
            );
            let chunk = reference["content"]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no content.text in {reference:?}"));
            assert!(
                chunk.starts_with(segment),
                "case {case:?}: expected the cited segment to open the passage it was quoted \
                 from, got {segment:?} of {chunk:?}"
            );
            assert_eq!(
                reference["location"]["type"].as_str(),
                Some("S3"),
                "case {case:?}: expected an S3 location, got {reference:?}"
            );
            let uri = reference["location"]["s3Location"]["uri"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no location.s3Location.uri in {reference:?}"));
            let ingested = self
                .passages
                .iter()
                .find(|(ingested, _)| ingested == uri)
                .unwrap_or_else(|| panic!("case {case:?}: {uri:?} is no object this suite ingested"));
            assert_eq!(
                ingested.1, chunk,
                "case {case:?}: expected the passage the ingestion recorded for {uri:?}, got \
                 {chunk:?}"
            );
            let metadata = reference["metadata"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: no metadata in {reference:?}"));
            assert!(
                metadata.contains_key(index_fields::DATA_SOURCE_ID_METADATA_KEY),
                "case {case:?}: expected the chunk's own metadata beside it, got {metadata:?}"
            );
            cited.push(Cited {
                start,
                end,
                segment: segment.to_string(),
                uri: uri.to_string(),
            });
        }
        cited
    }
}

/// One served response, with its citations already checked against the rule.
struct Served {
    session_id: String,
    text: String,
    cited: Vec<Cited>,
}

/// One cited segment of a served response.
struct Cited {
    start: usize,
    end: usize,
    segment: String,
    uri: String,
}

/// The characters of `text` up to, but not including, the offset `end`.
fn through(text: &str, end: usize) -> String {
    text.chars().take(end).collect()
}

/// RULE: every citation of a served response indexes that response, whichever query is
/// asked, however many passages it may cite, whichever filter narrows it and however
/// `modelArn` is spelled.
#[tokio::test]
async fn every_citation_span_indexes_the_generated_response() {
    let knowledge = Knowledge::new("generation-spans").await;
    let line_key = index_fields::DATA_SOURCE_ID_METADATA_KEY;
    // (case, the query, the knowledge-base configuration it is asked under, the passages the
    // response may cite)
    let rows: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "the whole knowledge base, under the model's own default page of results",
            "hydraulic pump service interval",
            json!({}),
            vec![
                format!("s3://{BUCKET}/line/pump.txt"),
                format!("s3://{BUCKET}/line/coolant.txt"),
                format!("s3://{BUCKET}/mess/menu.txt"),
            ],
        ),
        (
            "a query on the other topic",
            "barley stew on Monday",
            json!({}),
            vec![
                format!("s3://{BUCKET}/line/pump.txt"),
                format!("s3://{BUCKET}/line/coolant.txt"),
                format!("s3://{BUCKET}/mess/menu.txt"),
            ],
        ),
        (
            "numberOfResults 1, so one passage is quoted and one citation reported",
            "coolant drum",
            json!({ "retrievalConfiguration": { "vectorSearchConfiguration": { "numberOfResults": 1 } } }),
            vec![
                format!("s3://{BUCKET}/line/pump.txt"),
                format!("s3://{BUCKET}/line/coolant.txt"),
                format!("s3://{BUCKET}/mess/menu.txt"),
            ],
        ),
        (
            "a filter on the data source a chunk came from",
            "hydraulic",
            json!({
                "retrievalConfiguration": {
                    "vectorSearchConfiguration": {
                        "numberOfResults": 100,
                        "filter": { "equals": { "key": line_key, "value": knowledge.line } },
                    },
                },
            }),
            vec![
                format!("s3://{BUCKET}/line/pump.txt"),
                format!("s3://{BUCKET}/line/coolant.txt"),
            ],
        ),
        (
            "the model named by its published ARN rather than its bare id",
            "hydraulic pump",
            json!({ "modelArn": format!("arn:aws:bedrock:{REGION}::foundation-model/{MODEL}") }),
            vec![
                format!("s3://{BUCKET}/line/pump.txt"),
                format!("s3://{BUCKET}/line/coolant.txt"),
                format!("s3://{BUCKET}/mess/menu.txt"),
            ],
        ),
        (
            "a second published model",
            "flatbread",
            json!({ "modelArn": SECOND_MODEL }),
            vec![
                format!("s3://{BUCKET}/line/pump.txt"),
                format!("s3://{BUCKET}/line/coolant.txt"),
                format!("s3://{BUCKET}/mess/menu.txt"),
            ],
        ),
    ];

    for (case, query, configuration, citable) in rows {
        let requested = configuration["retrievalConfiguration"]["vectorSearchConfiguration"]
            ["numberOfResults"]
            .as_u64()
            .map_or(citable.len(), |results| {
                (results as usize).min(citable.len())
            });
        let served = knowledge.served(case, knowledge.request(query, configuration)).await;
        assert_eq!(
            served.cited.len(),
            requested,
            "case {case:?}: expected one citation per passage the retrieval answered, got \
             {:?} in {:?}",
            served.cited.len(),
            served.text
        );
        for cited in &served.cited {
            assert!(
                citable.contains(&cited.uri),
                "case {case:?}: expected a passage of {citable:?}, got {:?} from {:?}",
                cited.segment,
                cited.uri
            );
        }
        // No model generated the response, and it says so before it quotes anything: the
        // first cited segment therefore never opens the text.
        if let Some(first) = served.cited.first() {
            assert!(
                first.start > 0,
                "case {case:?}: expected the response to state that no model generated it \
                 before the first cited passage, got {:?}",
                served.text
            );
        }
        assert!(
            is_model_uuid(&served.session_id),
            "case {case:?}: expected a session this service minted, got {:?}",
            served.session_id
        );
    }
}

/// RULE: a query the knowledge base retrieves nothing for cites nothing — the response
/// carries no citation rather than a span over text no source supports.
#[tokio::test]
async fn a_query_that_retrieves_nothing_cites_nothing() {
    let knowledge = Knowledge::new("generation-empty").await;
    let served = knowledge
        .served(
            "a filter no chunk of this knowledge base satisfies",
            knowledge.request(
                "hydraulic pump",
                json!({
                    "retrievalConfiguration": {
                        "vectorSearchConfiguration": {
                            "filter": { "equals": { "key": "department", "value": "operations" } },
                        },
                    },
                }),
            ),
        )
        .await;
    assert!(
        served.cited.is_empty(),
        "expected no citation for a retrieval that answered nothing, got {:?}",
        served.cited.len()
    );
    assert!(
        !served.text.is_empty(),
        "expected the response to say that nothing was retrieved, got {:?}",
        served.text
    );
}

/// RULE: the generation budget and the stop sequences bound the response AND its citations
/// together — a passage that does not fit the budget is not quoted and not cited, and a
/// response a stop sequence halts cites only the text it still carries — so a span still
/// indexes the response however it was cut.
#[tokio::test]
async fn the_budget_and_a_stop_sequence_bound_the_response_and_its_citations_together() {
    let knowledge = Knowledge::new("generation-budget").await;
    let query = "hydraulic pump service interval";
    let whole = knowledge
        .served("no budget and no stop sequence", knowledge.request(query, json!({})))
        .await;
    assert!(
        whole.cited.len() >= 2,
        "setup: expected the unbounded response to quote more than one passage, got {:?}",
        whole.cited.len()
    );

    // The budget that fits the response exactly up to the end of the FIRST cited passage,
    // counted by the inference plane's own tokenizer — the one the generation budget is
    // measured in — rather than by a count written out here.
    let through_first = through(&whole.text, whole.cited[0].end);
    let budget = runtime_state::count_text_tokens(&through_first);
    let bounded = knowledge
        .served(
            "a maxTokens that fits one passage",
            knowledge.request(
                query,
                json!({
                    "generationConfiguration": {
                        "inferenceConfig": { "textInferenceConfig": { "maxTokens": budget } },
                    },
                }),
            ),
        )
        .await;
    assert_eq!(
        bounded.cited.len(),
        1,
        "expected the passages past the budget to be left out of the response and its \
         citations, got {:?} in {:?}",
        bounded.cited.len(),
        bounded.text
    );
    assert_eq!(
        bounded.text, through_first,
        "expected the response to be composed to fit the budget, got {:?}",
        bounded.text
    );

    // A stop sequence that completes INSIDE the first quoted passage: the response ends with
    // it, and the citation is clipped to the part of the passage the response carries.
    let halt: String = whole.cited[0].segment.chars().take(8).collect();
    let halted = knowledge
        .served(
            "a stop sequence inside the first quoted passage",
            knowledge.request(
                query,
                json!({
                    "generationConfiguration": {
                        "inferenceConfig": {
                            "textInferenceConfig": { "stopSequences": [halt.clone()] },
                        },
                    },
                }),
            ),
        )
        .await;
    assert!(
        halted.text.ends_with(&halt),
        "expected generation to end with the sequence that halted it, got {:?}",
        halted.text
    );
    assert_eq!(
        halted.cited.len(),
        1,
        "expected only the halted passage to be cited, got {:?} in {:?}",
        halted.cited.len(),
        halted.text
    );
    assert_eq!(
        halted.cited[0].segment, halt,
        "expected the citation to be clipped to the text the response still carries, got {:?}",
        halted.cited[0].segment
    );

    // A budget below the notice that opens every response cuts generation before anything is
    // quoted, so there is nothing to cite.
    let starved = knowledge
        .served(
            "a maxTokens below the response's own opening",
            knowledge.request(
                query,
                json!({
                    "generationConfiguration": {
                        "inferenceConfig": { "textInferenceConfig": { "maxTokens": 1 } },
                    },
                }),
            ),
        )
        .await;
    assert!(
        starved.cited.is_empty(),
        "expected nothing to be cited when the budget quotes no passage, got {:?}",
        starved.cited.len()
    );
    assert!(
        runtime_state::count_text_tokens(&starved.text) <= 1,
        "expected the response to be cut to the budget, got {:?}",
        starved.text
    );
}

/// RULE: the `sessionId` is this service's own — a request carrying none OPENS a session and
/// is answered its id, a request carrying that id CONTINUES it and records its turn beside
/// the first, and an id this service never minted is refused.
#[tokio::test]
async fn a_session_is_opened_by_the_service_and_continued_by_the_id_it_handed_back() {
    let knowledge = Knowledge::new("generation-sessions").await;
    let first = knowledge
        .served("the request that opens the session", knowledge.request("hydraulic pump", json!({})))
        .await;
    let mut opened = knowledge.request("coolant drum", json!({}));
    opened["sessionId"] = json!(first.session_id);
    let second = knowledge.served("the request that continues it", opened).await;
    assert_eq!(
        second.session_id, first.session_id,
        "expected the session the request named to be continued, got {:?}",
        second.session_id
    );

    // A fresh request opens a session of its own rather than joining the first.
    let other = knowledge
        .served("a request naming no session", knowledge.request("barley stew", json!({})))
        .await;
    assert_ne!(
        other.session_id, first.session_id,
        "expected a request naming no session to open one of its own, got {:?}",
        other.session_id
    );

    // The turns are read back out of the store the handler wrote them to: the session holds
    // the conversation, in the order it was served.
    let state = agent_runtime_state::service_state(&*knowledge.emulator.engine)
        .expect("this service's store is on the engine");
    let guard = state.lock();
    let session = guard
        .knowledge_base_sessions
        .get(&first.session_id)
        .unwrap_or_else(|| panic!("the opened session {:?} is in the store", first.session_id));
    assert_eq!(
        session.turns.len(),
        2,
        "expected both turns to be recorded on the session, got {:?}",
        session.turns.len()
    );
    assert_eq!(
        (
            session.turns[0].query.as_str(),
            session.turns[0].text.as_str(),
            session.turns[0].citations,
        ),
        ("hydraulic pump", first.text.as_str(), first.cited.len()),
        "expected the first turn as it was served, got {:?}",
        session.turns[0]
    );
    assert_eq!(
        (
            session.turns[1].query.as_str(),
            session.turns[1].text.as_str(),
        ),
        ("coolant drum", second.text.as_str()),
        "expected the second turn as it was served, got {:?}",
        session.turns[1]
    );
    assert_eq!(
        session.turns[1].model_id, MODEL,
        "expected the model the response was generated with, got {:?}",
        session.turns[1].model_id
    );
    assert!(
        session.last_used_at.secs() >= session.created_at.secs(),
        "expected the session to record when it was last used, got {:?}",
        session.last_used_at
    );
}

/// RULE: a request naming a leg this service cannot compute, or a resource it does not hold,
/// is refused with the modelled error for that class — never with a success that ignored the
/// member and never with the 501 of an unimplemented operation.
#[tokio::test]
async fn a_leg_this_service_cannot_compute_is_refused() {
    let knowledge = Knowledge::new("generation-refusals").await;
    let session = knowledge
        .served("a session to name", knowledge.request("hydraulic pump", json!({})))
        .await
        .session_id;
    // (case, the request body, the status and error code expected, a fragment of the message)
    let mut rows: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "a blank query",
            knowledge.request("   ", json!({})),
            400,
            "ValidationException",
            "input.text".to_string(),
        ),
        (
            "a userContext, which asks for per-document access control",
            {
                let mut body = knowledge.request("hydraulic pump", json!({}));
                body["userContext"] = json!({ "userId": "alice" });
                body
            },
            400,
            "ValidationException",
            "userContext".to_string(),
        ),
        (
            "a promptTemplate, which is what a model would be asked",
            knowledge.request(
                "hydraulic pump",
                json!({
                    "generationConfiguration": {
                        "promptTemplate": { "textPromptTemplate": "Answer from $search_results$" },
                    },
                }),
            ),
            400,
            "ValidationException",
            "promptTemplate".to_string(),
        ),
        (
            "a guardrailConfiguration, which asks for a guardrail policy",
            knowledge.request(
                "hydraulic pump",
                json!({
                    "generationConfiguration": {
                        "guardrailConfiguration": {
                            "guardrailId": "abc123",
                            "guardrailVersion": "DRAFT",
                        },
                    },
                }),
            ),
            400,
            "ValidationException",
            "guardrailConfiguration".to_string(),
        ),
        (
            "an orchestrationConfiguration, which asks a model to process the prompt",
            knowledge.request(
                "hydraulic pump",
                json!({
                    "orchestrationConfiguration": {
                        "queryTransformationConfiguration": { "type": "QUERY_DECOMPOSITION" },
                    },
                }),
            ),
            400,
            "ValidationException",
            "orchestrationConfiguration".to_string(),
        ),
        (
            "a rerankingConfiguration, which asks a model to reorder the results",
            knowledge.request(
                "hydraulic pump",
                json!({
                    "retrievalConfiguration": {
                        "vectorSearchConfiguration": {
                            "rerankingConfiguration": {
                                "type": "BEDROCK_RERANKING_MODEL",
                                "bedrockRerankingConfiguration": {
                                    "modelConfiguration": {
                                        "modelArn": format!(
                                            "arn:aws:bedrock:{REGION}::foundation-model/amazon.rerank-v1:0"
                                        ),
                                    },
                                },
                            },
                        },
                    },
                }),
            ),
            400,
            "ValidationException",
            "rerankingConfiguration".to_string(),
        ),
        (
            "an implicitFilterConfiguration, which asks for a model-generated filter",
            knowledge.request(
                "hydraulic pump",
                json!({
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
            ),
            400,
            "ValidationException",
            "implicitFilterConfiguration".to_string(),
        ),
        (
            "a model that generates no text",
            knowledge.request("hydraulic pump", json!({ "modelArn": EMBEDDING_MODEL })),
            400,
            "ValidationException",
            "modelArn".to_string(),
        ),
        (
            "an identifier naming no model this emulator publishes",
            knowledge.request("hydraulic pump", json!({ "modelArn": "not-a-model" })),
            400,
            "ValidationException",
            runtime_state::INVALID_MODEL_IDENTIFIER_MESSAGE.to_string(),
        ),
        (
            "an EXTERNAL_SOURCES configuration, whose sources are not ingested here",
            json!({
                "input": { "text": "hydraulic pump" },
                "retrieveAndGenerateConfiguration": {
                    "type": "EXTERNAL_SOURCES",
                    "externalSourcesConfiguration": {
                        "modelArn": MODEL,
                        "sources": [{
                            "sourceType": "S3",
                            "s3Location": { "uri": format!("s3://{BUCKET}/line/pump.txt") },
                        }],
                    },
                },
            }),
            400,
            "ValidationException",
            "type".to_string(),
        ),
        (
            "no retrieveAndGenerateConfiguration at all",
            json!({ "input": { "text": "hydraulic pump" } }),
            400,
            "ValidationException",
            "retrieveAndGenerateConfiguration".to_string(),
        ),
        (
            "a sessionId this service never minted",
            {
                let mut body = knowledge.request("hydraulic pump", json!({}));
                body["sessionId"] = json!("00000000-0000-4000-8000-000000000000");
                body
            },
            400,
            "ValidationException",
            "is not valid".to_string(),
        ),
        (
            "a knowledge base the control plane does not hold",
            {
                let mut body = knowledge.request("hydraulic pump", json!({}));
                body["retrieveAndGenerateConfiguration"]["knowledgeBaseConfiguration"]
                    ["knowledgeBaseId"] = json!("0000000000");
                body
            },
            404,
            "ResourceNotFoundException",
            "0000000000".to_string(),
        ),
    ];
    // A session this service DID mint, re-keyed by a later turn: the key encrypting a
    // session is settled when the session is opened.
    rows.push((
        "a sessionConfiguration other than the one the session was opened under",
        {
            let mut body = knowledge.request("hydraulic pump", json!({}));
            body["sessionId"] = json!(session);
            body["sessionConfiguration"] = json!({
                "kmsKeyArn": format!(
                    "arn:aws:kms:{REGION}:000000000000:key/11111111-2222-3333-4444-555555555555"
                ),
            });
            body
        },
        400,
        "ValidationException",
        "sessionConfiguration".to_string(),
    ));

    for (case, body, status, code, fragment) in rows {
        let (answered, answer) = knowledge.generate(body).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected {status}, got {answered} {answer:?}"
        );
        assert!(
            error_code(&answer).ends_with(code),
            "case {case:?}: expected {code}, got {answer:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&fragment),
            "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
        );
    }
}
