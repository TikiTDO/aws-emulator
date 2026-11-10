// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-ENFORCED guardrail, asserted where a client sees it: on the wire,
//! through every one of this service's inference surfaces.
//!
//! # The rule
//!
//! A guardrail the account enforces — installed through the Bedrock control plane
//! and held in this service's store
//! (`state::enforced_guardrail::EnforcedGuardrail`) — is applied SERVER-SIDE to
//! every invocation of a model its model enforcement covers, whatever the request
//! itself asked for: the request's input is evaluated against the guardrail's word
//! policy before generation, the invocation reports the action the guardrail took,
//! and an intervention answers with the guardrail's configured blocked messaging
//! instead of a completion. The invoke surfaces report the action in the
//! service-injected body member `amazon-bedrock-guardrailAction`
//! (`INTERVENED` / `NONE`); the Converse surfaces report it in the modelled
//! `StopReason` `guardrail_intervened`.
//!
//! An invocation NO enforced configuration covers is untouched: no action member,
//! and the turn the model would have served.
//!
//! # What the tables below generalise over
//!
//! The rows differ along the invocation surface (`InvokeModel`,
//! `InvokeModelWithResponseStream`, `Converse`, `ConverseStream`), the model and
//! its response family and request envelope, where the blocked word sits in the
//! request (the prompt, a system prompt, a later turn, a `guardContent` block),
//! the form of the configured word (one word, a phrase, a differently cased
//! occurrence), the side of the policy that blocks (input and output), the
//! selective-guarding mode of each part of the request, and which models the
//! configuration's model enforcement covers. The recorded case — one `InvokeModel`
//! call on a model named in `includedModels`, with a banned word in the prompt —
//! is ONE row.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state;
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::enforced_guardrail::{
    BlockedWord, EnforcedGuardrail, ManagedWordList, ACTION_INTERVENED, ACTION_NONE, ALL_MODELS,
    GUARDRAIL_ACTION_FIELD, STOP_REASON_GUARDRAIL_INTERVENED,
};
use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::bedrock_runtime_inference::post_json;
use crate::bedrock_runtime_streaming::{chunk_payloads, decode_frames};

/// What the enforced guardrail answers an input it blocked with, and what it
/// answers a generated response it blocked with: the two `blockedInputMessaging` /
/// `blockedOutputsMessaging` strings a guardrail is created with.
const BLOCKED_INPUT: &str = "This request was blocked by the account guardrail.";
const BLOCKED_OUTPUT: &str = "This response was blocked by the account guardrail.";

/// Which surface a row drives the inference through.
enum Surface {
    /// `POST /model/{id}/invoke` with the family's own body, and the JSON pointer
    /// its generated text is reported at.
    InvokeModel {
        body: Value,
        text_pointer: &'static str,
    },
    /// The same request as an event stream.
    InvokeStream { body: Value },
    /// `POST /model/{id}/converse`, with the request's system blocks and messages.
    Converse { system: Value, messages: Value },
    /// The same request as an event stream.
    ConverseStream { system: Value, messages: Value },
}

/// One row: a case name, the model to invoke, the surface to invoke it through,
/// the enforcement installed on the account, and the action the invocation must
/// report.
struct Row {
    case: &'static str,
    model_id: &'static str,
    surface: Surface,
    enforced: EnforcedGuardrail,
    /// The guardrail action the invocation must report, or `None` for an
    /// invocation no enforced configuration covers.
    expected: Option<&'static str>,
    /// The messaging the response must carry when the guardrail intervened.
    messaging: Option<&'static str>,
}

/// The enforcement a row installs: a word policy over `words`, enforced on
/// `included` less `excluded`.
fn enforcement(words: Vec<BlockedWord>, included: &[&str], excluded: &[&str]) -> EnforcedGuardrail {
    EnforcedGuardrail {
        guardrail_id: "gr0000000001".to_string(),
        guardrail_version: "1".to_string(),
        blocked_input_messaging: BLOCKED_INPUT.to_string(),
        blocked_outputs_messaging: BLOCKED_OUTPUT.to_string(),
        words,
        managed_word_lists: Vec::new(),
        content_filters: Vec::new(),
        included_models: included.iter().map(|id| (*id).to_string()).collect(),
        excluded_models: excluded.iter().map(|id| (*id).to_string()).collect(),
        system_guarded_selectively: false,
        messages_guarded_selectively: false,
    }
}

