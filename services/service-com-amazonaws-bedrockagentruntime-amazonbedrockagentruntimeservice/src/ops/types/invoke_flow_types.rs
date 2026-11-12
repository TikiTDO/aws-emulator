// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `invoke_flow` operation, HAND-AUTHORED: the
//! op-stub generator declined `InvokeFlow` because its `responseStream` output member is a
//! `@streaming` event stream it cannot frame (`../SKIPPED_STUBS.md`), and an operation with
//! no types and no handler is in no routing table at all — so no request ever reached a
//! bedrock-agent-runtime handler on `POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}`.
//!
//! The split follows the one the other hand-authored streaming surfaces of this service use
//! (`optimize_prompt_types.rs`): the members the framework's own codec can read are
//! declared, so this operation's `@required`, `@length` and `@pattern` members — including
//! the two URI labels — are checked against the MODEL rather than by hand, and the event
//! stream sits in `unsupported = [..]`, which keeps it out of the schema. The EVENTS the
//! stream carries are declared here, because the handler frames them itself; the modelled
//! exceptions the same stream can carry are the shapes `shared_types` already declares.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `InvokeFlowRequest`: the two URI labels that name the flow and its alias, and the one
/// input the flow is entered with (`inputs` is `@length(1, 1)`).
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#InvokeFlowRequest")]
#[derive(Debug, Clone, Default)]
pub struct InvokeFlowInput {
    pub flow_identifier: String,
    pub flow_alias_identifier: String,
    pub inputs: Vec<FlowInput>,
    pub enable_trace: Option<bool>,
    pub model_performance_configuration: Option<ModelPerformanceConfiguration>,
    pub execution_id: Option<String>,
}

/// `InvokeFlowResponse`: `responseStream` is the `@streaming` `@httpPayload`, which the
/// handler frames itself, so the declared half carries the one modelled response header.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#InvokeFlowResponse",
    unsupported = ["responseStream"]
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeFlowOutput {
    pub execution_id: Option<String>,
}

/// One input into a flow: the input node it enters at, and the document it carries.
///
/// Declared in `start_flow_execution_types`, which reaches the same three shapes
/// (`FlowInput`, `FlowInputContent`, `ModelPerformanceConfiguration`) — each shape is
/// declared exactly once across this service's `types/` modules, so this operation reads
/// them from there rather than redeclaring them.
/// The `flowOutputEvent` member of the response stream: one value that left the flow.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#FlowOutputEvent")]
#[derive(Debug, Clone)]
pub struct FlowOutputEvent {
    pub node_name: String,
    pub node_type: String,
    pub content: FlowOutputContent,
}

/// The content of an output from a flow.
#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentruntime#FlowOutputContent")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum FlowOutputContent {
    Document(::aws_smithy_types::Document),
}

/// The `flowCompletionEvent` member of the response stream: why the flow completed.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#FlowCompletionEvent")]
#[derive(Debug, Clone, Default)]
pub struct FlowCompletionEvent {
    pub completion_reason: String,
}
