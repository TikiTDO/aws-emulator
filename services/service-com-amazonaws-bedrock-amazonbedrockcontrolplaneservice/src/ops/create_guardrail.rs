// SPDX-License-Identifier: CC0-1.0

//! @op CreateGuardrail
//!
//! Mints a guardrail and PERSISTS it in [`crate::state`]: a fresh `guardrailId`, the
//! `guardrail/<id>` ARN for the signing region, the DRAFT revision carrying the
//! request's messaging strings and policy set, and the creation instant. Every member
//! of the response is read back out of the stored record.
//!
//! A replay of the same `clientRequestToken` answers with the guardrail that token
//! already minted (`@idempotencyToken`), and a name another guardrail already holds is
//! refused with `ConflictException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateGuardrail`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates a guardrail to block topics and to implement safeguards for your generative AI
//! applications. You can configure the following policies in a guardrail to avoid
//! undesirable and harmful content, filter out denied topics and words, and remove
//! sensitive information for privacy protection. Content filters - Adjust filter strengths
//! to block input prompts or model responses containing harmful content. Denied topics -
//! Define a set of topics that are undesirable in the context of your application. These
//! topics will be blocked if detected in user queries or model responses. Word filters -
//! Configure filters to block undesirable words, phrases, and profanity. Such words can
//! include offensive terms, competitor names etc. Sensitive information filters - Block or
//! mask sensitive information such as personally identifiable information (PII) or custom
//! regex in user inputs and model responses. In addition to the above policies, you can
//! also configure the messages to be returned to the user if a user input or model response
//! is in violation of the policies defined in the guardrail. For more information, see
//! Amazon Bedrock Guardrails in the Amazon Bedrock User Guide.
//!
//! ## Input
//!
//! * `automatedReasoningPolicyConfig` (GuardrailAutomatedReasoningPolicyConfig) — Optional
//!   configuration for integrating Automated Reasoning policies with the new guardrail.
//! * `blockedInputMessaging` (GuardrailBlockedMessaging, len 1..500) **required** — The
//!   message to return when the guardrail blocks a prompt.
//! * `blockedOutputsMessaging` (GuardrailBlockedMessaging, len 1..500) **required** — The
//!   message to return when the guardrail blocks a model response.
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than once. If
//!   this token matches a previous request, Amazon Bedrock ignores the request, but does not
//!   return an error. For more information, see Ensuring idempotency in the Amazon S3 User
//!   Guide.
//! * `contentPolicyConfig` (GuardrailContentPolicyConfig) — The content filter policies to
//!   configure for the guardrail.
//! * `contextualGroundingPolicyConfig` (GuardrailContextualGroundingPolicyConfig) — The
//!   contextual grounding policy configuration used to create a guardrail.
//! * `crossRegionConfig` (GuardrailCrossRegionConfig) — The system-defined guardrail profile
//!   that you're using with your guardrail. Guardrail profiles define the destination Amazon
//!   Web Services Regions where guardrail inference requests can be automatically routed. For
//!   more information, see the Amazon Bedrock User Guide.
//! * `description` (GuardrailDescription, len 1..200) — A description of the guardrail.
//! * `kmsKeyId` (KmsKeyId, len 1..2048, pattern
//!   ^(arn:aws(-[^:]+)?:kms:[a-zA-Z0-9-]*:[0-9]{12}:((key/[a-zA-Z0-9-]{36})|(alias/[a-zA-Z0-9-_/]+)))|([a-zA-Z0-9-]{36})|(ali
//!   …) — The ARN of the KMS key that you use to encrypt the guardrail.
//! * `name` (GuardrailName, len 1..50, pattern ^[0-9a-zA-Z-_]+$) **required** — The name to
//!   give the guardrail.
//! * `sensitiveInformationPolicyConfig` (GuardrailSensitiveInformationPolicyConfig) — The
//!   sensitive information policy to configure for the guardrail.
//! * `tags` (TagList, len 0..200) — The tags that you want to attach to the guardrail.
//! * `topicPolicyConfig` (GuardrailTopicPolicyConfig) — The topic policies to configure for
//!   the guardrail.
//! * `wordPolicyConfig` (GuardrailWordPolicyConfig) — The word policy you configure for the
//!   guardrail.
//!
//! ## Output
//!
//! * `createdAt` (Timestamp) **required** — The time at which the guardrail was created.
//! * `guardrailArn` (GuardrailArn, len ..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$) **required** —
//!   The ARN of the guardrail.
//! * `guardrailId` (GuardrailId, len ..64, pattern ^[a-z0-9]+$) **required** — The unique
//!   identifier of the guardrail that was created.
//! * `version` (GuardrailDraftVersion, len 5..5, pattern ^DRAFT$) **required** — The version
//!   of the guardrail that was created. This value will always be DRAFT.
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
//! * `TooManyTagsException` (HTTP 400) — The request contains more tags than can be
//!   associated with a resource (50 tags per resource). The maximum number of tags includes
//!   both existing tags and those included in your current request.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateGuardrailError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateGuardrailError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateGuardrail")]
#[derive(Debug)]
pub enum CreateGuardrailErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    TooManyTagsException(TooManyTagsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_guardrail::CreateGuardrailError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateGuardrailError = crate::scaffolded::Scaffolded<CreateGuardrailErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateGuardrailErrorModeled> for CreateGuardrailError {
    fn from(modeled: CreateGuardrailErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateGuardrail` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateGuardrail<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateGuardrail<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateGuardrail<Eng>
{
    type Input = CreateGuardrailInput;
    type Output = CreateGuardrailOutput;
    type Error = CreateGuardrailError;

    const NAME: &'static str = "CreateGuardrail";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GUARDRAIL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GUARDRAIL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateGuardrailErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("guardrail state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let token = input.client_request_token;

        let mut store = state.lock();

        // `@idempotencyToken`: a replay answers with what the first request minted.
        if let Some(minted) = token
            .as_deref()
            .and_then(|token| store.create_tokens.get(token))
        {
            if let Some(existing) = store.guardrails.get(minted) {
                return Ok(CreateGuardrailOutput {
                    guardrail_id: existing.id.clone(),
                    guardrail_arn: existing.arn.clone(),
                    version: existing.draft.version.clone(),
                    created_at: crate::state::date_time(existing.created_at),
                });
            }
        }

        if store.guardrail_names.contains_key(&input.name) {
            return Err(CreateGuardrailErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "a guardrail named {} already exists in this account",
                    input.name
                )),
            })
            .into());
        }

        // The account's guardrail quota. Enforcing it here is also what bounds this
        // service's store: without a server-side cap a caller could grow it with
        // nothing but fresh names.
        if store.guardrails.len() >= crate::state::MAX_GUARDRAILS_PER_ACCOUNT {
            return Err(CreateGuardrailErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "this account already holds the maximum of {} guardrails; delete one \
                         or request a quota increase",
                        crate::state::MAX_GUARDRAILS_PER_ACCOUNT
                    )),
                },
            )
            .into());
        }

        let id = store.mint_guardrail_id(now);
        let mut draft = crate::state::GuardrailRevision::new(
            crate::state::DRAFT_VERSION,
            input.blocked_input_messaging,
            input.blocked_outputs_messaging,
            now,
        );
        draft.description = input.description;
        draft.set_policies(
            crate::state::GuardrailPolicyConfigs {
                topic: input.topic_policy_config,
                content: input.content_policy_config,
                word: input.word_policy_config,
                sensitive_information: input.sensitive_information_policy_config,
                contextual_grounding: input.contextual_grounding_policy_config,
                automated_reasoning: input.automated_reasoning_policy_config,
                cross_region: input.cross_region_config,
            },
            &region,
        );

        let guardrail = crate::state::Guardrail {
            id: id.clone(),
            arn: crate::state::guardrail_arn(&region, &id),
            name: input.name.clone(),
            kms_key_arn: crate::state::kms_key_arn(input.kms_key_id.as_deref(), &region),
            tags: input.tags.unwrap_or_default(),
            created_at: now,
            draft,
            versions: ::std::collections::BTreeMap::new(),
            version_tokens: ::std::collections::HashMap::new(),
            next_version: 1,
        };

        store.guardrail_names.insert(input.name, id.clone());
        if let Some(key) = token {
            store.create_tokens.insert(key, id.clone());
        }
        store.guardrails.insert(id.clone(), guardrail);

        // Read the response back out of the STORED record, so what the caller is told
        // is exactly what a later GetGuardrail will find.
        let stored = store
            .guardrails
            .get(&id)
            .expect("the guardrail just inserted under this id")
            .clone();
        // The guardrail's revisions are what the inference plane applies, so the
        // record is published there with no store of this service held.
        drop(store);
        crate::state::publish_guardrail_to_inference(&*self.engine, &stored);
        Ok(CreateGuardrailOutput {
            guardrail_id: stored.id.clone(),
            guardrail_arn: stored.arn.clone(),
            version: stored.draft.version.clone(),
            created_at: crate::state::date_time(stored.created_at),
        })
    }
}
