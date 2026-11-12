// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two STREAMING retrieval routes of this plane —
//! `AgenticRetrieveStream` (`POST /agenticRetrieveStream`) and `RetrieveAndGenerateStream`
//! (`POST /retrieveAndGenerateStream`).
//!
//! THE RULES UNDER TEST:
//!
//! * a member outside a modelled ENUM value set is refused with the modelled
//!   `ValidationException` (HTTP 400) stating the constraint clause, before any store is
//!   read — whichever member and whichever enum it is;
//! * a knowledge base the Bedrock Agent build-time plane does not hold is the modelled
//!   `ResourceNotFoundException` (HTTP 404) with this plane's one sentence for that class,
//!   whichever operation of this plane resolved it;
//! * a retrieval the plane CAN serve answers an `application/vnd.amazon.eventstream` body
//!   carrying the members the operation's `@streaming` union declares, computed from the
//!   chunks the knowledge base really holds: every reported item is a chunk an ingestion
//!   recorded, and every citation's span indexes the answer it is reported beside;
//! * a leg of either request this emulator cannot compute is refused rather than ignored.
//!
//! The tables span what those rules generalise over: which enum member is wrong, the FORM
//! and the length of a knowledge-base identifier, how many retrievers a request names, how
//! many results it allows, whether it asks for an answer at all, and which unservable leg it
//! carries.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_and_flow_invocations::{decode_frames, frame_names, Frame, EVENT_STREAM};
use crate::knowledge_base_retrieval::{collapsed, Emulator};
use crate::sessions::REGION;

/// The S3 bucket the suite's sources live in.
const BUCKET: &str = "streaming-retrieval-sources";

/// The three objects the suite ingests, as `(key, text)`.
const SOURCES: [(&str, &str); 3] = [
    (
        "line/pump.txt",
        "Hydraulic pump P-12 in bay 2 is serviced every 90 days by the line crew.",
    ),
    (
        "line/coolant.txt",
        "Coolant for the hydraulic system is stored in drum 7 and must stay below 30 degrees.",
    ),
    (
        "mess/menu.txt",
        "Canteen service: barley stew on Monday and flatbread on Friday.",
    ),
];

/// A model the Bedrock Runtime data plane of this emulator publishes, which
/// `RetrieveAndGenerateStream` generates its response against.
const MODEL: &str = "anthropic.claude-3-5-sonnet-20240620-v1:0";

/// The `RetrieveAndGenerateType` this service serves.
const KNOWLEDGE_BASE_TYPE: &str = "KNOWLEDGE_BASE";

/// The media type an agentic retrieval reports a text chunk under.
const TEXT_MEDIA_TYPE: &str = "text/plain";

/// One HTTP answer, with the headers and the raw body a streaming response needs.
struct StreamReply {
    status: u16,
    content_type: String,
    /// The modelled response headers, as the client reads them.
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl StreamReply {
    /// The body as JSON, or `Value::Null` for an empty one.
    fn json(&self, case: &str) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({error})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code restJson1 carries in `__type`, or `""` on a success body.
    fn error_code(&self, case: &str) -> String {
        self.json(case)
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The error body's `message`.
    fn message(&self, case: &str) -> String {
        self.json(case)
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The value of one response header, or `None` when it carries none.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The frames of an event-stream body, asserting the response IS one.
    fn frames(&self, case: &str) -> Vec<Frame> {
        assert_eq!(
            self.content_type, EVENT_STREAM,
            "case {case:?}: expected an event-stream body, got {:?} {:?}",
            self.content_type,
            String::from_utf8_lossy(&self.body)
        );
        decode_frames(case, &self.body)
    }
}

/// One POST whose response is read with its headers and its raw body.
async fn stream_post(router: &MultiServiceRouter, path: &str, body: Value) -> StreamReply {
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{REGION}/bedrock/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the test body serializes"),
        ))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let content_type = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    StreamReply {
        status,
        content_type,
        headers,
        body: response.into_body(),
    }
}

/// The suite's knowledge base over one ingested S3 data source, and the passages it holds.
struct Knowledge {
    emulator: Emulator,
    knowledge_base_id: String,
    /// Every passage the knowledge base holds: a chunk's text is the whitespace-collapsed
    /// text of the object it was read from.
    passages: Vec<String>,
}

impl Knowledge {
    /// One emulator holding a knowledge base over the three ingested objects.
    async fn new(index: &str) -> Self {
        let emulator = Emulator::new(index);
        for (key, text) in SOURCES {
            emulator.put_object(BUCKET, key, text);
        }
        let knowledge_base_id = emulator.knowledge_base("plant", index).await;
        let data_source = emulator
            .data_source(&knowledge_base_id, "plant", BUCKET, None)
            .await;
        emulator.ingest(&knowledge_base_id, &data_source).await;
        Self {
            emulator,
            knowledge_base_id,
            passages: SOURCES
                .iter()
                .map(|(_, text)| collapsed(text))
                .collect(),
        }
    }

