// SPDX-License-Identifier: CC0-1.0

//! @op DeleteGuardrail
//!
//! Removes a guardrail, or one of its numbered versions, from [`crate::state`]. Per the
//! model's own documentation, an absent `guardrailVersion` removes the guardrail AND
//! every version of it, while a version removes only that snapshot. An identifier or
//! version this store does not hold is `ResourceNotFoundException`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteGuardrail`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Deletes a guardrail. To delete a guardrail, only specify the ARN of the guardrail in the
//! guardrailIdentifier field. If you delete a guardrail, all of its versions will be
//! deleted. To delete a version of a guardrail, specify the ARN of the guardrail in the
//! guardrailIdentifier field and the version in the guardrailVersion field.
//!
//! ## Input
//!
//! * `guardrailIdentifier` (GuardrailIdentifier, len ..2048, pattern
//!   ^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$)
//!   **required** — The unique identifier of the guardrail. This can be an ID or the ARN.
//! * `guardrailVersion` (GuardrailNumericalVersion, pattern ^[1-9][0-9]{0,7}$) — The version
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
//! * `ResourceInUseException` (HTTP 400) — Thrown when attempting to delete or modify a
//!   resource that is currently being used by other resources or operations. For example,
//!   trying to delete an Automated Reasoning policy that is referenced by an active
//!   guardrail.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteGuardrailError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteGuardrailError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#DeleteGuardrail")]
#[derive(Debug)]
pub enum DeleteGuardrailErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_guardrail::DeleteGuardrailError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteGuardrailError = crate::scaffolded::Scaffolded<DeleteGuardrailErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteGuardrailErrorModeled> for DeleteGuardrailError {
    fn from(modeled: DeleteGuardrailErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteGuardrail` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteGuardrail<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteGuardrail<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteGuardrail<Eng>
{
    type Input = DeleteGuardrailInput;
    type Output = DeleteGuardrailOutput;
    type Error = DeleteGuardrailError;

    const NAME: &'static str = "DeleteGuardrail";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_GUARDRAIL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_GUARDRAIL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DeleteGuardrailErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("guardrail state is not available on this engine".to_string()),
                },
            )
            .into());
        };

        let not_found = || {
            DeleteGuardrailErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::GUARDRAIL_NOT_FOUND.to_string()),
            })
        };

        let mut store = state.lock();
        let id = crate::state::guardrail_id_of(&input.guardrail_identifier).to_string();

        match input.guardrail_version.as_deref() {
            Some(version) => {
                let Some(number) = crate::state::version_number(version) else {
                    return Err(not_found().into());
                };
                let Some(guardrail) = store.guardrails.get_mut(&id) else {
                    return Err(not_found().into());
                };
                // `remove_version` also releases the idempotency tokens that named
                // this version, so a token never outlives the version it minted.
                if guardrail.remove_version(number).is_none() {
                    return Err(not_found().into());
                }
                // The guardrail stands, with one revision fewer: it is published
                // again so the version that is gone applies to nothing.
                let published = guardrail.clone();
                drop(store);
                crate::state::publish_guardrail_to_inference(&*self.engine, &published);
            }
            None => {
                let Some(removed) = store.guardrails.remove(&id) else {
                    return Err(not_found().into());
                };
                // The name is free again, and no replayed idempotency token may hand a
                // later caller the guardrail that is now gone. The version tokens went
                // with the record.
                store.guardrail_names.remove(&removed.name);
                store.create_tokens.retain(|_, minted| *minted != removed.id);
                // A guardrail that is gone applies to nothing, so it is withdrawn from
                // the inference plane with no store of this service held.
                drop(store);
                crate::state::withdraw_guardrail_from_inference(&*self.engine, &removed.id);
            }
        }

        Ok(DeleteGuardrailOutput {})
    }
}
