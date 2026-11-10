// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `OptimizePrompt` (`POST /optimize-prompt`).
//!
//! THE RULES UNDER TEST:
//!
//! * a request naming a model this emulator publishes is SERVED, as an
//!   `application/vnd.amazon.eventstream` body carrying the two events the modelled
//!   `OptimizedPromptStream` declares for a run that completes — `analyzePromptEvent` then
//!   `optimizedPromptEvent` — whose optimized prompt is non-empty and names the model the
//!   prompt was optimized for. No language model runs in this emulator, so that text says
//!   so and is never the submitted prompt handed back as if it had been rewritten;
//! * a `targetModelId` naming no model this emulator serves is refused with the modelled
//!   `ValidationException` (HTTP 400), which is the only client fault this operation
//!   declares for a model reference it cannot use — it models no
//!   `ResourceNotFoundException`;
//! * a request that breaks the model's own constraints is refused the same way, before any
//!   of the stream is framed.
//!
//! The tables span what those rules generalise over: the form of the model reference (a
//! bare model id, a `foundation-model` ARN, a cross-region inference-profile id), the
//! provider line the model belongs to, the length of the submitted prompt, and the KIND of
//! unusable reference (an id in no catalogue, a model past its end of life, an ARN of an
//! account-scoped resource this emulator holds none of).

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};

use crate::agent_memory::{Emulator, Reply, ACCOUNT_KEY, REGION};

/// The content type of an event-stream body, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The two events a completed optimization carries, in the order it carries them.
const EXPECTED_EVENTS: [&str; 2] = ["analyzePromptEvent", "optimizedPromptEvent"];

impl Emulator {
    /// One `OptimizePrompt` call.
    async fn optimize(&self, key: &str, body: Value) -> Reply {
        self.bedrock(key, "POST", "/optimize-prompt", Some(body)).await
    }
}

/// A request body optimizing `prompt` for `target_model_id`.
fn request(prompt: &str, target_model_id: &str) -> Value {
    json!({
        "input": { "textPrompt": { "text": prompt } },
        "targetModelId": target_model_id,
    })
}

/// Decode an event-stream response body, asserting the frame headers an SDK decoder
/// requires. Each entry is `(event type, payload)`.
fn decode_frames(case: &str, body: &[u8]) -> Vec<(String, Value)> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":message-type").as_deref(),
            Some("event"),
            "case {case:?}: a response frame must be an event, got {:?}",
            header(":message-type")
        );
        let event_type = header(":event-type")
            .unwrap_or_else(|| panic!("case {case:?}: a response frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case:?}: the {event_type} payload must be JSON: {error}")
        });
        frames.push((event_type, payload));
    }
    frames
}

/// RULE: a prompt optimization for a model this emulator publishes is served as the two
/// modelled events, carrying a non-empty optimized prompt that names the model and is not
/// the submitted prompt.
#[tokio::test]
async fn an_optimization_streams_an_analysis_and_a_non_empty_optimized_prompt() {
    /// One request, and the model id the answer must name.
    struct Row {
        case: &'static str,
        /// The reference the request carries.
        target_model_id: String,
        /// The model the catalogue resolves it to, which the answer names.
        resolved: &'static str,
        prompt: String,
    }

    let rows = [
        Row {
            case: "a bare model id",
            target_model_id: "amazon.nova-lite-v1:0".to_string(),
            resolved: "amazon.nova-lite-v1:0",
            prompt: "summarise the attached report".to_string(),
        },
        Row {
            case: "a bare model id of another provider line",
            target_model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            resolved: "anthropic.claude-3-5-sonnet-20241022-v2:0",
            prompt: "write a haiku".to_string(),
        },
        Row {
            case: "a foundation-model ARN",
            target_model_id: format!(
                "arn:aws:bedrock:{REGION}::foundation-model/meta.llama3-8b-instruct-v1:0"
            ),
            resolved: "meta.llama3-8b-instruct-v1:0",
            prompt: "translate the following".to_string(),
        },
        Row {
            case: "a long prompt",
            target_model_id: "amazon.nova-micro-v1:0".to_string(),
            resolved: "amazon.nova-micro-v1:0",
            prompt: "optimise this. ".repeat(500),
        },
        Row {
            case: "a one-character prompt, the shortest the model admits",
            target_model_id: "amazon.nova-micro-v1:0".to_string(),
            resolved: "amazon.nova-micro-v1:0",
            prompt: "?".to_string(),
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let reply = emulator
            .optimize(ACCOUNT_KEY, request(&row.prompt, &row.target_model_id))
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected 200, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type, EVENT_STREAM,
            "case {:?}: expected an event-stream body, got {:?}",
            row.case, reply.content_type
        );
        let frames = decode_frames(row.case, &reply.body);
        let order: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            order, EXPECTED_EVENTS,
            "case {:?}: expected the two modelled events in order",
            row.case
        );
        let analysis = frames[0].1["message"].as_str().unwrap_or_default();
        assert!(
            !analysis.is_empty() && analysis.contains(row.resolved),
            "case {:?}: the analysis must name the model, got {analysis:?}",
            row.case
        );
        let optimized = frames[1].1["optimizedPrompt"]["textPrompt"]["text"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the optimized event must carry a text prompt, got {:?}",
                    row.case, frames[1].1
                )
            });
        assert!(
            !optimized.is_empty(),
            "case {:?}: the optimized prompt must not be empty",
            row.case
        );
        assert!(
            optimized.contains(row.resolved),
            "case {:?}: the optimized prompt must name the model it was optimized for, got \
             {optimized:?}",
            row.case
        );
        assert!(
            !optimized.contains(row.prompt.trim()),
            "case {:?}: the optimized prompt must not be the submitted prompt handed back, \
             got {optimized:?}",
            row.case
        );
    }
}