    /// One `AgenticRetrieveStream` call.
    async fn agentic(&self, body: Value) -> StreamReply {
        stream_post(&self.emulator.router, "/agenticRetrieveStream", body).await
    }

    /// One `RetrieveAndGenerateStream` call.
    async fn generate_stream(&self, body: Value) -> StreamReply {
        stream_post(&self.emulator.router, "/retrieveAndGenerateStream", body).await
    }
}

/// The request body of one agentic retrieval over `retrievers`, with `extra` merged in.
fn agentic_request(query: &str, retrievers: Value, extra: Value) -> Value {
    let mut body = json!({
        "messages": [{ "role": "user", "content": { "text": query } }],
        "retrievers": retrievers,
        "agenticRetrieveConfiguration": {},
    });
    for (member, value) in extra.as_object().expect("a JSON object") {
        body[member] = value.clone();
    }
    body
}

/// One retriever naming `knowledge_base_id`, with `overrides` when the row asks for them.
fn retriever(knowledge_base_id: &str, overrides: Option<Value>) -> Value {
    let mut knowledge_base = json!({ "knowledgeBaseId": knowledge_base_id });
    if let Some(overrides) = overrides {
        knowledge_base["retrievalOverrides"] = overrides;
    }
    json!({ "configuration": { "knowledgeBase": knowledge_base } })
}

/// The request body of one streaming retrieve-and-generate over `knowledge_base_id`.
fn generate_request(query: &str, knowledge_base_id: &str, extra: Value) -> Value {
    let mut body = json!({
        "input": { "text": query },
        "retrieveAndGenerateConfiguration": {
            "type": KNOWLEDGE_BASE_TYPE,
            "knowledgeBaseConfiguration": {
                "knowledgeBaseId": knowledge_base_id,
                "modelArn": MODEL,
            },
        },
    });
    for (member, value) in extra.as_object().expect("a JSON object") {
        body[member] = value.clone();
    }
    body
}

/// The sentence this plane states when the build-time plane holds no knowledge base of the
/// identifier a request named. The identifier is the one variable part.
fn knowledge_base_not_found(identifier: &str) -> String {
    format!("Knowledge Base with id {identifier} does not exist")
}

/// The single-violation envelope over one enum clause, in the framework's own shape.
fn one_enum_error(value: &str, member: &str, values: &[&str]) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        values.join(", ")
    )
}

/// RULE: a member outside a modelled enum value set is refused with the modelled
/// `ValidationException` (400), stating the value, the member path and the enum's values in
/// the order the model declares them.
#[tokio::test]
async fn an_enum_member_outside_its_value_set_is_refused_with_the_constraint_clause() {
    // (case, body, the whole message the refusal must state). Each value set is the
    // model's own: `smithy_query(query="shape",shape="FoundationModelType")` -> CUSTOM,
    // MANAGED; `shape="AgenticRetrieveRerankingModelType"` -> CUSTOM, MANAGED, NONE;
    // `shape="ConversationRole"` -> user, assistant.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a foundationModelType outside its value set",
            agentic_request(
                "launch code",
                json!([retriever("KB12345678", None)]),
                json!({ "agenticRetrieveConfiguration": { "foundationModelType": "BOGUS_TYPE" } }),
            ),
            one_enum_error(
                "BOGUS_TYPE",
                "agenticRetrieveConfiguration.foundationModelType",
                &["CUSTOM", "MANAGED"],
            ),
        ),
        (
            "a rerankingModelType outside its value set",
            agentic_request(
                "launch code",
                json!([retriever("KB12345678", None)]),
                json!({ "agenticRetrieveConfiguration": { "rerankingModelType": "SOMETIMES" } }),
            ),
            one_enum_error(
                "SOMETIMES",
                "agenticRetrieveConfiguration.rerankingModelType",
                &["CUSTOM", "MANAGED", "NONE"],
            ),
        ),
        (
            "a conversation role outside its value set",
            json!({
                "messages": [{ "role": "robot", "content": { "text": "launch code" } }],
                "retrievers": [retriever("KB12345678", None)],
                "agenticRetrieveConfiguration": {},
            }),
            one_enum_error("robot", "messages.1.member.role", &["user", "assistant"]),
        ),
    ];

    let knowledge = Knowledge::new("enum-refusals").await;
    for (case, body, message) in rows {
        let reply = knowledge.agentic(body).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.json(case)
        );
        assert_eq!(
            reply.message(case),
            message,
            "case {case:?}: expected the constraint clause, got {:?}",
            reply.message(case)
        );
        assert_ne!(
            reply.content_type, EVENT_STREAM,
            "case {case:?}: a refused request must not answer an event stream"
        );
    }
}

