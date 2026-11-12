// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `retrieve_and_generate_stream` operation,
//! HAND-AUTHORED: the op-stub generator declined `RetrieveAndGenerateStream` because its
//! output carries a `@streaming` event-stream member it cannot frame
//! (`../SKIPPED_STUBS.md`), and an operation with no types and no handler is not in the
//! service's routing table at all — so nothing served `POST /retrieveAndGenerateStream`.
//!
//! The split follows the one the other hand-authored streaming surfaces of this service use
//! (`optimize_prompt_types.rs`): the members the framework's own codec can read are
//! declared, so this operation's `@required`, `@length` and `@pattern` members are checked
//! against the MODEL rather than by hand, and the event stream sits in `unsupported = [..]`,
//! which keeps it out of the schema. The EVENTS the stream carries are declared here,
//! because the handler frames them itself.
//!
//! Every shape of the REQUEST is the one `RetrieveAndGenerate` already reads
//! (`retrieve_and_generate_types.rs`): the two operations take the same query, the same
//! configuration and the same session, which is why only the request STRUCT itself is
//! declared here.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// `RetrieveAndGenerateStreamRequest`: the same query, configuration and session
/// `RetrieveAndGenerate` takes.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#RetrieveAndGenerateStreamRequest"
)]
#[derive(Debug, Clone, Default)]
pub struct RetrieveAndGenerateStreamInput {
    pub session_id: Option<String>,
    pub input: RetrieveAndGenerateInput2,
    pub retrieve_and_generate_configuration: Option<RetrieveAndGenerateConfiguration>,
    pub session_configuration: Option<RetrieveAndGenerateSessionConfiguration>,
    pub user_context: Option<UserContext>,
}

/// `RetrieveAndGenerateStreamResponse`: `stream` is the `@streaming` `@httpPayload`, which
/// the handler frames itself, so the declared half carries only the one modelled response
/// header (`sessionId`, bound to `x-amzn-bedrock-knowledge-base-session-id`).
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentruntime#RetrieveAndGenerateStreamResponse",
    unsupported = ["stream"]
)]
#[derive(Debug, Clone, Default)]
pub struct RetrieveAndGenerateStreamOutput {
    pub session_id: String,
}

/// The `output` member of the response stream: one segment of the generated response.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#RetrieveAndGenerateOutputEvent")]
#[derive(Debug, Clone, Default)]
pub struct RetrieveAndGenerateOutputEvent {
    pub text: String,
}

/// The `citation` member of the response stream: one cited passage of that response.
///
/// `citation` itself is `@deprecated` in favour of `generatedResponsePart` and
/// `retrievedReferences`, so a served stream carries the two replacements; the deprecated
/// member is declared because a client reading it must find the same citation there.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentruntime#CitationEvent")]
#[derive(Debug, Clone, Default)]
pub struct CitationEvent {
    pub citation: Option<Citation>,
    pub generated_response_part: Option<GeneratedResponsePart>,
    pub retrieved_references: Option<Vec<RetrievedReference>>,
}
