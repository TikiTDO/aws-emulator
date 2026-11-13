// SPDX-License-Identifier: CC0-1.0

//! The AI-prompt family of this service's state: the record an AI prompt is persisted as,
//! the projection every operation that reports one answers out of, and the lookups that
//! reach it inside its assistant.
//!
//! An AI prompt lives INSIDE the assistant it was created under — every modelled route is
//! `/assistants/{assistantId}/aiprompts/…` — so the record lives on [`Assistant`], the
//! way an AI agent's and an AI guardrail's do: an id of one assistant's prompt does not
//! resolve under another, and the prompt goes when its assistant goes. Nothing is
//! computed twice: a create writes one record and every read of the prompt renders that
//! record, so a create's answer and a later `GetAIPrompt` cannot disagree.

use std::collections::HashMap;
use std::time::SystemTime;

use super::*;
use crate::ops::types::{AIPromptData, AIPromptInferenceConfiguration, AIPromptTemplateConfiguration};

/// The fixed text for a request addressed to an AI prompt its named assistant does not
/// hold. See [`MESSAGE_ASSISTANT_NOT_FOUND`] for why the sentence quotes nothing from the
/// request, and [`MESSAGE_AI_GUARDRAIL_NOT_FOUND`] for the same sentence on the sibling
/// family, with the resource token spelled as the API itself names the resource.
pub const MESSAGE_AI_PROMPT_NOT_FOUND: &str = "AI Prompt does not exist.";

/// The most AI prompts one assistant may hold at a time. A create beyond it is refused
/// with the modelled `ServiceQuotaExceededException` (HTTP 402), which `CreateAIPrompt`
/// declares for exactly this, and which is also what keeps this store finite: a prompt is
/// released only by `DeleteAIPrompt` or by the delete of its assistant.
///
/// CHOSEN: AWS quotas AI prompts per assistant but does not publish the figure, so this
/// is the figure the sibling families already use
/// ([`MAX_AI_GUARDRAILS_PER_ASSISTANT`], [`MAX_AI_AGENTS_PER_ASSISTANT`]).
pub const MAX_AI_PROMPTS_PER_ASSISTANT: usize = 100;

/// The sentence a create past [`MAX_AI_PROMPTS_PER_ASSISTANT`] is refused with, stating
/// the RULE — the ceiling and what to do about it — and carrying nothing from the request
/// or from what the assistant already holds, the way [`message_ai_guardrail_quota`] does
/// for the sibling family.
pub fn message_ai_prompt_quota() -> String {
    format!(
        "The maximum number of AI Prompts ({MAX_AI_PROMPTS_PER_ASSISTANT}) in this \
         assistant has been reached. Delete an AI Prompt before creating another."
    )
}

/// This service's ARN for an AI prompt, which names its parent assistant too:
/// `arn:aws:wisdom:<region>:<account>:ai-prompt/<assistantId>/<aiPromptId>`.
///
/// The same nested two-id-segment form [`ai_agent_arn`] and [`ai_guardrail_arn`] use, for
/// the same reason: an AI prompt is reachable only through its assistant, and the prompt's
/// own id comes LAST so [`resolve_id`] reads it back out.
pub fn ai_prompt_arn(region: &str, assistant_id: &str, ai_prompt_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("ai-prompt/{assistant_id}/{ai_prompt_id}"),
    )
}

/// One Amazon Q in Connect AI prompt, as this emulator persists it.
///
/// Every member a create carried is kept here, so what the create answered is what every
/// later read answers.
#[derive(Debug, Clone)]
pub struct AiPrompt {
    pub ai_prompt_id: String,
    pub ai_prompt_arn: String,
    pub assistant_id: String,
    /// The ARN of the assistant this prompt belongs to, read off that stored record rather
    /// than rebuilt, so the prompt reports its parent's own ARN.
    pub assistant_arn: String,
    pub name: String,
    pub r#type: String,
    pub template_type: String,
    pub model_id: String,
    pub api_format: String,
    /// The prompt template this prompt runs: the union member the `templateType` it was
    /// created with is configured through.
    pub template_configuration: AIPromptTemplateConfiguration,
    pub inference_configuration: Option<AIPromptInferenceConfiguration>,
    pub description: Option<String>,
    pub visibility_status: String,
    /// Whether a CALLER or the SERVICE created this prompt — the model's `Origin` — so a
    /// prompt a `CreateAIPrompt` minted reports `CUSTOMER`.
    pub origin: String,
    /// The `Status` this prompt reports. The create answers with the prompt already in
    /// service — see [`AI_RESOURCE_STATUS_ACTIVE`] — so it is stored once and reported
    /// unchanged.
    pub status: String,
    pub tags: HashMap<String, String>,
    /// When this record was last written: the create that minted it, until something
    /// modifies it. It is what the reported `modifiedTime` renders.
    pub modified_at: SystemTime,
}