/// RULE: a knowledge base the build-time plane does not hold is the modelled
/// `ResourceNotFoundException` (404) with this plane's one sentence for that class, on both
/// streaming routes.
#[tokio::test]
async fn a_knowledge_base_the_plane_does_not_hold_is_refused_as_not_found() {
    let knowledge = Knowledge::new("not-found").await;
    let held = knowledge.knowledge_base_id.clone();

    // (case, the identifier the refusal must name, the agentic request, the generate
    // request) — the second is `None` where the shape of the row has no counterpart there.
    let rows: Vec<(&str, &str, Value, Option<Value>)> = vec![
        (
            "a ten-character id the plane never minted",
            "KB12345678",
            agentic_request(
                "launch code",
                json!([retriever("KB12345678", None)]),
                json!({}),
            ),
            Some(generate_request("launch code", "KB12345678", json!({}))),
        ),
        (
            "a one-character id, the shortest the model admits",
            "K",
            agentic_request("launch code", json!([retriever("K", None)]), json!({})),
            Some(generate_request("launch code", "K", json!({}))),
        ),
        (
            "two retrievers, the second naming no knowledge base",
            "KBNOTFOUND",
            agentic_request(
                "launch code",
                json!([retriever(&held, None), retriever("KBNOTFOUND", None)]),
                json!({}),
            ),
            None,
        ),
    ];

    for (case, identifier, agentic, generate) in rows {
        for (route, body) in [Some(("AgenticRetrieveStream", agentic)), generate.map(|body| ("RetrieveAndGenerateStream", body))]
            .into_iter()
            .flatten()
        {
            let case = format!("{case} on {route}");
            let reply = if route == "AgenticRetrieveStream" {
                knowledge.agentic(body).await
            } else {
                knowledge.generate_stream(body).await
            };
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected 404, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(&case),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled not-found error, got {:?}",
                reply.json(&case)
            );
            assert_eq!(
                reply.message(&case),
                knowledge_base_not_found(identifier),
                "case {case:?}: expected this plane's sentence for the class, got {:?}",
                reply.message(&case)
            );
        }
    }
}

