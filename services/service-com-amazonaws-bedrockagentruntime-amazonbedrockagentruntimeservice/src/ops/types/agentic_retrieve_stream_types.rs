// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `agentic_retrieve_stream` operation,
//! HAND-AUTHORED: the op-stub generator declined `AgenticRetrieveStream` because its
//! `stream` output member is a `@streaming` event stream it cannot frame
//! (`../SKIPPED_STUBS.md`), and an operation with no types and no handler is not in the
//! service's routing table at all — so nothing served `POST /agenticRetrieveStream`.
//!
//! The split follows the one the other hand-authored streaming surfaces of this service use
//! (`optimize_prompt_types.rs`): the members the framework's own codec can read are
//! declared, so this operation's `@required`, `@length`, `@pattern` and ENUM members — the
//! `agenticRetrieveConfiguration.foundationModelType` value set among them — are checked
//! against the MODEL rather than by hand, and the event stream sits in
//! `unsupported = [..]`, which keeps it out of the schema. The EVENTS the stream carries
//! are declared here, because the handler frames them itself.
//!
//! Every configuration the handler has to REFUSE is declared rather than left unsupported:
//! a member the schema drops is one the handler cannot see, and a request asking for work
//! this emulator performs none of has to be refused rather than served with the member
//! ignored. Where only the PRESENCE of such a configuration decides the refusal, its deeper
//! members are unsupported — that is what each `unsupported = [..]` list below is.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `AgenticRetrieveStreamRequest`: the conversation, the retrievers it is run against and
/// the configuration of the retrieval.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveStreamRequest")]
#[derive(Debug, Clone)]
pub struct AgenticRetrieveStreamInput {
    pub messages: Vec<AgenticRetrieveMessage>,
    pub retrievers: Vec<AgenticRetriever>,
    pub agentic_retrieve_configuration: AgenticRetrieveConfiguration,
    pub policy_configuration: Option<AgenticRetrievePolicyConfiguration>,
    pub next_token: Option<String>,
    pub user_context: Option<UserContext>,
    pub memory_configuration: Option<AgenticRetrieveMemoryConfiguration>,
    pub retrieval_instruction_configuration: Option<AgenticRetrieveInstructionConfiguration>,
    pub generate_response: bool,
}

/// `AgenticRetrieveStreamResponse`: `stream` is the `@streaming` `@httpPayload`, which the
/// handler frames itself, so the declared half carries no member at all — the stream IS the
/// response, and it binds no modelled header.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveStreamResponse",
    unsupported = ["stream"]
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveStreamOutput {}

/// One message of the conversation a retrieval is run for.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveMessage")]
#[derive(Debug, Clone)]
pub struct AgenticRetrieveMessage {
    pub content: AgenticRetrieveMessageContent,
    pub role: String,
}

/// The content of one such message.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveMessageContent")]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveMessageContent {
    pub text: Option<String>,
}

/// One retriever a retrieval reads.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetriever")]
#[derive(Debug, Clone)]
pub struct AgenticRetriever {
    pub description: Option<String>,
    pub configuration: RetrieverConfiguration,
}

/// What a retriever reads; the model declares one kind.
#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentruntime#RetrieverConfiguration")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RetrieverConfiguration {
    knowledgeBase(KnowledgeBaseRetrieverConfiguration),
}

/// The knowledge base one retriever reads, and what the request overrides about the read.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#KnowledgeBaseRetrieverConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct KnowledgeBaseRetrieverConfiguration {
    pub knowledge_base_id: String,
    pub retrieval_overrides: Option<RetrievalOverrides>,
}

/// What a retriever's read overrides: the filter every reported chunk has to satisfy, and
/// how many chunks it may report.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#RetrievalOverrides")]
#[derive(Debug, Clone, Default)]
pub struct RetrievalOverrides {
    pub filter: Option<RetrievalFilter>,
    pub max_number_of_results: Option<i32>,
}

/// The configuration of an agentic retrieval. `foundationModelType` and `maxAgentIteration`
/// carry the model's own `default`, so a request omitting either is served with it.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveConfiguration")]
#[derive(Debug, Clone)]
pub struct AgenticRetrieveConfiguration {
    pub foundation_model_type: String,
    pub foundation_model_configuration: Option<FoundationModelConfiguration>,
    pub reranking_model_type: Option<String>,
    pub reranking_configuration: Option<AgenticRetrieveRerankingConfiguration>,
    pub max_agent_iteration: i32,
}