impl AiPrompt {
    /// The `AIPromptData` every operation that reports this prompt answers with, rendered
    /// entirely out of the stored record.
    pub fn to_ai_prompt_data(&self) -> AIPromptData {
        AIPromptData {
            assistant_id: self.assistant_id.clone(),
            assistant_arn: self.assistant_arn.clone(),
            ai_prompt_id: self.ai_prompt_id.clone(),
            ai_prompt_arn: self.ai_prompt_arn.clone(),
            name: self.name.clone(),
            r#type: self.r#type.clone(),
            template_type: self.template_type.clone(),
            model_id: self.model_id.clone(),
            api_format: self.api_format.clone(),
            template_configuration: self.template_configuration.clone(),
            // A prompt created without inference parameters reports none rather than a
            // structure the caller never sent.
            inference_configuration: self.inference_configuration.clone(),
            modified_time: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            description: self.description.clone(),
            visibility_status: self.visibility_status.clone(),
            tags: tags_out(&self.tags),
            origin: Some(self.origin.clone()),
            status: Some(self.status.clone()),
        }
    }

    /// The same `AIPromptData` as [`Self::to_ai_prompt_data`], for an operation that
    /// addressed this prompt through a label that may carry a VERSION qualifier: every
    /// member comes out of the stored record, and the reported ARN names the version the
    /// request addressed ([`version_qualified_arn`]).
    pub fn to_ai_prompt_data_at_version(&self, qualifier: &str) -> AIPromptData {
        AIPromptData {
            ai_prompt_arn: version_qualified_arn(&self.ai_prompt_arn, qualifier),
            ..self.to_ai_prompt_data()
        }
    }
}

impl Assistant {
    /// An `aiPromptId` no AI prompt of this assistant holds, with the same contract as
    /// [`Self::mint_session_id`].
    pub fn mint_ai_prompt_id(&self, seq: u64) -> Option<String> {
        (0..MINT_ATTEMPTS as u64)
            .map(|attempt| mint_candidate(seq, attempt))
            .find(|candidate| !self.ai_prompts.contains_key(candidate))
    }

    /// The AI prompt a `UuidOrArnOrEitherWithQualifier` label names, or `None` when this
    /// assistant holds none under it.
    ///
    /// A label carrying the [`VERSION_QUALIFIER_LATEST`] qualifier names the prompt's own
    /// working copy, which is the record below — so the ARN a read reports, which names
    /// that version, addresses the same record it was read from. Any other qualifier names
    /// a numbered VERSION of the prompt, and this emulator publishes none —
    /// `CreateAIPromptVersion` is still a scaffold stub — so such a label names nothing
    /// here and the caller gets the operation's not-found answer.
    pub fn ai_prompt(&self, label: &str) -> Option<&AiPrompt> {
        let (id, qualifier) = resolve_qualified_id(label);
        match qualifier {
            Some(qualifier) if qualifier != VERSION_QUALIFIER_LATEST => None,
            _ => self.ai_prompts.get(id),
        }
    }
}

impl ComAmazonawsQconnectWisdomserviceState {
    /// The AI prompt a (`assistantId` label, `aiPromptId` label) pair names, with the same
    /// contract as [`Self::ai_guardrail`]: a prompt is reachable only through its
    /// assistant, so a prompt of ANOTHER assistant is not found under this one.
    pub fn ai_prompt(&self, assistant_label: &str, prompt_label: &str) -> Option<&AiPrompt> {
        self.assistant(assistant_label)?.ai_prompt(prompt_label)
    }

    /// Store an AI prompt inside the assistant it names. `false` — nothing stored — when
    /// this account holds no such assistant, or when that assistant already holds
    /// [`MAX_AI_PROMPTS_PER_ASSISTANT`], so the bound holds however the caller reaches
    /// this.
    pub fn insert_ai_prompt(&mut self, prompt: AiPrompt) -> bool {
        let Some(assistant) = self.assistants.get_mut(&prompt.assistant_id) else {
            return false;
        };
        if assistant.ai_prompts.len() >= MAX_AI_PROMPTS_PER_ASSISTANT {
            return false;
        }
        assistant
            .ai_prompts
            .insert(prompt.ai_prompt_id.clone(), prompt);
        true
    }
}