/// RULE: an agentic retrieval over a knowledge base this emulator holds is streamed as the
/// modelled `result` event — whose items are chunks an ingestion really recorded — and, when
/// the request asks for an answer, the `responseEvent` carrying it, with every citation's
/// span indexing that answer.
#[tokio::test]
async fn an_agentic_retrieval_streams_the_chunks_the_knowledge_base_holds() {
    /// One request, and what the stream must carry for it.
    struct Row {
        case: &'static str,
        /// The `retrievalOverrides` of the one retriever, when the row sets any.
        overrides: Option<Value>,
        /// Whether the request asks for a generated answer.
        generate: Option<bool>,
        /// The most items the stream may report.
        most: usize,
    }

    let rows = vec![
        Row {
            case: "the model's own default page of results",
            overrides: None,
            generate: None,
            most: 3,
        },
        Row {
            case: "one result allowed",
            overrides: Some(json!({ "maxNumberOfResults": 1 })),
            generate: None,
            most: 1,
        },
        Row {
            case: "two results allowed",
            overrides: Some(json!({ "maxNumberOfResults": 2 })),
            generate: None,
            most: 2,
        },
        Row {
            case: "a filter on the data source, narrowing the chunks",
            overrides: Some(json!({ "maxNumberOfResults": 3 })),
            generate: Some(true),
            most: 3,
        },
        Row {
            case: "a retrieval that asks for no answer at all",
            overrides: None,
            generate: Some(false),
            most: 3,
        },
    ];

    let knowledge = Knowledge::new("agentic-served").await;
    for row in rows {
        let mut extra = json!({});
        if let Some(generate) = row.generate {
            extra["generateResponse"] = json!(generate);
        }
        let body = agentic_request(
            "hydraulic pump service interval",
            json!([retriever(&knowledge.knowledge_base_id, row.overrides.clone())]),
            extra,
        );
        let reply = knowledge.agentic(body).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let frames = reply.frames(row.case);
        let answered = row.generate != Some(false);
        let expected: Vec<&str> = if answered {
            vec!["result", "responseEvent"]
        } else {
            vec!["result"]
        };
        assert_eq!(
            frame_names(&frames),
            expected,
            "case {:?}: expected the modelled events for this request",
            row.case
        );

        let result = &frames[0].payload;
        let items = result["results"].as_array().unwrap_or_else(|| {
            panic!("case {:?}: the result event must carry results, got {result:?}", row.case)
        });
        assert!(
            !items.is_empty() && items.len() <= row.most,
            "case {:?}: expected between 1 and {} items, got {}",
            row.case,
            row.most,
            items.len()
        );
        let texts: Vec<String> = items
            .iter()
            .map(|item| {
                assert_eq!(
                    item["content"]["mimeType"].as_str(),
                    Some(TEXT_MEDIA_TYPE),
                    "case {:?}: a text chunk is reported under that media type, got {item:?}",
                    row.case
                );
                assert_eq!(
                    item["sourceRetriever"]["identifier"].as_str(),
                    Some(knowledge.knowledge_base_id.as_str()),
                    "case {:?}: every item names the retriever it came from, got {item:?}",
                    row.case
                );
                let text = item["content"]["text"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {:?}: every item carries its text, got {item:?}", row.case)
                    })
                    .to_string();
                assert!(
                    knowledge.passages.contains(&text),
                    "case {:?}: every item must be a chunk an ingestion recorded, got \
                     {text:?}",
                    row.case
                );
                text
            })
            .collect();
        assert!(
            result.get("nextToken").is_none(),
            "case {:?}: one page carries every item, so no nextToken is handed out, got \
             {result:?}",
            row.case
        );

        if !answered {
            assert!(
                result.get("generatedResponse").is_none(),
                "case {:?}: a retrieval asked for no answer reports none, got {result:?}",
                row.case
            );
            continue;
        }
        let generated = &result["generatedResponse"];
        let answer = generated["answer"].as_str().unwrap_or_else(|| {
            panic!("case {:?}: the result must carry the answer, got {result:?}", row.case)
        });
        for text in &texts {
            assert!(
                answer.contains(text.as_str()),
                "case {:?}: the answer must quote the passage {text:?}, got {answer:?}",
                row.case
            );
        }
        assert_eq!(
            frames[1].payload["text"].as_str(),
            Some(answer),
            "case {:?}: the response event must carry the same answer, got {:?}",
            row.case,
            frames[1].payload
        );
        let characters: Vec<char> = answer.chars().collect();
        for citation in generated["citations"].as_array().cloned().unwrap_or_default() {
            let start = citation["startIndex"].as_u64().unwrap_or_else(|| {
                panic!("case {:?}: a citation carries its start, got {citation:?}", row.case)
            }) as usize;
            let end = citation["endIndex"].as_u64().unwrap_or_else(|| {
                panic!("case {:?}: a citation carries its end, got {citation:?}", row.case)
            }) as usize;
            assert!(
                start < end && end <= characters.len(),
                "case {:?}: expected a span inside the {} characters of the answer, got \
                 start {start} end {end}",
                row.case,
                characters.len()
            );
            let segment: String = characters[start..end].iter().collect();
            let index = citation["references"][0]["resultIndex"]
                .as_u64()
                .unwrap_or_else(|| {
                    panic!("case {:?}: a citation names its item, got {citation:?}", row.case)
                }) as usize;
            assert_eq!(
                segment,
                texts[index],
                "case {:?}: the cited segment must be the item it points at, got \
                 {segment:?}",
                row.case
            );
        }
    }
}

