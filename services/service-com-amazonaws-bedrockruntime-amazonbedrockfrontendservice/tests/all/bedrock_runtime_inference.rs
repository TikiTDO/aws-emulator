// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Bedrock Runtime emulator's first slice: synchronous
//! inference (`InvokeModel`, `Converse`, `CountTokens`) and the async-invoke
//! resource lifecycle (`StartAsyncInvoke`, `GetAsyncInvoke`,
//! `ListAsyncInvokes`).
//!
//! Every test drives real HTTP requests through the production router, so what
//! is asserted is the wire behaviour a client sees — status, headers, body —
//! never a handler called directly.
//!
//! Each assertion spans a SET of inputs: the tables below differ along what the
//! rule they check generalises over (model family, prompt size, message count,
//! token cap, page size, sort order, filter), so no row is the specification on
//! its own.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ops::types::{
    AsyncInvokeOutputDataConfig, AsyncInvokeS3OutputDataConfig,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState as ServiceState;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host-style endpoint for the service, so the request routes even if the
/// SigV4 scope below ever changes.
const ENDPOINT: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header scoped to this service's signing name
/// (`bedrock`) and to `us-east-1`, which is what `can_handle` routes on and what
/// the emulator reads the ARN region from. Request authentication is off in
/// these tests, so the signature is not checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One recognised model per `InvokeModel` response family, with the family's own
/// request field for the prompt and the response fields the family answers in.
///
/// Model ids from "Amazon Bedrock base model IDs (on-demand throughput)" and the
/// request/response envelopes from "Inference request parameters and response
/// fields for foundation models", both in the Amazon Bedrock User Guide. Each
/// row names a DIFFERENT family, which is what `InvokeModel` generalises over.
const FAMILY_ROWS: &[FamilyRow] = &[
    FamilyRow {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        family: state::ModelFamily::AnthropicMessages,
        request: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"Summarize the Mars mission in one line."}]}]}"#,
        capped_request: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":3,"messages":[{"role":"user","content":[{"type":"text","text":"Summarize the Mars mission in one line."}]}]}"#,
        text_pointer: "/content/0/text",
        input_token_pointer: "/usage/input_tokens",
        output_token_pointer: "/usage/output_tokens",
        stop_pointer: "/stop_reason",
        completed_stop: "end_turn",
        capped_stop: "max_tokens",
    },
    FamilyRow {
        model_id: "amazon.titan-text-premier-v1:0",
        family: state::ModelFamily::TitanText,
        request: r#"{"inputText":"Write a haiku about persistent state.","textGenerationConfig":{"maxTokenCount":512}}"#,
        capped_request: r#"{"inputText":"Write a haiku about persistent state.","textGenerationConfig":{"maxTokenCount":3}}"#,
        text_pointer: "/results/0/outputText",
        input_token_pointer: "/inputTextTokenCount",
        output_token_pointer: "/results/0/tokenCount",
        stop_pointer: "/results/0/completionReason",
        completed_stop: "FINISH",
        capped_stop: "LENGTH",
    },
    FamilyRow {
        model_id: "amazon.nova-pro-v1:0",
        family: state::ModelFamily::Nova,
        request: r#"{"system":[{"text":"You are terse."}],"messages":[{"role":"user","content":[{"text":"Name two planets."}]}]}"#,
        capped_request: r#"{"inferenceConfig":{"max_new_tokens":3},"system":[{"text":"You are terse."}],"messages":[{"role":"user","content":[{"text":"Name two planets."}]}]}"#,
        text_pointer: "/output/message/content/0/text",
        input_token_pointer: "/usage/inputTokens",
        output_token_pointer: "/usage/outputTokens",
        stop_pointer: "/stopReason",
        completed_stop: "end_turn",
        capped_stop: "max_tokens",
    },
    FamilyRow {
        model_id: "meta.llama3-8b-instruct-v1:0",
        family: state::ModelFamily::Llama,
        request: r#"{"prompt":"Explain idempotency to a cat.","max_gen_len":128}"#,
        capped_request: r#"{"prompt":"Explain idempotency to a cat.","max_gen_len":3}"#,
        text_pointer: "/generation",
        input_token_pointer: "/prompt_token_count",
        output_token_pointer: "/generation_token_count",
        stop_pointer: "/stop_reason",
        completed_stop: "stop",
        capped_stop: "length",
    },
    FamilyRow {
        model_id: "mistral.mistral-7b-instruct-v0:2",
        family: state::ModelFamily::Mistral,
        request: r#"{"prompt":"<s>[INST] List three primes. [/INST]","max_tokens":64}"#,
        capped_request: r#"{"prompt":"<s>[INST] List three primes. [/INST]","max_tokens":3}"#,
        text_pointer: "/outputs/0/text",
        input_token_pointer: "",
        output_token_pointer: "",
        stop_pointer: "/outputs/0/stop_reason",
        completed_stop: "stop",
        capped_stop: "length",
    },
    FamilyRow {
        model_id: "cohere.command-text-v14",
        family: state::ModelFamily::CohereCommand,
        request: r#"{"prompt":"Describe a page token.","max_tokens":64}"#,
        capped_request: r#"{"prompt":"Describe a page token.","max_tokens":3}"#,
        text_pointer: "/generations/0/text",
        input_token_pointer: "",
        output_token_pointer: "",
        stop_pointer: "/generations/0/finish_reason",
        completed_stop: "COMPLETE",
        capped_stop: "MAX_TOKENS",
    },
    // A provider Amazon Bedrock serves on the OpenAI-compatible chat completions
    // schema: the body binds `messages` and `max_tokens`, and the answer is a
    // `chat.completion` object whose one choice carries the assistant's message, the
    // finish reason and the prompt/completion token counts.
    FamilyRow {
        model_id: "moonshotai.kimi-k2.5",
        family: state::ModelFamily::OpenAiChat,
        request: r#"{"messages":[{"role":"user","content":"Name two planets."}],"max_tokens":256}"#,
        capped_request: r#"{"messages":[{"role":"user","content":"Name two planets."}],"max_tokens":3}"#,
        text_pointer: "/choices/0/message/content",
        input_token_pointer: "/usage/prompt_tokens",
        output_token_pointer: "/usage/completion_tokens",
        stop_pointer: "/choices/0/finish_reason",
        completed_stop: "stop",
        capped_stop: "length",
    },
];

/// A row of [`FAMILY_ROWS`].
struct FamilyRow {
    model_id: &'static str,
    /// The response family the model above belongs to.
    family: state::ModelFamily,
    /// A request whose generation cap is far above what the emulator returns.
    request: &'static str,
    /// The same request with the family's generation-cap field set to 3 tokens.
    capped_request: &'static str,
    /// JSON pointer to the generated text in the family's response envelope.
    text_pointer: &'static str,
    /// JSON pointer to the input-token count, empty when the family's envelope
    /// reports no token counts (Mistral and Cohere do not).
    input_token_pointer: &'static str,
    /// JSON pointer to the output-token count, empty as above.
    output_token_pointer: &'static str,
    /// JSON pointer to the family's stop/completion reason.
    stop_pointer: &'static str,
    /// The family's reason for a completion that ran to its end.
    completed_stop: &'static str,
    /// The family's reason for a completion cut off by the generation cap.
    capped_stop: &'static str,
}

/// A `Converse` conversation, spanning message count, text size, system blocks
/// and non-ASCII text.
struct ConversationRow {
    case: &'static str,
    system: serde_json::Value,
    messages: serde_json::Value,
}

fn conversation_rows() -> Vec<ConversationRow> {
    vec![
        ConversationRow {
            case: "single short user turn",
            system: serde_json::json!([]),
            messages: serde_json::json!([{"role": "user", "content": [{"text": "hello"}]}]),
        },
        ConversationRow {
            case: "three turns",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "What is a page token?"}]},
                {"role": "assistant", "content": [{"text": "An opaque cursor."}]},
                {"role": "user", "content": [{"text": "Why opaque?"}]},
            ]),
        },
        ConversationRow {
            case: "system blocks plus one turn",
            system: serde_json::json!([{"text": "You answer in one sentence."}, {"text": "Be literal."}]),
            messages: serde_json::json!([{"role": "user", "content": [{"text": "Define idempotency."}]}]),
        },
        ConversationRow {
            case: "long prompt",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "word ".repeat(500)}]},
            ]),
        },
        ConversationRow {
            case: "non-ascii prompt",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "日本語のトークンと émojis 🙂 を数える"}]},
            ]),
        },
        ConversationRow {
            case: "several content blocks in one turn",
            system: serde_json::json!([]),
            messages: serde_json::json!([
                {"role": "user", "content": [{"text": "first part,"}, {"text": "second part."}]},
            ]),
        },
    ]
}

/// A row of the `InvokeModel` refusal table: the case, the request headers and
/// the request body.
struct InvokeRefusalRow {
    case: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

/// The emulator under test: the production router with this one service mounted
/// on a shared engine, exactly as `core/router` assembles it. The engine is
/// returned too, so a test can read the service's own state the way the next
/// request would.
fn emulator_with_engine() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// The emulator under test, when a test does not read the state directly.
///
/// `pub(crate)` so the other suites of this target drive the SAME harness rather
/// than each growing its own copy of it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().1
}

