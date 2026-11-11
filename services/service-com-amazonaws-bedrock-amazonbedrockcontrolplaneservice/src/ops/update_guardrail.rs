// SPDX-License-Identifier: CC0-1.0

//! @op UpdateGuardrail
//!
//! Mutates the stored guardrail's DRAFT in place: the request's `name`, `description`,
//! messaging strings, KMS key and WHOLE policy set replace the DRAFT's, and `updatedAt`
//! advances. The policy set is replaced rather than merged — a policy the request omits
//! is cleared — and the numbered versions already snapshotted are untouched.
//!
//! An identifier this store does not hold is `ResourceNotFoundException`; a name another
//! guardrail already holds is `ConflictException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateGuardrail`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Updates a guardrail with the values you specify. Specify a name and optional
//! description. Specify messages for when the guardrail successfully blocks a prompt or a
//! model response in the blockedInputMessaging and blockedOutputsMessaging fields. Specify
//! topics for the guardrail to deny in the topicPolicyConfig object. Each
//! GuardrailTopicConfig object in the topicsConfig list pertains to one topic. Give a name
//! and description so that the guardrail can properly identify the topic. Specify DENY in
//! the type field. (Optional) Provide up to five prompts that you would categorize as
//! belonging to the topic in the examples list. Specify filter strengths for the harmful
//! categories defined in Amazon Bedrock in the contentPolicyConfig object. Each
//! GuardrailContentFilterConfig object in the filtersConfig list pertains to a harmful
//! category. For more information, see Content filters. For more information about the
//! fields in a content filter, see GuardrailContentFilterConfig. Specify the category in
//! the type field. Specify the strength of the filter for prompts in the inputStrength
//! field and for model responses in the strength field of the GuardrailContentFilterConfig.
//! (Optional) For security, include the ARN of a KMS key in the kmsKeyId field.
//!
//! ## Input
//!
//! * `automatedReasoningPolicyConfig` (GuardrailAutomatedReasoningPolicyConfig) — Updated
//!   configuration for Automated Reasoning policies associated with the guardrail.
//! * `blockedInputMessaging` (GuardrailBlockedMessaging, len 1..500) **required** — The
//!   message to return when the guardrail blocks a prompt.
//! * `blockedOutputsMessaging` (GuardrailBlockedMessaging, len 1..500) **required** — The
//!   message to return when the guardrail blocks a model response.
//! * `contentPolicyConfig` (GuardrailContentPolicyConfig) — The content policy to configure
//!   for the guardrail.
//! * `contextualGroundingPolicyConfig` (GuardrailContextualGroundingPolicyConfig) — The
//!   contextual grounding policy configuration used to update a guardrail.
//! * `crossRegionConfig` (GuardrailCrossRegionConfig) — The system-defined guardrail profile
//!   that you're using with your guardrail. Guardrail profiles define the destination Amazon
//!   Web Services Regions where guardrail inference requests can be automatically routed. For
//!   more information, see the Amazon Bedrock User Guide.
//! * `description` (GuardrailDescription, len 1..200) — A description of the guardrail.
//! * `guardrailIdentifier` (GuardrailIdentifier, len ..2048, pattern
//!   ^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$)
//!   **required** — The unique identifier of the guardrail. This can be an ID or the ARN.
//! * `kmsKeyId` (KmsKeyId, len 1..2048, pattern
//!   ^(arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:((key/[a-zA-Z0-9-]{36})|(alias/[a-zA-Z0-9-_/]+)))|([a-zA-Z0-9-]{36})|(ali
//!   …) — The ARN of the KMS key with which to encrypt the guardrail.
//! * `name` (GuardrailName, len 1..50, pattern ^[0-9a-zA-Z-_]+$) **required** — A name for
//!   the guardrail.
//! * `sensitiveInformationPolicyConfig` (GuardrailSensitiveInformationPolicyConfig) — The
//!   sensitive information policy to configure for the guardrail.
//! * `topicPolicyConfig` (GuardrailTopicPolicyConfig) — The topic policy to configure for the
//!   guardrail.
//! * `wordPolicyConfig` (GuardrailWordPolicyConfig) — The word policy to configure for the
//!   guardrail.
//!
//! ## Output
//!
//! * `guardrailArn` (GuardrailArn, len ..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$) **required** —
//!   The ARN of the guardrail.
//! * `guardrailId` (GuardrailId, len ..64, pattern ^[a-z0-9]+$) **required** — The unique
//!   identifier of the guardrail
//! * `updatedAt` (Timestamp) **required** — The date and time at which the guardrail was
//!   updated.
//! * `version` (GuardrailDraftVersion, len 5..5, pattern ^DRAFT$) **required** — The version
//!   of the guardrail.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `ConflictException` (HTTP 400) — Error occurred because of a conflict while performing
//!   an operation.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ServiceQuotaExceededException` (HTTP 400) — The number of requests exceeds the service
//!   quota. Resubmit your request later.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateGuardrailError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateGuardrailError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#UpdateGuardrail")]
#[derive(Debug)]
pub enum UpdateGuardrailErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_guardrail::UpdateGuardrailError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateGuardrailError = crate::scaffolded::Scaffolded<UpdateGuardrailErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateGuardrailErrorModeled> for UpdateGuardrailError {
    fn from(modeled: UpdateGuardrailErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateGuardrail` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateGuardrail<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateGuardrail<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateGuardrail<Eng>
{
    type Input = UpdateGuardrailInput;
    type Output = UpdateGuardrailOutput;
    type Error = UpdateGuardrailError;

    const NAME: &'static str = "UpdateGuardrail";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_GUARDRAIL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_GUARDRAIL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(UpdateGuardrailErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("guardrail state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        let mut store = state.lock();

        let Some(id) = store
            .guardrail(&input.guardrail_identifier)
            .map(|guardrail| guardrail.id.clone())
        else {
            return Err(UpdateGuardrailErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::GUARDRAIL_NOT_FOUND.to_string()),
                },
            )
            .into());
        };

        // A name identifies a guardrail within the account, so it may move to this
        // guardrail only while no other one holds it.
        if store
            .guardrail_names
            .get(&input.name)
            .is_some_and(|owner| *owner != id)
        {
            return Err(UpdateGuardrailErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "a guardrail named {} already exists in this account",
                    input.name
                )),
            })
            .into());
        }

        let policies = crate::state::GuardrailPolicyConfigs {
            topic: input.topic_policy_config,
            content: input.content_policy_config,
            word: input.word_policy_config,
            sensitive_information: input.sensitive_information_policy_config,
            contextual_grounding: input.contextual_grounding_policy_config,
            automated_reasoning: input.automated_reasoning_policy_config,
            cross_region: input.cross_region_config,
        };
        let kms_key_arn = crate::state::kms_key_arn(input.kms_key_id.as_deref(), &region);

        let guardrail = store
            .guardrails
            .get_mut(&id)
            .expect("the guardrail this id was just read from");
        let previous_name = std::mem::replace(&mut guardrail.name, input.name.clone());
        guardrail.kms_key_arn = kms_key_arn;
        let draft = &mut guardrail.draft;
        draft.description = input.description;
        draft.blocked_input_messaging = input.blocked_input_messaging;
        draft.blocked_outputs_messaging = input.blocked_outputs_messaging;
        draft.set_policies(policies, &region);
        draft.updated_at = now;

        let output = UpdateGuardrailOutput {
            guardrail_id: guardrail.id.clone(),
            guardrail_arn: guardrail.arn.clone(),
            version: guardrail.draft.version.clone(),
            updated_at: crate::state::date_time(guardrail.draft.updated_at),
        };
        // The DRAFT this update replaced is a revision inference can APPLY, so the
        // guardrail is published again with the policies it now holds.
        let published = guardrail.clone();

        if previous_name != input.name {
            store.guardrail_names.remove(&previous_name);
            store.guardrail_names.insert(input.name, id);
        }

        drop(store);
        crate::state::publish_guardrail_to_inference(&*self.engine, &published);

        Ok(output)
    }
}