/// A word the policy blocks both ways — a policy entry naming no action.
fn word(text: &str) -> BlockedWord {
    BlockedWord::new(text.to_string(), None, None, None, None)
}

/// A word the policy blocks on the OUTPUT side only.
fn output_word(text: &str) -> BlockedWord {
    BlockedWord::new(text.to_string(), Some("NONE"), None, Some("BLOCK"), None)
}

/// The same enforcement with one or both parts of the request guarded
/// SELECTIVELY — `selectiveContentGuarding.system` / `.messages` set to
/// `SELECTIVE`, so only the content the request MARKED for guarding is evaluated.
fn selectively(mut enforced: EnforcedGuardrail, system: bool, messages: bool) -> EnforcedGuardrail {
    enforced.system_guarded_selectively = system;
    enforced.messages_guarded_selectively = messages;
    enforced
}

/// A `guardContent` block carrying `text`: the marking a selective guarding mode
/// narrows to (`GuardrailConverseContentBlock$text`).
fn guard_content(text: &str) -> Value {
    json!({ "guardContent": { "text": { "text": text } } })
}

/// The per-run banned token a row puts in its request: a word nothing else in a
/// request or a completion carries.
const BANNED: &str = "zephyrine";

/// A word that is in the text this emulator SERVES, read off its own completion
/// floor rather than written out here, so the output-side row cannot drift from
/// what the service generates.
fn generated_word() -> &'static str {
    state::EMULATED_COMPLETION_FLOOR
        .split_whitespace()
        .nth(1)
        .expect("the completion floor is a sentence of several words")
}