/// RULE: a leg of an agentic retrieval this emulator cannot compute is refused with the
/// modelled `ValidationException` (400) naming the member, never served with the member
/// ignored.
#[tokio::test]
async fn an_agentic_retrieval_leg_this_service_cannot_compute_is_refused() {
    let knowledge = Knowledge::new("agentic-refusals").await;
    let held = knowledge.knowledge_base_id.clone();
    let retrievers = json!([retriever(&held, None)]);

    // (case, the members merged into the request, the member the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a user context, which would narrow the items by per-document ACLs",
            json!({ "userContext": { "userId": "u-1" } }),
            "userContext",
        ),
        (
            "a resume token no page of this service handed out",
            json!({ "nextToken": "cursor" }),
            "nextToken",
        ),
        (
            "a guardrail policy",
            json!({
                "policyConfiguration": {
                    "bedrockGuardrailConfiguration": {
                        "guardrailId": "abc123", "guardrailVersion": "DRAFT",
                    },
                },
            }),
            "policyConfiguration",
        ),
        (
            "an AgentCore memory to retrieve from",
            json!({ "memoryConfiguration": { "memoryId": "Memory-abcdefghij" } }),
            "memoryConfiguration",
        ),
        (
            "a retrieval instruction to apply",
            json!({ "retrievalInstructionConfiguration": { "retrievalInstructionId": "ri1" } }),
            "retrievalInstructionConfiguration",
        ),
        (
            "a managed reranking model",
            json!({ "agenticRetrieveConfiguration": { "rerankingModelType": "MANAGED" } }),
            "rerankingModelType",
        ),
        (
            "a custom reranking model configuration",
            json!({
                "agenticRetrieveConfiguration": {
                    "rerankingConfiguration": { "type": "BEDROCK_RERANKING_MODEL" },
                },
            }),
            "rerankingModelType",
        ),
        (
            "a Mantle foundation model, which this emulator does not publish",
            json!({
                "agenticRetrieveConfiguration": {
                    "foundationModelType": "CUSTOM",
                    "foundationModelConfiguration": { "type": "MANTLE_FOUNDATION_MODEL" },
                },
            }),
            "foundationModelConfiguration.type",
        ),
        (
            "a custom foundation model naming no model at all",
            json!({ "agenticRetrieveConfiguration": { "foundationModelType": "CUSTOM" } }),
            "foundationModelConfiguration",
        ),
    ];

    for (case, extra, member) in rows {
        let body = agentic_request("launch code", retrievers.clone(), extra);
        let reply = knowledge.agentic(body).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.json(case)
        );
        assert!(
            reply.message(case).contains(member),
            "case {case:?}: the refusal must name {member:?}, got {:?}",
            reply.message(case)
        );
    }
}

/// RULE: how much work ONE request may ask for is bounded before any of it runs: each
/// retriever is a separate pass over a vector index, and the modelled list carries no length
/// ceiling, so past this emulator's own guard the retrieval answers its modelled
/// `ServiceQuotaExceededException` instead of multiplying the work by a count the request
/// chose.
#[tokio::test]
async fn the_number_of_retrievers_one_retrieval_reads_is_bounded() {
    let knowledge = Knowledge::new("retriever-bound").await;
    let held = knowledge.knowledge_base_id.clone();
    let most = service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::MAX_RETRIEVERS_PER_RETRIEVAL;

    // (case, how many retrievers the request names, whether it is served)
    let rows: [(&str, usize, bool); 3] = [
        ("one retriever", 1, true),
        ("the most retrievers the guard admits", most, true),
        ("one retriever past the guard", most + 1, false),
    ];

    for (case, count, served) in rows {
        let retrievers: Vec<Value> = (0..count).map(|_| retriever(&held, None)).collect();
        let reply = knowledge
            .agentic(agentic_request(
                "hydraulic pump",
                json!(retrievers),
                json!({ "generateResponse": false }),
            ))
            .await;
        if served {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected the retrieval to be served, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            continue;
        }
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            "ServiceQuotaExceededException",
            "case {case:?}: expected the modelled quota error, got {:?}",
            reply.json(case)
        );
        assert!(
            reply.message(case).contains(&most.to_string()),
            "case {case:?}: the refusal must state the bound, got {:?}",
            reply.message(case)
        );
    }
}