/// The foundation model a `CUSTOM` retrieval names. The Mantle leg is unsupported: `type`
/// already names which leg the request carries, and a Mantle model is not one this emulator
/// publishes.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#FoundationModelConfiguration",
    unsupported = ["mantleFoundationModelConfiguration"]
)]
#[derive(Debug, Clone, Default)]
pub struct FoundationModelConfiguration {
    pub r#type: String,
    pub bedrock_foundation_model_configuration: Option<BedrockFoundationModelConfiguration>,
}

/// The Bedrock leg of that configuration.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#BedrockFoundationModelConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct BedrockFoundationModelConfiguration {
    pub model_configuration: BedrockFoundationModelModelConfiguration,
}

/// The model that leg names.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#BedrockFoundationModelModelConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct BedrockFoundationModelModelConfiguration {
    pub model_arn: String,
}

/// The reranking model a retrieval names, declared for its PRESENCE: this service runs no
/// reranking model, so a request naming one is refused and the model it names is never read.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveRerankingConfiguration",
    unsupported = ["bedrockRerankingConfiguration"]
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveRerankingConfiguration {
    pub r#type: String,
}

/// The policies a retrieval is run under.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrievePolicyConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrievePolicyConfiguration {
    pub bedrock_guardrail_configuration: Option<AgenticRetrieveBedrockGuardrailConfiguration>,
}

/// The guardrail such a policy names.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveBedrockGuardrailConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveBedrockGuardrailConfiguration {
    pub guardrail_id: String,
    pub guardrail_version: String,
}

/// The AgentCore memory a retrieval reads, declared for its PRESENCE: this emulator holds
/// no such memory, so a request naming one is refused and its namespaces are never read.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveMemoryConfiguration",
    unsupported = ["sessionBinding", "retrievalConfigs"]
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveMemoryConfiguration {
    pub memory_id: String,
    pub persistence_mode: Option<String>,
}

/// The retrieval instruction a request applies.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveInstructionConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveInstructionConfiguration {
    pub retrieval_instruction_id: String,
    pub version: Option<String>,
}

/// The `result` member of the response stream: what the retrieval found, and the answer
/// generated from it when the request asked for one.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveResultEvent")]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveResultEvent {
    pub results: Vec<AgenticRetrieveResultItem>,
    pub generated_response: Option<AgenticRetrieveGeneratedResponse>,
    pub next_token: Option<String>,
}

/// One retrieved item: its content, the metadata recorded with it, and the retriever it
/// came from.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveResultItem")]
#[derive(Debug, Clone)]
pub struct AgenticRetrieveResultItem {
    pub content: RetrievalContent,
    pub metadata: Option<std::collections::HashMap<String, ::aws_smithy_types::Document>>,
    pub source_retriever: AgenticRetrieveSourceRetriever,
}

/// The content of one retrieved item.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#RetrievalContent")]
#[derive(Debug, Clone, Default)]
pub struct RetrievalContent {
    pub byte_content: Option<Vec<u8>>,
    pub text: Option<String>,
    pub mime_type: String,
}

/// The retriever one item came from.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveSourceRetriever")]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveSourceRetriever {
    pub identifier: String,
}

/// The answer generated from what the retrieval found, and the items it cites.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveGeneratedResponse"
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveGeneratedResponse {
    pub answer: String,
    pub citations: Option<Vec<AgenticRetrieveCitation>>,
}

/// One citation of that answer: the span of it a retrieved item was quoted into, and which
/// item that is.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveCitation")]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveCitation {
    pub start_index: i32,
    pub end_index: i32,
    pub references: Vec<AgenticRetrieveCitationReference>,
}

/// The retrieved item one citation points at, by its position in `results`.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveCitationReference"
)]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveCitationReference {
    pub result_index: i32,
}

/// The `responseEvent` member of the response stream: the generated answer, emitted only
/// when the request asked for one.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#AgenticRetrieveResponseEvent")]
#[derive(Debug, Clone, Default)]
pub struct AgenticRetrieveResponseEvent {
    pub text: String,
}