/// The table.
///
/// Model ids are from "Amazon Bedrock base model IDs (on-demand throughput)" and
/// the request/response envelopes from "Inference request parameters and response
/// fields for foundation models", both in the Amazon Bedrock User Guide.
fn rows() -> Vec<Row> {
    let banned_prompt = format!("Tell me about {BANNED} in one line.");
    vec![
        // The recorded case: an InvokeModel call naming no guardrail, on a model
        // the configuration includes by name, whose prompt carries the word.
        Row {
            case: "InvokeModel, Nova envelope, the banned word in the prompt",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::InvokeModel {
                body: json!({"messages": [{"role": "user", "content": [{"text": banned_prompt}]}]}),
                text_pointer: "/output/message/content/0/text",
            },
            enforced: enforcement(vec![word(BANNED)], &["amazon.nova-lite-v1:0"], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "InvokeModel, Nova envelope, a benign prompt under the same configuration",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::InvokeModel {
                body: json!({"messages": [{"role": "user", "content": [{"text": "Name two planets."}]}]}),
                text_pointer: "/output/message/content/0/text",
            },
            enforced: enforcement(vec![word(BANNED)], &["amazon.nova-lite-v1:0"], &[]),
            expected: Some(ACTION_NONE),
            messaging: None,
        },
        Row {
            case: "InvokeModel, Titan Text envelope, the wildcard enforcement, capitalised word",
            model_id: "amazon.titan-text-premier-v1:0",
            surface: Surface::InvokeModel {
                body: json!({
                    "inputText": format!("Write a haiku about {}.", BANNED.to_uppercase()),
                    "textGenerationConfig": {"maxTokenCount": 512},
                }),
                text_pointer: "/results/0/outputText",
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "InvokeModel, Llama envelope, a configured PHRASE in the prompt",
            model_id: "meta.llama3-8b-instruct-v1:0",
            surface: Surface::InvokeModel {
                body: json!({
                    "prompt": "Pass me the Red Pencil, please.",
                    "max_gen_len": 128,
                }),
                text_pointer: "/generation",
            },
            enforced: enforcement(vec![word("red pencil")], &[], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "InvokeModel, Anthropic envelope, a model the configuration excludes",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::InvokeModel {
                body: json!({
                    "anthropic_version": "bedrock-2023-05-31",
                    "max_tokens": 256,
                    "messages": [{"role": "user", "content": [{"type": "text", "text": format!("What is {BANNED}?")}]}],
                }),
                text_pointer: "/content/0/text",
            },
            enforced: enforcement(
                vec![word(BANNED)],
                &[ALL_MODELS],
                &["anthropic.claude-3-haiku-20240307-v1:0"],
            ),
            expected: None,
            messaging: None,
        },
        Row {
            case: "InvokeModel, Anthropic envelope, a word the policy blocks on the output side",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::InvokeModel {
                body: json!({
                    "anthropic_version": "bedrock-2023-05-31",
                    "max_tokens": 256,
                    "messages": [{"role": "user", "content": [{"type": "text", "text": "Summarize the Mars mission."}]}],
                }),
                text_pointer: "/content/0/text",
            },
            enforced: enforcement(vec![output_word(generated_word())], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_OUTPUT),
        },
        Row {
            case: "InvokeModelWithResponseStream, Nova envelope, the banned word in the prompt",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::InvokeStream {
                body: json!({"messages": [{"role": "user", "content": [{"text": format!("Describe {BANNED}.")}]}]}),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "InvokeModelWithResponseStream, Anthropic envelope, a benign prompt",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::InvokeStream {
                body: json!({
                    "anthropic_version": "bedrock-2023-05-31",
                    "max_tokens": 256,
                    "messages": [{"role": "user", "content": [{"type": "text", "text": "Name one moon."}]}],
                }),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_NONE),
            messaging: None,
        },
        Row {
            case: "Converse, the banned word in the only turn",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([]),
                messages: json!([{"role": "user", "content": [{"text": format!("Explain {BANNED}.")}]}]),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "Converse, the banned word in the SYSTEM prompt",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([{"text": format!("You are {BANNED}.")}]),
                messages: json!([{"role": "user", "content": [{"text": "Name two planets."}]}]),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "Converse, the banned word in a LATER turn of the conversation",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::Converse {
                system: json!([]),
                messages: json!([
                    {"role": "user", "content": [{"text": "Name a colour."}]},
                    {"role": "assistant", "content": [{"text": "Blue."}]},
                    {"role": "user", "content": [{"text": format!("And {BANNED}?")}]},
                ]),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "Converse, a benign conversation under the same configuration",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([]),
                messages: json!([{"role": "user", "content": [{"text": "Name two planets."}]}]),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_NONE),
            messaging: None,
        },
        Row {
            case: "Converse, a model the configuration does not include",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([]),
                messages: json!([{"role": "user", "content": [{"text": format!("Explain {BANNED}.")}]}]),
            },
            enforced: enforcement(vec![word(BANNED)], &["amazon.nova-lite-v1:0"], &[]),
            expected: None,
            messaging: None,
        },
        Row {
            case: "ConverseStream, the banned word in the only turn",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::ConverseStream {
                system: json!([]),
                messages: json!([{"role": "user", "content": [{"text": format!("Describe {BANNED}.")}]}]),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "ConverseStream, a benign turn under the same configuration",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::ConverseStream {
                system: json!([]),
                messages: json!([{"role": "user", "content": [{"text": "Name one moon."}]}]),
            },
            enforced: enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
            expected: Some(ACTION_NONE),
            messaging: None,
        },
        // Selective guarding: the mode narrows the evaluation to the content the
        // request MARKED for guarding, per part of the request.
        Row {
            case: "Converse, messages guarded selectively, the word in a marked block",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([]),
                messages: json!([{
                    "role": "user",
                    "content": [guard_content(&format!("Explain {BANNED}."))],
                }]),
            },
            enforced: selectively(
                enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
                false,
                true,
            ),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "Converse, messages guarded selectively, the word in UNMARKED text",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([]),
                messages: json!([{
                    "role": "user",
                    "content": [
                        {"text": format!("Explain {BANNED}.")},
                        guard_content("And name two planets."),
                    ],
                }]),
            },
            enforced: selectively(
                enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
                false,
                true,
            ),
            expected: Some(ACTION_NONE),
            messaging: None,
        },
        Row {
            case: "Converse, the system prompt guarded selectively, the word in a marked block",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([guard_content(&format!("You are {BANNED}."))]),
                messages: json!([{"role": "user", "content": [{"text": "Name two planets."}]}]),
            },
            enforced: selectively(
                enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
                true,
                false,
            ),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            case: "Converse, the system prompt guarded selectively, the word in UNMARKED system \
                   text",
            model_id: "amazon.nova-pro-v1:0",
            surface: Surface::Converse {
                system: json!([{"text": format!("You are {BANNED}.")}]),
                messages: json!([{"role": "user", "content": [{"text": "Name two planets."}]}]),
            },
            enforced: selectively(
                enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
                true,
                false,
            ),
            expected: Some(ACTION_NONE),
            messaging: None,
        },
        Row {
            case: "Converse, both parts guarded selectively, the word marked in the system part",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            surface: Surface::Converse {
                system: json!([guard_content(&format!("Never mention {BANNED}."))]),
                messages: json!([{"role": "user", "content": [{"text": "Name one moon."}]}]),
            },
            enforced: selectively(
                enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
                true,
                true,
            ),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
        Row {
            // The invoke surface carries no `guardContent` marking at all, so a
            // selective mode has nothing to narrow to and the prompt is evaluated
            // whole: an account control is not quietly not applied.
            case: "InvokeModel under a selective messages mode, the word in the prompt",
            model_id: "amazon.nova-lite-v1:0",
            surface: Surface::InvokeModel {
                body: json!({"messages": [{"role": "user", "content": [{"text": format!("About {BANNED}?")}]}]}),
                text_pointer: "/output/message/content/0/text",
            },
            enforced: selectively(
                enforcement(vec![word(BANNED)], &[ALL_MODELS], &[]),
                true,
                true,
            ),
            expected: Some(ACTION_INTERVENED),
            messaging: Some(BLOCKED_INPUT),
        },
    ]
}

/// The emulator under test: the production router with this service mounted on a
/// shared engine, plus the engine itself so a row can install the enforcement the
/// control plane would have installed.
fn emulator_with_engine() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// RULE: every invocation surface applies the account-enforced guardrail to a
/// request that asked for no guardrail, reports the action it took, and answers an
/// intervention with the guardrail's blocked messaging.
#[tokio::test]
async fn every_invocation_surface_applies_the_account_enforced_guardrail() {
    for row in rows() {
        let case = row.case;
        let (engine, router) = emulator_with_engine();
        assert!(
            state::enforced_guardrail::install(&*engine, Some(row.enforced.clone())),
            "case {case:?}: the enforcement must be installable on the engine the service holds"
        );
        let model_id = row.model_id;

        match &row.surface {
            Surface::InvokeModel { body, text_pointer } => {
                let call = post_json(&router, &format!("/model/{model_id}/invoke"), body).await;
                assert_eq!(
                    call.status,
                    200,
                    "case {case:?}: the invocation must still be served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                let answered = call.json();
                let action = answered
                    .get(GUARDRAIL_ACTION_FIELD)
                    .and_then(Value::as_str);
                assert_eq!(
                    action, row.expected,
                    "case {case:?}: expected the guardrail action {:?}, got {action:?} in \
                     {answered}",
                    row.expected
                );
                let text = answered
                    .pointer(text_pointer)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no generated text at {text_pointer} in {answered}")
                    });
                match row.messaging {
                    Some(messaging) => assert_eq!(
                        text, messaging,
                        "case {case:?}: a blocked invocation must answer the guardrail's \
                         messaging, got {text:?}"
                    ),
                    None => assert!(
                        !text.contains(BANNED) && text != BLOCKED_INPUT && text != BLOCKED_OUTPUT,
                        "case {case:?}: an invocation the guardrail passed must answer the \
                         model's own turn, got {text:?}"
                    ),
                }
            }
            Surface::InvokeStream { body } => {
                let call = post_json(
                    &router,
                    &format!("/model/{model_id}/invoke-with-response-stream"),
                    body,
                )
                .await;
                assert_eq!(
                    call.status,
                    200,
                    "case {case:?}: the invocation must still be served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                // The service's own members travel on the TERMINAL chunk, beside
                // the invocation metrics.
                let chunks = chunk_payloads(case, &call.body);
                let last = chunks
                    .last()
                    .unwrap_or_else(|| panic!("case {case:?}: the stream carried no chunk"));
                let action = last.get(GUARDRAIL_ACTION_FIELD).and_then(Value::as_str);
                assert_eq!(
                    action, row.expected,
                    "case {case:?}: expected the guardrail action {:?} on the terminal chunk, got \
                     {action:?} in {last}",
                    row.expected
                );
                let streamed = serde_json::to_string(&chunks).expect("the chunks render");
                match row.messaging {
                    Some(messaging) => assert!(
                        streamed.contains(messaging),
                        "case {case:?}: a blocked invocation must stream the guardrail's \
                         messaging, got {streamed}"
                    ),
                    None => assert!(
                        !streamed.contains(BANNED) && !streamed.contains(BLOCKED_INPUT),
                        "case {case:?}: an invocation the guardrail passed must stream the \
                         model's own turn, got {streamed}"
                    ),
                }
            }
            Surface::Converse { system, messages } => {
                let answered = converse(&router, model_id, system, messages, case).await;
                let stop_reason = answered.get("stopReason").and_then(Value::as_str);
                let text = answered
                    .pointer("/output/message/content/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the turn carried no text: {answered}")
                    });
                assert_guarded_turn(case, row.expected, row.messaging, stop_reason, text);
            }
            Surface::ConverseStream { system, messages } => {
                let call = post_json(
                    &router,
                    &format!("/model/{model_id}/converse-stream"),
                    &converse_body(system, messages),
                )
                .await;
                assert_eq!(
                    call.status,
                    200,
                    "case {case:?}: ConverseStream must still be served, got {} / {}",
                    call.status,
                    String::from_utf8_lossy(&call.body)
                );
                let frames = decode_frames(case, &call.body);
                let event = |name: &str| {
                    frames
                        .iter()
                        .find(|(event_type, _)| event_type == name)
                        .map(|(_, payload)| payload.clone())
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: the stream carried no {name} event: {frames:?}")
                        })
                };
                let stop = event("messageStop");
                let stop_reason = stop.get("stopReason").and_then(Value::as_str);
                // The block arrives as the increments a client appends, so the
                // turn's text is the deltas reassembled.
                let text: String = frames
                    .iter()
                    .filter(|(event_type, _)| event_type == "contentBlockDelta")
                    .map(|(_, payload)| {
                        payload
                            .pointer("/delta/text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert!(
                    !text.is_empty(),
                    "case {case:?}: the stream delivered no text: {frames:?}"
                );
                assert_guarded_turn(case, row.expected, row.messaging, stop_reason, &text);
            }
        }
    }
}

