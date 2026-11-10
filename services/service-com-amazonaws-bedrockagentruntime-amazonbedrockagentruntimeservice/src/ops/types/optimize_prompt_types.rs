// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `optimize_prompt` operation, HAND-AUTHORED: the
//! op-stub generator declined `OptimizePrompt` because its output carries a `@streaming`
//! event-stream member it cannot frame (`../SKIPPED_STUBS.md`), and an operation with no
//! types and no handler is not in the service's routing table at all — so nothing served
//! `POST /optimize-prompt`.
//!
//! The split follows the one the other hand-authored streaming surface of this service
//! uses (`invoke_inline_agent_types.rs`): the members the framework's own codec can read
//! are declared, so this operation's `@required`, `@length` and `@pattern` members are
//! checked against the MODEL rather than by hand, and the event stream sits in
//! `unsupported = [..]`, which keeps it out of the schema. The EVENTS the stream carries
//! are declared here too, because the handler frames them itself.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `OptimizePromptRequest`: the prompt to optimize and the model to optimize it for. Both
/// members are `@required`, so the framework refuses a request missing either before the
/// handler runs.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#OptimizePromptRequest")]
#[derive(Debug, Clone)]
pub struct OptimizePromptInput {
    pub input: InputPrompt,
    pub target_model_id: String,
}

/// `OptimizePromptResponse`: `optimizedPrompt` is the `@streaming` `@httpPayload`, which
/// the handler frames itself, so the declared half of the output carries no member at all
/// — the stream IS the response, and it binds no modelled header.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#OptimizePromptResponse",
    unsupported = ["optimizedPrompt"]
)]
#[derive(Debug, Clone, Default)]
pub struct OptimizePromptOutput {}

/// The prompt a request asks to have optimized.
#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentruntime#InputPrompt")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum InputPrompt {
    textPrompt(TextPrompt),
}

/// The prompt an optimization answers with.
#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentruntime#OptimizedPrompt")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum OptimizedPrompt {
    textPrompt(TextPrompt),
}

/// One text prompt, on the way in and on the way out.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#TextPrompt")]
#[derive(Debug, Clone, Default)]
pub struct TextPrompt {
    pub text: String,
}

/// The `optimizedPromptEvent` member of the response stream.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#OptimizedPromptEvent")]
#[derive(Debug, Clone, Default)]
pub struct OptimizedPromptEvent {
    pub optimized_prompt: Option<OptimizedPrompt>,
}

/// The `analyzePromptEvent` member of the response stream.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AnalyzePromptEvent")]
#[derive(Debug, Clone, Default)]
pub struct AnalyzePromptEvent {
    pub message: Option<String>,
}
