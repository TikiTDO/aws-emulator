// SPDX-License-Identifier: CC0-1.0

//! @op CreateGuardrailVersion
//!
//! Snapshots the stored DRAFT into a new immutable numbered version
//! (`GuardrailNumericalVersion`, `^[1-9][0-9]{0,7}$`), counting from `1` per guardrail.
//! A later `UpdateGuardrail` changes the DRAFT only, so `GetGuardrail` on the number
//! keeps returning the configuration as it was when the version was cut.
//!
//! A replay of the same `clientRequestToken` answers with the version that token
//! already cut (`@idempotencyToken`); an identifier this store does not hold is
//! `ResourceNotFoundException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateGuardrailVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Creates a version of the guardrail. Use this API to create a snapshot of the guardrail
//! when you are satisfied with a configuration, or to compare the configuration with
//! another version.
//!
//! ## Input
//!
//! * `clientRequestToken` (IdempotencyToken, len 1..256, pattern
//!   ^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$, @idempotencyToken) — A unique,
//!   case-sensitive identifier to ensure that the API request completes no more than once. If
//!   this token matches a previous request, Amazon Bedrock ignores the request, but does not
//!   return an error. For more information, see Ensuring idempotency in the Amazon S3 User
//!   Guide.
//! * `description` (GuardrailDescription, len 1..200) — A description of the guardrail
//!   version.
//! * `guardrailIdentifier` (GuardrailIdentifier, len ..2048, pattern
//!   ^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$)
//!   **required** — The unique identifier of the guardrail. This can be an ID or the ARN.
//!
//! ## Output
//!
//! * `guardrailId` (GuardrailId, len ..64, pattern ^[a-z0-9]+$) **required** — The unique
//!   identifier of the guardrail.
//! * `version` (GuardrailNumericalVersion, pattern ^[1-9][0-9]{0,7}$) **required** — The
//!   number of the version of the guardrail.
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
// `CreateGuardrailVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateGuardrailVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#CreateGuardrailVersion")]
#[derive(Debug)]
pub enum CreateGuardrailVersionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_guardrail_version::CreateGuardrailVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateGuardrailVersionError = crate::scaffolded::Scaffolded<CreateGuardrailVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateGuardrailVersionErrorModeled> for CreateGuardrailVersionError {
    fn from(modeled: CreateGuardrailVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateGuardrailVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateGuardrailVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateGuardrailVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateGuardrailVersion<Eng>
{
    type Input = CreateGuardrailVersionInput;
    type Output = CreateGuardrailVersionOutput;
    type Error = CreateGuardrailVersionError;

    const NAME: &'static str = "CreateGuardrailVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GUARDRAIL_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_GUARDRAIL_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateGuardrailVersionErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("guardrail state is not available on this engine".to_string()),
                },
            )
            .into());
        };
        let now = ::std::time::SystemTime::now();
        let token = input.client_request_token;

        let mut store = state.lock();

        let Some(guardrail) = store.guardrail_mut(&input.guardrail_identifier) else {
            return Err(CreateGuardrailVersionErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::GUARDRAIL_NOT_FOUND.to_string()),
                },
            )
            .into());
        };

        // `@idempotencyToken`: a replay answers with the version the first request cut,
        // rather than cutting a second one. The record the token names is held on the
        // guardrail, so the token cannot outlive the version it minted.
        if let Some(minted) = token
            .as_deref()
            .and_then(|token| guardrail.version_tokens.get(token))
            .and_then(|number| guardrail.versions.get(number))
        {
            return Ok(CreateGuardrailVersionOutput {
                guardrail_id: guardrail.id.clone(),
                version: minted.version.clone(),
            });
        }

        let number = guardrail.next_version;
        // The per-guardrail version quota, which is what bounds how many snapshots one
        // guardrail can accumulate, and the largest number the modelled
        // `GuardrailNumericalVersion` can spell, since numbers are never reused.
        if guardrail.versions.len() >= crate::state::MAX_VERSIONS_PER_GUARDRAIL
            || number > crate::state::MAX_VERSION_NUMBER
        {
            return Err(
                CreateGuardrailVersionErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "this guardrail already holds the maximum of {} versions; delete \
                             one or request a quota increase",
                            crate::state::MAX_VERSIONS_PER_GUARDRAIL
                        )),
                    },
                )
                .into(),
            );
        }
        let mut snapshot = guardrail.draft.clone();
        snapshot.version = number.to_string();
        // The version carries the description the request gives it, and otherwise the
        // description of the DRAFT it is a snapshot of.
        snapshot.description = input.description.or(snapshot.description);
        snapshot.created_at = now;
        snapshot.updated_at = now;

        let id = guardrail.id.clone();
        let version = snapshot.version.clone();
        guardrail.versions.insert(number, snapshot);
        guardrail.next_version = number.saturating_add(1);
        if let Some(key) = token {
            guardrail.version_tokens.insert(key, number);
        }

        // The version this cut minted is a revision inference can APPLY, so the
        // guardrail is published to the inference plane with it, with no store of this
        // service held.
        let published = guardrail.clone();
        drop(store);
        crate::state::publish_guardrail_to_inference(&*self.engine, &published);

        Ok(CreateGuardrailVersionOutput {
            guardrail_id: id,
            version,
        })
    }
}