/// One HTTP exchange.
pub(crate) struct Call {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Call {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets, or the
    /// body's `__type` when a value could not be rendered as a header.
    pub(crate) fn error_code(&self) -> String {
        if let Some(value) = self.headers.get("x-amzn-errortype") {
            return value.to_str().unwrap_or_default().to_string();
        }
        self.json()
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    pub(crate) fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Call {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("{ENDPOINT}{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", "bedrock-runtime.us-east-1.amazonaws.com");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

pub(crate) async fn post_json(
    router: &MultiServiceRouter,
    path: &str,
    body: &serde_json::Value,
) -> Call {
    send(
        router,
        "POST",
        path,
        &[("content-type", "application/json")],
        serde_json::to_vec(body).expect("a serializable test body"),
    )
    .await
}

pub(crate) async fn get(router: &MultiServiceRouter, path: &str) -> Call {
    send(router, "GET", path, &[], Vec::new()).await
}

/// `CountTokens` over a `Converse`-shaped input.
async fn count_converse_tokens(
    router: &MultiServiceRouter,
    model_id: &str,
    system: &serde_json::Value,
    messages: &serde_json::Value,
) -> i64 {
    let mut converse = serde_json::json!({ "messages": messages });
    if system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
        converse["system"] = system.clone();
    }
    count_converse_input_tokens(router, model_id, &converse).await
}

/// `CountTokens` over a whole `Converse`-shaped input, `toolConfig` included.
async fn count_converse_input_tokens(
    router: &MultiServiceRouter,
    model_id: &str,
    converse: &serde_json::Value,
) -> i64 {
    let call = post_json(
        router,
        &format!("/model/{model_id}/count-tokens"),
        &serde_json::json!({ "input": { "converse": converse } }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "CountTokens must succeed, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["inputTokens"]
        .as_i64()
        .expect("inputTokens is a modelled integer")
}

/// `CountTokens` over an `InvokeModel` body.
async fn count_invoke_body_tokens(
    router: &MultiServiceRouter,
    model_id: &str,
    body: &serde_json::Value,
) -> i64 {
    let encoded = base64(&serde_json::to_vec(body).expect("a serializable test body"));
    let call = post_json(
        router,
        &format!("/model/{model_id}/count-tokens"),
        &serde_json::json!({ "input": { "invokeModel": { "body": encoded } } }),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "CountTokens must succeed, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["inputTokens"]
        .as_i64()
        .expect("inputTokens is a modelled integer")
}

/// The tokens a set of prompt pieces is, by the emulator's one tokenizer.
///
/// `pub(crate)` so the other suites of this target count with the SAME helper.
pub(crate) fn tokens_of(pieces: &[&str]) -> i64 {
    pieces
        .iter()
        .map(|piece| i64::from(state::count_text_tokens(piece)))
        .sum()
}

// ---------------------------------------------------------------------------
// Synchronous inference
// ---------------------------------------------------------------------------

/// RULE: `CountTokens` and `Converse` count the tokens of one input with the
/// same tokenizer, so for identical input the count the two report is the same
/// one, and a `Converse` turn reports `usage.totalTokens` as the sum of the
/// input and output counts it actually served.
#[tokio::test]
async fn count_tokens_and_converse_report_the_same_input_token_count() {
    let router = emulator();
    let model_id = "anthropic.claude-3-sonnet-20240229-v1:0";

    for row in conversation_rows() {
        let case = row.case;
        let counted =
            count_converse_tokens(&router, model_id, &row.system, &row.messages).await;

        let mut request = serde_json::json!({ "messages": row.messages });
        if row.system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
            request["system"] = row.system.clone();
        }
        let call = post_json(&router, &format!("/model/{model_id}/converse"), &request).await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();

        assert!(
            counted > 0,
            "case {case:?}: expected a positive token count, got {counted}"
        );
        let usage = &body["usage"];
        assert_eq!(
            usage["inputTokens"].as_i64(),
            Some(counted),
            "case {case:?}: expected Converse usage.inputTokens to equal the CountTokens \
             result {counted}, got {:?}",
            usage["inputTokens"]
        );
        let input = usage["inputTokens"].as_i64().unwrap_or_default();
        let output = usage["outputTokens"].as_i64().unwrap_or_default();
        assert_eq!(
            usage["totalTokens"].as_i64(),
            Some(input + output),
            "case {case:?}: expected totalTokens == inputTokens + outputTokens, got {:?}",
            usage["totalTokens"]
        );
        assert_eq!(
            body["output"]["message"]["role"].as_str(),
            Some("assistant"),
            "case {case:?}: expected an assistant message, got {:?}",
            body["output"]["message"]["role"]
        );
        let text = body["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !text.is_empty(),
            "case {case:?}: expected a non-empty content block, got {:?}",
            body["output"]["message"]["content"]
        );
        assert_eq!(
            state::count_text_tokens(text) as i64,
            output,
            "case {case:?}: expected outputTokens to be the tokens of the returned text, \
             got {output} for {text:?}"
        );
        assert_eq!(
            body["stopReason"].as_str(),
            Some("end_turn"),
            "case {case:?}: expected stopReason end_turn, got {:?}",
            body["stopReason"]
        );
        assert!(
            body["metrics"]["latencyMs"].as_i64().unwrap_or(-1) > 0,
            "case {case:?}: expected a measured latency, got {:?}",
            body["metrics"]["latencyMs"]
        );
    }
}

/// A row of the prompt-accounting table: one kind of content a `Converse`
/// request can carry beyond plain user text, and every piece of text that kind
/// sends the model.
struct PromptPieceRow {
    case: &'static str,
    /// Content blocks appended to the base user turn.
    extra_content: serde_json::Value,
    /// The request's `toolConfig`, or `null` for a row that offers no tool.
    tool_config: serde_json::Value,
    /// Every piece of prompt text this row adds. EMPTY for a row whose content
    /// carries no text at all, which must leave the count unchanged.
    pieces: &'static [&'static str],
}

/// The content kinds a `Converse` message and a `toolConfig` can carry, one row
/// each, with the prompt text each one sends the model.
///
/// Shapes and member names from `smithy_query(query="shape",
/// shape="ContentBlock,SystemContentBlock,Tool,ToolResultContentBlock")` on this
/// service's model; the pieces are what the rule under test says reaches the
/// model.
fn prompt_piece_rows() -> Vec<PromptPieceRow> {
    vec![
        PromptPieceRow {
            case: "a tool definition: name, description and JSON input schema",
            extra_content: serde_json::json!([]),
            tool_config: serde_json::json!({
                "tools": [{"toolSpec": {
                    "name": "get_weather",
                    "description": "Look up the weather.",
                    "inputSchema": {"json": {"type": "object"}},
                }}],
            }),
            pieces: &["get_weather", "Look up the weather.", r#"{"type":"object"}"#],
        },
        PromptPieceRow {
            case: "a tool definition and a tool choice naming it",
            extra_content: serde_json::json!([]),
            tool_config: serde_json::json!({
                "tools": [{"toolSpec": {
                    "name": "get_weather",
                    "description": "Look up the weather.",
                    "inputSchema": {"json": {"type": "object"}},
                }}],
                "toolChoice": {"tool": {"name": "get_weather"}},
            }),
            pieces: &[
                "get_weather",
                "Look up the weather.",
                r#"{"type":"object"}"#,
                "get_weather",
            ],
        },
        PromptPieceRow {
            case: "a document carried as text",
            extra_content: serde_json::json!([{"document": {
                "format": "txt",
                "name": "release notes",
                "source": {"text": "The emulator persists every record."},
            }}]),
            tool_config: serde_json::Value::Null,
            pieces: &["release notes", "The emulator persists every record."],
        },
        PromptPieceRow {
            case: "a tool use and the tool result text answering it",
            extra_content: serde_json::json!([
                {"toolUse": {
                    "toolUseId": "tooluse-1",
                    "name": "get_weather",
                    "input": {"city": "Lisbon"},
                }},
                {"toolResult": {
                    "toolUseId": "tooluse-1",
                    "content": [{"text": "Twenty three degrees and clear."}],
                }},
            ]),
            tool_config: serde_json::Value::Null,
            pieces: &[
                "get_weather",
                r#"{"city":"Lisbon"}"#,
                "Twenty three degrees and clear.",
            ],
        },
        PromptPieceRow {
            // The tool request this result answers is carried too: a tool result
            // addresses the id of a tool request the conversation holds.
            case: "a tool result carrying JSON",
            extra_content: serde_json::json!([
                {"toolUse": {
                    "toolUseId": "tooluse-2",
                    "name": "get_celsius",
                    "input": {"city": "Lisbon"},
                }},
                {"toolResult": {
                    "toolUseId": "tooluse-2",
                    "content": [{"json": {"celsius": 23}}],
                }},
            ]),
            tool_config: serde_json::Value::Null,
            pieces: &[
                "get_celsius",
                r#"{"city":"Lisbon"}"#,
                r#"{"celsius":23}"#,
            ],
        },
        PromptPieceRow {
            case: "guarded content",
            extra_content: serde_json::json!([{"guardContent": {
                "text": {"text": "Guard this sentence."},
            }}]),
            tool_config: serde_json::Value::Null,
            pieces: &["Guard this sentence."],
        },
        PromptPieceRow {
            case: "reasoning text",
            extra_content: serde_json::json!([{"reasoningContent": {
                "reasoningText": {"text": "Weigh both answers first.", "signature": "c2ln"},
            }}]),
            tool_config: serde_json::Value::Null,
            pieces: &["Weigh both answers first."],
        },
        PromptPieceRow {
            case: "a search result",
            extra_content: serde_json::json!([{"searchResult": {
                "source": "https://example.invalid/weather",
                "title": "Lisbon weather",
                "content": [{"text": "Clear skies all week."}],
            }}]),
            tool_config: serde_json::Value::Null,
            pieces: &[
                "https://example.invalid/weather",
                "Lisbon weather",
                "Clear skies all week.",
            ],
        },
        PromptPieceRow {
            case: "an image, whose payload is not text",
            extra_content: serde_json::json!([{"image": {
                "format": "png",
                "source": {"bytes": "aGVsbG8gaW1hZ2UgYnl0ZXM="},
            }}]),
            tool_config: serde_json::Value::Null,
            pieces: &[],
        },
        PromptPieceRow {
            case: "a cache point, which is a marker and not prompt text",
            extra_content: serde_json::json!([{"cachePoint": {"type": "default"}}]),
            tool_config: serde_json::Value::Null,
            pieces: &[],
        },
    ]
}

/// RULE: the input-token count an inference call reports covers every piece of
/// text the request sends the model — each message's content blocks of EVERY
/// kind and the complete definition of each tool offered, not just the
/// plain-text blocks — so adding a content kind that carries text raises the
/// count by exactly the tokens of that text, while content that carries no text
/// leaves it unchanged. `CountTokens` reports the same count for the same input.
#[tokio::test]
async fn an_inference_call_counts_every_piece_of_prompt_text_the_request_carries() {
    let router = emulator();
    let model_id = "anthropic.claude-3-sonnet-20240229-v1:0";
    let base_text = "Summarize the weather for me.";

    let base = serde_json::json!({
        "messages": [{"role": "user", "content": [{"text": base_text}]}],
    });
    let base_tokens = count_converse_input_tokens(&router, model_id, &base).await;
    assert_eq!(
        base_tokens,
        tokens_of(&[base_text]),
        "case \"base turn\": expected the plain user text to be counted, got {base_tokens}"
    );

    for row in prompt_piece_rows() {
        let case = row.case;
        let mut content = vec![serde_json::json!({"text": base_text})];
        for block in row.extra_content.as_array().into_iter().flatten() {
            content.push(block.clone());
        }
        let mut request = serde_json::json!({
            "messages": [{"role": "user", "content": content}],
        });
        if !row.tool_config.is_null() {
            request["toolConfig"] = row.tool_config.clone();
        }

        let expected = base_tokens + tokens_of(row.pieces);
        let counted = count_converse_input_tokens(&router, model_id, &request).await;
        assert_eq!(
            counted, expected,
            "case {case:?}: expected CountTokens to report {expected} \
             ({base_tokens} for the base turn plus {} for this row), got {counted}",
            tokens_of(row.pieces)
        );

        let call = post_json(&router, &format!("/model/{model_id}/converse"), &request).await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected Converse to answer 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        // The prompt tokens a turn reports are its `inputTokens` plus whatever a
        // cache checkpoint moved into the cache counters, so the comparison
        // holds for a request that marks a cacheable prefix too.
        let usage = &call.json()["usage"];
        let reported = usage["inputTokens"].as_i64().map(|input| {
            input
                + usage["cacheReadInputTokens"].as_i64().unwrap_or_default()
                + usage["cacheWriteInputTokens"].as_i64().unwrap_or_default()
        });
        assert_eq!(
            reported,
            Some(expected),
            "case {case:?}: expected Converse to account for {expected} prompt tokens, \
             got {usage}"
        );
        if row.pieces.is_empty() {
            assert_eq!(
                reported,
                Some(base_tokens),
                "case {case:?}: content that carries no text must not change the count, \
                 got {reported:?} against {base_tokens}"
            );
        } else {
            assert!(
                reported.unwrap_or_default() > base_tokens,
                "case {case:?}: text added to the prompt must raise the count, got \
                 {reported:?} against {base_tokens}"
            );
        }
    }
}

/// A row of the invoke-body tool table: a family's body with and without the
/// tool definitions it can carry, and the pieces those definitions add.
struct InvokeToolRow {
    case: &'static str,
    model_id: &'static str,
    base: serde_json::Value,
    /// The member the family binds its tool definitions to, with its value.
    tools_member: (&'static str, serde_json::Value),
    pieces: &'static [&'static str],
    /// Whether AWS publishes `CountTokens` for this model, so the two
    /// operations can be compared on the same body.
    counts_tokens: bool,
}

/// RULE: an `InvokeModel` body's prompt is every piece of text the body sends
/// the model — the fields the family binds prompt text to AND the whole of each
/// tool definition it carries, which reaches the model verbatim — so adding the
/// family's tool definitions to an otherwise identical body raises the reported
/// input count by exactly the tokens of their text.
///
/// Member names from the per-family request formats in "Inference request
/// parameters and response fields for foundation models" (Amazon Bedrock User
/// Guide): Anthropic Claude's `tools`, Nova's `toolConfig`.
#[tokio::test]
async fn an_invoke_body_counts_the_tool_definitions_it_carries() {
    let router = emulator();
    let rows = vec![
        InvokeToolRow {
            case: "anthropic claude messages body with a tools array",
            model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
            base: serde_json::json!({
                "anthropic_version": "bedrock-2023-05-31",
                "max_tokens": 256,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Weather?"}]}],
            }),
            tools_member: (
                "tools",
                serde_json::json!([{
                    "name": "get_weather",
                    "description": "Look up the weather.",
                    "input_schema": {"type": "object"},
                }]),
            ),
            pieces: &["get_weather", "Look up the weather.", "object"],
            counts_tokens: true,
        },
        InvokeToolRow {
            case: "nova body with a toolConfig",
            model_id: "amazon.nova-pro-v1:0",
            base: serde_json::json!({
                "messages": [{"role": "user", "content": [{"text": "Weather?"}]}],
            }),
            tools_member: (
                "toolConfig",
                serde_json::json!({
                    "tools": [{"toolSpec": {
                        "name": "get_weather",
                        "description": "Look up the weather.",
                        "inputSchema": {"json": {"type": "object"}},
                    }}],
                }),
            ),
            pieces: &["get_weather", "Look up the weather.", "object"],
            // AWS publishes `CountTokens` for Anthropic Claude models only, and
            // the emulator's inventory follows it, so this row compares the two
            // operations and the Nova row does not.
            counts_tokens: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let model_id = row.model_id;
        let mut with_tools = row.base.clone();
        with_tools[row.tools_member.0] = row.tools_member.1.clone();

        let plain = invoke_input_tokens(&router, model_id, &row.base).await;
        assert!(
            plain > 0,
            "case {case:?}: expected a positive count for the plain body, got {plain}"
        );
        let expected = plain + tokens_of(row.pieces);
        let reported = invoke_input_tokens(&router, model_id, &with_tools).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the tool definitions to add {} tokens to {plain}, \
             got {reported}",
            tokens_of(row.pieces)
        );

        if row.counts_tokens {
            // One tokenizer, one rule: `CountTokens` reports what the invoke
            // path reports for the same body.
            for (label, body) in [("without tools", &row.base), ("with tools", &with_tools)] {
                let counted = count_invoke_body_tokens(&router, model_id, body).await;
                let served = invoke_input_tokens(&router, model_id, body).await;
                assert_eq!(
                    counted, served,
                    "case {case:?} ({label}): expected CountTokens and InvokeModel to report \
                     one count, got {counted} and {served}"
                );
            }
        }
    }
}

/// The input-token count the invoked family's own response envelope reports.
async fn invoke_input_tokens(
    router: &MultiServiceRouter,
    model_id: &str,
    body: &serde_json::Value,
) -> i64 {
    let call = send(
        router,
        "POST",
        &format!("/model/{model_id}/invoke"),
        &[("content-type", "application/json")],
        serde_json::to_vec(body).expect("a serializable test body"),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "InvokeModel must succeed, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let served = call.json();
    served
        .pointer("/usage/input_tokens")
        .or_else(|| served.pointer("/usage/inputTokens"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| panic!("the family envelope reports an input count: {served}"))
}

/// One generation-cap row: the `inferenceConfig.maxTokens` a request carries and
/// the model it is sent to.
struct CapRow {
    case: &'static str,
    model_id: &'static str,
    /// `inferenceConfig.maxTokens`, or `None` for a request that omits
    /// `inferenceConfig` altogether.
    max_tokens: Option<i64>,
}

/// The generation-cap table: the published bound of
/// `InferenceConfiguration.maxTokens` (`@range` min 1, from
/// `smithy_query(query="shape", shape="InferenceConfiguration")`), caps just
/// below, at and just above the shortest completion this emulator can serve,
/// the modest cap a short turn is usually sent with, a cap far above any
/// completion, and a request that supplies no cap at all — across several model
/// families, which is what the cap rule generalises over.
fn cap_rows() -> Vec<CapRow> {
    let floor = i64::from(state::count_text_tokens(state::EMULATED_COMPLETION_FLOOR));
    vec![
        CapRow {
            case: "the modelled minimum cap",
            model_id: "amazon.nova-lite-v1:0",
            max_tokens: Some(1),
        },
        CapRow {
            case: "a cap of two tokens",
            model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
            max_tokens: Some(2),
        },
        CapRow {
            case: "one token below the shortest completion",
            model_id: "amazon.nova-lite-v1:0",
            max_tokens: Some(floor - 1),
        },
        CapRow {
            case: "exactly the shortest completion",
            model_id: "mistral.mistral-7b-instruct-v0:2",
            max_tokens: Some(floor),
        },
        CapRow {
            case: "one token above the shortest completion",
            model_id: "amazon.titan-text-premier-v1:0",
            max_tokens: Some(floor + 1),
        },
        CapRow {
            case: "the modest cap a short turn is sent with",
            model_id: "amazon.nova-lite-v1:0",
            max_tokens: Some(20),
        },
        CapRow {
            case: "a cap far above any completion",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            max_tokens: Some(4096),
        },
        CapRow {
            case: "no inferenceConfig at all",
            model_id: "amazon.nova-lite-v1:0",
            max_tokens: None,
        },
    ]
}

/// RULE: `inferenceConfig.maxTokens` is the maximum number of tokens to ALLOW in
/// the generated response — a ceiling, not a target — so a turn whose completion
/// finishes inside that ceiling stops on `end_turn` however much of the budget
/// is left over, and `max_tokens` is reported only by a completion the ceiling
/// genuinely cut off; either way the output count never exceeds the ceiling.
#[tokio::test]
async fn converse_reports_max_tokens_only_for_a_completion_the_ceiling_cut_off() {
    let router = emulator();
    let floor = i64::from(state::count_text_tokens(state::EMULATED_COMPLETION_FLOOR));
    let messages = serde_json::json!([{
        "role": "user",
        "content": [{"text": "Reply with only the lowercase English word for the result of 2+3."}],
    }]);

    for row in cap_rows() {
        let case = row.case;
        let mut request = serde_json::json!({ "messages": messages });
        if let Some(cap) = row.max_tokens {
            request["inferenceConfig"] = serde_json::json!({ "maxTokens": cap, "temperature": 0.0 });
        }
        let call = post_json(
            &router,
            &format!("/model/{}/converse", row.model_id),
            &request,
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        let output = body["usage"]["outputTokens"].as_i64().unwrap_or_default();
        if let Some(cap) = row.max_tokens {
            assert!(
                output <= cap,
                "case {case:?}: expected at most {cap} output tokens, got {output}"
            );
        }
        let text = body["output"]["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            i64::from(state::count_text_tokens(&text)),
            output,
            "case {case:?}: expected outputTokens to be the tokens of {text:?}, got {output}"
        );
        // A completion is cut off only by a ceiling it cannot fit the shortest
        // completion in; every other ceiling leaves the turn finished.
        let expected = match row.max_tokens {
            Some(cap) if cap < floor => "max_tokens",
            _ => "end_turn",
        };
        assert_eq!(
            body["stopReason"].as_str(),
            Some(expected),
            "case {case:?}: expected stopReason {expected} for maxTokens={:?} against a \
             {floor}-token shortest completion, got {:?} with text {text:?}",
            row.max_tokens,
            body["stopReason"]
        );
        // `metrics` is a required member reporting the latency this invocation
        // itself took, and serving one takes time, so the value is positive.
        assert!(
            body["metrics"]["latencyMs"].as_i64().unwrap_or(-1) > 0,
            "case {case:?}: expected a positive measured latency, got {:?}",
            body["metrics"]["latencyMs"]
        );
        if expected == "end_turn" {
            assert!(
                !text.ends_with(" generated") && text.ends_with('.'),
                "case {case:?}: a finished turn answers a whole completion, got {text:?}"
            );
        }
    }
}

/// RULE (same ceiling rule, on the `InvokeModel` surface): the generation cap a
/// model family's own request body carries is a ceiling too, so a body whose cap
/// admits the completion is answered with that family's completed stop reason,
/// and its length stop reason is reported only for a completion the cap cut off.
#[tokio::test]
async fn invoke_model_reports_a_length_stop_only_for_a_completion_the_cap_cut_off() {
    let router = emulator();
    let floor = state::count_text_tokens(state::EMULATED_COMPLETION_FLOOR);

    // One row per family, each sent twice: once with a cap that admits the
    // shortest completion and once with a cap below it. The cap member per
    // family is the one `state::body_max_tokens` documents.
    let rows: &[(&str, state::ModelFamily, &str, &str, &str)] = &[
        (
            "anthropic messages",
            state::ModelFamily::AnthropicMessages,
            "anthropic.claude-3-haiku-20240307-v1:0",
            r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":CAP,"messages":[{"role":"user","content":[{"type":"text","text":"Name one planet."}]}]}"#,
            "/stop_reason",
        ),
        (
            "titan text",
            state::ModelFamily::TitanText,
            "amazon.titan-text-premier-v1:0",
            r#"{"inputText":"Name one planet.","textGenerationConfig":{"maxTokenCount":CAP}}"#,
            "/results/0/completionReason",
        ),
        (
            "nova",
            state::ModelFamily::Nova,
            "amazon.nova-lite-v1:0",
            r#"{"messages":[{"role":"user","content":[{"text":"Name one planet."}]}],"inferenceConfig":{"max_new_tokens":CAP}}"#,
            "/stopReason",
        ),
        (
            "llama",
            state::ModelFamily::Llama,
            "meta.llama3-8b-instruct-v1:0",
            r#"{"prompt":"Name one planet.","max_gen_len":CAP}"#,
            "/stop_reason",
        ),
        (
            "mistral",
            state::ModelFamily::Mistral,
            "mistral.mistral-7b-instruct-v0:2",
            r#"{"prompt":"Name one planet.","max_tokens":CAP}"#,
            "/outputs/0/stop_reason",
        ),
        (
            "cohere command",
            state::ModelFamily::CohereCommand,
            "cohere.command-text-v14",
            r#"{"prompt":"Name one planet.","max_tokens":CAP}"#,
            "/generations/0/finish_reason",
        ),
    ];

    for (case, family, model_id, template, stop_pointer) in rows {
        for (cap, cut_off) in [(floor, false), (floor - 1, true), (1, true), (512, false)] {
            let case = format!("{case} at a cap of {cap}");
            let body = template.replace("CAP", &cap.to_string());
            let call = send(
                &router,
                "POST",
                &format!("/model/{model_id}/invoke"),
                &[("content-type", "application/json"), ("accept", "application/json")],
                body.into_bytes(),
            )
            .await;
            assert_eq!(
                call.status,
                200,
                "case {case:?}: expected 200, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let served = call.json();
            let expected = state::family_stop_reason(*family, cut_off);
            assert_eq!(
                served.pointer(stop_pointer).and_then(serde_json::Value::as_str),
                Some(expected),
                "case {case:?}: expected the stop reason {expected:?} at {stop_pointer} \
                 against a {floor}-token shortest completion, got {served}"
            );
        }
    }
}

/// RULE: `InvokeModel` answers in the response envelope of the invoked model's
/// own family, as a JSON body with a `Content-Type` of `application/json`, and
/// the token counts it reports are the ones the tokenizer computes for the
/// request body — the same count `CountTokens` reports for that body.
#[tokio::test]
async fn invoke_model_answers_in_each_family_response_envelope() {
    let router = emulator();

    for row in FAMILY_ROWS {
        let case = row.model_id;
        let call = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[("content-type", "application/json"), ("accept", "application/json")],
            row.request.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.header("content-type"),
            "application/json",
            "case {case:?}: expected the modelled Content-Type response header, got {:?}",
            call.header("content-type")
        );
        assert_eq!(
            call.header("x-amzn-bedrock-performanceconfig-latency"),
            "standard",
            "case {case:?}: expected the defaulted performanceConfigLatency echoed back, got {:?}",
            call.header("x-amzn-bedrock-performanceconfig-latency")
        );

        let body = call.json();
        let text = body
            .pointer(row.text_pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: no text at {} in {body}", row.text_pointer)
            });
        assert!(
            !text.is_empty(),
            "case {case:?}: expected a non-empty completion at {}",
            row.text_pointer
        );

        assert_eq!(
            body.pointer(row.stop_pointer)
                .and_then(serde_json::Value::as_str),
            Some(row.completed_stop),
            "case {case:?}: expected the family's completed stop reason at {} in {body}",
            row.stop_pointer
        );
        if !row.input_token_pointer.is_empty() {
            assert_eq!(
                body.pointer(row.output_token_pointer)
                    .and_then(serde_json::Value::as_i64),
                Some(i64::from(state::count_text_tokens(text))),
                "case {case:?}: expected {} to be the tokens of the returned text in {body}",
                row.output_token_pointer
            );
        }

        let counted = post_json(
            &router,
            &format!("/model/{}/count-tokens", row.model_id),
            &serde_json::json!({
                "input": { "invokeModel": { "body": base64(row.request.as_bytes()) } },
            }),
        )
        .await;
        // `CountTokens` counts with the MODEL's own tokenizer, which AWS
        // publishes for the Anthropic Claude family; a model outside it refuses
        // the count rather than answering one computed with another tokenizer.
        if !row.model_id.starts_with("anthropic.") {
            assert_eq!(
                counted.status,
                400,
                "case {case:?}: a model CountTokens does not serve must refuse the count, \
                 got {} / {}",
                counted.status,
                String::from_utf8_lossy(&counted.body)
            );
            assert_eq!(
                counted.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                counted.error_code()
            );
            continue;
        }
        // For a model it does serve, the count it reports is the one the
        // family's response envelope reports as the input tokens: one tokenizer,
        // two operations.
        assert_eq!(
            counted.status,
            200,
            "case {case:?}: expected CountTokens to answer 200, got {} / {}",
            counted.status,
            String::from_utf8_lossy(&counted.body)
        );
        let counted = counted.json()["inputTokens"].as_i64().unwrap_or_default();
        assert!(
            counted > 0,
            "case {case:?}: expected a positive token count for the body, got {counted}"
        );
        if !row.input_token_pointer.is_empty() {
            assert_eq!(
                body.pointer(row.input_token_pointer)
                    .and_then(serde_json::Value::as_i64),
                Some(counted),
                "case {case:?}: expected {} to be the CountTokens result {counted} in {body}",
                row.input_token_pointer
            );
        }
    }
}

/// RULE: `InvokeModel` honours the generation cap the request body carries for
/// the invoked model's family: the completion is cut to at most that many tokens
/// and the family's response reports its own length stop reason.
#[tokio::test]
async fn invoke_model_honours_the_generation_cap_the_body_carries() {
    let router = emulator();

    for row in FAMILY_ROWS {
        let case = row.model_id;
        let call = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", row.model_id),
            &[("content-type", "application/json")],
            row.capped_request.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        let text = body
            .pointer(row.text_pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no text at {} in {body}", row.text_pointer));
        assert!(
            state::count_text_tokens(text) <= 3,
            "case {case:?}: expected at most the 3 tokens the body asked for, got {} for {text:?}",
            state::count_text_tokens(text)
        );
        assert_eq!(
            body.pointer(row.stop_pointer)
                .and_then(serde_json::Value::as_str),
            Some(row.capped_stop),
            "case {case:?}: expected the family's length stop reason at {} in {body}",
            row.stop_pointer
        );
        if !row.output_token_pointer.is_empty() {
            assert_eq!(
                body.pointer(row.output_token_pointer)
                    .and_then(serde_json::Value::as_i64),
                Some(i64::from(state::count_text_tokens(text))),
                "case {case:?}: expected {} to be the tokens of the cut text in {body}",
                row.output_token_pointer
            );
        }
    }
}

/// RULE: every served inference call is recorded in the service's own inference
/// log — which API served it, for which model, and the token counts it reported
/// — and the log is bounded, so it keeps at most its retention limit of entries.
#[tokio::test]
async fn every_served_inference_call_is_recorded_in_the_inference_log() {
    let (engine, router) = emulator_with_engine();
    let model_id = "amazon.nova-lite-v1:0";

    let invoked = send(
        &router,
        "POST",
        &format!("/model/{model_id}/invoke"),
        &[("content-type", "application/json")],
        br#"{"messages":[{"role":"user","content":[{"text":"count me"}]}]}"#.to_vec(),
    )
    .await;
    assert_eq!(invoked.status, 200, "InvokeModel must be served");
    let conversed = post_json(
        &router,
        &format!("/model/{model_id}/converse"),
        &serde_json::json!({"messages": [{"role": "user", "content": [{"text": "count me too"}]}]}),
    )
    .await;
    assert_eq!(conversed.status, 200, "Converse must be served");
    // Asynchronous invocation is published only for the video generation
    // models, so the async leg of the log is exercised against one of those.
    let started = post_json(
        &router,
        "/async-invoke",
        &serde_json::json!({
            "modelId": "luma.ray-v2:0",
            "modelInput": {"prompt": "and me"},
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/out"}},
        }),
    )
    .await;
    assert_eq!(started.status, 200, "StartAsyncInvoke must be served");
    // A reporting operation must add nothing to the log. `CountTokens` counts with
    // the model's own tokenizer, which AWS publishes for the Anthropic Claude
    // family, so the read is made against a model it serves.
    let _ = count_converse_tokens(
        &router,
        "anthropic.claude-3-haiku-20240307-v1:0",
        &serde_json::json!([]),
        &serde_json::json!([{"role": "user", "content": [{"text": "not an inference call"}]}]),
    )
    .await;

    let state_handle = engine.state::<ServiceState>();
    let logged = state_handle.lock().invocations.clone();
    assert_eq!(
        logged.len(),
        3,
        "expected one log entry per served inference call, got {logged:?}"
    );
    let kinds: Vec<state::InvokeKind> = logged.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![
            state::InvokeKind::Sync,
            state::InvokeKind::Converse,
            state::InvokeKind::Async
        ],
        "expected the log in call order with each API's own kind, got {kinds:?}"
    );
    // The synchronous calls name the text model, the asynchronous one the video
    // model it is the only API published for.
    for (entry, expected) in logged.iter().zip([model_id, model_id, "luma.ray-v2:0"]) {
        assert_eq!(
            entry.model_id, expected,
            "expected the resolved model id on every entry, got {entry:?}"
        );
        assert!(
            entry.input_tokens > 0,
            "expected the counted input tokens on every entry, got {entry:?}"
        );
    }
    assert!(
        logged.len() <= state::MAX_INVOCATION_LOG_ENTRIES,
        "expected a bounded log, got {} entries",
        logged.len()
    );
}

/// One `modelId` as a client may spell it, and what the service owes for it.
///
/// The rows differ along the class of identifier — a served base model in each
/// spelling the `modelId` pattern allows, an identifier that names no model the
/// service offers, an ARN naming an account-scoped resource, and a model past
/// its end of life — which is what the resolution rule generalises over.
struct IdentifierRow {
    case: &'static str,
    model_id: &'static str,
    status: u16,
    /// The modelled error code, empty for a row that must be served.
    code: &'static str,
    /// The message, where the service's message for the class is a fixed one.
    message: Option<&'static str>,
    /// An `InvokeModel` body the named model's family accepts. A row that is
    /// refused on its identifier never reaches the body.
    body: &'static str,
}

/// A body the Amazon Nova family accepts, and one the Meta Llama family does.
const NOVA_BODY: &str = r#"{"messages":[{"role":"user","content":[{"text":"hi"}]}]}"#;
const LLAMA_BODY: &str = r#"{"prompt":"hi"}"#;
/// A body the Anthropic Claude Messages API accepts: it binds the three members
/// that API documents as required.
const CLAUDE_BODY: &str = r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#;
/// A body the OpenAI-compatible chat completions envelope accepts.
const OPENAI_CHAT_BODY: &str = r#"{"messages":[{"role":"user","content":"hi"}],"max_tokens":64}"#;

fn identifier_rows() -> Vec<IdentifierRow> {
    let invalid = Some(state::INVALID_MODEL_IDENTIFIER_MESSAGE);
    let retired = Some(state::END_OF_LIFE_MESSAGE);
    vec![
        // Spellings of a base model the inventory carries, all served.
        IdentifierRow {
            case: "bare base model id",
            model_id: "amazon.nova-micro-v1:0",
            status: 200,
            code: "",
            message: None,
            body: NOVA_BODY,
        },
        IdentifierRow {
            case: "cross-region inference profile id",
            model_id: "us.amazon.nova-micro-v1:0",
            status: 200,
            code: "",
            message: None,
            body: NOVA_BODY,
        },
        IdentifierRow {
            case: "foundation-model ARN without an account",
            model_id: "arn:aws:bedrock:us-east-1::foundation-model/meta.llama3-70b-instruct-v1:0",
            status: 200,
            code: "",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "inference-profile ARN of a system-defined profile",
            model_id:
                "arn:aws:bedrock:us-east-1:000000000000:inference-profile/eu.amazon.nova-lite-v1:0",
            status: 200,
            code: "",
            message: None,
            body: NOVA_BODY,
        },
        // Identifiers that name no model the service offers: a client input
        // fault, so 400 with the service's fixed message.
        IdentifierRow {
            case: "bare id of no model",
            model_id: "nobody.such-model-v1",
            status: 400,
            code: "ValidationException",
            message: invalid,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "an id with no provider at all",
            model_id: "claude",
            status: 400,
            code: "ValidationException",
            message: invalid,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "geography prefix on no model",
            model_id: "apac.nobody.such-model-v1",
            status: 400,
            code: "ValidationException",
            message: invalid,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "a catalogued model of a provider served on the OpenAI-compatible schema",
            model_id: "moonshotai.kimi-k2.5",
            status: 200,
            code: "",
            message: None,
            body: OPENAI_CHAT_BODY,
        },
        IdentifierRow {
            case: "a published provider's id at a version the catalogue does not carry",
            model_id: "ai21.jamba-9-9-mega-v9:0",
            status: 400,
            code: "ValidationException",
            message: invalid,
            body: LLAMA_BODY,
        },
        // ARNs: well-formed references to a resource, so an absent one is 404.
        IdentifierRow {
            case: "foundation-model ARN of no model",
            model_id: "arn:aws:bedrock:us-east-1::foundation-model/nobody.nope-v1",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "provisioned throughput ARN",
            model_id: "arn:aws:bedrock:us-east-1:000000000000:provisioned-model/abc123def456",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "custom model ARN",
            model_id:
                "arn:aws:bedrock:us-east-1:000000000000:custom-model/amazon.titan-text-express-v1/abc123def456",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "imported model ARN",
            model_id: "arn:aws:bedrock:us-east-1:000000000000:imported-model/abc123def456",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "custom model deployment ARN",
            model_id:
                "arn:aws:bedrock:us-east-1:000000000000:custom-model-deployment/abc123def456",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "application inference profile ARN",
            model_id:
                "arn:aws:bedrock:us-east-1:000000000000:application-inference-profile/zz9plural",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "prompt router ARN",
            model_id: "arn:aws:bedrock:us-east-1:000000000000:prompt-router/my-router",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "SageMaker endpoint ARN",
            model_id: "arn:aws:sagemaker:us-east-1:000000000000:endpoint/my-endpoint",
            status: 404,
            code: "ResourceNotFoundException",
            message: None,
            body: LLAMA_BODY,
        },
        // A model past its end of life: the identifier resolves, the model is
        // gone, so it is the resource that is not found.
        IdentifierRow {
            case: "retired Anthropic model",
            model_id: "anthropic.claude-v2",
            status: 404,
            code: "ResourceNotFoundException",
            message: retired,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "retired AI21 model",
            model_id: "ai21.j2-ultra-v1",
            status: 404,
            code: "ResourceNotFoundException",
            message: retired,
            body: LLAMA_BODY,
        },
        IdentifierRow {
            case: "retired model named by ARN",
            model_id: "arn:aws:bedrock:us-east-1::foundation-model/meta.llama2-70b-chat-v1",
            status: 404,
            code: "ResourceNotFoundException",
            message: retired,
            body: LLAMA_BODY,
        },
    ]
}

/// RULE: every operation that takes a `modelId` resolves it before it serves
/// anything, and the refusal it owes depends on the CLASS of identifier, not on
/// the operation: an identifier the service cannot interpret as naming a model
/// it offers is a client input fault — `ValidationException` (400) with the
/// service's fixed message — while an identifier that does name a resource (an
/// ARN, or a base model the inventory carries as past its end of life) whose
/// resource is not available is `ResourceNotFoundException` (404). A base model
/// the inventory carries is served under every spelling the `modelId` pattern
/// allows: the bare id, a cross-region inference profile id, a
/// `foundation-model` ARN and an `inference-profile` ARN.
#[tokio::test]
async fn a_model_identifier_is_refused_by_the_class_of_identifier_it_is() {
    let router = emulator();

    for row in identifier_rows() {
        let encoded = encode_path(row.model_id);
        let case = row.case;

        // Every operation that resolves a modelId must answer the same way.
        let mut calls = vec![(
            "InvokeModel",
            send(
                &router,
                "POST",
                &format!("/model/{encoded}/invoke"),
                &[("content-type", "application/json")],
                row.body.as_bytes().to_vec(),
            )
            .await,
        )];
        if row.status != 200 {
            // A served row is asserted against `InvokeModel` only: `Converse`
            // and `CountTokens` each have their own rule about which models
            // they serve, pinned separately. The identifier rule itself is what
            // every one of them must answer identically, which is what the
            // refused rows check.
            calls.push((
                "CountTokens",
                post_json(
                    &router,
                    &format!("/model/{encoded}/count-tokens"),
                    &serde_json::json!({
                        "input": { "invokeModel": { "body": base64(b"{\"prompt\":\"hi\"}") } },
                    }),
                )
                .await,
            ));
            calls.push((
                "Converse",
                post_json(
                    &router,
                    &format!("/model/{encoded}/converse"),
                    &serde_json::json!({
                        "messages": [{"role": "user", "content": [{"text": "hi"}]}],
                    }),
                )
                .await,
            ));
        }

        for (operation, call) in calls {
            assert_eq!(
                call.status,
                row.status,
                "case {case:?} {operation} {:?}: expected {}, got {} / {}",
                row.model_id,
                row.status,
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            if !row.code.is_empty() {
                assert_eq!(
                    call.error_code(),
                    row.code,
                    "case {case:?} {operation} {:?}: expected {}, got {:?}",
                    row.model_id,
                    row.code,
                    call.error_code()
                );
            }
            if let Some(expected) = row.message {
                assert_eq!(
                    call.json()["message"].as_str(),
                    Some(expected),
                    "case {case:?} {operation} {:?}: expected message {expected:?}, got {:?}",
                    row.model_id,
                    call.json()["message"]
                );
            }
        }
    }
}

/// RULE: the model inventory is what `modelId` resolution answers from, so every
/// active text model it carries is served by `InvokeModel` in its family's own
/// response envelope — no model id is served or refused as a special case.
#[tokio::test]
async fn every_active_text_model_in_the_inventory_is_served_in_its_family_envelope() {
    let (engine, router) = emulator_with_engine();
    let inventory: Vec<(String, Option<state::ModelFamily>, state::ModelModality)> = engine
        .state::<ServiceState>()
        .lock()
        .models
        .values()
        .filter(|entry| entry.lifecycle == state::ModelLifecycle::Active)
        .map(|entry| (entry.model_id.clone(), entry.family, entry.modality))
        .collect();
    assert!(
        inventory.len() >= FAMILY_ROWS.len(),
        "case \"inventory\": expected at least one model per response family, got {}",
        inventory.len()
    );

    let mut served = 0usize;
    for (model_id, family, modality) in inventory {
        let Some(family) = family else { continue };
        assert_eq!(
            modality,
            state::ModelModality::Text,
            "case {model_id:?}: only a text model has a response envelope"
        );
        served += 1;
        // The family's own request body, from the family table above.
        let request = FAMILY_ROWS
            .iter()
            .find(|row| row.family == family)
            .map(|row| row.request)
            .unwrap_or_else(|| panic!("case {model_id:?}: no request body for family {family:?}"));
        let call = send(
            &router,
            "POST",
            &format!("/model/{}/invoke", encode_path(&model_id)),
            &[("content-type", "application/json")],
            request.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            200,
            "case {model_id:?}: expected the inventory's model to be served, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let body = call.json();
        // The family's own envelope: its generated-text member must be there.
        // Pointers as in [`FAMILY_ROWS`], one per documented envelope.
        let text_pointer = match family {
            state::ModelFamily::AnthropicMessages => "/content/0/text",
            state::ModelFamily::TitanText => "/results/0/outputText",
            state::ModelFamily::Nova => "/output/message/content/0/text",
            state::ModelFamily::Llama => "/generation",
            state::ModelFamily::Mistral => "/outputs/0/text",
            state::ModelFamily::CohereCommand => "/generations/0/text",
            state::ModelFamily::OpenAiChat => "/choices/0/message/content",
        };
        assert!(
            body.pointer(text_pointer)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.is_empty()),
            "case {model_id:?}: expected generated text at {text_pointer}, got {body}"
        );
        if family == state::ModelFamily::Nova {
            // Nova answers the Converse-shaped envelope, whose usage carries the
            // prompt-cache counters as well as the three token counts.
            let usage = &body["usage"];
            for member in [
                "inputTokens",
                "outputTokens",
                "totalTokens",
                "cacheReadInputTokenCount",
                "cacheWriteInputTokenCount",
            ] {
                assert!(
                    usage[member].is_i64(),
                    "case {model_id:?}: expected usage.{member} to be an integer, got {usage}"
                );
            }
            assert_eq!(
                usage["totalTokens"].as_i64(),
                Some(
                    usage["inputTokens"].as_i64().unwrap_or_default()
                        + usage["outputTokens"].as_i64().unwrap_or_default()
                ),
                "case {model_id:?}: totalTokens must be the sum of the two counts, got {usage}"
            );
            assert!(
                body["output"]["message"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {model_id:?}: expected output.message.content[0].text, got {body}"
            );
            assert_eq!(
                body["stopReason"].as_str(),
                Some("end_turn"),
                "case {model_id:?}: expected stopReason end_turn, got {body}"
            );
        }
    }
    assert!(
        served >= FAMILY_ROWS.len(),
        "case \"served models\": expected every family to be represented, served {served}"
    );
}

/// RULE: a model id the inventory carries for a modality this emulator computes
/// nothing for is a RECOGNISED identifier — so it is never reported as an
/// invalid one — and an invocation of it is refused with the modelled
/// `ModelErrorException` (424) rather than answered with a fabricated payload;
/// an operation that needs no model output (counting the input's tokens) still
/// succeeds for it.
#[tokio::test]
async fn a_model_this_emulator_runs_no_inference_for_is_refused_not_invented() {
    let router = emulator();
    for (case, model_id) in [
        ("embedding model", "amazon.titan-embed-text-v2:0"),
        ("embedding model, other provider", "cohere.embed-english-v3"),
        ("image model", "amazon.nova-canvas-v1:0"),
        ("video model", "amazon.nova-reel-v1:0"),
    ] {
        let encoded = encode_path(model_id);
        let call = send(
            &router,
            "POST",
            &format!("/model/{encoded}/invoke"),
            &[("content-type", "application/json")],
            br#"{"inputText":"a cat"}"#.to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            424,
            "case {case:?} {model_id:?}: expected 424, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ModelErrorException",
            "case {case:?} {model_id:?}: expected ModelErrorException, got {:?}",
            call.error_code()
        );
        assert_eq!(
            call.json()["resourceName"].as_str(),
            Some(model_id),
            "case {case:?} {model_id:?}: expected the refusal to name the model, got {}",
            call.json()
        );

        let counted = post_json(
            &router,
            &format!("/model/{encoded}/count-tokens"),
            &serde_json::json!({
                "input": { "invokeModel": { "body": base64(b"{\"inputText\":\"a cat\"}") } },
            }),
        )
        .await;
        // Counting the input's tokens needs no model output, but the count is
        // the MODEL's own tokenizer's: a model `CountTokens` is not published
        // for is refused the count rather than answered with another
        // tokenizer's, and the identifier is still a recognised one (400, never
        // the 400 for an unreadable identifier).
        assert_eq!(
            counted.status,
            400,
            "case {case:?} {model_id:?}: expected the count to be refused, got {} / {}",
            counted.status,
            String::from_utf8_lossy(&counted.body)
        );
        assert_eq!(
            counted.json()["message"].as_str(),
            Some("The provided model doesn't support counting tokens."),
            "case {case:?} {model_id:?}: expected the unsupported-count refusal, got {}",
            counted.json()
        );
    }
}

/// RULE: a request that breaks a rule the service states but the model cannot
/// express is refused with the modelled `ValidationException` (400) — for
/// `Converse`, a conversation with no turn to answer or a model the Converse API
/// does not serve; for `InvokeModel`, a body that is not `application/json`, an
/// empty body, and the guardrail combinations the operation documents as errors.
#[tokio::test]
async fn a_request_that_breaks_a_stated_rule_is_refused_with_validation_exception() {
    let router = emulator();
    let claude = "anthropic.claude-3-haiku-20240307-v1:0";

    let converse_rows: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "no messages at all",
            claude,
            serde_json::json!({ "messages": [] }),
        ),
        (
            "a model the Converse API does not serve",
            "cohere.command-text-v14",
            serde_json::json!({
                "messages": [{"role": "user", "content": [{"text": "hi"}]}],
            }),
        ),
    ];
    for (case, model_id, request) in converse_rows {
        let call = post_json(&router, &format!("/model/{model_id}/converse"), &request).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }

    let invoke_rows = vec![
        InvokeRefusalRow {
            case: "a non-JSON content type",
            headers: vec![("content-type", "text/plain")],
            body: br#"{"prompt":"hi"}"#.to_vec(),
        },
        InvokeRefusalRow {
            case: "an unsupported accept type",
            headers: vec![
                ("content-type", "application/json"),
                ("accept", "image/png"),
            ],
            body: br#"{"prompt":"hi"}"#.to_vec(),
        },
        InvokeRefusalRow {
            case: "an empty body",
            headers: vec![("content-type", "application/json")],
            body: Vec::new(),
        },
        InvokeRefusalRow {
            case: "a guardrail identifier without a version",
            headers: vec![
                ("content-type", "application/json"),
                ("x-amzn-bedrock-guardrailidentifier", "abc123"),
            ],
            body: br#"{"prompt":"hi"}"#.to_vec(),
        },
        InvokeRefusalRow {
            case: "an inline guardrail config without an identifier",
            headers: vec![("content-type", "application/json")],
            body: br#"{"prompt":"hi","amazon-bedrock-guardrailConfig":{"tagSuffix":"x"}}"#
                .to_vec(),
        },
    ];
    for row in invoke_rows {
        let case = row.case;
        let call = send(
            &router,
            "POST",
            &format!("/model/{claude}/invoke"),
            &row.headers,
            row.body,
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }

    // The same request with the guardrail version supplied is served.
    let served = send(
        &router,
        "POST",
        &format!("/model/{claude}/invoke"),
        &[
            ("content-type", "application/json"),
            ("x-amzn-bedrock-guardrailidentifier", "abc123"),
            ("x-amzn-bedrock-guardrailversion", "1"),
        ],
        CLAUDE_BODY.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "case \"a complete guardrail configuration\": expected 200, got {} / {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
}

/// One family's `InvokeModel` body rules: a body the family accepts, and a body
/// that omits a member its documented request schema requires together with the
/// key name the refusal must report.
///
/// Required members per family from "Inference request parameters and response
/// fields for foundation models" in the Amazon Bedrock User Guide. The rows
/// differ along the family, which is what the body schema generalises over, and
/// along which member is missing.
struct BodySchemaRow {
    case: &'static str,
    model_id: &'static str,
    accepted: &'static str,
    rejected: &'static str,
    missing_key: &'static str,
}

fn body_schema_rows() -> Vec<BodySchemaRow> {
    vec![
        BodySchemaRow {
            case: "Anthropic Claude without messages",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            accepted: CLAUDE_BODY,
            rejected: r#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":64}"#,
            missing_key: "messages",
        },
        BodySchemaRow {
            case: "Anthropic Claude without max_tokens",
            model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0",
            accepted: CLAUDE_BODY,
            rejected: r#"{"anthropic_version":"bedrock-2023-05-31","messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#,
            missing_key: "max_tokens",
        },
        BodySchemaRow {
            case: "Amazon Nova without messages",
            model_id: "amazon.nova-micro-v1:0",
            accepted: NOVA_BODY,
            rejected: r#"{"inferenceConfig":{"max_new_tokens":16}}"#,
            missing_key: "messages",
        },
        BodySchemaRow {
            case: "Amazon Titan Text without inputText",
            model_id: "amazon.titan-text-lite-v1",
            accepted: r#"{"inputText":"hi"}"#,
            rejected: r#"{"textGenerationConfig":{"maxTokenCount":16}}"#,
            missing_key: "inputText",
        },
        BodySchemaRow {
            case: "Meta Llama without prompt",
            model_id: "meta.llama3-2-3b-instruct-v1:0",
            accepted: LLAMA_BODY,
            rejected: r#"{"max_gen_len":16}"#,
            missing_key: "prompt",
        },
        BodySchemaRow {
            // Mistral takes either a raw prompt or a chat transcript, so a body
            // that binds the alternative member satisfies the same slot.
            case: "Mistral with neither prompt nor messages",
            model_id: "mistral.mistral-large-2407-v1:0",
            accepted: r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            rejected: r#"{"max_tokens":16}"#,
            missing_key: "prompt",
        },
        BodySchemaRow {
            case: "Cohere Command with neither prompt nor message",
            model_id: "cohere.command-r-plus-v1:0",
            accepted: r#"{"message":"hi"}"#,
            rejected: r#"{"max_tokens":16}"#,
            missing_key: "prompt",
        },
    ]
}

/// RULE: `InvokeModel`'s body is model-specific, so it is checked against the
/// invoked family's own documented request schema: a body that binds every
/// member the family requires is served, one that omits a required member is
/// refused with `ValidationException` (400) naming that member as the missing
/// key, and a body that is not a JSON object at all is refused as unreadable.
#[tokio::test]
async fn an_invoke_body_is_checked_against_the_invoked_family_request_schema() {
    let router = emulator();

    for row in body_schema_rows() {
        let case = row.case;
        let path = format!("/model/{}/invoke", encode_path(row.model_id));

        let served = send(
            &router,
            "POST",
            &path,
            &[("content-type", "application/json")],
            row.accepted.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: expected the family's own body to be served, got {} / {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );

        let refused = send(
            &router,
            "POST",
            &path,
            &[("content-type", "application/json")],
            row.rejected.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code()
        );
        let expected = format!(
            "Malformed input request: #: required key [{}] not found, please reformat your input \
             and try again.",
            row.missing_key
        );
        assert_eq!(
            refused.json()["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {:?}",
            refused.json()["message"]
        );
    }

    // A body that is not a request of the family at all: bytes whose JSON cannot be parsed
    // are a syntax fault in the document, and a document that parses but is not an object
    // is no request of any family. The two are DIFFERENT refusals, and the literal sentence
    // of each is pinned here rather than compared against the constant the handler itself
    // carries.
    for (case, body, expected) in [
        (
            "plain text",
            "not json at all",
            "The JSON object in the request body is malformed. Fix the syntax errors and \
             retry your request.",
        ),
        (
            "a truncated object",
            "{\"prompt\":",
            "The JSON object in the request body is malformed. Fix the syntax errors and \
             retry your request.",
        ),
        (
            "a JSON array",
            "[1,2,3]",
            "Malformed input request, please reformat your input and try again.",
        ),
        (
            "a bare JSON string",
            "\"hi\"",
            "Malformed input request, please reformat your input and try again.",
        ),
    ] {
        let call = send(
            &router,
            "POST",
            "/model/meta.llama3-8b-instruct-v1%3A0/invoke",
            &[("content-type", "application/json")],
            body.as_bytes().to_vec(),
        )
        .await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.json()["message"].as_str(),
            Some(expected),
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            call.json()["message"]
        );
    }
}

// ---------------------------------------------------------------------------
// The async-invoke lifecycle
// ---------------------------------------------------------------------------
/// RULE: `StartAsyncInvoke` persists one invocation record and `GetAsyncInvoke`
/// reports exactly that record; the operation is idempotent, so the same
/// `clientRequestToken` with the same payload names the invocation already
/// minted and with a different payload is a `ConflictException`; and an ARN this
/// emulator never minted is refused, never answered with a record.
#[tokio::test]
async fn start_async_invoke_persists_a_record_that_get_async_invoke_reports_back() {
    let router = emulator();
    // An asynchronously invocable model: `StartAsyncInvoke` is published for the
    // video generation models, and `modelInput` is checked against this one's
    // own request schema.
    let model_id = "luma.ray-v2:0";

    // Rows differ in token (present/absent), tags and output configuration.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "with a token and one tag",
            serde_json::json!({
                "modelId": model_id,
                "clientRequestToken": "tok-1",
                "modelInput": {"prompt": "render a kite", "duration": "9s"},
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/prefix"}},
                "tags": [{"key": "team", "value": "platform"}],
            }),
        ),
        (
            "without a token",
            serde_json::json!({
                "modelId": model_id,
                "modelInput": {"prompt": "render something else"},
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://other-bucket/out"}},
            }),
        ),
        (
            "with a bucket owner and a KMS key",
            serde_json::json!({
                "modelId": model_id,
                "clientRequestToken": "tok-3",
                "modelInput": {"prompt": "third", "resolution": "540p"},
                "outputDataConfig": {"s3OutputDataConfig": {
                    "s3Uri": "s3://bucket/third",
                    "bucketOwner": "000000000000",
                }},
            }),
        ),
    ];

    for (case, request) in &rows {
        let started = post_json(&router, "/async-invoke", request).await;
        assert_eq!(
            started.status,
            200,
            "case {case:?}: expected StartAsyncInvoke to answer 200, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let arn = started.json()["invocationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_invocation_arn(&arn),
            "case {case:?}: expected an invocationArn matching the modelled pattern, got {arn:?}"
        );

        let fetched = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: expected GetAsyncInvoke to answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        let record = fetched.json();
        assert_eq!(
            record["invocationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the stored invocationArn, got {:?}",
            record["invocationArn"]
        );
        assert_eq!(
            record["modelArn"].as_str(),
            Some(
                format!("arn:aws:bedrock:us-east-1:000000000000:foundation-model/{model_id}")
                    .as_str()
            ),
            "case {case:?}: expected the modelArn resolved from the submitted modelId, got {:?}",
            record["modelArn"]
        );
        assert_eq!(
            record["status"].as_str(),
            Some("InProgress"),
            "case {case:?}: expected a freshly submitted invocation to be InProgress, got {:?}",
            record["status"]
        );
        assert_eq!(
            record["outputDataConfig"],
            request["outputDataConfig"],
            "case {case:?}: expected the stored outputDataConfig, got {:?}",
            record["outputDataConfig"]
        );
        assert_eq!(
            record["clientRequestToken"].as_str(),
            request["clientRequestToken"].as_str(),
            "case {case:?}: expected the stored clientRequestToken, got {:?}",
            record["clientRequestToken"]
        );
        let submitted = instant_of(&record["submitTime"]).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a submitTime, got {:?}",
                record["submitTime"]
            )
        });
        assert!(
            (submitted - now_seconds()).abs() < 60.0,
            "case {case:?}: expected submitTime to be the instant of the call, got {submitted}"
        );
        assert!(
            record["failureMessage"].is_null(),
            "case {case:?}: expected no failureMessage while InProgress, got {:?}",
            record["failureMessage"]
        );
    }

    // Idempotency: the identical request under a used token returns the same ARN.
    let first = post_json(&router, "/async-invoke", &rows[0].1).await;
    let repeat = post_json(&router, "/async-invoke", &rows[0].1).await;
    assert_eq!(
        repeat.status, 200,
        "case \"idempotent repeat\": expected 200, got {} / {}",
        repeat.status,
        String::from_utf8_lossy(&repeat.body)
    );
    assert_eq!(
        repeat.json()["invocationArn"],
        first.json()["invocationArn"],
        "case \"idempotent repeat\": expected the same invocationArn, got {:?} and {:?}",
        first.json()["invocationArn"],
        repeat.json()["invocationArn"]
    );

    // The same token with a different payload conflicts.
    for (case, change) in [
        ("a different modelInput", serde_json::json!({"prompt": "different"})),
        ("a different output location", serde_json::Value::Null),
    ] {
        let mut conflicting = rows[0].1.clone();
        if change.is_null() {
            conflicting["outputDataConfig"] =
                serde_json::json!({"s3OutputDataConfig": {"s3Uri": "s3://bucket/elsewhere"}});
        } else {
            conflicting["modelInput"] = change;
        }
        let call = post_json(&router, "/async-invoke", &conflicting).await;
        assert_eq!(
            call.status,
            400,
            "case {case:?}: expected a 400 ConflictException, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            call.error_code()
        );
    }

    // A well-formed ARN this emulator never minted resolves to nothing.
    for unminted in [
        "arn:aws:bedrock:us-east-1:000000000000:async-invoke/abcdef123456",
        "arn:aws:bedrock:eu-west-1:000000000000:async-invoke/000000000000",
    ] {
        let call = get(&router, &format!("/async-invoke/{}", encode_path(unminted))).await;
        assert_eq!(
            call.status,
            400,
            "case {unminted:?}: expected the modelled 400 refusal, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {unminted:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }

    // An unrecognised model cannot be invoked asynchronously either: the
    // identifier is not one the service can interpret, so it is a 400.
    let unknown = post_json(
        &router,
        "/async-invoke",
        &serde_json::json!({
            "modelId": "nobody.such-model-v1",
            "modelInput": {"inputText": "x"},
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/x"}},
        }),
    )
    .await;
    assert_eq!(
        unknown.status, 400,
        "case \"unrecognised model\": expected 400, got {} / {}",
        unknown.status,
        String::from_utf8_lossy(&unknown.body)
    );
    assert_eq!(
        unknown.error_code(),
        "ValidationException",
        "case \"unrecognised model\": expected ValidationException, got {:?}",
        unknown.error_code()
    );
}

/// RULE: a submitted invocation leaves `InProgress` only when the clock makes
/// the transition due, and — because no model runs in this emulator and nothing
/// is written to the caller's S3 location — it ends in the modelled `Failed`
/// state carrying the reason, with `endTime` and `lastModifiedTime` set.
#[tokio::test]
async fn an_in_progress_invocation_settles_into_the_modelled_failure_state() {
    let router = emulator();
    let started = post_json(
        &router,
        "/async-invoke",
        &serde_json::json!({
            "modelId": "luma.ray-v2:0",
            "modelInput": {"prompt": "render"},
            "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/prefix"}},
        }),
    )
    .await;
    let arn = started.json()["invocationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let immediate = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    assert_eq!(
        immediate.json()["status"].as_str(),
        Some("InProgress"),
        "case \"read immediately\": expected InProgress, got {:?}",
        immediate.json()["status"]
    );

    // The transition is due a fixed span after the submitting call; nothing in
    // the emulator sleeps for it, so the test is what waits.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(state::ASYNC_INVOKE_SETTLE_MILLIS).unwrap_or(500) + 150,
    ));

    let settled = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    let record = settled.json();
    assert_eq!(
        record["status"].as_str(),
        Some("Failed"),
        "case \"read after the transition is due\": expected Failed, got {:?}",
        record["status"]
    );
    assert_eq!(
        record["failureMessage"].as_str(),
        Some(state::ASYNC_INVOKE_FAILURE_MESSAGE),
        "case \"read after the transition is due\": expected the recorded reason, got {:?}",
        record["failureMessage"]
    );
    let submitted = instant_of(&record["submitTime"]).unwrap_or_default();
    let ended = instant_of(&record["endTime"]).unwrap_or_else(|| {
        panic!(
            "case \"read after the transition is due\": expected an endTime, got {:?}",
            record["endTime"]
        )
    });
    assert!(
        ended >= submitted,
        "case \"read after the transition is due\": expected endTime {ended} at or after \
         submitTime {submitted}"
    );
    assert!(
        instant_of(&record["lastModifiedTime"]).is_some(),
        "case \"read after the transition is due\": expected a lastModifiedTime, got {:?}",
        record["lastModifiedTime"]
    );

    // Reading again reports the same terminal record: a read changes nothing a
    // later caller sees.
    let again = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
    assert_eq!(
        again.json(),
        record,
        "case \"read twice\": expected an identical record, got {:?}",
        again.json()
    );
}

/// RULE: `ListAsyncInvokes` reports the invocations `StartAsyncInvoke` created,
/// sorted by submit time in the requested order, cut into pages of at most
/// `maxResults` with a `nextToken` that resumes exactly after the last item of
/// the page it came with and is absent on the last page; every record appears
/// once across the pages.
#[tokio::test]
async fn list_async_invokes_pages_in_the_requested_order_without_gaps_or_duplicates() {
    let router = emulator();
    let mut created = Vec::new();
    for index in 0..5 {
        let started = post_json(
            &router,
            "/async-invoke",
            &serde_json::json!({
                "modelId": "luma.ray-v2:0",
                "clientRequestToken": format!("tok-{index}"),
                "modelInput": {"prompt": format!("job {index}")},
                "outputDataConfig": {"s3OutputDataConfig": {
                    "s3Uri": format!("s3://bucket/job-{index}"),
                }},
            }),
        )
        .await;
        assert_eq!(
            started.status,
            200,
            "case \"create job {index}\": expected 200, got {} / {}",
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        created.push(
            started.json()["invocationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        // Distinct submit times, so the ordering assertions below are about the
        // sort key rather than about the tie-break.
        std::thread::sleep(Duration::from_millis(5));
    }

    // Every page size, including one page per record and one page for all of
    // them, must enumerate the same records in the same order.
    let mut per_page_size = Vec::new();
    for page_size in [1_usize, 2, 3, 5, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let call = get(&router, &format!("/async-invoke{query}")).await;
            assert_eq!(
                call.status,
                200,
                "case page_size={page_size}: expected 200, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let body = call.json();
            let summaries = body["asyncInvokeSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case page_size={page_size}: no summaries in {body}"))
                .clone();
            assert!(
                summaries.len() <= page_size,
                "case page_size={page_size}: expected at most {page_size} summaries, got {}",
                summaries.len()
            );
            for summary in &summaries {
                seen.push(
                    summary["invocationArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case page_size={page_size}: paging did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            created.len(),
            "case page_size={page_size}: expected every record exactly once, got {seen:?}"
        );
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case page_size={page_size}: expected no duplicates across pages, got {seen:?}"
        );
        per_page_size.push(seen);
    }
    for (index, enumeration) in per_page_size.iter().enumerate() {
        assert_eq!(
            enumeration, &per_page_size[0],
            "case page_size row {index}: expected paging to enumerate the same order as a \
             single page, got {enumeration:?}"
        );
    }

    // Descending (the modelled default) is newest-first; Ascending is its
    // reverse.
    let descending = listed_arns(&router, "?sortOrder=Descending").await;
    let defaulted = listed_arns(&router, "").await;
    let ascending = listed_arns(&router, "?sortOrder=Ascending").await;
    assert_eq!(
        defaulted, descending,
        "case \"defaulted sortOrder\": expected the modelled default Descending, got {defaulted:?}"
    );
    assert_eq!(
        descending,
        created.iter().rev().cloned().collect::<Vec<_>>(),
        "case \"Descending\": expected newest-first, got {descending:?}"
    );
    assert_eq!(
        ascending, created,
        "case \"Ascending\": expected oldest-first, got {ascending:?}"
    );

    // An opaque token this emulator did not mint is refused.
    for bad_token in ["not-a-token", "6162"] {
        let call = get(&router, &format!("/async-invoke?nextToken={bad_token}")).await;
        assert_eq!(
            call.status,
            400,
            "case {bad_token:?}: expected 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {bad_token:?}: expected ValidationException, got {:?}",
            call.error_code()
        );
    }
}

/// RULE: `ListAsyncInvokes` applies its filters to the records it holds: a
/// status filter keeps the records in that status, and the submit-time bounds
/// keep the records submitted within them.
#[tokio::test]
async fn list_async_invokes_applies_its_status_and_submit_time_filters() {
    let router = emulator();
    let mut arns = Vec::new();
    let mut submitted = Vec::new();
    for index in 0..3 {
        let started = post_json(
            &router,
            "/async-invoke",
            &serde_json::json!({
                "modelId": "amazon.nova-reel-v1:0",
                "modelInput": {
                    "taskType": "TEXT_VIDEO",
                    "textToVideoParams": {"text": format!("job {index}")},
                    "videoGenerationConfig": {"durationSeconds": 6, "fps": 24},
                },
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://bucket/out"}},
            }),
        )
        .await;
        let arn = started.json()["invocationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let fetched = get(&router, &format!("/async-invoke/{}", encode_path(&arn))).await;
        submitted.push(
            instant_of(&fetched.json()["submitTime"])
                .expect("GetAsyncInvoke reports a submitTime"),
        );
        arns.push(arn);
        std::thread::sleep(Duration::from_millis(5));
    }

    // While the transition is not yet due every record is InProgress, and no
    // record is in any other status.
    for (status, expected) in [("InProgress", 3_usize), ("Completed", 0), ("Failed", 0)] {
        let listed = listed_arns(&router, &format!("?statusEquals={status}")).await;
        assert_eq!(
            listed.len(),
            expected,
            "case statusEquals={status}: expected {expected} record(s), got {listed:?}"
        );
    }

    // The submit-time bounds keep the records submitted inside them. The bounds
    // are set between two records' submit instants, so what is asserted is the
    // filter and not the formatting precision of one boundary value.
    let between_first_two = rfc3339_query(midpoint(submitted[0], submitted[1]));
    let after = listed_arns(
        &router,
        &format!("?submitTimeAfter={between_first_two}&sortOrder=Ascending"),
    )
    .await;
    assert_eq!(
        after,
        arns[1..].to_vec(),
        "case submitTimeAfter=between the first two: expected the later records, got {after:?}"
    );
    let between_last_two = rfc3339_query(midpoint(submitted[1], submitted[2]));
    let before = listed_arns(
        &router,
        &format!("?submitTimeBefore={between_last_two}&sortOrder=Ascending"),
    )
    .await;
    assert_eq!(
        before,
        arns[..2].to_vec(),
        "case submitTimeBefore=between the last two: expected the earlier records, got \
         {before:?}"
    );
    let window = listed_arns(
        &router,
        &format!(
            "?submitTimeAfter={between_first_two}&submitTimeBefore={between_last_two}\
             &sortOrder=Ascending"
        ),
    )
    .await;
    assert_eq!(
        window,
        arns[1..2].to_vec(),
        "case \"both bounds\": expected only the middle record, got {window:?}"
    );

    // Once the clock has made the transition due, the status filter follows the
    // records into their terminal state.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(state::ASYNC_INVOKE_SETTLE_MILLIS).unwrap_or(500) + 150,
    ));
    let failed = listed_arns(&router, "?statusEquals=Failed").await;
    assert_eq!(
        failed.len(),
        3,
        "case \"statusEquals=Failed after the transition is due\": expected 3 records, got {failed:?}"
    );
    let still_running = listed_arns(&router, "?statusEquals=InProgress").await;
    assert!(
        still_running.is_empty(),
        "case \"statusEquals=InProgress after the transition is due\": expected none, got \
         {still_running:?}"
    );
}

/// RULE: the tokenizer every operation counts with is a function of the text
/// alone: counting is monotonic in the text, cutting a text to N tokens yields
/// at most N tokens, and a text already shorter than the cut is returned whole.
#[tokio::test]
async fn the_tokenizer_counts_and_cuts_consistently() {
    let cases = [
        "",
        "a",
        "hello",
        "hello world",
        "tokens, punctuation; and symbols!",
        "s3://bucket/prefix",
        "日本語のトークン",
        "a very long sentence that keeps going and going and going for quite a while",
    ];
    for text in cases {
        let total = state::count_text_tokens(text);
        for cut in [0_u32, 1, 2, 3, 7, 50] {
            let truncated = state::truncate_to_tokens(text, cut);
            let truncated_tokens = state::count_text_tokens(&truncated);
            assert!(
                truncated_tokens <= cut.min(total),
                "case {text:?} cut to {cut}: expected at most {} tokens, got {truncated_tokens} \
                 for {truncated:?}",
                cut.min(total)
            );
            if cut >= total {
                assert_eq!(
                    truncated, text,
                    "case {text:?} cut to {cut}: expected the whole text back, got {truncated:?}"
                );
            }
            assert!(
                text.starts_with(truncated.trim_end()),
                "case {text:?} cut to {cut}: expected a prefix of the text, got {truncated:?}"
            );
        }
        let longer = format!("{text} plus more words here");
        assert!(
            state::count_text_tokens(&longer) >= total,
            "case {text:?}: expected appending text not to lower the count, got {} then {}",
            total,
            state::count_text_tokens(&longer)
        );
    }
}

/// RULE: the async-invoke store is bounded: past its retention limit the OLDEST
/// records are evicted, together with the idempotency-index entries that point
/// at them, so a caller cannot grow the store without limit.
#[tokio::test]
async fn the_async_invoke_store_evicts_its_oldest_records_past_the_retention_bound() {
    let mut store = ServiceState::default();
    let config = AsyncInvokeOutputDataConfig::s3OutputDataConfig(AsyncInvokeS3OutputDataConfig {
        s3_uri: "s3://bucket/out".to_string(),
        kms_key_id: None,
        bucket_owner: None,
    });
    let overflow = 5;
    let total = state::MAX_ASYNC_INVOKE_RECORDS + overflow;
    for index in 0..total {
        let invocation_arn =
            format!("arn:aws:bedrock:us-east-1:000000000000:async-invoke/{index:012}");
        store.insert_async_invoke(state::AsyncInvokeRecord {
            invocation_arn: invocation_arn.clone(),
            model_id: "amazon.nova-lite-v1:0".to_string(),
            model_arn: "arn:aws:bedrock:us-east-1:000000000000:foundation-model/amazon.nova-lite-v1:0"
                .to_string(),
            client_request_token: Some(format!("tok-{index}")),
            status: state::AsyncInvokeStatus::InProgress,
            submit_time: 1_000 + index as i64,
            last_modified_time: None,
            end_time: None,
            failure_message: None,
            output_data_config: config.clone(),
            model_input: aws_smithy_types::Document::Null,
            tags: Vec::new(),
            settles_at: 1_000 + index as i64 + state::ASYNC_INVOKE_SETTLE_MILLIS,
            request_fingerprint: format!("fingerprint-{index}"),
        });
        let kept = store.async_invokes.len();
        assert!(
            kept <= state::MAX_ASYNC_INVOKE_RECORDS,
            "case after {} insert(s): expected at most {} records, got {kept}",
            index + 1,
            state::MAX_ASYNC_INVOKE_RECORDS
        );
    }
    assert_eq!(
        store.async_invokes.len(),
        state::MAX_ASYNC_INVOKE_RECORDS,
        "case \"store filled past the bound\": expected the bound to hold, got {}",
        store.async_invokes.len()
    );
    assert_eq!(
        store.idempotency_index.len(),
        state::MAX_ASYNC_INVOKE_RECORDS,
        "case \"store filled past the bound\": expected the index to shrink with the store, got {}",
        store.idempotency_index.len()
    );
    for index in 0..overflow {
        let evicted = format!("arn:aws:bedrock:us-east-1:000000000000:async-invoke/{index:012}");
        assert!(
            !store.async_invokes.contains_key(&evicted),
            "case \"oldest record {index}\": expected it to be evicted, it is still stored"
        );
        assert!(
            !store.idempotency_index.contains_key(&format!("tok-{index}")),
            "case \"oldest record {index}\": expected its idempotency entry to be evicted too"
        );
    }
    let newest = format!(
        "arn:aws:bedrock:us-east-1:000000000000:async-invoke/{:012}",
        total - 1
    );
    assert!(
        store.async_invokes.contains_key(&newest),
        "case \"newest record\": expected eviction to keep it, got {:?}",
        store.async_invokes.keys().next_back()
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The `invocationArn` values a `ListAsyncInvokes` query reports, in order.
async fn listed_arns(router: &MultiServiceRouter, query: &str) -> Vec<String> {
    let call = get(router, &format!("/async-invoke{query}")).await;
    assert_eq!(
        call.status,
        200,
        "case {query:?}: expected ListAsyncInvokes to answer 200, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()["asyncInvokeSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["invocationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Whether `value` matches the modelled `InvocationArn` pattern
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:async-invoke/[a-z0-9]{12}$`,
/// checked field by field rather than with a regex dependency.
pub(crate) fn is_invocation_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.split(':').collect();
    let [arn, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let Some(id) = resource.strip_prefix("async-invoke/") else {
        return false;
    };
    *arn == "arn"
        && (*partition == "aws" || partition.starts_with("aws-"))
        && *service == "bedrock"
        && !region.is_empty()
        && region.len() <= 20
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && id.len() == 12
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Percent-encode the characters an ARN carries that cannot sit in a path
/// segment unescaped. Only `:` and `/` occur in the ARNs these tests build; a
/// future ARN shape with other reserved characters would need more.
pub(crate) fn encode_path(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// A modelled timestamp as epoch seconds, however the protocol rendered it: the
/// restJson1 default (epoch seconds) or an RFC3339 string. `None` when the
/// member is absent.
fn instant_of(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
        .map(|parsed| parsed.as_secs_f64()),
        _ => None,
    }
}

/// Now, in epoch seconds.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The instant halfway between two instants.
fn midpoint(left: f64, right: f64) -> f64 {
    (left + right) / 2.0
}

/// An instant as the `@httpQuery` timestamp value a date-time member takes,
/// percent-encoded for a query string.
fn rfc3339_query(seconds: f64) -> String {
    aws_smithy_types::DateTime::from_secs_f64(seconds)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable timestamp")
        .replace(':', "%3A")
}

/// Base64 of `bytes`, the JSON encoding of a Smithy blob member. The codec is
/// the one the wire protocol itself uses, from `aws-smithy-types`.
fn base64(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}