/// RULE: a `targetModelId` naming no model this emulator serves is refused with
/// `ValidationException` (400) — whatever KIND of reference it is — and never served with a
/// stream.
#[tokio::test]
async fn an_unusable_target_model_is_refused_with_a_validation_exception() {
    /// One model reference this emulator cannot optimize a prompt for, and the sentence
    /// AWS refuses it with where that sentence is known.
    struct Row {
        case: &'static str,
        target_model_id: String,
        message: Option<&'static str>,
    }

    /// The sentence a reference this service cannot read as a model identifier earns: the
    /// sentence the model documents for `ValidationException`, then what was wrong.
    const UNREADABLE: &str = "Input validation failed. Check your request parameters and \
                              retry the request. Invalid model ID: The provided model \
                              identifier is invalid.";

    /// The sentence an ARN of another region or partition earns.
    const WRONG_PLACE: &str = "Invalid ARN format: Incorrect region/partition.";

    let rows = [
        Row {
            case: "a bare id in no catalogue",
            target_model_id: "no-such-model".to_string(),
            message: Some(UNREADABLE),
        },
        Row {
            case: "a model past its end of life",
            // Source: the Bedrock Runtime catalogue's end-of-life rows ("Model lifecycle").
            target_model_id: "anthropic.claude-v2".to_string(),
            message: None,
        },
        Row {
            case: "a foundation-model ARN naming no published model",
            target_model_id: format!(
                "arn:aws:bedrock:{REGION}::foundation-model/acme.not-a-model-v1:0"
            ),
            message: None,
        },
        Row {
            case: "an ARN of an account-scoped resource this emulator holds none of",
            target_model_id: format!(
                "arn:aws:bedrock:{REGION}:000000000000:provisioned-model/abcdefghijkl"
            ),
            message: Some(UNREADABLE),
        },
        Row {
            case: "a cross-region inference-profile id, which names no base model",
            target_model_id: "us.amazon.nova-pro-v1:0".to_string(),
            message: Some(UNREADABLE),
        },
        Row {
            case: "a model ARN carrying an account, which a foundation-model ARN has none of",
            target_model_id: format!(
                "arn:aws:bedrock:{REGION}:000000000000:foundation-model/amazon.nova-lite-v1:0"
            ),
            message: Some(UNREADABLE),
        },
        Row {
            case: "a model ARN of another region than the request was signed for",
            target_model_id:
                "arn:aws:bedrock:eu-central-1::foundation-model/amazon.nova-lite-v1:0".to_string(),
            message: Some(WRONG_PLACE),
        },
        Row {
            case: "a model ARN of another partition",
            target_model_id:
                "arn:aws-cn:bedrock:cn-north-1::foundation-model/amazon.nova-lite-v1:0"
                    .to_string(),
            message: Some(WRONG_PLACE),
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let reply = emulator
            .optimize(ACCOUNT_KEY, request("optimise this", &row.target_model_id))
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected 400, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(row.case),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            reply.json(row.case)
        );
        assert!(
            !reply.message(row.case).is_empty(),
            "case {:?}: the refusal must state why the model cannot be used",
            row.case
        );
        if let Some(expected) = row.message {
            assert_eq!(
                reply.message(row.case),
                expected,
                "case {:?}: expected the service's sentence for this class, got {:?}",
                row.case,
                reply.message(row.case)
            );
        }
    }
}

/// RULE: a request that breaks the model's own constraints is refused with
/// `ValidationException` (400) before any of the stream is framed.
#[tokio::test]
async fn a_request_that_breaks_the_model_is_refused_before_the_stream() {
    /// One malformed request, and the member the refusal has to name.
    struct Row {
        case: &'static str,
        body: Value,
        names: &'static str,
    }

    let rows = [
        Row {
            case: "no targetModelId",
            body: json!({ "input": { "textPrompt": { "text": "optimise this" } } }),
            names: "targetModelId",
        },
        Row {
            case: "no input",
            body: json!({ "targetModelId": "amazon.nova-lite-v1:0" }),
            names: "input",
        },
        Row {
            case: "a text prompt below the modelled minimum length",
            body: request("", "amazon.nova-lite-v1:0"),
            names: "text",
        },
        Row {
            case: "an empty targetModelId",
            body: request("optimise this", ""),
            names: "targetModelId",
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let reply = emulator.optimize(ACCOUNT_KEY, row.body.clone()).await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected 400, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(row.case),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            reply.json(row.case)
        );
        assert!(
            reply.message(row.case).contains(row.names),
            "case {:?}: the refusal must name {:?}, got {:?}",
            row.case,
            row.names,
            reply.message(row.case)
        );
        assert_ne!(
            reply.content_type, EVENT_STREAM,
            "case {:?}: a refused request must not answer an event stream",
            row.case
        );
    }
}