/// The Converse-API half of the rule: a blocked turn ends on
/// `guardrail_intervened` and carries the guardrail's messaging; a turn the
/// guardrail passed — and one no configuration covers — ends on `end_turn` and
/// carries the model's own text.
fn assert_guarded_turn(
    case: &str,
    expected: Option<&str>,
    messaging: Option<&str>,
    stop_reason: Option<&str>,
    text: &str,
) {
    let intervened = expected == Some(ACTION_INTERVENED);
    let expected_stop = if intervened {
        STOP_REASON_GUARDRAIL_INTERVENED
    } else {
        "end_turn"
    };
    assert_eq!(
        stop_reason,
        Some(expected_stop),
        "case {case:?}: expected the turn to stop on {expected_stop:?}, got {stop_reason:?}"
    );
    match messaging {
        Some(messaging) => assert_eq!(
            text, messaging,
            "case {case:?}: a blocked turn must carry the guardrail's messaging, got {text:?}"
        ),
        None => assert!(
            !text.contains(BANNED) && text != BLOCKED_INPUT && text != BLOCKED_OUTPUT,
            "case {case:?}: a turn the guardrail passed must carry the model's own text, got \
             {text:?}"
        ),
    }
}

/// A `Converse` body with the row's system blocks, omitted when it has none.
fn converse_body(system: &Value, messages: &Value) -> Value {
    let mut body = json!({"messages": messages});
    if system.as_array().is_some_and(|blocks| !blocks.is_empty()) {
        body["system"] = system.clone();
    }
    body
}