/// RULE: a streaming retrieve-and-generate answers the modelled `output` event carrying the
/// generated text and one `citation` event per cited passage, with the session the turn was
/// recorded under in the modelled response header.
#[tokio::test]
async fn a_streaming_retrieve_and_generate_streams_the_output_and_its_citations() {
    /// One request, and what the stream must carry for it.
    struct Row {
        case: &'static str,
        /// The members merged into the request beside the query.
        extra: Value,
        /// The query, which decides how much is retrieved.
        query: &'static str,
    }

    let rows = vec![
        Row {
            case: "a query the knowledge base has passages for",
            extra: json!({}),
            query: "hydraulic pump service interval",
        },
        Row {
            case: "a query that retrieves nothing, so the answer cites nothing",
            extra: json!({}),
            query: "zqxjv unrelated token",
        },
        Row {
            case: "a query bounded to one result",
            extra: json!({}),
            query: "coolant drum",
        },
    ];

    let knowledge = Knowledge::new("generate-stream").await;
    for row in rows {
        let body = generate_request(row.query, &knowledge.knowledge_base_id, row.extra.clone());
        let reply = knowledge.generate_stream(body).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let session = reply
            .header("x-amzn-bedrock-knowledge-base-session-id")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the response must report the session, got {:?}",
                    row.case, reply.headers
                )
            })
            .to_string();
        assert!(
            !session.is_empty(),
            "case {:?}: the session the turn was recorded under must be reported",
            row.case
        );

        let frames = reply.frames(row.case);
        let names = frame_names(&frames);
        assert_eq!(
            names.first(),
            Some(&"output"),
            "case {:?}: the stream opens with the generated output, got {names:?}",
            row.case
        );
        assert!(
            names[1..].iter().all(|name| *name == "citation"),
            "case {:?}: every frame after the output is a citation, got {names:?}",
            row.case
        );
        let text = frames[0].payload["text"].as_str().unwrap_or_else(|| {
            panic!(
                "case {:?}: the output event carries the text, got {:?}",
                row.case, frames[0].payload
            )
        });
        let characters: Vec<char> = text.chars().collect();
        for frame in &frames[1..] {
            let part = &frame.payload["generatedResponsePart"]["textResponsePart"];
            let segment = part["text"].as_str().unwrap_or_else(|| {
                panic!(
                    "case {:?}: a citation carries the segment it cites, got {:?}",
                    row.case, frame.payload
                )
            });
            let start = part["span"]["start"].as_u64().unwrap_or_else(|| {
                panic!("case {:?}: a citation carries its span, got {:?}", row.case, frame.payload)
            }) as usize;
            let end = part["span"]["end"].as_u64().unwrap_or_else(|| {
                panic!("case {:?}: a citation carries its span, got {:?}", row.case, frame.payload)
            }) as usize;
            assert!(
                start < end && end <= characters.len(),
                "case {:?}: expected a span inside the {} characters of the response, got \
                 start {start} end {end}",
                row.case,
                characters.len()
            );
            let cited: String = characters[start..end].iter().collect();
            assert_eq!(
                cited, segment,
                "case {:?}: the span must index the segment reported beside it",
                row.case
            );
            assert!(
                knowledge.passages.contains(&segment.to_string()),
                "case {:?}: a cited segment must be a passage the knowledge base holds, got \
                 {segment:?}",
                row.case
            );
            // The deprecated `citation` member carries the same citation, so a client
            // reading either it or the pair beside it finds one answer.
            assert_eq!(
                frame.payload["citation"]["generatedResponsePart"]["textResponsePart"], *part,
                "case {:?}: the deprecated member must carry the same citation, got {:?}",
                row.case, frame.payload
            );
        }

        // The session the response reported is one this service holds: continuing it is
        // served, which is the round trip through the id it handed back.
        let continued = knowledge
            .generate_stream(generate_request(
                row.query,
                &knowledge.knowledge_base_id,
                json!({ "sessionId": session }),
            ))
            .await;
        assert_eq!(
            continued.status,
            200,
            "case {:?}: the session handed back must be one this service holds, got {} {:?}",
            row.case,
            continued.status,
            String::from_utf8_lossy(&continued.body)
        );
        assert_eq!(
            continued.header("x-amzn-bedrock-knowledge-base-session-id"),
            Some(session.as_str()),
            "case {:?}: continuing a session reports that same session",
            row.case
        );
    }
}
