// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_agent` operation, HAND-AUTHORED: the
//! op-stub generator declined `InvokeAgent` because its `completion` output member is a
//! `@streaming` event stream it cannot frame (`../SKIPPED_STUBS.md`), and an operation with
//! no types and no handler is in no routing table at all — so no request ever reached a
//! bedrock-agent-runtime handler on
//! `POST /agents/{agentId}/agentAliases/{agentAliasId}/sessions/{sessionId}/text`.
//!
//! The split follows the one the other hand-authored streaming surfaces of this service use
//! (`optimize_prompt_types.rs`): the members the framework's own codec can read are
//! declared, so this operation's `@required`, `@length` and `@pattern` members — including
//! the three URI labels — are checked against the MODEL rather than by hand, and the event
//! stream sits in `unsupported = [..]`, which keeps it out of the schema. The EVENTS the
//! stream carries are declared here, because the handler frames them itself.
//!
//! `sessionState` is declared with every member of it unsupported: the handler only has to
//! see THAT a request carries one, because what a session state does — replace prompt
//! placeholders, return an action group's results, carry files into a code interpreter — is
//! work nothing in this emulator performs, so a request carrying one is refused rather than
//! served with the member ignored.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `InvokeAgentRequest`: the three URI labels that name the agent alias and the session,
/// plus the body members an invocation carries.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#InvokeAgentRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeAgentInput {
    pub session_state: Option<SessionState>,
    pub agent_id: String,
    pub agent_alias_id: String,
    pub session_id: String,
    pub end_session: Option<bool>,
    pub enable_trace: Option<bool>,
    pub input_text: Option<String>,
    pub memory_id: Option<String>,
    pub bedrock_model_configurations: Option<BedrockModelConfigurations>,
    pub streaming_configurations: Option<StreamingConfigurations>,
    pub prompt_creation_configurations: Option<PromptCreationConfigurations>,
    pub source_arn: Option<String>,
}

/// `InvokeAgentResponse`: `completion` is the `@streaming` `@httpPayload`, which the
/// handler frames itself, so the declared half carries the three modelled response headers.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InvokeAgentResponse",
    unsupported = ["completion"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeAgentOutput {
    pub content_type: String,
    pub session_id: String,
    pub memory_id: Option<String>,
}

/// `SessionState`, declared for its PRESENCE alone: see the module docs for why every
/// member of it is unsupported.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#SessionState",
    unsupported = [
        "sessionAttributes",
        "promptSessionAttributes",
        "returnControlInvocationResults",
        "invocationId",
        "files",
        "knowledgeBaseConfigurations",
        "conversationHistory"
    ]
)]
#[derive(Debug, Clone, Default)]
pub struct SessionState {}

/// `BedrockModelConfigurations`: the settings for the model the agent calls. Its one member
/// names a latency profile, which does not change what an answer says.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#BedrockModelConfigurations")]
#[derive(Debug, Clone, Default)]
pub struct BedrockModelConfigurations {
    pub performance_config: Option<PerformanceConfiguration>,
}

/// `StreamingConfigurations`: whether the final response is streamed, and how often a
/// guardrail is applied while it is generated.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#StreamingConfigurations")]
#[derive(Debug, Clone, Default)]
pub struct StreamingConfigurations {
    pub stream_final_response: bool,
    pub apply_guardrail_interval: Option<i32>,
}

/// `PromptCreationConfigurations`: which of the session's earlier turns the agent prompt is
/// populated from.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#PromptCreationConfigurations")]
#[derive(Debug, Clone, Default)]
pub struct PromptCreationConfigurations {
    pub previous_conversation_turns_to_include: Option<i32>,
    pub exclude_previous_thinking_steps: bool,
}

/// The `chunk` member of the response stream: one part of the agent's answer, and the
/// citations for it.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#PayloadPart")]
#[derive(Debug, Clone, Default)]
pub struct PayloadPart {
    pub bytes: Option<Vec<u8>>,
    pub attribution: Option<Attribution>,
}

/// The citations for one part of an agent response.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#Attribution")]
#[derive(Debug, Clone, Default)]
pub struct Attribution {
    pub citations: Option<Vec<Citation>>,
}

/// `ModelNotReadyException` (HTTP 424): the model the invocation would call is not ready to
/// serve inference requests. Declared here because the agent invocation surface is the only
/// operation of this service that models it.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#ModelNotReadyException")]
#[derive(Debug, Clone, Default)]
pub struct ModelNotReadyException {
    pub message: Option<String>,
}