/// One `Converse` call, requiring a 200.
async fn converse(
    router: &MultiServiceRouter,
    model_id: &str,
    system: &Value,
    messages: &Value,
    case: &str,
) -> Value {
    let call = post_json(
        router,
        &format!("/model/{model_id}/converse"),
        &converse_body(system, messages),
    )
    .await;
    assert_eq!(
        call.status,
        200,
        "case {case:?}: Converse must still be served, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    call.json()
}

/// RULE (the other side of it): with NO configuration installed, an invocation
/// carries no guardrail action at all — enforcement is something the account
/// installed, not a default — and withdrawing an installed configuration restores
/// that.
#[tokio::test]
async fn an_account_that_enforces_nothing_reports_no_guardrail_action() {
    let (engine, router) = emulator_with_engine();
    let body = json!({"messages": [{"role": "user", "content": [{"text": format!("Tell me about {BANNED}.")}]}]});
    let path = "/model/amazon.nova-lite-v1:0/invoke";

    let unguarded = post_json(&router, path, &body).await.json();
    assert_eq!(
        unguarded.get(GUARDRAIL_ACTION_FIELD),
        None,
        "case unconfigured: an unguarded invocation carries no action member, got {unguarded}"
    );

    state::enforced_guardrail::install(
        &*engine,
        Some(enforcement(vec![word(BANNED)], &[ALL_MODELS], &[])),
    );
    let guarded = post_json(&router, path, &body).await.json();
    assert_eq!(
        guarded.get(GUARDRAIL_ACTION_FIELD).and_then(Value::as_str),
        Some(ACTION_INTERVENED),
        "case installed: the installed configuration must be applied, got {guarded}"
    );

    state::enforced_guardrail::install(&*engine, None);
    let withdrawn = post_json(&router, path, &body).await.json();
    assert_eq!(
        withdrawn.get(GUARDRAIL_ACTION_FIELD),
        None,
        "case withdrawn: an account that enforces nothing again carries no action member, got \
         {withdrawn}"
    );
}

/// RULE: the word policy's MANAGED word list is enforced the same way its
/// configured words are — a prompt the named list places something in is blocked,
/// and one it places nothing in is passed.
#[tokio::test]
async fn a_managed_word_list_is_enforced_like_a_configured_word() {
    // (case, prompt, blocked). The managed list AWS publishes is the profanity
    // list (`GuardrailManagedWordsType` -> [PROFANITY]).
    let rows: [(&str, &str, bool); 3] = [
        ("profanity in the prompt", "you are a fucking idiot", true),
        ("profanity in a question", "why the shit does this fail", true),
        ("nothing the list places", "please summarise this report", false),
    ];
    for (case, prompt, blocked) in rows {
        let (engine, router) = emulator_with_engine();
        let mut enforced = enforcement(Vec::new(), &[ALL_MODELS], &[]);
        enforced.managed_word_lists = vec![ManagedWordList::new(
            "PROFANITY".to_string(),
            None,
            None,
            Some("NONE"),
            None,
        )];
        state::enforced_guardrail::install(&*engine, Some(enforced));

        let answered = post_json(
            &router,
            "/model/amazon.nova-lite-v1:0/invoke",
            &json!({"messages": [{"role": "user", "content": [{"text": prompt}]}]}),
        )
        .await
        .json();
        let expected = if blocked { ACTION_INTERVENED } else { ACTION_NONE };
        assert_eq!(
            answered
                .get(GUARDRAIL_ACTION_FIELD)
                .and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected the guardrail action {expected}, got {answered}"
        );
    }
}
